use crate::agent::events::{generate_question_id, AgentEvent, EventBus, QuestionChannel};
use crate::agent::subagent_manager::SubagentManager;
use crate::error::{OSAgentError, Result};
use crate::tools::question::{Question, QuestionOption};
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::db::WorkflowDb;
use crate::workflow::events::WorkflowEvent;
use crate::workflow::graph::{parse_litegraph_json, topological_sort, GraphValidator};
use crate::workflow::types::*;
use base64::Engine;
use chrono::Utc;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{broadcast, oneshot};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

const MAX_ATTACHMENT_BYTES: usize = 12 * 1024 * 1024;
const MAX_ATTACHMENT_TEXT_CHARS: usize = 24_000;

#[derive(Debug, Clone)]
struct ProcessedAttachment {
    filename: String,
    mime: String,
    data_url: String,
    text_content: Option<String>,
    size_bytes: usize,
}

pub struct WorkflowExecutor {
    db: Arc<WorkflowDb>,
    artifact_store: Arc<ArtifactStore>,
    subagent_manager: Arc<SubagentManager>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    event_bus: EventBus,
}

impl WorkflowExecutor {
    pub fn new(
        db: Arc<WorkflowDb>,
        artifact_store: Arc<ArtifactStore>,
        subagent_manager: Arc<SubagentManager>,
        event_bus: EventBus,
    ) -> (Self, broadcast::Receiver<WorkflowEvent>) {
        let (event_tx, event_rx) = broadcast::channel(100);
        (
            Self {
                db,
                artifact_store,
                subagent_manager,
                event_tx,
                event_bus,
            },
            event_rx,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_workflow(
        &self,
        workflow_id: &str,
        workflow_name: &str,
        graph_json: &str,
        version: i32,
        initial_context: Option<ChatContext>,
        parameters: HashMap<String, serde_json::Value>,
        parent_session_id: Option<String>,
        attachments: Vec<WorkflowAttachment>,
        images: Vec<WorkflowAttachment>,
        source: Option<String>,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
    ) -> Result<WorkflowResult> {
        let run_id = Uuid::new_v4().to_string();
        let event_session_id = parent_session_id.clone().unwrap_or_else(|| run_id.clone());

        let notify_channels = if notify_channels.is_empty() {
            vec!["web".to_string()]
        } else {
            notify_channels
        };

        let graph: WorkflowGraph = match parse_litegraph_json(graph_json) {
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
        self.event_bus.emit(AgentEvent::WorkflowStarted {
            session_id: event_session_id.clone(),
            sequence: 0,
            workflow_id: workflow_id.to_string(),
            workflow_name: workflow_name.to_string(),
            run_id: run_id.clone(),
            source,
            notify_channels: notify_channels.clone(),
            discord_channel_id,
            timestamp: SystemTime::now(),
        });

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
                self.event_bus.emit(AgentEvent::WorkflowFailed {
                    session_id: event_session_id,
                    sequence: 0,
                    workflow_id: workflow_id.to_string(),
                    run_id: run_id.clone(),
                    error: e.to_string(),
                    notify_channels,
                    discord_channel_id,
                    timestamp: SystemTime::now(),
                });

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
        let processed_attachments = self.process_attachments(&attachments, &images)?;
        if !processed_attachments.is_empty() {
            let attachment_meta: Vec<serde_json::Value> = processed_attachments
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "filename": a.filename,
                        "mime": a.mime,
                        "size_bytes": a.size_bytes,
                        "has_text": a.text_content.is_some(),
                    })
                })
                .collect();
            shared_state.insert(
                "trigger_attachments".to_string(),
                serde_json::Value::Array(attachment_meta),
            );

            let attachment_contents: Vec<serde_json::Value> = processed_attachments
                .iter()
                .enumerate()
                .map(|(index, attachment)| {
                    serde_json::json!({
                        "index": index,
                        "filename": attachment.filename,
                        "mime": attachment.mime,
                        "content": attachment.text_content,
                    })
                })
                .collect();
            shared_state.insert(
                "trigger_attachment_contents".to_string(),
                serde_json::Value::Array(attachment_contents),
            );
        }

        let mut node_output_map: HashMap<String, serde_json::Value> = HashMap::new();
        let mut condition_routes: HashMap<String, String> = HashMap::new();
        let mut last_output: Option<serde_json::Value> = None;

        for node_id in &node_order {
            let node = graph
                .nodes
                .iter()
                .find(|n| &n.id == node_id)
                .ok_or_else(|| OSAgentError::Workflow(format!("Node not found: {}", node_id)))?;

            let incoming_edges: Vec<&WorkflowEdge> = graph
                .edges
                .iter()
                .filter(|edge| edge.target_node_id == node.id)
                .collect();

            let active_incoming: Vec<&WorkflowEdge> = incoming_edges
                .iter()
                .copied()
                .filter(|edge| self.is_edge_active(edge, &condition_routes, &node_output_map))
                .collect();

            let log_id = Uuid::new_v4().to_string();
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
            self.event_bus.emit(AgentEvent::WorkflowNodeStarted {
                session_id: event_session_id.clone(),
                sequence: 0,
                workflow_id: workflow_id.to_string(),
                run_id: run_id.clone(),
                node_id: node.id.clone(),
                node_type: node.node_type.as_str().to_string(),
                timestamp: SystemTime::now(),
            });

            if !incoming_edges.is_empty() && active_incoming.is_empty() {
                let skipped_output = serde_json::json!({
                    "skipped": true,
                    "reason": "No active input path"
                });
                let _ = self.db.update_node_log(
                    &log_id,
                    "completed",
                    Some(&serde_json::to_string(&skipped_output).unwrap_or_default()),
                );

                let _ = self.event_tx.send(WorkflowEvent::node_completed(
                    &run_id,
                    workflow_id,
                    &node.id,
                    node.node_type.as_str(),
                    Some(skipped_output.clone()),
                ));
                self.event_bus.emit(AgentEvent::WorkflowNodeCompleted {
                    session_id: event_session_id.clone(),
                    sequence: 0,
                    workflow_id: workflow_id.to_string(),
                    run_id: run_id.clone(),
                    node_id: node.id.clone(),
                    node_type: node.node_type.as_str().to_string(),
                    output_preview: Some("skipped".to_string()),
                    timestamp: SystemTime::now(),
                });

                continue;
            }

            let input = if active_incoming.is_empty() {
                serde_json::Value::Null
            } else if active_incoming.len() == 1 {
                node_output_map
                    .get(&active_incoming[0].source_node_id)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let mut by_node = serde_json::Map::new();
                let mut inputs = Vec::new();

                for edge in active_incoming {
                    let value = node_output_map
                        .get(&edge.source_node_id)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    by_node.insert(edge.source_node_id.clone(), value.clone());
                    inputs.push(value);
                }

                serde_json::json!({
                    "inputs": inputs,
                    "by_node": by_node,
                })
            };

            let output = match self
                .execute_node(
                    node,
                    &input,
                    &mut shared_state,
                    &mut chat_context,
                    parent_session_id.as_deref(),
                    &processed_attachments,
                    workflow_id,
                    &run_id,
                    &event_session_id,
                    &notify_channels,
                    discord_channel_id,
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
                    self.event_bus.emit(AgentEvent::WorkflowNodeFailed {
                        session_id: event_session_id.clone(),
                        sequence: 0,
                        workflow_id: workflow_id.to_string(),
                        run_id: run_id.clone(),
                        node_id: node.id.clone(),
                        node_type: node.node_type.as_str().to_string(),
                        error: e.to_string(),
                        timestamp: SystemTime::now(),
                    });

                    let _ = self
                        .db
                        .update_run_status(&run_id, "failed", Some(&e.to_string()));
                    let _ = self.event_tx.send(WorkflowEvent::workflow_run_failed(
                        &run_id,
                        workflow_id,
                        &e.to_string(),
                    ));
                    self.event_bus.emit(AgentEvent::WorkflowFailed {
                        session_id: event_session_id,
                        sequence: 0,
                        workflow_id: workflow_id.to_string(),
                        run_id: run_id.clone(),
                        error: e.to_string(),
                        notify_channels,
                        discord_channel_id,
                        timestamp: SystemTime::now(),
                    });

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

            if matches!(node.node_type, NodeType::Condition | NodeType::Approval) {
                if let Some(route) = output.get("route").and_then(|v| v.as_str()) {
                    condition_routes.insert(node.id.clone(), route.to_string());
                }
            }

            node_output_map.insert(node.id.clone(), output.clone());
            last_output = Some(output.clone());

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
            self.event_bus.emit(AgentEvent::WorkflowNodeCompleted {
                session_id: event_session_id.clone(),
                sequence: 0,
                workflow_id: workflow_id.to_string(),
                run_id: run_id.clone(),
                node_id: node.id.clone(),
                node_type: node.node_type.as_str().to_string(),
                output_preview: Some(Self::output_preview(&output)),
                timestamp: SystemTime::now(),
            });
        }

        let final_output = last_output.unwrap_or(serde_json::Value::Null);

        let _ = self.db.update_run_status(&run_id, "completed", None);
        let _ = self.event_tx.send(WorkflowEvent::workflow_run_completed(
            &run_id,
            workflow_id,
            "completed",
            Some(final_output.clone()),
        ));
        self.event_bus.emit(AgentEvent::WorkflowCompleted {
            session_id: event_session_id,
            sequence: 0,
            workflow_id: workflow_id.to_string(),
            run_id: run_id.clone(),
            output: Some(final_output.clone()),
            notify_channels,
            discord_channel_id,
            timestamp: SystemTime::now(),
        });

        Ok(WorkflowResult {
            run_id,
            status: "completed".to_string(),
            output: Some(final_output),
            error: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_node(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
        chat_context: &mut ChatContext,
        parent_session_id: Option<&str>,
        attachments: &[ProcessedAttachment],
        workflow_id: &str,
        run_id: &str,
        event_session_id: &str,
        notify_channels: &[String],
        discord_channel_id: Option<u64>,
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
            NodeType::FileInput => self.execute_file_input(node, attachments).await,
            NodeType::FileOutput => self.execute_file_output(node, input, shared_state).await,
            NodeType::Approval => {
                self.execute_approval(
                    node,
                    input,
                    workflow_id,
                    run_id,
                    event_session_id,
                    notify_channels,
                    discord_channel_id,
                )
                .await
            }
            NodeType::ForEach => self.execute_foreach(node, input, shared_state).await,
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
                file_context: None,
            });

        let mut task = self.render_template(&config.task_template, input, shared_state);

        if let Some(file_context_template) = config.file_context.as_deref() {
            let file_context = self.render_template(file_context_template, input, shared_state);
            if !file_context.trim().is_empty() {
                task = format!("File context:\n{}\n\nTask:\n{}", file_context, task);
            }
        }

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
        let (_status, result, _tool_count) = self
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

        if expr_lower.starts_with("contains(") && expression.trim().ends_with(')') {
            let inner = &expression["contains(".len()..expression.len().saturating_sub(1)];
            if let Some((left_raw, right_raw)) = inner.split_once(',') {
                let left = self.resolve_condition_value(left_raw.trim(), input, shared_state);
                let right = self.resolve_condition_value(right_raw.trim(), input, shared_state);
                return Ok(left.contains(&right));
            }
        }

        if expr_lower.starts_with("not_contains(") && expression.trim().ends_with(')') {
            let inner = &expression["not_contains(".len()..expression.len().saturating_sub(1)];
            if let Some((left_raw, right_raw)) = inner.split_once(',') {
                let left = self.resolve_condition_value(left_raw.trim(), input, shared_state);
                let right = self.resolve_condition_value(right_raw.trim(), input, shared_state);
                return Ok(!left.contains(&right));
            }
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

        let normalized = expression.replace("==", "=");

        if normalized.contains("!=") {
            let parts: Vec<&str> = normalized.split("!=").collect();
            if parts.len() == 2 {
                let left = self.resolve_condition_value(parts[0].trim(), input, shared_state);
                let right = self.resolve_condition_value(parts[1].trim(), input, shared_state);
                return Ok(left.trim() != right.trim());
            }
        }

        let parts: Vec<&str> = normalized.split('=').collect();
        if parts.len() == 2 {
            let left = self.resolve_condition_value(parts[0].trim(), input, shared_state);
            let right = self.resolve_condition_value(parts[1].trim(), input, shared_state);
            return Ok(left.trim() == right.trim());
        }

        Ok(false)
    }

    fn resolve_condition_value(
        &self,
        raw: &str,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> String {
        let trimmed = raw.trim();
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            return trimmed[1..trimmed.len().saturating_sub(1)].to_string();
        }

        if trimmed.contains("{{") {
            return self.render_template(trimmed, input, shared_state);
        }

        if trimmed == "input" {
            return input.to_string();
        }

        if let Some(value) = shared_state.get(trimmed) {
            return match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "null".to_string(),
                _ => value.to_string(),
            };
        }

        trimmed.to_string()
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

    async fn execute_file_input(
        &self,
        node: &WorkflowNode,
        attachments: &[ProcessedAttachment],
    ) -> Result<serde_json::Value> {
        let config: FileInputNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(FileInputNodeConfig {
                path: None,
                use_attachment: true,
                attachment_index: Some(0),
            });

        if config.use_attachment || config.path.is_none() {
            let index = config.attachment_index.unwrap_or(0);
            let attachment = attachments.get(index).ok_or_else(|| {
                OSAgentError::Workflow(format!("No attachment available at index {}", index))
            })?;

            return Ok(serde_json::json!({
                "content": attachment.text_content,
                "metadata": {
                    "filename": attachment.filename,
                    "mime": attachment.mime,
                    "size_bytes": attachment.size_bytes,
                },
                "data_url": attachment.data_url,
            }));
        }

        let path = config.path.unwrap_or_default();
        if path.trim().is_empty() {
            return Err(OSAgentError::Workflow(
                "FileInput node must specify a path or attachment source".to_string(),
            ));
        }

        let bytes = tokio::fs::read(&path).await.map_err(|e| {
            OSAgentError::Workflow(format!("Failed to read file '{}': {}", path, e))
        })?;

        let content = String::from_utf8_lossy(&bytes).to_string();
        let metadata = tokio::fs::metadata(&path).await.ok();

        Ok(serde_json::json!({
            "content": content,
            "metadata": {
                "path": path,
                "size_bytes": metadata.map(|m| m.len()).unwrap_or(bytes.len() as u64),
            },
            "raw_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        }))
    }

    async fn execute_file_output(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let config: FileOutputNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(FileOutputNodeConfig {
                path: String::new(),
                content_template: "{{input}}".to_string(),
                create_dirs: true,
            });

        if config.path.trim().is_empty() {
            return Err(OSAgentError::Workflow(
                "FileOutput node requires a non-empty path".to_string(),
            ));
        }

        if config.create_dirs {
            if let Some(parent) = Path::new(&config.path).parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        OSAgentError::Workflow(format!(
                            "Failed to create output directory '{}': {}",
                            parent.display(),
                            e
                        ))
                    })?;
                }
            }
        }

        let content = self.render_template(&config.content_template, input, shared_state);
        tokio::fs::write(&config.path, content.as_bytes())
            .await
            .map_err(|e| {
                OSAgentError::Workflow(format!("Failed to write file '{}': {}", config.path, e))
            })?;

        Ok(serde_json::json!({
            "path": config.path,
            "bytes_written": content.len(),
            "written": true,
        }))
    }

    async fn execute_approval(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        workflow_id: &str,
        run_id: &str,
        event_session_id: &str,
        notify_channels: &[String],
        discord_channel_id: Option<u64>,
    ) -> Result<serde_json::Value> {
        let config: ApprovalNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(ApprovalNodeConfig {
                prompt: "Approve workflow step?".to_string(),
                approve_label: "Approve".to_string(),
                reject_label: "Reject".to_string(),
            });

        let question_id = generate_question_id();
        let (response_tx, response_rx) = oneshot::channel::<Vec<Vec<String>>>();
        let questions = vec![Question {
            question: config.prompt.clone(),
            header: "Approval Required".to_string(),
            options: vec![
                QuestionOption {
                    label: config.approve_label.clone(),
                    description: "Continue workflow".to_string(),
                },
                QuestionOption {
                    label: config.reject_label.clone(),
                    description: "Stop or branch workflow".to_string(),
                },
            ],
            multiple: false,
        }];

        self.event_bus
            .register_question(
                event_session_id.to_string(),
                QuestionChannel {
                    question_id: question_id.clone(),
                    questions: questions.clone(),
                    response_tx,
                },
            )
            .await;

        self.event_bus.emit(AgentEvent::WorkflowApprovalRequested {
            session_id: event_session_id.to_string(),
            sequence: 0,
            workflow_id: workflow_id.to_string(),
            run_id: run_id.to_string(),
            node_id: node.id.clone(),
            question_id,
            prompt: config.prompt.clone(),
            approve_label: config.approve_label.clone(),
            reject_label: config.reject_label.clone(),
            notify_channels: notify_channels.to_vec(),
            discord_channel_id,
            timestamp: SystemTime::now(),
        });

        let answers = response_rx.await.map_err(|_| {
            OSAgentError::Workflow(
                "Approval request expired before receiving an answer".to_string(),
            )
        })?;
        let answer = answers
            .first()
            .and_then(|row| row.first())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let approve_label = config.approve_label.to_lowercase();
        let approved =
            answer == approve_label || matches!(answer.as_str(), "approve" | "1" | "yes");

        Ok(serde_json::json!({
            "approved": approved,
            "answer": answer,
            "route": if approved { "true" } else { "false" },
            "input": input,
        }))
    }

    async fn execute_foreach(
        &self,
        node: &WorkflowNode,
        input: &serde_json::Value,
        shared_state: &mut HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let config: ForEachNodeConfig =
            serde_json::from_value(node.config.clone()).unwrap_or(ForEachNodeConfig {
                items_template: "{{input}}".to_string(),
                item_variable: "item".to_string(),
            });

        let rendered = self.render_template(&config.items_template, input, shared_state);
        let items: Vec<serde_json::Value> =
            match serde_json::from_str::<serde_json::Value>(&rendered) {
                Ok(value) => value.as_array().cloned().unwrap_or_default(),
                Err(_) => input.as_array().cloned().unwrap_or_default(),
            };

        let mut results = Vec::new();
        for (index, item) in items.iter().enumerate() {
            shared_state.insert(config.item_variable.clone(), item.clone());
            results.push(serde_json::json!({
                "index": index,
                "item": item,
            }));
        }
        shared_state.remove(&config.item_variable);

        Ok(serde_json::json!({
            "items_processed": items.len(),
            "results": results,
        }))
    }

    fn render_template(
        &self,
        template: &str,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> String {
        let re = regex::Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").expect("valid template regex");
        re.replace_all(template, |caps: &regex::Captures<'_>| {
            let path = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            if let Some(value) = Self::lookup_template_value(path, input, shared_state) {
                Self::template_value_to_string(&value)
            } else {
                caps.get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .to_string()
    }

    fn lookup_template_value(
        path: &str,
        input: &serde_json::Value,
        shared_state: &HashMap<String, serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return None;
        }

        if trimmed == "input" {
            return Some(input.clone());
        }

        if let Some(rest) = trimmed.strip_prefix("input.") {
            return Self::lookup_path(input, rest).cloned();
        }

        if let Some(value) = shared_state.get(trimmed) {
            return Some(value.clone());
        }

        if let Some((root, rest)) = trimmed.split_once('.') {
            if let Some(value) = shared_state.get(root) {
                return Self::lookup_path(value, rest).cloned();
            }
        }

        None
    }

    fn lookup_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let mut current = value;
        for segment in path.split('.') {
            if segment.is_empty() {
                return None;
            }

            if let Ok(index) = segment.parse::<usize>() {
                current = current.as_array()?.get(index)?;
            } else {
                current = current.as_object()?.get(segment)?;
            }
        }
        Some(current)
    }

    fn template_value_to_string(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => "null".to_string(),
            _ => value.to_string(),
        }
    }

    fn process_attachments(
        &self,
        attachments: &[WorkflowAttachment],
        images: &[WorkflowAttachment],
    ) -> Result<Vec<ProcessedAttachment>> {
        let mut processed = Vec::new();

        for attachment in attachments.iter().chain(images.iter()) {
            let (mime, bytes) = Self::parse_data_url(&attachment.data_url).map_err(|e| {
                OSAgentError::Workflow(format!(
                    "Invalid attachment '{}': {}",
                    attachment.filename, e
                ))
            })?;

            if bytes.len() > MAX_ATTACHMENT_BYTES {
                return Err(OSAgentError::Workflow(format!(
                    "Attachment '{}' exceeds {} bytes",
                    attachment.filename, MAX_ATTACHMENT_BYTES
                )));
            }

            let mime = if attachment.mime.trim().is_empty() {
                mime
            } else {
                attachment.mime.clone()
            };

            let text_content = if Self::is_pdf_attachment(&mime, &attachment.filename) {
                let extracted = pdf_extract::extract_text_from_mem(&bytes).unwrap_or_default();
                Some(Self::truncate_chars(&extracted, MAX_ATTACHMENT_TEXT_CHARS).0)
            } else if Self::is_text_attachment(&mime, &attachment.filename) {
                let extracted = String::from_utf8_lossy(&bytes).to_string();
                Some(Self::truncate_chars(&extracted, MAX_ATTACHMENT_TEXT_CHARS).0)
            } else {
                None
            };

            processed.push(ProcessedAttachment {
                filename: attachment.filename.clone(),
                mime,
                data_url: attachment.data_url.clone(),
                text_content,
                size_bytes: bytes.len(),
            });
        }

        Ok(processed)
    }

    fn parse_data_url(data_url: &str) -> std::result::Result<(String, Vec<u8>), String> {
        let Some((meta, payload)) = data_url.split_once(',') else {
            return Err("Invalid data URL".to_string());
        };
        if !meta.starts_with("data:") || !meta.contains(";base64") {
            return Err("Unsupported attachment encoding".to_string());
        }

        let mime = meta
            .trim_start_matches("data:")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|_| "Invalid base64 payload".to_string())?;

        Ok((mime, bytes))
    }

    fn attachment_extension(filename: &str) -> String {
        std::path::Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }

    fn is_pdf_attachment(mime: &str, filename: &str) -> bool {
        mime.eq_ignore_ascii_case("application/pdf")
            || Self::attachment_extension(filename) == "pdf"
    }

    fn is_text_attachment(mime: &str, filename: &str) -> bool {
        if mime.starts_with("text/") {
            return true;
        }

        matches!(
            Self::attachment_extension(filename).as_str(),
            "txt"
                | "md"
                | "markdown"
                | "json"
                | "csv"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "rs"
                | "py"
                | "html"
                | "css"
                | "toml"
                | "yaml"
                | "yml"
                | "xml"
                | "sql"
                | "sh"
                | "ps1"
                | "bat"
                | "ini"
                | "log"
        )
    }

    fn truncate_chars(value: &str, limit: usize) -> (String, bool) {
        let char_count = value.chars().count();
        if char_count <= limit {
            return (value.to_string(), false);
        }

        let truncated = value.chars().take(limit).collect::<String>();
        (truncated, true)
    }

    fn is_edge_active(
        &self,
        edge: &WorkflowEdge,
        condition_routes: &HashMap<String, String>,
        node_output_map: &HashMap<String, serde_json::Value>,
    ) -> bool {
        if let Some(route) = condition_routes.get(&edge.source_node_id) {
            return Self::port_matches_route(&edge.source_port, route);
        }

        node_output_map.contains_key(&edge.source_node_id)
    }

    fn port_matches_route(source_port: &str, route: &str) -> bool {
        let port = source_port.trim().to_ascii_lowercase();
        match route {
            "true" => matches!(port.as_str(), "0" | "true" | "output"),
            "false" => matches!(port.as_str(), "1" | "false"),
            _ => true,
        }
    }

    fn output_preview(output: &serde_json::Value) -> String {
        let mut preview = if let Some(text) = output.as_str() {
            text.to_string()
        } else {
            serde_json::to_string(output).unwrap_or_default()
        };

        if preview.chars().count() > 220 {
            preview = preview.chars().take(220).collect::<String>() + "...";
        }

        preview
    }

    pub async fn cancel_run(&self, run_id: &str) -> Result<()> {
        self.db
            .update_run_status(run_id, "cancelled", Some("Cancelled by user"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::WorkflowExecutor;
    use crate::agent::events::AgentEvent;
    use crate::agent::runtime::AgentRuntime;
    use crate::config::{Config, WorkspacePath, WorkspacePermission};
    use crate::workflow::artifact_store::ArtifactStore;
    use crate::workflow::db::WorkflowDb;
    use crate::workflow::graph::to_litegraph_json;
    use crate::workflow::types::{
        NodeType, OutputNodeConfig, Position, Workflow, WorkflowAttachment, WorkflowEdge,
        WorkflowGraph, WorkflowNode, WorkflowVersion,
    };
    use base64::Engine;
    use chrono::Utc;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::time::{timeout, Duration};

    fn test_config(temp_root: &Path) -> Config {
        let workspace = temp_root.join("workspace");
        let database = temp_root.join("osagent.db");

        std::fs::create_dir_all(&workspace).expect("create workspace dir");

        let mut config = Config::default_config();
        config.search.enabled = false;
        config.storage.database = database.to_string_lossy().to_string();
        config.agent.workspace = workspace.to_string_lossy().to_string();
        config.agent.active_workspace = Some("default".to_string());

        if let Some(default_workspace) = config.agent.workspaces.get_mut(0) {
            default_workspace.path = workspace.to_string_lossy().to_string();
            default_workspace.paths = vec![WorkspacePath {
                path: workspace.to_string_lossy().to_string(),
                permission: WorkspacePermission::ReadWrite,
                description: Some("Workflow tests workspace".to_string()),
            }];
        }

        config
    }

    fn build_executor(temp_root: &Path) -> (Arc<WorkflowExecutor>, Arc<WorkflowDb>) {
        let config = test_config(temp_root);
        let runtime = Arc::new(AgentRuntime::new(config).expect("create runtime"));

        let workflow_db_path = temp_root.join("workflow.db");
        let workflow_db = Arc::new(WorkflowDb::new(workflow_db_path));
        workflow_db.init_tables().expect("init workflow tables");

        let artifact_store = Arc::new(ArtifactStore::new(temp_root.join("workflow_artifacts")));
        artifact_store.init().expect("init artifact store");

        let (executor, _event_rx) = WorkflowExecutor::new(
            workflow_db.clone(),
            artifact_store,
            runtime.get_subagent_manager(),
            runtime.event_bus().clone(),
        );

        (Arc::new(executor), workflow_db)
    }

    fn node(id: &str, node_type: NodeType, config: serde_json::Value) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            node_type,
            position: Position { x: 0.0, y: 0.0 },
            config,
        }
    }

    fn seed_workflow(db: &WorkflowDb, workflow_id: &str, name: &str, graph_json: &str) {
        let now = Utc::now().to_rfc3339();
        db.create_workflow(&Workflow {
            id: workflow_id.to_string(),
            name: name.to_string(),
            description: Some("test workflow".to_string()),
            default_workspace_id: None,
            current_version: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
        })
        .expect("create test workflow");

        db.create_version(&WorkflowVersion {
            id: format!("{}_v1", workflow_id),
            workflow_id: workflow_id.to_string(),
            version: 1,
            graph_json: graph_json.to_string(),
            created_at: now,
        })
        .expect("create test workflow version");
    }

    #[tokio::test]
    async fn executes_attachment_file_input_end_to_end() {
        let temp_dir = tempdir().expect("temp dir");
        let (executor, workflow_db) = build_executor(temp_dir.path());

        let graph = WorkflowGraph {
            nodes: vec![
                node("trigger_1", NodeType::Trigger, json!({})),
                node(
                    "file_input_1",
                    NodeType::FileInput,
                    json!({ "use_attachment": true, "attachment_index": 0 }),
                ),
                node(
                    "output_1",
                    NodeType::Output,
                    serde_json::to_value(OutputNodeConfig {
                        format: "text".to_string(),
                        template: "{{input.content}}".to_string(),
                        destination: None,
                    })
                    .expect("output config"),
                ),
            ],
            edges: vec![
                WorkflowEdge {
                    id: "e1".to_string(),
                    source_node_id: "trigger_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "file_input_1".to_string(),
                    target_port: "0".to_string(),
                },
                WorkflowEdge {
                    id: "e2".to_string(),
                    source_node_id: "file_input_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "output_1".to_string(),
                    target_port: "0".to_string(),
                },
            ],
        };

        let graph_json = to_litegraph_json(&graph).expect("graph json");
        seed_workflow(
            workflow_db.as_ref(),
            "wf_attachment",
            "Attachment Workflow",
            &graph_json,
        );
        let content = "hello workflow attachment";
        let data_url = format!(
            "data:text/plain;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(content)
        );

        let result = executor
            .execute_workflow(
                "wf_attachment",
                "Attachment Workflow",
                &graph_json,
                1,
                None,
                HashMap::new(),
                None,
                vec![WorkflowAttachment {
                    filename: "notes.txt".to_string(),
                    mime: "text/plain".to_string(),
                    data_url,
                }],
                vec![],
                Some("web".to_string()),
                vec!["web".to_string()],
                None,
            )
            .await
            .expect("execute workflow");

        assert_eq!(result.status, "completed");
        let output_text = result
            .output
            .and_then(|value| value.get("output").cloned())
            .and_then(|value| value.as_str().map(|s| s.to_string()))
            .expect("output text");
        assert_eq!(output_text, content);
    }

    #[tokio::test]
    async fn condition_branch_skips_inactive_path() {
        let temp_dir = tempdir().expect("temp dir");
        let (executor, workflow_db) = build_executor(temp_dir.path());

        let graph = WorkflowGraph {
            nodes: vec![
                node("trigger_1", NodeType::Trigger, json!({})),
                node(
                    "condition_1",
                    NodeType::Condition,
                    json!({ "expression": "true" }),
                ),
                node(
                    "output_true",
                    NodeType::Output,
                    json!({ "format": "text", "template": "true-branch" }),
                ),
                node(
                    "output_false",
                    NodeType::Output,
                    json!({ "format": "text", "template": "false-branch" }),
                ),
            ],
            edges: vec![
                WorkflowEdge {
                    id: "e1".to_string(),
                    source_node_id: "trigger_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "condition_1".to_string(),
                    target_port: "0".to_string(),
                },
                WorkflowEdge {
                    id: "e2".to_string(),
                    source_node_id: "condition_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "output_true".to_string(),
                    target_port: "0".to_string(),
                },
                WorkflowEdge {
                    id: "e3".to_string(),
                    source_node_id: "condition_1".to_string(),
                    source_port: "1".to_string(),
                    target_node_id: "output_false".to_string(),
                    target_port: "0".to_string(),
                },
            ],
        };

        let graph_json = to_litegraph_json(&graph).expect("graph json");
        seed_workflow(
            workflow_db.as_ref(),
            "wf_branch",
            "Branch Workflow",
            &graph_json,
        );
        let result = executor
            .execute_workflow(
                "wf_branch",
                "Branch Workflow",
                &graph_json,
                1,
                None,
                HashMap::new(),
                None,
                vec![],
                vec![],
                Some("web".to_string()),
                vec!["web".to_string()],
                None,
            )
            .await
            .expect("execute workflow");

        assert_eq!(result.status, "completed");
        let run_id = result.run_id.clone();
        let logs = workflow_db.get_node_logs(&run_id).expect("node logs");

        let false_log = logs
            .iter()
            .find(|log| log.node_id == "output_false")
            .expect("false branch log");
        let false_output = false_log.output_json.clone().unwrap_or_default();
        assert!(false_output.contains("\"skipped\":true"));

        let true_log = logs
            .iter()
            .find(|log| log.node_id == "output_true")
            .expect("true branch log");
        assert_eq!(true_log.status, "completed");
    }

    #[tokio::test]
    async fn approval_node_resumes_after_answer() {
        let temp_dir = tempdir().expect("temp dir");
        let config = test_config(temp_dir.path());
        let runtime = Arc::new(AgentRuntime::new(config).expect("create runtime"));

        let workflow_db_path = temp_dir.path().join("workflow.db");
        let workflow_db = Arc::new(WorkflowDb::new(workflow_db_path));
        workflow_db.init_tables().expect("init workflow tables");

        let artifact_store = Arc::new(ArtifactStore::new(
            temp_dir.path().join("workflow_artifacts"),
        ));
        artifact_store.init().expect("init artifact store");

        let event_bus = runtime.event_bus().clone();
        let (executor, _event_rx) = WorkflowExecutor::new(
            workflow_db.clone(),
            artifact_store,
            runtime.get_subagent_manager(),
            event_bus.clone(),
        );
        let executor = Arc::new(executor);

        let graph = WorkflowGraph {
            nodes: vec![
                node("trigger_1", NodeType::Trigger, json!({})),
                node(
                    "approval_1",
                    NodeType::Approval,
                    json!({ "prompt": "Ship this?", "approve_label": "Approve", "reject_label": "Reject" }),
                ),
                node(
                    "output_approved",
                    NodeType::Output,
                    json!({ "format": "text", "template": "approved" }),
                ),
                node(
                    "output_rejected",
                    NodeType::Output,
                    json!({ "format": "text", "template": "rejected" }),
                ),
            ],
            edges: vec![
                WorkflowEdge {
                    id: "e1".to_string(),
                    source_node_id: "trigger_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "approval_1".to_string(),
                    target_port: "0".to_string(),
                },
                WorkflowEdge {
                    id: "e2".to_string(),
                    source_node_id: "approval_1".to_string(),
                    source_port: "0".to_string(),
                    target_node_id: "output_approved".to_string(),
                    target_port: "0".to_string(),
                },
                WorkflowEdge {
                    id: "e3".to_string(),
                    source_node_id: "approval_1".to_string(),
                    source_port: "1".to_string(),
                    target_node_id: "output_rejected".to_string(),
                    target_port: "0".to_string(),
                },
            ],
        };

        let graph_json = to_litegraph_json(&graph).expect("graph json");
        seed_workflow(
            workflow_db.as_ref(),
            "wf_approval",
            "Approval Workflow",
            &graph_json,
        );

        let run_task = tokio::spawn({
            let executor = executor.clone();
            async move {
                executor
                    .execute_workflow(
                        "wf_approval",
                        "Approval Workflow",
                        &graph_json,
                        1,
                        None,
                        HashMap::new(),
                        None,
                        vec![],
                        vec![],
                        Some("web".to_string()),
                        vec!["web".to_string()],
                        None,
                    )
                    .await
            }
        });

        let mut rx = event_bus.subscribe();
        let mut answered = false;
        while !answered {
            let event = timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("wait for workflow approval event")
                .expect("receive event");

            if let AgentEvent::WorkflowApprovalRequested { question_id, .. } = event {
                let ok = event_bus
                    .answer_question(&question_id, vec![vec!["Approve".to_string()]])
                    .await;
                assert!(ok, "approval answer should be accepted");
                answered = true;
            }
        }

        let result = run_task
            .await
            .expect("join run task")
            .expect("workflow result");
        assert_eq!(result.status, "completed");

        let output = result
            .output
            .and_then(|value| value.get("output").cloned())
            .and_then(|value| value.as_str().map(|s| s.to_string()))
            .expect("approved output");
        assert_eq!(output, "approved");
    }
}
