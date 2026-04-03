use crate::agent::prompt::PromptMode;
use crate::agent::provider::{
    OpenAICompatibleProvider, Provider, ProviderResponse, ToolDefinition,
};
use crate::config::Config;
use crate::prompt_eval::test_case::TestCase;
use crate::prompt_eval::variation::{build_system_prompt_with_config, PromptConfig};
use crate::storage::models::{Message, ToolCall};
use crate::storage::SqliteStorage;
use crate::tools::registry::ToolRegistry;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalConfig {
    pub temperature: f32,
    pub max_tokens: usize,
    pub execute_tools: bool,
    pub workspace_path: PathBuf,
    pub timeout_secs: u64,
}

impl Default for EvalConfig {
    fn default() -> Self {
        EvalConfig {
            temperature: 0.0,
            max_tokens: 2048,
            execute_tools: true,
            workspace_path: PathBuf::from("prompt_eval_workspace"),
            timeout_secs: 60,
        }
    }
}

impl EvalConfig {
    pub fn resolve_workspace(&self) -> PathBuf {
        if self.workspace_path.is_absolute() {
            self.workspace_path.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&self.workspace_path)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub name: String,
    pub arguments: String,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub test_name: String,
    pub response: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pub turns_taken: usize,
    pub tokens_used: usize,
    pub error: Option<String>,
    pub duration_ms: u64,
}

pub struct EvaluationRunner {
    provider: Arc<dyn Provider>,
    tool_registry: Arc<ToolRegistry>,
    config: EvalConfig,
}

impl EvaluationRunner {
    pub async fn new(osa_config: Config, eval_config: EvalConfig) -> Result<Self, EvalError> {
        let provider = Self::build_provider(&osa_config)?;

        let storage = Arc::new(SqliteStorage::new_in_memory()?);
        let tool_registry = Arc::new(ToolRegistry::new(osa_config.clone(), storage)?);

        Ok(EvaluationRunner {
            provider,
            tool_registry,
            config: eval_config,
        })
    }

    fn build_provider(config: &Config) -> Result<Arc<dyn Provider>, EvalError> {
        let provider_config = if !config.providers.is_empty() {
            let active_id = if config.default_provider.is_empty() {
                config.providers.first().map(|p| p.provider_type.clone())
            } else {
                Some(config.default_provider.clone())
            };

            config
                .providers
                .iter()
                .find(|p| &p.provider_type == active_id.as_ref().unwrap_or(&String::new()))
                .or_else(|| config.providers.first())
                .cloned()
                .ok_or_else(|| EvalError::ConfigError("No provider configured".into()))?
        } else {
            config.provider.clone()
        };

        let provider = OpenAICompatibleProvider::new(provider_config)
            .map_err(|e| EvalError::ProviderError(e.to_string()))?;

        Ok(Arc::new(provider))
    }

    pub async fn run_test(&self, test: &TestCase, prompt_config: &PromptConfig) -> EvalResult {
        let start = Instant::now();
        let test_name = test.name.clone();

        match self.run_test_internal(test, prompt_config).await {
            Ok(result) => result,
            Err(e) => EvalResult {
                test_name,
                response: String::new(),
                tool_calls: Vec::new(),
                turns_taken: 0,
                tokens_used: 0,
                error: Some(e.to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    }

    async fn setup_workspace(&self, test: &TestCase) -> Result<(), EvalError> {
        let workspace = self.config.resolve_workspace();

        // Clean workspace for this test
        if workspace.exists() {
            std::fs::remove_dir_all(&workspace)
                .map_err(|e| EvalError::ToolError(format!("Failed to clean workspace: {}", e)))?;
        }
        std::fs::create_dir_all(&workspace)
            .map_err(|e| EvalError::ToolError(format!("Failed to create workspace: {}", e)))?;

        if test.setup_files.is_empty() {
            return Ok(());
        }

        for file in &test.setup_files {
            let full_path = workspace.join(&file.path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| EvalError::ToolError(format!("Failed to create dir: {}", e)))?;
            }
            std::fs::write(&full_path, &file.content).map_err(|e| {
                EvalError::ToolError(format!("Failed to write {}: {}", file.path, e))
            })?;
        }

        Ok(())
    }

    async fn run_test_internal(
        &self,
        test: &TestCase,
        prompt_config: &PromptConfig,
    ) -> Result<EvalResult, EvalError> {
        let start = Instant::now();
        self.setup_workspace(test).await?;
        let system_prompt = build_system_prompt_with_config(prompt_config);

        let mut messages: Vec<Message> = vec![Message::system(system_prompt)];
        messages.push(Message::user(test.input.clone()));

        let tools = self.tool_registry.get_tool_definitions();
        let mut all_tool_calls = Vec::new();
        let mut turns_taken = 0;
        let mut total_tokens = 0;
        let mut final_response = String::new();

        let max_turns = test.max_turns.min(25);
        let timeout = Duration::from_secs(test.timeout_secs.unwrap_or(self.config.timeout_secs));

        loop {
            turns_taken += 1;

            if turns_taken > max_turns {
                break;
            }

            let result = tokio::time::timeout(timeout, self.call_provider(&messages, &tools))
                .await
                .map_err(|_| EvalError::Timeout)?;

            let response = result?;

            if let Some(ref usage) = response.usage {
                total_tokens += usage.total;
            }

            if let Some(content) = &response.content {
                final_response = content.clone();

                let assistant_msg =
                    Message::assistant(content.clone(), response.tool_calls.clone());
                messages.push(assistant_msg);
            }

            if let Some(tool_calls) = &response.tool_calls {
                if tool_calls.is_empty() {
                    break;
                }

                for tc in tool_calls {
                    let args_str =
                        serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string());
                    let record = ToolCallRecord {
                        name: tc.name.clone(),
                        arguments: args_str.clone(),
                        result: None,
                    };
                    all_tool_calls.push(record);

                    if self.config.execute_tools {
                        let tool_result = self.execute_tool(&tc.name, &args_str).await;

                        if let Some(last) = all_tool_calls.last_mut() {
                            last.result = Some(tool_result.clone());
                        }

                        messages.push(Message::tool_result(tc.id.clone(), tool_result));
                    } else {
                        let simulated_result =
                            format!("[Simulated] Tool {} executed successfully", tc.name);

                        if let Some(last) = all_tool_calls.last_mut() {
                            last.result = Some(simulated_result.clone());
                        }

                        messages.push(Message::tool_result(tc.id.clone(), simulated_result));
                    }
                }
            } else {
                break;
            }

            if response.finish_reason == "stop"
                || response.content.is_some() && response.tool_calls.is_none()
            {
                break;
            }
        }

        Ok(EvalResult {
            test_name: test.name.clone(),
            response: final_response,
            tool_calls: all_tool_calls,
            turns_taken,
            tokens_used: total_tokens,
            error: None,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn call_provider(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, EvalError> {
        self.provider
            .complete(messages, tools)
            .await
            .map_err(|e| EvalError::ProviderError(e.to_string()))
    }

    async fn execute_tool(&self, name: &str, args: &str) -> String {
        let args_value: serde_json::Value =
            serde_json::from_str(args).unwrap_or(serde_json::json!({}));

        match self.tool_registry.execute(name, args_value).await {
            Ok(result) => result,
            Err(e) => format!("Error: {}", e),
        }
    }

    pub async fn run_tests_parallel(
        &self,
        tests: &[TestCase],
        prompt_config: &PromptConfig,
        parallelism: usize,
    ) -> Vec<EvalResult> {
        let has_setup_files = tests.iter().any(|t| !t.setup_files.is_empty());
        let effective_parallelism = if has_setup_files { 1 } else { parallelism };
        let semaphore = Arc::new(tokio::sync::Semaphore::new(effective_parallelism));
        let mut handles = Vec::new();

        for test in tests {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let runner = self.clone_ref();
            let test = test.clone();
            let config = prompt_config.clone();

            let handle = tokio::spawn(async move {
                let result = runner.run_test(&test, &config).await;
                drop(permit);
                result
            });

            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }

        results
    }

    fn clone_ref(&self) -> Self {
        EvaluationRunner {
            provider: self.provider.clone(),
            tool_registry: self.tool_registry.clone(),
            config: self.config.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Tool execution error: {0}")]
    ToolError(String),
    #[error("Timeout")]
    Timeout,
    #[error("Storage error: {0}")]
    StorageError(String),
}

impl From<crate::error::OSAgentError> for EvalError {
    fn from(e: crate::error::OSAgentError) -> Self {
        EvalError::ConfigError(e.to_string())
    }
}
