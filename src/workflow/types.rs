use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub default_workspace_id: Option<String>,
    pub current_version: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowVersion {
    pub id: String,
    pub workflow_id: String,
    pub version: i32,
    pub graph_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub workflow_version: i32,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLog {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub node_type: String,
    pub status: String,
    pub input_json: Option<String>,
    pub output_json: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContext {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub node_type: NodeType,
    pub position: Position,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub id: String,
    pub source_node_id: String,
    pub source_port: String,
    pub target_node_id: String,
    pub target_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowGraph {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Trigger,
    Agent,
    Condition,
    Transform,
    Delay,
    Output,
    FileInput,
    FileOutput,
    Approval,
    ForEach,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Trigger => "trigger",
            NodeType::Agent => "agent",
            NodeType::Condition => "condition",
            NodeType::Transform => "transform",
            NodeType::Delay => "delay",
            NodeType::Output => "output",
            NodeType::FileInput => "file_input",
            NodeType::FileOutput => "file_output",
            NodeType::Approval => "approval",
            NodeType::ForEach => "foreach",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "trigger" => Some(NodeType::Trigger),
            "agent" => Some(NodeType::Agent),
            "condition" => Some(NodeType::Condition),
            "transform" => Some(NodeType::Transform),
            "delay" => Some(NodeType::Delay),
            "output" => Some(NodeType::Output),
            "file_input" => Some(NodeType::FileInput),
            "file_output" => Some(NodeType::FileOutput),
            "approval" => Some(NodeType::Approval),
            "foreach" => Some(NodeType::ForEach),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNodeConfig {
    pub agent_id: String,
    pub system_prompt: Option<String>,
    pub task_template: String,
    #[serde(default)]
    pub input_mapping: HashMap<String, String>,
    #[serde(default)]
    pub file_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionNodeConfig {
    pub expression: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformNodeConfig {
    pub script: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelayNodeConfig {
    pub milliseconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputNodeConfig {
    pub format: String,
    pub template: String,
    pub destination: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInputNodeConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub use_attachment: bool,
    #[serde(default)]
    pub attachment_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOutputNodeConfig {
    pub path: String,
    #[serde(default = "default_file_output_template")]
    pub content_template: String,
    #[serde(default)]
    pub create_dirs: bool,
}

fn default_file_output_template() -> String {
    "{{input}}".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalNodeConfig {
    #[serde(default = "default_approval_prompt")]
    pub prompt: String,
    #[serde(default = "default_approval_label")]
    pub approve_label: String,
    #[serde(default = "default_reject_label")]
    pub reject_label: String,
}

fn default_approval_prompt() -> String {
    "Approve workflow step?".to_string()
}

fn default_approval_label() -> String {
    "Approve".to_string()
}

fn default_reject_label() -> String {
    "Reject".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForEachNodeConfig {
    #[serde(default = "default_foreach_items_template")]
    pub items_template: String,
    #[serde(default = "default_foreach_item_variable")]
    pub item_variable: String,
}

fn default_foreach_items_template() -> String {
    "{{input}}".to_string()
}

fn default_foreach_item_variable() -> String {
    "item".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowAttachment {
    pub filename: String,
    pub mime: String,
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub run_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
    pub graph_json: Option<String>,
    #[serde(default)]
    pub default_workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWorkflowRequest {
    #[serde(default)]
    pub graph_json: Option<String>,
    #[serde(default)]
    pub default_workspace_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteWorkflowRequest {
    pub initial_context: Option<ChatContext>,
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<WorkflowAttachment>,
    #[serde(default)]
    pub images: Vec<WorkflowAttachment>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub notify_channels: Vec<String>,
    #[serde(default)]
    pub discord_channel_id: Option<u64>,
}
