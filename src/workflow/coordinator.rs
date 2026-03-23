use crate::agent::subagent_manager::SubagentManager;
use crate::error::{OSAgentError, Result};
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::coordination::{
    Capability, CapabilityType, CoordinatedContext, EscalationPolicy, JobCard, JobInput, JobResult,
    JobStatus, MessageType, OutputSchema, ResultType, StructuredMessage, TaskType,
};
use crate::workflow::db::WorkflowDb;
use crate::workflow::events::WorkflowEvent;
use crate::workflow::graph::{topological_sort, GraphValidator};
use crate::workflow::types::*;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::time::sleep;
use uuid::Uuid;

pub struct SafeWorkflowCoordinator {
    db: Arc<WorkflowDb>,
    artifact_store: Arc<ArtifactStore>,
    subagent_manager: Arc<SubagentManager>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    config: CoordinationConfig,
}

#[derive(Debug, Clone)]
pub struct CoordinationConfig {
    pub max_parallel_jobs: usize,
    pub default_timeout_seconds: u64,
    pub retry_on_failure: bool,
    pub max_retries: u32,
    pub escalation_policy: EscalationPolicy,
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            max_parallel_jobs: 5,
            default_timeout_seconds: 300,
            retry_on_failure: true,
            max_retries: 2,
            escalation_policy: EscalationPolicy::NotifyParent,
        }
    }
}

impl SafeWorkflowCoordinator {
    pub fn new(
        db: Arc<WorkflowDb>,
        artifact_store: Arc<ArtifactStore>,
        subagent_manager: Arc<SubagentManager>,
        config: Option<CoordinationConfig>,
    ) -> (Self, broadcast::Receiver<WorkflowEvent>) {
        let (event_tx, event_rx) = broadcast::channel(100);
        (
            Self {
                db,
                artifact_store,
                subagent_manager,
                event_tx,
                config: config.unwrap_or_default(),
            },
            event_rx,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    pub async fn execute_workflow_safe(
        &self,
        workflow_id: &str,
        graph_json: &str,
        version: i32,
        initial_context: Option<ChatContext>,
        parameters: HashMap<String, serde_json::Value>,
        parent_session_id: Option<String>,
    ) -> Result<WorkflowResult> {
        let run_id = Uuid::new_v4().to_string();

        let graph: WorkflowGraph = match serde_json::from_str(graph_json) {
            Ok(g) => g,
            Err(e) => {
                return Ok(WorkflowResult {
                    run_id: run_id.clone(),
                    status: "failed".to_string(),
                    output: None,
                    error: Some(format!("Invalid graph JSON: {}", e)),
                });
            }
        };

        let mut validator = GraphValidator::new();
        if !validator.validate(&graph) {
            let errors: Vec<String> = validator
                .get_errors()
                .iter()
                .map(|e| e.message.clone())
                .collect();
            return Ok(WorkflowResult {
                run_id: run_id.clone(),
                status: "failed".to_string(),
                output: None,
                error: Some(format!("Validation errors: {}", errors.join("; "))),
            });
        }

        let run = WorkflowRun {
            id: run_id.clone(),
            workflow_id: workflow_id.to_string(),
            workflow_version: version,
            status: "running".to_string(),
            started_at: Utc::now().to_rfc3339(),
            completed_at: None,
            error_message: None,
        };

        self.db.create_run(&run)?;

        let _ = self.event_tx.send(WorkflowEvent::workflow_run_started(
            &run_id,
            workflow_id,
            version,
        ));

        let node_order = match topological_sort(&graph) {
            Ok(order) => order,
            Err(e) => {
                let _ = self
                    .db
                    .update_run_status(&run_id, "failed", Some(&e.to_string()));
                let _ = self.event_tx.send(WorkflowEvent::workflow_run_failed(
                    &run_id,
                    workflow_id,
                    &e.to_string(),
                ));
                return Ok(WorkflowResult {
                    run_id,
                    status: "failed".to_string(),
                    output: None,
                    error: Some(e.to_string()),
                });
            }
        };

        let mut coordinated_context = CoordinatedContext::new(None);

        coordinated_context.shared_state =
            serde_json::to_value(&parameters).unwrap_or(serde_json::json!({}));

        if let Some(ctx) = initial_context {
            coordinated_context.messages = ctx
                .messages
                .into_iter()
                .map(|m| StructuredMessage {
                    message_id: Uuid::new_v4().to_string(),
                    from_job_id: "init".to_string(),
                    to_job_id: None,
                    message_type: MessageType::TaskAssigned,
                    content: serde_json::json!({ "role": m.role, "content": m.content }),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                })
                .collect();
        }

        for node_id in &node_order {
            let node = graph
                .nodes
                .iter()
                .find(|n| &n.id == node_id)
                .ok_or_else(|| OSAgentError::Workflow(format!("Node not found: {}", node_id)))?;

            let job_id = node_id.clone();

            let _ = self.event_tx.send(WorkflowEvent::node_started(
                &run_id,
                workflow_id,
                &node.id,
                node.node_type.as_str(),
            ));

            let input_data = coordinated_context.shared_state.clone();
            let job_result = self
                .execute_node_safe(
                    node,
                    &input_data,
                    &job_id,
                    &mut coordinated_context,
                    parent_session_id.as_deref(),
                )
                .await;

            let output = match job_result {
                Ok(result) => {
                    let verification = coordinated_context.verify_output(&job_id, &result);
                    let status = if verification.verified {
                        JobStatus::Verified
                    } else {
                        JobStatus::Completed
                    };

                    let job_result = JobResult {
                        job_id: job_id.clone(),
                        status,
                        output: Some(result.clone()),
                        error: None,
                        verification: Some(verification),
                        completed_at: Some(chrono::Utc::now().to_rfc3339()),
                        token_usage: None,
                    };

                    coordinated_context.add_result(job_result);

                    let _ = self.event_tx.send(WorkflowEvent::node_completed(
                        &run_id,
                        workflow_id,
                        &node.id,
                        node.node_type.as_str(),
                        Some(result.clone()),
                    ));

                    result
                }
                Err(e) => {
                    let job_result = JobResult {
                        job_id: job_id.clone(),
                        status: JobStatus::Failed,
                        output: None,
                        error: Some(e.to_string()),
                        verification: None,
                        completed_at: Some(chrono::Utc::now().to_rfc3339()),
                        token_usage: None,
                    };

                    coordinated_context.add_result(job_result);

                    let _ = self.event_tx.send(WorkflowEvent::node_failed(
                        &run_id,
                        workflow_id,
                        &node.id,
                        node.node_type.as_str(),
                        &e.to_string(),
                    ));

                    let _ = self
                        .db
                        .update_run_status(&run_id, "failed", Some(&e.to_string()));
                    let _ = self.event_tx.send(WorkflowEvent::workflow_run_failed(
                        &run_id,
                        workflow_id,
                        &e.to_string(),
                    ));

                    return Ok(WorkflowResult {
                        run_id,
                        status: "failed".to_string(),
                        output: Some(serde_json::json!({
                            "failed_node": node.id,
                            "error": e.to_string()
                        })),
                        error: Some(e.to_string()),
                    });
                }
            };

            coordinated_context.shared_state = output;
        }

        let final_output = coordinated_context.shared_state.clone();

        let _ = self.db.update_run_status(&run_id, "completed", None);
        let _ = self.event_tx.send(WorkflowEvent::workflow_run_completed(
            &run_id,
            workflow_id,
            "completed",
            Some(final_output.clone()),
        ));

        Ok(WorkflowResult {
            run_id,
            status: "completed".to_string(),
            output: Some(final_output),
            error: None,
        })
    }

    async fn execute_node_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        job_id: &str,
        context: &mut CoordinatedContext,
        parent_session_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        match node.node_type {
            NodeType::Trigger => self.execute_trigger_safe(node, input, context).await,
            NodeType::Agent => {
                self.execute_agent_safe(node, input, job_id, context, parent_session_id)
                    .await
            }
            NodeType::Condition => self.execute_condition_safe(node, input, context).await,
            NodeType::Transform => self.execute_transform_safe(node, input, context).await,
            NodeType::Delay => self.execute_delay_safe(node, input, context).await,
            NodeType::Output => self.execute_output_safe(node, input, context).await,
        }
    }

    async fn execute_trigger_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        context: &mut CoordinatedContext,
    ) -> Result<serde_json::Value> {
        let job = JobCard {
            job_id: node.id.clone(),
            parent_job_id: None,
            task: "Initialize workflow".to_string(),
            task_type: TaskType::Trigger,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["triggered".to_string()],
                optional_fields: vec![],
                result_type: ResultType::Object,
            },
            timeout_seconds: 10,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        let output = serde_json::json!({
            "triggered": true,
            "node_id": node.id,
            "context_id": context.context_id,
            "version": context.version
        });

        context.send_message(&node.id, None, MessageType::TaskCompleted, output.clone());

        Ok(output)
    }

    async fn execute_agent_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        job_id: &str,
        context: &mut CoordinatedContext,
        parent_session_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let config: AgentNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(AgentNodeConfig {
                agent_id: "main".to_string(),
                system_prompt: None,
                task_template: "{{input}}".to_string(),
                input_mapping: HashMap::new(),
            });

        let task = self.render_template(&config.task_template, input, context);

        let _output_schema = OutputSchema {
            required_fields: vec!["result".to_string()],
            optional_fields: vec!["metadata".to_string(), "artifacts".to_string()],
            result_type: ResultType::Object,
        };

        let _capabilities = self.compute_capabilities(&config);

        let job = JobCard {
            job_id: job_id.to_string(),
            parent_job_id: context.parent_context_id.clone(),
            task: task.clone(),
            task_type: TaskType::Agent,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["result".to_string()],
                optional_fields: vec![],
                result_type: ResultType::Object,
            },
            timeout_seconds: self.config.default_timeout_seconds,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        context.send_message(
            job_id,
            None,
            MessageType::TaskAssigned,
            serde_json::json!({
                "task": task,
                "timeout_seconds": self.config.default_timeout_seconds
            }),
        );

        let parent_id = parent_session_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let subagent_session_id = self
            .subagent_manager
            .spawn_subagent(
                parent_id,
                format!("Workflow agent: {}", config.agent_id),
                task.clone(),
                "general".to_string(),
            )
            .await?;

        let start = Instant::now();
        let timeout_duration = Duration::from_secs(self.config.default_timeout_seconds);

        loop {
            if start.elapsed() > timeout_duration {
                context.send_message(job_id, None, MessageType::Timeout, serde_json::json!({}));
                return Err(OSAgentError::Workflow(
                    "Agent execution timed out".to_string(),
                ));
            }

            if !self
                .subagent_manager
                .is_subagent_running(&subagent_session_id)
            {
                break;
            }

            sleep(Duration::from_millis(100)).await;
        }

        context.send_message(
            job_id,
            None,
            MessageType::TaskCompleted,
            serde_json::json!({
                "agent_id": config.agent_id,
                "session_id": subagent_session_id,
                "result": format!("Agent {} completed (result pending)", config.agent_id)
            }),
        );

        let output = serde_json::json!({
            "result": format!("Agent {} executed successfully", config.agent_id),
            "agent_id": config.agent_id,
            "session_id": subagent_session_id,
            "job_id": job_id,
            "context_version": context.version
        });

        Ok(output)
    }

    fn compute_capabilities(&self, _config: &AgentNodeConfig) -> Vec<Capability> {
        vec![
            Capability {
                cap_type: CapabilityType::ReadOnly,
                resource: None,
                permissions: vec!["read".to_string()],
            },
            Capability {
                cap_type: CapabilityType::Bash,
                resource: None,
                permissions: vec![
                    "grep".to_string(),
                    "glob".to_string(),
                    "read_file".to_string(),
                ],
            },
        ]
    }

    async fn execute_condition_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        context: &mut CoordinatedContext,
    ) -> Result<serde_json::Value> {
        let config: ConditionNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(ConditionNodeConfig {
                expression: "true".to_string(),
            });

        let job = JobCard {
            job_id: node.id.clone(),
            parent_job_id: None,
            task: format!("Evaluate condition: {}", config.expression),
            task_type: TaskType::Condition,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["condition_result".to_string(), "route".to_string()],
                optional_fields: vec![],
                result_type: ResultType::Object,
            },
            timeout_seconds: 5,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        let result = self.evaluate_condition(&config.expression, input, context)?;

        let output = serde_json::json!({
            "condition_result": result,
            "route": if result { "true" } else { "false" },
            "expression": config.expression,
            "context_version": context.version
        });

        context.send_message(&node.id, None, MessageType::ResultShared, output.clone());

        Ok(output)
    }

    fn evaluate_condition(
        &self,
        expression: &str,
        input: &serde_json::Value,
        context: &CoordinatedContext,
    ) -> Result<bool> {
        let mut eval_context: HashMap<String, serde_json::Value> =
            serde_json::from_value(context.shared_state.clone()).unwrap_or_default();
        eval_context.insert("input".to_string(), input.clone());

        let expr_lower = expression.to_lowercase().trim().to_string();

        if expr_lower == "true" {
            return Ok(true);
        }
        if expr_lower == "false" {
            return Ok(false);
        }

        for (key, val) in &eval_context {
            if key == expression {
                return Ok(val.as_bool().unwrap_or(false));
            }
        }

        let val_str = expression.replace("==", "=");
        let parts: Vec<&str> = val_str.split('=').collect();
        if parts.len() == 2 {
            let left = parts[0].trim();
            let right = parts[1].trim().trim_matches('"').trim_matches('\'');

            let left_val = eval_context
                .get(left)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => v.to_string(),
                })
                .unwrap_or_default();

            return Ok(left_val.trim() == right.trim());
        }

        Ok(false)
    }

    async fn execute_transform_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        context: &mut CoordinatedContext,
    ) -> Result<serde_json::Value> {
        let config: TransformNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(TransformNodeConfig {
                script: "{{input}}".to_string(),
            });

        let job = JobCard {
            job_id: node.id.clone(),
            parent_job_id: None,
            task: format!("Transform: {}", config.script),
            task_type: TaskType::Transform,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["output".to_string()],
                optional_fields: vec![],
                result_type: ResultType::Object,
            },
            timeout_seconds: 10,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        let output = self.render_template(&config.script, input, context);

        let result = serde_json::json!({
            "output": output,
            "transformed": true,
            "context_version": context.version
        });

        context.send_message(&node.id, None, MessageType::ResultShared, result.clone());

        Ok(result)
    }

    async fn execute_delay_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        context: &mut CoordinatedContext,
    ) -> Result<serde_json::Value> {
        let config: DelayNodeConfig = serde_json::from_value(node.config.clone())
            .unwrap_or(DelayNodeConfig { milliseconds: 1000 });

        let job = JobCard {
            job_id: node.id.clone(),
            parent_job_id: None,
            task: format!("Delay: {}ms", config.milliseconds),
            task_type: TaskType::Delay,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["delayed".to_string()],
                optional_fields: vec![],
                result_type: ResultType::Object,
            },
            timeout_seconds: (config.milliseconds / 1000) + 5,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        sleep(Duration::from_millis(config.milliseconds)).await;

        let output = serde_json::json!({
            "delayed": true,
            "milliseconds": config.milliseconds,
            "context_version": context.version
        });

        context.send_message(&node.id, None, MessageType::TaskCompleted, output.clone());

        Ok(output)
    }

    async fn execute_output_safe(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        context: &mut CoordinatedContext,
    ) -> Result<serde_json::Value> {
        let config: OutputNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(OutputNodeConfig {
                format: "text".to_string(),
                template: "{{input}}".to_string(),
                destination: None,
            });

        let job = JobCard {
            job_id: node.id.clone(),
            parent_job_id: None,
            task: "Format output".to_string(),
            task_type: TaskType::Output,
            input: JobInput {
                data: input.clone(),
                source_agent_id: None,
                source_job_id: None,
            },
            output_schema: OutputSchema {
                required_fields: vec!["output".to_string()],
                optional_fields: vec!["format".to_string(), "destination".to_string()],
                result_type: ResultType::Object,
            },
            timeout_seconds: 10,
            capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            metadata: HashMap::new(),
        };

        context.add_job(job);

        let output_text = self.render_template(&config.template, input, context);

        let output = serde_json::json!({
            "output": output_text,
            "format": config.format,
            "destination": config.destination,
            "context_id": context.context_id,
            "context_version": context.version,
            "job_count": context.jobs.len(),
            "completed_jobs": context.get_all_completed_results().len()
        });

        context.send_message(&node.id, None, MessageType::TaskCompleted, output.clone());

        Ok(output)
    }

    fn render_template(
        &self,
        template: &str,
        input: &serde_json::Value,
        context: &CoordinatedContext,
    ) -> String {
        let mut result = template.to_string();

        let mut context_map: HashMap<String, String> = HashMap::new();

        context_map.insert("input".to_string(), input.to_string());

        if let Some(obj) = input.as_object() {
            for (k, v) in obj {
                context_map.insert(format!("input.{}", k), v.to_string());
            }
        }

        let shared: HashMap<String, serde_json::Value> =
            serde_json::from_value(context.shared_state.clone()).unwrap_or_default();
        for (key, val) in shared {
            let val_str = match val {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "null".to_string(),
                _ => val.to_string(),
            };
            context_map.insert(key, val_str);
        }

        for (key, val_str) in &context_map {
            let pattern = format!("{{{{{}}}}}", key);
            result = result.replace(&pattern, val_str);
        }

        result = result.replace("{{input}}", &input.to_string());

        result
    }

    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        self.db
            .update_run_status(run_id, "cancelled", Some("Cancelled by user"))?;
        Ok(())
    }
}
