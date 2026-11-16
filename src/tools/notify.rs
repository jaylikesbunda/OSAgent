//! `notify`: reach the user outside the chat. Discord DMs go only to the bot's
//! owner (the configured notify user), never to anyone a model names.

use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Default)]
pub struct NotifyTool;

#[async_trait]
impl Tool for NotifyTool {
    fn name(&self) -> &str {
        "notify"
    }

    fn description(&self) -> &str {
        "Notify the user outside this chat. channel=discord sends them a direct message through OSA's Discord bot (ping, DM, message me on Discord). It only ever reaches the bot's owner."
    }

    fn when_to_use(&self) -> &str {
        "the user asked to be pinged or messaged on Discord, e.g. when a long task finishes"
    }

    fn when_not_to_use(&self) -> &str {
        "the user did not ask for a notification, or to message anyone other than the user"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "Tell the user a task is done".into(),
            input: json!({"channel": "discord", "message": "The installer build finished."}),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "enum": ["discord"], "description": "Where to notify. Default discord."},
                "message": {"type": "string", "description": "Short, self-contained text for the user."}
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let message = args["message"].as_str().unwrap_or_default();
        match args["channel"].as_str().unwrap_or("discord") {
            "discord" => send_discord(message).await,
            other => Err(OSAgentError::ToolExecution(format!(
                "Unknown notification channel '{}'. Use discord.",
                other
            ))),
        }
    }
}

#[cfg(feature = "discord")]
async fn send_discord(message: &str) -> Result<String> {
    crate::discord::notify_owner(message)
        .await
        .map_err(OSAgentError::ToolExecution)
}

#[cfg(not(feature = "discord"))]
async fn send_discord(_message: &str) -> Result<String> {
    Err(OSAgentError::ToolExecution(
        "This OSA build has no Discord support.".into(),
    ))
}
