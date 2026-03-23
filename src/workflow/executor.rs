use crate::agent::subagent_manager::SubagentManager;
use crate::error::{OSAgentError, Result};
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::db::WorkflowDb;
use crate::workflow::events::WorkflowEvent;
use crate::workflow::graph::{topological_sort, GraphValidator};
use crate::workflow::types::*;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

pub struct WorkflowExecutor {
    db: Arc<WorkflowDb>,
    artifact_store: Arc<ArtifactStore>,
    subagent_manager: Arc<SubagentManager>,
    event_tx: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowExecutor {
    pub fn new(
        db: Arc<WorkflowDb>,
        artifact_store: Arc<ArtifactStore>,
        subagent_manager: Arc<SubagentManager>,
    ) -> (Self, broadcast::Receiver<WorkflowEvent>) {
        let (event_tx, event_rx) = broadcast::channel(100);
        (
            Self {
                db,
                artifact_store,
                subagent_manager,
                event_tx,
            },
            event_rx,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    pub async fn execute_workflow(
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

        let mut shared_state: HashMap<String, serde_json::Value> = parameters;
        let mut chat_context = initial_context.unwrap_or(ChatContext {
            messages: Vec::new(),
            metadata: HashMap::new(),
        });

        let mut node_output_map: HashMap<String, serde_json::Value> = HashMap::new();
        let mut node_log_ids: HashMap<String, String> = HashMap::new();

        for node_id in &node_order {
            let node = graph
                .nodes
                .iter()
                .find(|n| &n.id == node_id)
                .ok_or_else(|| OSAgentError::Workflow(format!("Node not found: {}", node_id)))?;

            let log_id = Uuid::new_v4().to_string();
            node_log_ids.insert(node_id.clone(), log_id.clone());

            let node_log = NodeLog {
                id: log_id.clone(),
                run_id: run_id.clone(),
                node_id: node.id.clone(),
                node_type: node.node_type.as_str().to_string(),
                status: "started".to_string(),
                input_json: Some(serde_json::to_string(&shared_state).unwrap_or_default()),
                output_json: None,
                started_at: Utc::now().to_rfc3339(),
                completed_at: None,
            };

            let _ = self.db.create_node_log(&node_log);

            let _ = self.event_tx.send(WorkflowEvent::node_started(
                &run_id,
                workflow_id,
                &node.id,
                node.node_type.as_str(),
            ));

            let input = node_output_map
                .get(node_id)
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let output = match self
                .execute_node(
                    node,
                    &input,
                    &mut shared_state,
                    &mut chat_context,
                    parent_session_id.as_deref(),
                )
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    let _ = self.db.update_node_log(&log_id, "failed", None);
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

            node_output_map.insert(node.id.clone(), output.clone());

            let output_json = serde_json::to_string(&output).unwrap_or_default();
            let _ = self
                .db
                .update_node_log(&log_id, "completed", Some(&output_json));

            let _ = self.event_tx.send(WorkflowEvent::node_completed(
                &run_id,
                workflow_id,
                &node.id,
                node.node_type.as_str(),
                Some(output.clone()),
            ));
        }

        let final_output = node_output_map
            .values()
            .last()
            .cloned()
            .unwrap_or(serde_json::Value::Null);

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

    async fn execute_node(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
        chat_context: &mut ChatContext,
        parent_session_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        match node.node_type {
            NodeType::Trigger => {
                self.execute_trigger(node, input, shared_state, chat_context)
                    .await
            }
            NodeType::Agent => {
                self.execute_agent(node, input, shared_state, chat_context, parent_session_id)
                    .await
            }
            NodeType::Condition => self.execute_condition(node, input, shared_state).await,
            NodeType::Transform => self.execute_transform(node, input, shared_state).await,
            NodeType::Delay => self.execute_delay(node, input).await,
            NodeType::Output => self.execute_output(node, input, shared_state).await,
        }
    }

    async fn execute_trigger(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
        _chat_context: &mut ChatContext,
    ) -> Result<serde_json::Value> {
        shared_state.insert(
            "trigger_node_id".to_string(),
            serde_json::Value::String(node.id.clone()),
        );
        shared_state.insert("trigger_output".to_string(), input.clone());

        Ok(serde_json::json!({
            "triggered": true,
            "node_id": node.id
        }))
    }

    async fn execute_agent(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
        chat_context: &mut ChatContext,
        parent_session_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let config: AgentNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(AgentNodeConfig {
                agent_id: "main".to_string(),
                system_prompt: None,
                task_template: "{{input}}".to_string(),
                input_mapping: HashMap::new(),
            });

        let task = self.render_template(&config.task_template, input, shared_state);

        let _system_prompt = config.system_prompt.unwrap_or_else(|| {
            r#"You are a specialized subagent focused on completing specific tasks efficiently.

You have access to tools but should:
- Focus on the specific task given
- Use tools efficiently
- Report progress clearly
- Complete the task in as few steps as possible

You cannot spawn additional subagents."#
                .to_string()
        });

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

        let result = self.wait_for_subagent(&subagent_session_id).await?;

        chat_context.messages.push(Message {
            role: "user".to_string(),
            content: task,
            tool_calls: None,
            tool_call_id: None,
        });
        chat_context.messages.push(Message {
            role: "assistant".to_string(),
            content: result.clone(),
            tool_calls: None,
            tool_call_id: None,
        });

        let output = serde_json::json!({
            "result": result,
            "agent_id": config.agent_id,
            "session_id": subagent_session_id
        });

        shared_state.insert(
            format!("agent_{}_result", node.id).to_string(),
            output.clone(),
        );

        Ok(output)
    }

    async fn wait_for_subagent(&self, session_id: &str) -> Result<String> {
        let (status, result) = self
            .subagent_manager
            .wait_for_subagent(session_id, 300)
            .await?;
        Ok(result)
    }

    async fn execute_condition(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let config: ConditionNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(ConditionNodeConfig {
                expression: "true".to_string(),
            });

        let result = self.evaluate_condition(&config.expression, input, shared_state)?;

        Ok(serde_json::json!({
            "condition_result": result,
            "route": if result { "true" } else { "false" },
            "expression": config.expression
        }))
    }

    fn evaluate_condition(
        &self,
        expression: &str,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> Result<bool> {
        let mut eval_context = shared_state.clone();
        eval_context.insert("input".to_string(), input.clone());

        let expr_lower = expression.to_lowercase().trim().to_string();

        if expr_lower == "true" {
            return Ok(true);
        }
        if expr_lower == "false" {
            return Ok(false);
        }

        if let Some(val) = eval_context.get(
            &expression
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>(),
        ) {
            return Ok(val.as_bool().unwrap_or(false));
        }

        for (key, val) in &eval_context {
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "null".to_string(),
                _ => val.to_string(),
            };

            let pattern = format!("{{{{{}}}}}", key);
            if expression.contains(&pattern) {
                let new_expr = expression.replace(&pattern, &val_str);
                if new_expr.to_lowercase() == "true" {
                    return Ok(true);
                }
                if new_expr.to_lowercase() == "false" {
                    return Ok(false);
                }
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

    async fn execute_transform(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let config: TransformNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(TransformNodeConfig {
                script: "{{input}}".to_string(),
            });

        let output = self.render_template(&config.script, input, shared_state);

        Ok(serde_json::json!({
            "output": output,
            "transformed": true
        }))
    }

    async fn execute_delay(
        &self,
        node: &WorkflowNode,
        _input: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let config: DelayNodeConfig = serde_json::from_value(node.config.clone())
            .unwrap_or(DelayNodeConfig { milliseconds: 1000 });

        sleep(Duration::from_millis(config.milliseconds)).await;

        Ok(serde_json::json!({
            "delayed": true,
            "milliseconds": config.milliseconds
        }))
    }

    async fn execute_output(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let config: OutputNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(OutputNodeConfig {
                format: "text".to_string(),
                template: "{{input}}".to_string(),
                destination: None,
            });

        let output = self.render_template(&config.template, input, shared_state);

        Ok(serde_json::json!({
            "output": output,
            "format": config.format,
            "destination": config.destination
        }))
    }

    fn render_template(
        &self,
        template: &str,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> String {
        let mut result = template.to_string();

        let mut context: HashMap<String, String> = HashMap::new();

        context.insert("input".to_string(), input.to_string());

        if let Some(obj) = input.as_object() {
            for (k, v) in obj {
                context.insert(format!("input.{}", k), v.to_string());
            }
        }

        for (key, val) in shared_state {
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "null".to_string(),
                _ => val.to_string(),
            };
            context.insert(key.clone(), val_str);
        }

        for (key, val_str) in &context {
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
