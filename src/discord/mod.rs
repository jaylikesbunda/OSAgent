use crate::agent::events::AgentEvent;
use crate::agent::runtime::AgentRuntime;
use crate::config::DiscordConfig;
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::db::WorkflowDb;
use crate::workflow::executor::WorkflowExecutor;
use dashmap::DashMap;
use serenity::{
    async_trait,
    builder::{
        CreateActionRow, CreateAutocompleteResponse, CreateButton, CreateCommand, CreateEmbed,
        CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
        CreateMessage, EditInteractionResponse,
    },
    model::{
        application::{ButtonStyle, Command, CommandInteraction, CommandOptionType},
        channel::Message,
        colour::Colour,
        gateway::Ready,
        id::ChannelId,
    },
    prelude::*,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, time::SystemTime};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tracing::{error, info, warn};

const DEFAULT_WORKSPACE_CHOICE_LIMIT: usize = 25;

static SESSION_TO_CHANNEL: std::sync::OnceLock<
    tokio::sync::RwLock<std::collections::HashMap<String, u64>>,
> = std::sync::OnceLock::new();

static LAST_DISCORD_CHANNEL: std::sync::OnceLock<tokio::sync::RwLock<u64>> =
    std::sync::OnceLock::new();

fn get_last_discord_channel() -> &'static tokio::sync::RwLock<u64> {
    LAST_DISCORD_CHANNEL.get_or_init(|| tokio::sync::RwLock::new(0))
}

pub async fn get_last_discord_channel_id() -> u64 {
    let lock = get_last_discord_channel().read().await;
    *lock
}

pub async fn set_last_discord_channel_id(channel_id: u64) {
    let mut lock = get_last_discord_channel().write().await;
    *lock = channel_id;
}

struct DiscordBotState {
    running: bool,
    stop_tx: Option<oneshot::Sender<()>>,
}

static DISCORD_BOT_STATE: std::sync::OnceLock<tokio::sync::Mutex<DiscordBotState>> =
    std::sync::OnceLock::new();

fn get_session_to_channel() -> &'static tokio::sync::RwLock<std::collections::HashMap<String, u64>>
{
    SESSION_TO_CHANNEL.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

fn get_discord_bot_state() -> &'static tokio::sync::Mutex<DiscordBotState> {
    DISCORD_BOT_STATE.get_or_init(|| {
        tokio::sync::Mutex::new(DiscordBotState {
            running: false,
            stop_tx: None,
        })
    })
}

const EMBED_COLOR_PRIMARY: Colour = Colour::from_rgb(124, 129, 141);
const EMBED_COLOR_SUCCESS: Colour = Colour::from_rgb(87, 242, 135);
const EMBED_COLOR_ERROR: Colour = Colour::from_rgb(237, 66, 69);
const EMBED_COLOR_WARNING: Colour = Colour::from_rgb(254, 231, 92);
const EMBED_COLOR_INFO: Colour = Colour::from_rgb(150, 155, 167);
const DISCORD_TYPING_PULSE_SECS: u64 = 8;
const DISCORD_TASK_JOIN_TIMEOUT_MS: u64 = 150;

pub struct Handler {
    agent: Arc<AgentRuntime>,
    config_path: PathBuf,
    sessions: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, String>>>,
    channel_sessions: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, String>>>,
    channel_locks: Arc<DashMap<u64, Arc<Mutex<()>>>>,
    pending_question: Arc<tokio::sync::Mutex<Option<PendingQuestion>>>,
}

#[derive(Debug, Clone)]
struct PendingQuestion {
    question_id: String,
    session_id: String,
    channel_id: u64,
    questions: Vec<crate::tools::question::Question>,
}

impl Handler {
    async fn send_question_embed(
        http: &serenity::http::Http,
        pending_q: &Arc<tokio::sync::Mutex<Option<PendingQuestion>>>,
        session_id: &str,
        question_id: &str,
        channel_id: u64,
        questions: &[crate::tools::question::Question],
    ) {
        for q in questions {
            let mut desc = format!("**{}**\n\n", q.question);

            let options: Vec<(usize, String)> = q
                .options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let label = if !opt.label.is_empty() {
                        opt.label.clone()
                    } else {
                        format!("Option {}", i + 1)
                    };
                    (i + 1, label)
                })
                .collect();

            for (idx, label) in &options {
                desc.push_str(&format!("`{idx}` - {}\n", label));
            }

            if !options.is_empty() {
                desc.push_str("\nReply with `/answer <number>` or `/answer <your text>`");
            } else {
                desc.push_str("\nReply with `/answer <your text>`");
            }

            let embed = CreateEmbed::new()
                .title(if !q.header.is_empty() {
                    q.header.as_str()
                } else {
                    "Question"
                })
                .description(&desc)
                .colour(EMBED_COLOR_WARNING);

            let cid = ChannelId::new(channel_id);
            if let Err(e) = cid
                .send_message(http, CreateMessage::new().embed(embed))
                .await
            {
                error!("Discord: Failed to send question embed: {}", e);
            }
        }

        let pending = PendingQuestion {
            question_id: question_id.to_string(),
            session_id: session_id.to_string(),
            channel_id,
            questions: questions.to_vec(),
        };
        let mut lock = pending_q.lock().await;
        *lock = Some(pending);
    }

    async fn handle_answer_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        answer: &str,
    ) {
        let pending_guard = self.pending_question.lock().await;
        let pending = match pending_guard.as_ref() {
            Some(p) => p.clone(),
            None => {
                let embed = CreateEmbed::new()
                    .title("No Pending Question")
                    .description("There is no question waiting for an answer.")
                    .colour(EMBED_COLOR_ERROR);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
                return;
            }
        };
        drop(pending_guard);

        let parsed_answer = answer.trim();
        let answer_vec: Vec<String> = parsed_answer
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let answers: Vec<Vec<String>> = vec![answer_vec];

        let found = self
            .agent
            .answer_question(&pending.question_id, answers)
            .await;

        {
            let mut lock = self.pending_question.lock().await;
            *lock = None;
        }

        let (title, desc, color) = if found {
            (
                "Answer Submitted",
                "Your answer has been sent to the agent.",
                EMBED_COLOR_SUCCESS,
            )
        } else {
            (
                "Answer Failed",
                "This question may have already been answered or expired.",
                EMBED_COLOR_ERROR,
            )
        };

        let embed = CreateEmbed::new()
            .title(title)
            .description(desc)
            .colour(color);
        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_workflow_component(
        &self,
        ctx: &Context,
        component: &serenity::model::application::ComponentInteraction,
    ) {
        let custom_id = component.data.custom_id.as_str();
        let (question_id, answer, is_approve) =
            if let Some(qid) = custom_id.strip_prefix("wf_approve:") {
                (qid, "Approve", true)
            } else if let Some(qid) = custom_id.strip_prefix("wf_reject:") {
                (qid, "Reject", false)
            } else {
                return;
            };

        let found = self
            .agent
            .answer_question(question_id, vec![vec![answer.to_string()]])
            .await;

        let label = if is_approve { "Approved" } else { "Rejected" };
        let response_text = if found {
            format!("{} by {}", label, component.user.name)
        } else {
            "This approval request is no longer active.".to_string()
        };

        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content(response_text)
                        .components(vec![CreateActionRow::Buttons(vec![CreateButton::new(
                            "workflow-approval-handled",
                        )
                        .label(label)
                        .style(if is_approve {
                            ButtonStyle::Success
                        } else {
                            ButtonStyle::Danger
                        })
                        .disabled(true)])]),
                ),
            )
            .await;
    }

    pub fn new(agent: Arc<AgentRuntime>, config_path: PathBuf) -> Self {
        Self {
            agent,
            config_path,
            sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            channel_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            channel_locks: Arc::new(DashMap::new()),
            pending_question: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn session_is_archived(session: &crate::storage::Session) -> bool {
        session
            .metadata
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    async fn get_active_session_id_for_user(&self, user_id: u64) -> Option<String> {
        {
            let sessions = self.sessions.read().await;
            if let Some(session_id) = sessions.get(&user_id) {
                if let Ok(Some(session)) = self.agent.get_session(session_id).await {
                    if !Self::session_is_archived(&session) {
                        return Some(session.id);
                    }
                }
            }
        }

        let owner_key = format!("discord:{}", user_id);
        let resolved = self
            .agent
            .get_session_id_for_user(&owner_key)
            .await
            .ok()
            .flatten();

        if let Some(session_id) = resolved.as_ref() {
            let mut sessions = self.sessions.write().await;
            sessions.insert(user_id, session_id.clone());
        }

        resolved
    }

    async fn archive_current_session_for_user(
        &self,
        user_id: u64,
    ) -> Result<Option<String>, String> {
        let session_id = self.get_active_session_id_for_user(user_id).await;

        if let Some(session_id) = session_id {
            self.agent
                .archive_session(&session_id)
                .await
                .map_err(|e| format!("Failed to archive session: {}", e))?;
            let mut sessions = self.sessions.write().await;
            sessions.remove(&user_id);
            Ok(Some(session_id))
        } else {
            Ok(None)
        }
    }

    async fn get_or_create_session(&self, user_id: u64) -> Result<String, String> {
        if let Some(active_session_id) = self.get_active_session_id_for_user(user_id).await {
            return Ok(active_session_id);
        }

        let owner_key = format!("discord:{}", user_id);
        match self
            .agent
            .create_session_for_user(&owner_key, "discord")
            .await
        {
            Ok(session) => {
                let mut sessions = self.sessions.write().await;
                sessions.insert(user_id, session.id.clone());
                Ok(session.id)
            }
            Err(e) => Err(format!("Failed to create session: {}", e)),
        }
    }

    async fn get_or_create_channel_session(&self, channel_id: u64) -> Result<String, String> {
        let sessions = self.channel_sessions.read().await;
        if let Some(session_id) = sessions.get(&channel_id) {
            if self
                .agent
                .get_session(session_id)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                return Ok(session_id.clone());
            }
        }
        drop(sessions);

        let owner_key = format!("discord-channel:{}", channel_id);
        match self
            .agent
            .create_session_for_user(&owner_key, "discord")
            .await
        {
            Ok(session) => {
                let mut sessions = self.channel_sessions.write().await;
                sessions.insert(channel_id, session.id.clone());
                get_session_to_channel()
                    .write()
                    .await
                    .insert(session.id.clone(), channel_id);
                Ok(session.id)
            }
            Err(e) => Err(format!("Failed to create channel session: {}", e)),
        }
    }

    fn get_channel_lock(&self, channel_id: u64) -> Arc<Mutex<()>> {
        self.channel_locks
            .entry(channel_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn start_typing_loop(
        http: Arc<serenity::http::Http>,
        channel_id: ChannelId,
        done: Arc<tokio::sync::Notify>,
    ) {
        let _ = channel_id.broadcast_typing(&http).await;
        loop {
            tokio::select! {
                _ = done.notified() => break,
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(DISCORD_TYPING_PULSE_SECS)) => {
                    let _ = channel_id.broadcast_typing(&http).await;
                }
            }
        }
    }

    async fn process_channel_message(
        agent: Arc<AgentRuntime>,
        channel_lock: Arc<Mutex<()>>,
        ctx: Context,
        msg: Message,
        session_id: String,
        content: String,
        user_id: u64,
    ) {
        let _channel_guard = channel_lock.lock().await;

        let typing_done = Arc::new(tokio::sync::Notify::new());
        let mut typing_task = tokio::spawn(Self::start_typing_loop(
            ctx.http.clone(),
            msg.channel_id,
            typing_done.clone(),
        ));

        let mut event_rx = agent.subscribe_to_events();
        let session_for_events = session_id.clone();
        let http_for_events = ctx.http.clone();
        let channel_for_events = msg.channel_id;

        let mut tool_event_task = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(AgentEvent::ToolComplete {
                        session_id,
                        tool_name,
                        success,
                        output,
                        duration_ms,
                        ..
                    }) => {
                        if session_id != session_for_events {
                            continue;
                        }
                        if !success {
                            Self::send_tool_complete_embed(
                                &http_for_events,
                                channel_for_events,
                                &tool_name,
                                &output,
                                success,
                                duration_ms,
                            )
                            .await;
                        }
                    }
                    Ok(AgentEvent::ResponseComplete { session_id, .. }) => {
                        if session_id == session_for_events {
                            break;
                        }
                    }
                    Ok(AgentEvent::Error { session_id, .. }) => {
                        if session_id == session_for_events {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let process_future = agent.process_message(
            &session_id,
            content.to_string(),
            format!("discord:{}", user_id),
        );

        match tokio::time::timeout(tokio::time::Duration::from_secs(3600), process_future).await {
            Ok(result) => {
                info!("Discord: process_message completed");
                match result {
                    Ok(response) => {
                        if response.trim().is_empty() {
                            let workspace_note = agent
                                .get_session_workspace(&session_id)
                                .await
                                .map(|ws| format!("Workspace: `{}`", ws.path))
                                .unwrap_or_else(|_| "Workspace: current active".to_string());
                            let embed = CreateEmbed::new()
                                .title("Complete")
                                .description(format!("Task completed.\n{}", workspace_note))
                                .colour(EMBED_COLOR_SUCCESS);
                            let _ = msg
                                .channel_id
                                .send_message(&ctx.http, CreateMessage::new().embed(embed))
                                .await;
                        } else {
                            Self::send_text_chunks(&ctx.http, msg.channel_id, &response).await;
                        }
                    }
                    Err(e) => {
                        error!("Discord: Error processing message: {}", e);
                        let embed = CreateEmbed::new()
                            .title("Error")
                            .description(format!("```\n{}\n```", e))
                            .colour(EMBED_COLOR_ERROR);
                        let _ = msg
                            .channel_id
                            .send_message(&ctx.http, CreateMessage::new().embed(embed))
                            .await;
                    }
                }
            }
            Err(_) => {
                error!(
                    "Discord: Timeout processing message for session {}",
                    session_id
                );
                let embed = CreateEmbed::new()
                    .title("Timeout")
                    .description("Request timed out after 60 minutes.")
                    .colour(EMBED_COLOR_ERROR);
                let _ = msg
                    .channel_id
                    .send_message(&ctx.http, CreateMessage::new().embed(embed))
                    .await;
            }
        }

        typing_done.notify_waiters();
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(DISCORD_TASK_JOIN_TIMEOUT_MS),
            &mut typing_task,
        )
        .await;
        if !typing_task.is_finished() {
            typing_task.abort();
        }

        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(DISCORD_TASK_JOIN_TIMEOUT_MS),
            &mut tool_event_task,
        )
        .await;
        if !tool_event_task.is_finished() {
            tool_event_task.abort();
        }
    }

    async fn send_text_chunks(http: &serenity::http::Http, channel_id: ChannelId, response: &str) {
        const DISCORD_MESSAGE_LIMIT: usize = 1800;
        let mut remaining = response.trim();

        while !remaining.is_empty() {
            let mut end = remaining.len().min(DISCORD_MESSAGE_LIMIT);
            while end > 0 && !remaining.is_char_boundary(end) {
                end -= 1;
            }
            let chunk = &remaining[..end];
            if channel_id.say(http, chunk).await.is_err() {
                break;
            }
            remaining = remaining[end..].trim_start();
        }
    }

    async fn is_authorized(&self, user_id: u64) -> bool {
        match self.agent.discord_config().await {
            Some(discord) => {
                discord.allowed_users.is_empty() || discord.allowed_users.contains(&user_id)
            }
            None => true,
        }
    }

    async fn persist_discord_channel_id(&self, channel_id: u64) {
        let config = self.agent.config();
        let mut cfg = config.write().await;
        let changed = match &mut cfg.discord {
            Some(d) => {
                let changed = d.last_channel_id != Some(channel_id);
                d.last_channel_id = Some(channel_id);
                changed
            }
            None => {
                let mut dc = crate::config::DiscordConfig::default();
                dc.last_channel_id = Some(channel_id);
                cfg.discord = Some(dc);
                true
            }
        };
        drop(cfg);
        if changed {
            if let Err(e) = self.agent.save_config(&self.config_path).await {
                warn!("Failed to persist discord channel_id: {}", e);
            }
        }
    }

    fn workflow_paths() -> (PathBuf, PathBuf) {
        let base = PathBuf::from(std::env::var("OSAGENT_DATA_DIR").unwrap_or_else(|_| {
            std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string())
        }));
        (base.join("workflow.db"), base.join("workflow_artifacts"))
    }

    fn build_workflow_services(
        &self,
    ) -> std::result::Result<(Arc<WorkflowDb>, Arc<WorkflowExecutor>), String> {
        let (db_path, artifact_path) = Self::workflow_paths();
        let workflow_db = Arc::new(WorkflowDb::new(db_path));
        workflow_db
            .init_tables()
            .map_err(|e| format!("Failed to initialize workflow db: {}", e))?;

        let artifact_store = Arc::new(ArtifactStore::new(artifact_path));
        artifact_store
            .init()
            .map_err(|e| format!("Failed to initialize workflow artifacts: {}", e))?;

        let subagent_manager = self.agent.get_subagent_manager();
        let (executor, _event_rx) = WorkflowExecutor::new(
            workflow_db.clone(),
            artifact_store,
            subagent_manager,
            self.agent.event_bus().clone(),
        );
        Ok((workflow_db, Arc::new(executor)))
    }

    fn format_workflow_output(output: &serde_json::Value) -> String {
        let text = if let Some(s) = output.as_str() {
            s.to_string()
        } else if let Some(obj) = output.as_object() {
            if let Some(val) = obj.get("output") {
                if let Some(s) = val.as_str() {
                    s.to_string()
                } else {
                    val.to_string()
                }
            } else {
                output.to_string()
            }
        } else {
            output.to_string()
        };

        if text.chars().count() > 1800 {
            let clipped = text.chars().take(1800).collect::<String>();
            format!("{}...", clipped)
        } else {
            text
        }
    }

    async fn register_commands(&self, http: &serenity::http::Http) {
        let commands = vec![
            CreateCommand::new("new").description("Create a new AI session"),
            CreateCommand::new("status").description("Show current session status"),
            CreateCommand::new("session")
                .description("Manage your session")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "Action to perform",
                    )
                    .add_string_choice("new", "new")
                    .add_string_choice("status", "status")
                    .add_string_choice("archive", "archive")
                    .required(true),
                ),
            CreateCommand::new("reset")
                .description("Permanently delete current session (advanced)")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "confirm",
                        "Must be true to permanently delete",
                    )
                    .required(true),
                ),
            CreateCommand::new("permissions")
                .description("Manage external directory permissions")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "Action to perform",
                    )
                    .add_string_choice("list", "list")
                    .add_string_choice("allow", "allow")
                    .add_string_choice("deny", "deny")
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "path",
                        "File or directory path (required for allow/deny)",
                    )
                    .required(false),
                ),
            CreateCommand::new("mode")
                .description("Set agent mode (build or plan)")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "mode",
                        "The mode to set",
                    )
                    .add_string_choice("build", "build")
                    .add_string_choice("plan", "plan")
                    .required(true),
                ),
            CreateCommand::new("model")
                .description("Set the active AI model")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "id",
                        "The model ID to use (e.g., gpt-4o, claude-sonnet-4)",
                    )
                    .required(true),
                ),
            CreateCommand::new("workspace")
                .description("Manage workspaces")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "Action to perform",
                    )
                    .add_string_choice("list", "list")
                    .add_string_choice("set", "set")
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "id",
                        "Workspace ID (required for set)",
                    )
                    .set_autocomplete(true)
                    .required(false),
                ),
            CreateCommand::new("subagent")
                .description("Run a subagent task")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "type",
                        "Subagent type (general or explore)",
                    )
                    .add_string_choice("general", "general")
                    .add_string_choice("explore", "explore")
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "description",
                        "Task description",
                    )
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "prompt",
                        "What the subagent should do",
                    )
                    .required(true),
                ),
            CreateCommand::new("lsp")
                .description("Run LSP operations")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "operation",
                        "LSP operation",
                    )
                    .add_string_choice("goto_definition", "goToDefinition")
                    .add_string_choice("references", "findReferences")
                    .add_string_choice("hover", "hover")
                    .add_string_choice("symbols", "documentSymbol")
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "file",
                        "File path",
                    )
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "line",
                        "Line number",
                    )
                    .required(false),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::Integer,
                        "character",
                        "Character position",
                    )
                    .required(false),
                ),
            CreateCommand::new("settings").description("Show current configuration"),
            CreateCommand::new("help").description("Show help and available commands"),
            CreateCommand::new("answer")
                .description("Answer a pending question")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "answer",
                        "Your answer (number for option, or custom text)",
                    )
                    .required(true),
                ),
            CreateCommand::new("chat")
                .description("Send a message to the AI")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "message",
                        "Your message to the AI",
                    )
                    .required(true),
                ),
            CreateCommand::new("workflow")
                .description("Run a workflow")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "name",
                        "Workflow name",
                    )
                    .set_autocomplete(true)
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "input",
                        "Optional input for the workflow trigger",
                    )
                    .required(false),
                ),
            CreateCommand::new("persona")
                .description("Manage personas for this session")
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "action",
                        "Action to perform",
                    )
                    .add_string_choice("list", "list")
                    .add_string_choice("set", "set")
                    .add_string_choice("clear", "clear")
                    .required(true),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "id",
                        "Persona ID (e.g., default, code, plan, custom)",
                    )
                    .set_autocomplete(true)
                    .required(false),
                )
                .add_option(
                    serenity::builder::CreateCommandOption::new(
                        CommandOptionType::String,
                        "character",
                        "Custom character for roleplay (use with id: custom)",
                    )
                    .required(false),
                ),
        ];

        if let Err(e) = Command::set_global_commands(http, commands).await {
            error!("Discord: Failed to register global commands: {}", e);
        } else {
            info!("Discord: Slash commands registered successfully");
        }
    }

    async fn send_ephemeral_embed_command(
        ctx: &Context,
        command: &CommandInteraction,
        embed: CreateEmbed,
    ) {
        if let Err(e) = command
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .ephemeral(true),
                ),
            )
            .await
        {
            error!("Discord: Failed to send ephemeral response: {}", e);
        }
    }

    async fn handle_new_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let archived_previous = matches!(
            self.archive_current_session_for_user(user_id).await,
            Ok(Some(_))
        );

        let owner_key = format!("discord:{}", user_id);

        match self.agent.get_or_create_session_for_user(&owner_key).await {
            Ok(session) => {
                let id = session.id.clone();
                let mut sessions = self.sessions.write().await;
                sessions.insert(user_id, id.clone());

                let embed = CreateEmbed::new()
                    .title("New Session Created")
                    .description(format!(
                        "A fresh session has been initialized{}\n```\n{}\n```",
                        if archived_previous {
                            " and your previous one was archived."
                        } else {
                            "."
                        },
                        id,
                    ))
                    .colour(EMBED_COLOR_SUCCESS)
                    .footer(CreateEmbedFooter::new("Ready to assist you!"));

                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
            Err(e) => {
                let embed = CreateEmbed::new()
                    .title("Session Creation Failed")
                    .description(format!("Unable to create a new session.\n```\n{}\n```", e))
                    .colour(EMBED_COLOR_ERROR);

                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
        }
    }

    async fn handle_status_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let active_session_id = self.get_active_session_id_for_user(user_id).await;
        let embed = match active_session_id {
            Some(session_id) => match self.agent.get_session(&session_id).await {
                Ok(Some(session)) => CreateEmbed::new()
                    .title("Session Status")
                    .field("Session ID", format!("```\n{}\n```", session.id), false)
                    .field("Messages", format!("`{}`", session.messages.len()), true)
                    .field(
                        "Source",
                        format!(
                            "`{}`",
                            session
                                .metadata
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or("discord")
                        ),
                        true,
                    )
                    .field("Status", format!("`{}`", session.task_status), true)
                    .colour(EMBED_COLOR_PRIMARY)
                    .footer(CreateEmbedFooter::new("Session is ready for interactions")),
                _ => CreateEmbed::new()
                    .title("Session Not Found")
                    .description(
                        "Your session could not be located.\nUse `/new` to create a fresh one.",
                    )
                    .colour(EMBED_COLOR_WARNING),
            },
            None => CreateEmbed::new()
                .title("No Active Session")
                .description("You don't have an active session.\nUse `/new` to create one.")
                .colour(EMBED_COLOR_INFO),
        };

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_archive_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let embed = match self.archive_current_session_for_user(user_id).await {
            Ok(Some(_)) => CreateEmbed::new()
                .title("Session Archived")
                .description("Your current session was archived. Your next message will start a new session.")
                .colour(EMBED_COLOR_INFO),
            Ok(None) => CreateEmbed::new()
                .title("No Active Session")
                .description("You don't have an active session to archive.")
                .colour(EMBED_COLOR_INFO),
            Err(error) => CreateEmbed::new()
                .title("Archive Failed")
                .description(format!("```
{}
```", error))
                .colour(EMBED_COLOR_ERROR),
        };

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_reset_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        confirm: bool,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        if !confirm {
            let embed = CreateEmbed::new()
                .title("Confirmation Required")
                .description(
                    "`/reset` permanently deletes your current session. Use `/reset confirm:true`.",
                )
                .colour(EMBED_COLOR_WARNING);
            Self::send_ephemeral_embed_command(ctx, command, embed).await;
            return;
        }

        let session_id = self.get_active_session_id_for_user(user_id).await;
        if session_id.is_some() {
            let mut sessions = self.sessions.write().await;
            sessions.remove(&user_id);
        }

        let embed = match session_id {
            Some(session_id) => {
                let _ = self.agent.delete_session(&session_id).await;
                CreateEmbed::new()
                    .title("Session Deleted")
                    .description("Your session was permanently deleted. Use `/new` to start fresh.")
                    .colour(EMBED_COLOR_ERROR)
            }
            None => CreateEmbed::new()
                .title("Nothing to Reset")
                .description("You don't have an active session to reset.")
                .colour(EMBED_COLOR_INFO),
        };

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_session_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        action: &str,
    ) {
        match action {
            "new" => self.handle_new_command(ctx, command).await,
            "status" => self.handle_status_command(ctx, command).await,
            "archive" => self.handle_archive_command(ctx, command).await,
            _ => {
                let embed = CreateEmbed::new()
                    .title("Unknown Action")
                    .description("Use `new`, `status`, or `archive`.")
                    .colour(EMBED_COLOR_WARNING);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
        }
    }

    async fn handle_model_command(&self, ctx: &Context, command: &CommandInteraction, model: &str) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let model_str = model.to_string();
        self.agent.set_current_model(model_str.clone()).await;

        self.agent
            .set_provider_model_in_config(model_str.clone())
            .await;
        if let Err(e) = self.agent.save_config(&self.config_path).await {
            error!("Failed to save config: {}", e);
        }

        let embed = CreateEmbed::new()
            .title("Model Updated")
            .description(format!("Now using model:\n```\n{}\n```", model))
            .colour(EMBED_COLOR_SUCCESS)
            .footer(CreateEmbedFooter::new("Configuration saved"));

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_settings_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let config = self.agent.get_config().await;
        let active_workspace = self.agent.get_active_workspace().await;
        let embed = CreateEmbed::new()
            .title("Current Settings")
            .colour(EMBED_COLOR_PRIMARY)
            .field(
                "Server",
                format!(
                    "Bind: `{}`\nPort: `{}`",
                    config.server.bind, config.server.port
                ),
                false,
            )
            .field(
                "Provider",
                format!(
                    "Type: `{}`\nModel: `{}`\nBase URL: `{}`",
                    config.provider.provider_type, config.provider.model, config.provider.base_url
                ),
                false,
            )
            .field(
                "Agent",
                format!(
                    "Max Tokens: `{}`\nTemperature: `{}`\nActive Workspace: `{}`",
                    config.agent.max_tokens, config.agent.temperature, active_workspace.id
                ),
                false,
            )
            .footer(CreateEmbedFooter::new("Configure via WebUI or config file"));

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_mode_command(&self, ctx: &Context, command: &CommandInteraction, mode: &str) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let embed = match mode {
            "build" => CreateEmbed::new()
                .title("Agent Mode")
                .description("Switched to build mode.\nYou now have full tool access for editing files and running commands.")
                .colour(EMBED_COLOR_SUCCESS),
            "plan" => CreateEmbed::new()
                .title("Agent Mode")
                .description("Switched to plan mode.\nYou now have read-only access for research and planning.")
                .colour(EMBED_COLOR_INFO),
            _ => CreateEmbed::new()
                .title("Unknown Mode")
                .description("Use 'build' or 'plan' mode.")
                .colour(EMBED_COLOR_WARNING),
        };

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_permissions_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        action: &str,
        path: Option<&str>,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        match action {
            "list" => {
                let prompts = self.agent.get_pending_permission_prompts().await;
                if prompts.is_empty() {
                    let embed = CreateEmbed::new()
                        .title("External Directory Permissions")
                        .description("No pending permission requests.")
                        .colour(EMBED_COLOR_INFO);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                } else {
                    let lines: Vec<String> = prompts
                        .iter()
                        .take(10)
                        .map(|p| format!("`{}` - {}", p.path, p.source))
                        .collect();
                    let embed = CreateEmbed::new()
                        .title("External Directory Permissions")
                        .description(lines.join("\n"))
                        .colour(EMBED_COLOR_PRIMARY);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                }
            }
            "allow" | "deny" => {
                let Some(path) = path else {
                    let embed = CreateEmbed::new()
                        .title("Missing Path")
                        .description("Usage: `/permissions action:allow path:<path>`")
                        .colour(EMBED_COLOR_WARNING);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    return;
                };

                let allowed = action == "allow";
                let _ = self
                    .agent
                    .respond_to_permission_prompt(path, allowed, false)
                    .await;

                let embed = CreateEmbed::new()
                    .title("Permission Updated")
                    .description(format!(
                        "{} access to `{}`",
                        if allowed { "Allowed" } else { "Denied" },
                        path
                    ))
                    .colour(if allowed {
                        EMBED_COLOR_SUCCESS
                    } else {
                        EMBED_COLOR_ERROR
                    });
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
            _ => {
                let embed = CreateEmbed::new()
                    .title("Unknown Action")
                    .description("Use `list`, `allow`, or `deny`.")
                    .colour(EMBED_COLOR_WARNING);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
        }
    }

    async fn handle_persona_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        action: &str,
        persona_id: Option<&str>,
        character: Option<&str>,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let owner_key = format!("discord:{}", user_id);
        let session_id = match self.agent.get_session_id_for_user(&owner_key).await {
            Ok(Some(id)) => id,
            _ => {
                let embed = CreateEmbed::new()
                    .title("No Session")
                    .description("Create a session first with `/new`")
                    .colour(EMBED_COLOR_WARNING);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
                return;
            }
        };

        match action {
            "list" => {
                let personas = self.agent.list_personas();
                let lines: Vec<String> = personas
                    .iter()
                    .map(|p| format!("`{}` - {} _{}_", p.id, p.name, p.summary))
                    .collect();

                let embed = CreateEmbed::new()
                    .title("Available Personas")
                    .description(lines.join("\n"))
                    .footer(CreateEmbedFooter::new(
                        "Use `/persona action:set id:<id>` to activate",
                    ))
                    .colour(EMBED_COLOR_PRIMARY);

                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
            "set" => {
                let Some(persona_id) = persona_id else {
                    let embed = CreateEmbed::new()
                        .title("Missing Persona ID")
                        .description("Usage: `/persona action:set id:<id> character:<optional>`")
                        .colour(EMBED_COLOR_WARNING);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    return;
                };

                let character_opt = character.filter(|s| !s.trim().is_empty());

                match self
                    .agent
                    .set_session_persona(
                        &session_id,
                        persona_id.to_string(),
                        character_opt.map(|s| s.to_string()),
                    )
                    .await
                {
                    Ok(active) => {
                        let mut desc =
                            format!("**{}** ({})\n{}", active.id, active.name, active.summary);
                        if let Some(ch) = &active.roleplay_character {
                            desc.push_str(&format!("\n\n_Roleplaying as: {}_", ch));
                        }

                        let embed = CreateEmbed::new()
                            .title("Persona Activated")
                            .description(desc)
                            .colour(EMBED_COLOR_SUCCESS);

                        Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    }
                    Err(e) => {
                        let embed = CreateEmbed::new()
                            .title("Failed to Set Persona")
                            .description(format!("```\n{}\n```", e))
                            .colour(EMBED_COLOR_ERROR);
                        Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    }
                }
            }
            "clear" => {
                if let Err(e) = self.agent.reset_session_persona(&session_id).await {
                    let embed = CreateEmbed::new()
                        .title("Failed to Clear Persona")
                        .description(format!("```\n{}\n```", e))
                        .colour(EMBED_COLOR_ERROR);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                } else {
                    let embed = CreateEmbed::new()
                        .title("Persona Cleared")
                        .description("Reverted to default behavior.")
                        .colour(EMBED_COLOR_SUCCESS);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                }
            }
            _ => {
                let embed = CreateEmbed::new()
                    .title("Unknown Action")
                    .description("Use `list`, `set`, or `clear`.")
                    .colour(EMBED_COLOR_WARNING);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
        }
    }

    async fn handle_subagent_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let subagent_type = command
            .data
            .options
            .iter()
            .find(|o| o.name == "type")
            .and_then(|o| o.value.as_str())
            .unwrap_or("general");

        let description = command
            .data
            .options
            .iter()
            .find(|o| o.name == "description")
            .and_then(|o| o.value.as_str())
            .unwrap_or("");

        let embed = CreateEmbed::new()
            .title("Subagent Task")
            .description(format!(
                "Starting {} subagent task: {}\n\nUse /chat to provide the prompt.",
                subagent_type, description
            ))
            .colour(EMBED_COLOR_INFO);

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_lsp_command(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let operation = command
            .data
            .options
            .iter()
            .find(|o| o.name == "operation")
            .and_then(|o| o.value.as_str())
            .unwrap_or("");

        let file_path = command
            .data
            .options
            .iter()
            .find(|o| o.name == "file")
            .and_then(|o| o.value.as_str())
            .unwrap_or("");

        let line = command
            .data
            .options
            .iter()
            .find(|o| o.name == "line")
            .and_then(|o| o.value.as_i64())
            .unwrap_or(1) as u32;

        let embed = CreateEmbed::new()
            .title("LSP Operation")
            .description(format!(
                "Operation: {}\nFile: {}\nLine: {}\n\nUse /chat to request this LSP operation on the code.",
                operation, file_path, line
            ))
            .colour(EMBED_COLOR_INFO);

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_help_command(ctx: &Context, command: &CommandInteraction) {
        let embed = CreateEmbed::new()
            .title("OSA Discord Bot")
            .description("Your AI coding assistant powered by advanced language models.")
            .colour(EMBED_COLOR_PRIMARY)
            .field("/new", "Create a new session and archive current", false)
            .field("/status", "Show current session status", false)
            .field(
                "/session <new|status|archive>",
                "Preferred session controls",
                false,
            )
            .field(
                "/reset confirm:true",
                "Permanently delete current session (advanced)",
                false,
            )
            .field(
                "/permissions <action> [path]",
                "Manage external directory permissions",
                false,
            )
            .field(
                "/mode <build|plan>",
                "Set agent mode (build or plan)",
                false,
            )
            .field("/model <id>", "Set active model for this bot", false)
            .field(
                "/workspace <action> [id]",
                "List or set active workspace",
                false,
            )
            .field(
                "/subagent <type> <description>",
                "Run a subagent task",
                false,
            )
            .field("/lsp <operation> <file>", "Run LSP operations", false)
            .field("/settings", "Show current configuration", false)
            .field("/chat <message>", "Send a message to the AI", false)
            .field("/workflow <name> [input]", "Run a workflow", false)
            .field("/help", "Show this help message", false)
            .footer(CreateEmbedFooter::new(
                "Tip: Just type a message in the channel to chat with the AI",
            ));

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    async fn handle_chat_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        message: &str,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let channel_id = command.channel_id;

        let embed = CreateEmbed::new()
            .title("Processing")
            .description("Your request is being processed...")
            .colour(EMBED_COLOR_INFO);

        if let Err(e) = command
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .ephemeral(true),
                ),
            )
            .await
        {
            error!("Discord: Failed to send processing response: {}", e);
            return;
        }

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(id) => id,
            Err(e) => {
                error!("Failed to get/create session: {}", e);
                let embed = CreateEmbed::new()
                    .title("Session Error")
                    .description("Failed to create session. Please try again.")
                    .colour(EMBED_COLOR_ERROR);

                let _ = command
                    .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                    .await;
                return;
            }
        };

        info!(
            "Discord: Processing slash command from user {}: {}",
            user_id, message
        );

        let mut event_rx = self.agent.subscribe_to_events();
        let session_for_events = session_id.clone();
        let http_for_events = ctx.http.clone();
        let channel_for_events = channel_id;

        let mut tool_event_task = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(AgentEvent::ToolComplete {
                        session_id,
                        tool_name,
                        success,
                        output,
                        duration_ms,
                        ..
                    }) => {
                        if session_id != session_for_events {
                            continue;
                        }
                        Self::send_tool_complete_embed(
                            &http_for_events,
                            channel_for_events,
                            &tool_name,
                            &output,
                            success,
                            duration_ms,
                        )
                        .await;
                    }
                    Ok(AgentEvent::ResponseComplete { session_id, .. }) => {
                        if session_id == session_for_events {
                            break;
                        }
                    }
                    Ok(AgentEvent::Error { session_id, .. }) => {
                        if session_id == session_for_events {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let process_future = self.agent.process_message(
            &session_id,
            message.to_string(),
            format!("discord:{}", user_id),
        );

        match tokio::time::timeout(tokio::time::Duration::from_secs(3600), process_future).await {
            Ok(result) => match result {
                Ok(response) => {
                    if response.trim().is_empty() {
                        let workspace_note = self
                            .agent
                            .get_session_workspace(&session_id)
                            .await
                            .map(|ws| format!("Workspace: `{}`", ws.path))
                            .unwrap_or_else(|_| "Workspace: current active".to_string());
                        let embed = CreateEmbed::new()
                            .title("Complete")
                            .description(format!(
                                "Task completed successfully.\n{}",
                                workspace_note
                            ))
                            .colour(EMBED_COLOR_SUCCESS);

                        let _ = command
                            .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                            .await;
                    } else {
                        let _ = command
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content("Response:"),
                            )
                            .await;
                        Self::send_text_chunks(&ctx.http, channel_id, &response).await;
                    }
                }
                Err(e) => {
                    error!("Discord: Error processing message: {}", e);
                    let embed = CreateEmbed::new()
                        .title("Error")
                        .description(format!("```\n{}\n```", e))
                        .colour(EMBED_COLOR_ERROR);

                    let _ = command
                        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                        .await;
                }
            },
            Err(_) => {
                error!(
                    "Discord: Timeout processing message for session {}",
                    session_id
                );
                let embed = CreateEmbed::new()
                    .title("Timeout")
                    .description("Your request timed out after 60 minutes. Please try again with a simpler request.")
                    .colour(EMBED_COLOR_ERROR);

                let _ = command
                    .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                    .await;
            }
        }

        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(DISCORD_TASK_JOIN_TIMEOUT_MS),
            &mut tool_event_task,
        )
        .await;
        if !tool_event_task.is_finished() {
            tool_event_task.abort();
        }
    }

    async fn handle_workflow_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        workflow_name: &str,
        input: Option<&str>,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let embed = CreateEmbed::new()
            .title("Workflow Starting")
            .description(format!("Running workflow `{}`...", workflow_name))
            .colour(EMBED_COLOR_INFO);

        if let Err(e) = command
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .ephemeral(true),
                ),
            )
            .await
        {
            error!(
                "Discord: Failed to send workflow processing response: {}",
                e
            );
            return;
        }

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(id) => id,
            Err(e) => {
                error!("Discord: failed to get/create session for workflow: {}", e);
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Session Error")
                                .description("Failed to create session.")
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
                return;
            }
        };

        let (workflow_db, executor) = match self.build_workflow_services() {
            Ok(svc) => svc,
            Err(e) => {
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Workflow Service Error")
                                .description(format!("```\n{}\n```", e))
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
                return;
            }
        };

        let workflow = match workflow_db.list_workflows().and_then(|workflows| {
            workflows
                .into_iter()
                .find(|wf| wf.name.eq_ignore_ascii_case(workflow_name))
                .ok_or_else(|| {
                    crate::error::OSAgentError::Workflow(format!(
                        "Workflow '{}' not found",
                        workflow_name
                    ))
                })
        }) {
            Ok(wf) => wf,
            Err(e) => {
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Workflow Not Found")
                                .description(format!("```\n{}\n```", e))
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
                return;
            }
        };

        let version = match workflow_db.get_version(&workflow.id, workflow.current_version) {
            Ok(Some(v)) => v,
            Ok(None) => {
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Workflow Version Missing")
                                .description("Current workflow version not found.")
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
                return;
            }
            Err(e) => {
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Workflow Error")
                                .description(format!("```\n{}\n```", e))
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
                return;
            }
        };

        let mut parameters = HashMap::new();
        if let Some(trigger_input) = input {
            if !trigger_input.trim().is_empty() {
                parameters.insert(
                    "trigger_input".to_string(),
                    serde_json::Value::String(trigger_input.to_string()),
                );
            }
        }

        let channel_id = command.channel_id.get();
        set_last_discord_channel_id(channel_id).await;
        self.persist_discord_channel_id(channel_id).await;

        if let Some(workspace_id) = workflow.default_workspace_id.as_deref() {
            if let Err(e) = self
                .agent
                .set_session_workspace(&session_id, workspace_id)
                .await
            {
                warn!(
                    "Discord: Failed to apply workflow workspace lock '{}' for session {}: {}",
                    workspace_id, session_id, e
                );
            }
        }

        let result = executor
            .execute_workflow(
                &workflow.id,
                &workflow.name,
                &version.graph_json,
                workflow.current_version,
                None,
                parameters,
                Some(session_id.clone()),
                vec![],
                vec![],
                Some(format!("discord:{}", user_id)),
                vec!["discord".to_string()],
                Some(channel_id),
            )
            .await;

        match result {
            Ok(run_result) => {
                let embed = if run_result.status == "completed" {
                    let output = run_result
                        .output
                        .as_ref()
                        .map(Self::format_workflow_output)
                        .unwrap_or_else(|| "Workflow completed.".to_string());

                    CreateEmbed::new()
                        .title("Workflow Completed")
                        .description(output)
                        .field("Workflow", workflow.name, true)
                        .field(
                            "Run",
                            run_result.run_id.chars().take(8).collect::<String>(),
                            true,
                        )
                        .colour(EMBED_COLOR_SUCCESS)
                } else {
                    CreateEmbed::new()
                        .title("Workflow Failed")
                        .description(
                            run_result
                                .error
                                .unwrap_or_else(|| "Unknown workflow failure".to_string()),
                        )
                        .field("Workflow", workflow.name, true)
                        .field(
                            "Run",
                            run_result.run_id.chars().take(8).collect::<String>(),
                            true,
                        )
                        .colour(EMBED_COLOR_ERROR)
                };

                let _ = command
                    .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                    .await;
            }
            Err(e) => {
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().embed(
                            CreateEmbed::new()
                                .title("Workflow Execution Error")
                                .description(format!("```\n{}\n```", e))
                                .colour(EMBED_COLOR_ERROR),
                        ),
                    )
                    .await;
            }
        }
    }

    async fn handle_workspace_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        action: &str,
        id: Option<&str>,
    ) {
        let user_id = command.user.id.get();

        if !self.is_authorized(user_id).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        match action {
            "list" => {
                let workspaces = self.agent.get_workspaces().await;
                let active = self.agent.get_active_workspace().await;

                let lines = workspaces
                    .iter()
                    .take(DEFAULT_WORKSPACE_CHOICE_LIMIT)
                    .map(|w| {
                        let marker = if w.id == active.id { "*" } else { " " };
                        format!("{} `{}` -> `{}`", marker, w.id, w.path)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let embed = CreateEmbed::new()
                    .title("Workspaces")
                    .description(if lines.is_empty() {
                        "No workspaces configured.".to_string()
                    } else {
                        lines
                    })
                    .footer(CreateEmbedFooter::new("* marks active workspace"))
                    .colour(EMBED_COLOR_PRIMARY);

                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
            "set" => {
                let Some(workspace_id) = id else {
                    let embed = CreateEmbed::new()
                        .title("Missing Workspace ID")
                        .description("Usage: `/workspace action:set id:<workspace-id>`")
                        .colour(EMBED_COLOR_WARNING);
                    Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    return;
                };

                match self.agent.set_active_workspace(workspace_id).await {
                    Ok(active) => {
                        if let Err(e) = self.agent.save_config(&self.config_path).await {
                            error!("Failed to save config after workspace switch: {}", e);
                        }

                        let owner_key = format!("discord:{}", user_id);

                        if let Some(session_id) = self
                            .agent
                            .get_session_id_for_user(&owner_key)
                            .await
                            .ok()
                            .flatten()
                        {
                            if let Err(e) = self
                                .agent
                                .set_session_workspace(&session_id, &active.id)
                                .await
                            {
                                warn!("Failed to switch current session workspace: {}", e);
                            }
                        }

                        let embed = CreateEmbed::new()
                            .title("Workspace Updated")
                            .description(format!(
                                "Active workspace set to `{}`\nPath: `{}`",
                                active.id, active.path
                            ))
                            .colour(EMBED_COLOR_SUCCESS);
                        Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    }
                    Err(e) => {
                        let embed = CreateEmbed::new()
                            .title("Workspace Update Failed")
                            .description(format!("```\n{}\n```", e))
                            .colour(EMBED_COLOR_ERROR);
                        Self::send_ephemeral_embed_command(ctx, command, embed).await;
                    }
                }
            }
            _ => {
                let embed = CreateEmbed::new()
                    .title("Unknown Workspace Action")
                    .description("Use `list` or `set`.")
                    .colour(EMBED_COLOR_WARNING);
                Self::send_ephemeral_embed_command(ctx, command, embed).await;
            }
        }
    }

    async fn send_unauthorized_response_command(ctx: &Context, command: &CommandInteraction) {
        let embed = CreateEmbed::new()
            .title("Access Denied")
            .description("You are not authorized to use this bot.")
            .colour(EMBED_COLOR_ERROR);

        Self::send_ephemeral_embed_command(ctx, command, embed).await;
    }

    fn parse_response(&self, response: &str) -> Vec<ResponsePart> {
        let mut parts = Vec::new();
        let mut current_text = String::new();
        let mut in_code_block = false;
        let mut code_lang = String::new();
        let mut code_content = String::new();
        let mut chars = response.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '`' && chars.peek() == Some(&'`') && {
                let peeked: String = chars.clone().take(2).collect();
                peeked == "``"
            } {
                chars.next();
                chars.next();

                if in_code_block {
                    parts.push(ResponsePart::CodeBlock {
                        language: code_lang.clone(),
                        content: code_content.clone(),
                    });
                    code_lang.clear();
                    code_content.clear();
                    in_code_block = false;
                } else {
                    if !current_text.trim().is_empty() {
                        parts.push(ResponsePart::Text(current_text.clone()));
                        current_text.clear();
                    }
                    while let Some(&ch) = chars.peek() {
                        if ch == '\n' || ch == ' ' {
                            chars.next();
                            if ch == '\n' {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    while let Some(&ch) = chars.peek() {
                        if ch.is_alphanumeric() || ch == '-' || ch == '+' || ch == ' ' {
                            code_lang.push(ch);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    code_lang = code_lang.trim().to_string();
                    in_code_block = true;
                }
                continue;
            }

            if in_code_block {
                code_content.push(c);
            } else {
                current_text.push(c);
            }
        }

        if !current_text.trim().is_empty() {
            parts.push(ResponsePart::Text(current_text));
        }

        parts
    }

    fn is_diff(&self, content: &str) -> bool {
        content.contains("+++")
            || content.contains("---")
            || content
                .lines()
                .any(|l| l.starts_with('+') || l.starts_with('-') || l.starts_with("@@"))
    }

    fn get_language_color(language: &str) -> Colour {
        match language {
            "bash" | "sh" | "shell" => Colour::from_rgb(88, 166, 255),
            "python" | "py" => Colour::from_rgb(55, 118, 171),
            "rust" | "rs" => Colour::from_rgb(222, 165, 132),
            "javascript" | "js" | "typescript" | "ts" => Colour::from_rgb(247, 223, 30),
            "json" => Colour::from_rgb(0, 122, 204),
            "html" => Colour::from_rgb(227, 76, 38),
            "css" => Colour::from_rgb(86, 61, 124),
            "sql" => Colour::from_rgb(0, 112, 192),
            "markdown" | "md" => Colour::from_rgb(8, 63, 161),
            _ => Colour::from_rgb(100, 100, 100),
        }
    }

    fn summarize_tool_event(tool_name: &str, output: &str, success: bool) -> (String, Colour) {
        let lower = output.to_lowercase();
        let is_error = !success || lower.contains("error") || lower.contains("failed");
        let color = if is_error {
            EMBED_COLOR_ERROR
        } else {
            EMBED_COLOR_SUCCESS
        };

        let mut detail = output
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .replace(['{', '}'], "")
            .replace('`', "'")
            .replace('"', "");

        if detail.len() > 180 {
            detail.truncate(180);
            detail.push_str("...");
        }

        let status = if is_error { "failed" } else { "completed" };
        let summary = if detail.is_empty() {
            format!("Tool `{}` {}", tool_name, status)
        } else {
            format!("Tool `{}` {}: {}", tool_name, status, detail)
        };

        (summary, color)
    }

    async fn send_tool_complete_embed(
        http: &serenity::http::Http,
        channel_id: ChannelId,
        tool_name: &str,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) {
        let (summary, color) = Self::summarize_tool_event(tool_name, output, success);
        let status_icon = if success { "✓" } else { "✗" };

        let embed = CreateEmbed::new()
            .title(format!("{} Tool: {}", status_icon, tool_name))
            .description(summary)
            .field("Duration", format!("`{}ms`", duration_ms), true)
            .colour(color);

        if let Err(e) = channel_id
            .send_message(http, CreateMessage::new().embed(embed))
            .await
        {
            error!("Discord: Failed to send tool embed: {}", e);
        }
    }

    async fn send_embeds(
        &self,
        http: &serenity::http::Http,
        channel_id: ChannelId,
        response: &str,
    ) {
        let parts = self.parse_response(response);
        let mut embeds: Vec<CreateEmbed> = Vec::new();
        let mut current_text = String::new();

        for part in parts {
            match part {
                ResponsePart::Text(text) => {
                    current_text.push_str(&text);
                    if current_text.len() > 1500 {
                        embeds.push(
                            CreateEmbed::new()
                                .description(&current_text[..current_text.len().min(4000)])
                                .colour(EMBED_COLOR_PRIMARY),
                        );
                        current_text.clear();
                    }
                }
                ResponsePart::CodeBlock { language, content } => {
                    if !current_text.trim().is_empty() {
                        embeds.push(
                            CreateEmbed::new()
                                .description(current_text.clone())
                                .colour(EMBED_COLOR_PRIMARY),
                        );
                        current_text.clear();
                    }

                    if language == "tool" {
                        continue;
                    }

                    let color = if self.is_diff(&content) {
                        EMBED_COLOR_SUCCESS
                    } else if language == "error" || content.to_lowercase().contains("error") {
                        EMBED_COLOR_ERROR
                    } else if !language.is_empty() {
                        Self::get_language_color(&language)
                    } else {
                        Colour::from_rgb(100, 100, 100)
                    };

                    let title = if self.is_diff(&content) {
                        "Diff"
                    } else if !language.is_empty() {
                        &language
                    } else {
                        "Code"
                    };

                    let truncated = if content.len() > 3900 {
                        format!("{}...", &content[..3900])
                    } else {
                        content.clone()
                    };

                    let formatted = format!("```{}\n{}\n```", language, truncated);

                    embeds.push(
                        CreateEmbed::new()
                            .title(title)
                            .description(formatted)
                            .colour(color),
                    );
                }
            }
        }

        if !current_text.trim().is_empty() {
            embeds.push(
                CreateEmbed::new()
                    .description(&current_text)
                    .colour(EMBED_COLOR_PRIMARY),
            );
        }

        if embeds.is_empty() {
            let embed = CreateEmbed::new()
                .title("Complete")
                .description("Task finished.")
                .colour(EMBED_COLOR_SUCCESS);

            let _ = channel_id
                .send_message(http, CreateMessage::new().embed(embed))
                .await;
            return;
        }

        for chunk in embeds.chunks(10) {
            let builder = CreateMessage::new().embeds(chunk.to_vec());
            if let Err(e) = channel_id.send_message(http, builder).await {
                error!("Discord: Failed to send embed: {}", e);
            }
        }
    }

    async fn handle_persona_autocomplete(&self, ctx: &Context, command: &CommandInteraction) {
        let action = command
            .data
            .options
            .iter()
            .find(|o| o.name == "action")
            .and_then(|o| o.value.as_str())
            .unwrap_or("list");

        if action != "set" {
            let _ = command
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Autocomplete(CreateAutocompleteResponse::new()),
                )
                .await;
            return;
        }

        let query = command
            .data
            .options
            .iter()
            .find(|o| o.name == "id")
            .and_then(|o| o.value.as_str())
            .unwrap_or("")
            .to_lowercase();

        let mut choices: Vec<(String, String)> = self
            .agent
            .list_personas()
            .into_iter()
            .filter(|p| {
                if query.is_empty() {
                    true
                } else {
                    p.id.to_lowercase().contains(&query) || p.name.to_lowercase().contains(&query)
                }
            })
            .take(DEFAULT_WORKSPACE_CHOICE_LIMIT)
            .map(|p| (p.name, p.id))
            .collect();

        if choices.is_empty() {
            choices.push(("No matches".to_string(), "default".to_string()));
        }

        let mut response = CreateAutocompleteResponse::new();
        for (name, value) in choices {
            response = response.add_string_choice(name, value);
        }

        if let Err(e) = command
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(response))
            .await
        {
            error!("Discord: Failed to respond to persona autocomplete: {}", e);
        }
    }

    async fn handle_workspace_autocomplete(&self, ctx: &Context, command: &CommandInteraction) {
        let action = command
            .data
            .options
            .iter()
            .find(|o| o.name == "action")
            .and_then(|o| o.value.as_str())
            .unwrap_or("list");

        if action != "set" {
            let _ = command
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Autocomplete(CreateAutocompleteResponse::new()),
                )
                .await;
            return;
        }

        let query = command
            .data
            .options
            .iter()
            .find(|o| o.name == "id")
            .and_then(|o| o.value.as_str())
            .unwrap_or("")
            .to_lowercase();

        let mut choices: Vec<(String, String)> = self
            .agent
            .get_workspaces()
            .await
            .into_iter()
            .filter(|ws| {
                if query.is_empty() {
                    true
                } else {
                    ws.id.to_lowercase().contains(&query) || ws.name.to_lowercase().contains(&query)
                }
            })
            .take(DEFAULT_WORKSPACE_CHOICE_LIMIT)
            .map(|ws| (ws.name, ws.id))
            .collect();

        if choices.is_empty() {
            choices.push(("No matches".to_string(), "default".to_string()));
        }

        let mut response = CreateAutocompleteResponse::new();
        for (name, value) in choices {
            response = response.add_string_choice(name, value);
        }

        if let Err(e) = command
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(response))
            .await
        {
            error!("Discord: Failed to respond to autocomplete: {}", e);
        }
    }

    async fn handle_workflow_autocomplete(&self, ctx: &Context, command: &CommandInteraction) {
        let query = command
            .data
            .options
            .iter()
            .find(|o| o.name == "name")
            .and_then(|o| o.value.as_str())
            .unwrap_or("")
            .to_lowercase();

        let choices = match self.build_workflow_services() {
            Ok((db, _executor)) => match db.list_workflows() {
                Ok(workflows) => {
                    let mut values: Vec<(String, String)> = workflows
                        .into_iter()
                        .filter(|wf| {
                            query.is_empty()
                                || wf.name.to_lowercase().contains(&query)
                                || wf.id.to_lowercase().contains(&query)
                        })
                        .take(DEFAULT_WORKSPACE_CHOICE_LIMIT)
                        .map(|wf| (wf.name.clone(), wf.name))
                        .collect();
                    if values.is_empty() {
                        values.push(("No workflows found".to_string(), "".to_string()));
                    }
                    values
                }
                Err(e) => {
                    warn!("Discord: Failed to list workflows for autocomplete: {}", e);
                    vec![("Workflow lookup failed".to_string(), "".to_string())]
                }
            },
            Err(e) => {
                warn!(
                    "Discord: Failed to initialize workflow services for autocomplete: {}",
                    e
                );
                vec![("Workflow service unavailable".to_string(), "".to_string())]
            }
        };

        let mut response = CreateAutocompleteResponse::new();
        for (label, value) in choices.into_iter().filter(|(_, value)| !value.is_empty()) {
            response = response.add_string_choice(label, value);
        }

        if let Err(e) = command
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(response))
            .await
        {
            error!("Discord: Failed to respond to workflow autocomplete: {}", e);
        }
    }
}

enum ResponsePart {
    Text(String),
    CodeBlock { language: String, content: String },
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
        info!("Registering slash commands...");
        self.register_commands(&ctx.http).await;
        info!("Discord bot is ready to receive interactions");

        let agent = self.agent.clone();
        let pending_q = self.pending_question.clone();
        let http = ctx.http.clone();
        tokio::spawn(async move {
            let mut event_rx = agent.subscribe_to_events();
            loop {
                match event_rx.recv().await {
                    Ok(AgentEvent::QuestionAsked {
                        question_id,
                        session_id,
                        questions,
                        ..
                    }) => {
                        let channel_id = {
                            let map = get_session_to_channel().read().await;
                            map.get(&session_id).copied()
                        };
                        if let Some(cid) = channel_id {
                            Self::send_question_embed(
                                &http,
                                &pending_q,
                                &session_id,
                                &question_id,
                                cid,
                                &questions,
                            )
                            .await;
                        } else {
                            warn!("Discord: QuestionAsked for unknown session: {}", session_id);
                        }
                    }
                    Ok(AgentEvent::ScheduledJobFired {
                        notify_channels,
                        message,
                        job_id,
                        job_type,
                        session_id,
                        discord_channel_id,
                        ..
                    }) => {
                        if !notify_channels.iter().any(|c| c == "discord") {
                            continue;
                        }
                        let session_channel = {
                            if let Some(sid) = &session_id {
                                let map = get_session_to_channel().read().await;
                                map.get(sid).copied()
                            } else {
                                None
                            }
                        };

                        let cid = match discord_channel_id {
                            Some(cid) => cid,
                            None => match session_channel {
                                Some(cid) => cid,
                                None => {
                                    let last = get_last_discord_channel_id().await;
                                    if last == 0 {
                                        info!("Discord: ScheduledJobFired has discord channel but no session/channel to deliver to");
                                        continue;
                                    }
                                    last
                                }
                            },
                        };

                        let title_label = match job_type.as_str() {
                            "daily_briefing" => "Daily Briefing",
                            _ => &job_type,
                        };
                        let embed = CreateEmbed::new()
                            .title(format!("Scheduled {}", title_label.replace('_', " ")))
                            .description(&message)
                            .field("Job ID", &job_id[..8], true)
                            .field("Type", &job_type, true)
                            .color(EMBED_COLOR_INFO);
                        if let Err(e) = ChannelId::new(cid)
                            .send_message(&http, CreateMessage::new().embed(embed))
                            .await
                        {
                            warn!("Discord: Failed to send scheduled job embed: {}", e);
                        }
                    }
                    Ok(AgentEvent::WorkflowApprovalRequested {
                        notify_channels,
                        discord_channel_id,
                        prompt,
                        approve_label,
                        reject_label,
                        question_id,
                        workflow_id,
                        run_id,
                        ..
                    }) => {
                        if !notify_channels.iter().any(|c| c == "discord") {
                            continue;
                        }

                        let cid = match discord_channel_id {
                            Some(cid) => cid,
                            None => {
                                let last = get_last_discord_channel_id().await;
                                if last == 0 {
                                    continue;
                                }
                                last
                            }
                        };

                        let embed = CreateEmbed::new()
                            .title("Workflow Approval Required")
                            .description(prompt)
                            .field("Workflow", &workflow_id, true)
                            .field("Run", run_id.chars().take(8).collect::<String>(), true)
                            .colour(EMBED_COLOR_WARNING);

                        let components = vec![CreateActionRow::Buttons(vec![
                            CreateButton::new(format!("wf_approve:{}", question_id))
                                .label(approve_label)
                                .style(ButtonStyle::Success),
                            CreateButton::new(format!("wf_reject:{}", question_id))
                                .label(reject_label)
                                .style(ButtonStyle::Danger),
                        ])];

                        if let Err(e) = ChannelId::new(cid)
                            .send_message(
                                &http,
                                CreateMessage::new().embed(embed).components(components),
                            )
                            .await
                        {
                            warn!("Discord: Failed to send workflow approval embed: {}", e);
                        }
                    }
                    Ok(AgentEvent::WorkflowCompleted {
                        notify_channels,
                        discord_channel_id,
                        output,
                        workflow_id,
                        run_id,
                        ..
                    }) => {
                        if !notify_channels.iter().any(|c| c == "discord") {
                            continue;
                        }

                        let cid = match discord_channel_id {
                            Some(cid) => cid,
                            None => {
                                let last = get_last_discord_channel_id().await;
                                if last == 0 {
                                    continue;
                                }
                                last
                            }
                        };

                        let output_text = output
                            .as_ref()
                            .map(Handler::format_workflow_output)
                            .unwrap_or_else(|| "Workflow completed.".to_string());

                        let embed = CreateEmbed::new()
                            .title("Workflow Completed")
                            .description(output_text)
                            .field("Workflow", &workflow_id, true)
                            .field("Run", run_id.chars().take(8).collect::<String>(), true)
                            .colour(EMBED_COLOR_SUCCESS);

                        if let Err(e) = ChannelId::new(cid)
                            .send_message(&http, CreateMessage::new().embed(embed))
                            .await
                        {
                            warn!("Discord: Failed to send workflow completion embed: {}", e);
                        }
                    }
                    Ok(AgentEvent::WorkflowFailed {
                        notify_channels,
                        discord_channel_id,
                        error,
                        workflow_id,
                        run_id,
                        ..
                    }) => {
                        if !notify_channels.iter().any(|c| c == "discord") {
                            continue;
                        }

                        let cid = match discord_channel_id {
                            Some(cid) => cid,
                            None => {
                                let last = get_last_discord_channel_id().await;
                                if last == 0 {
                                    continue;
                                }
                                last
                            }
                        };

                        let embed = CreateEmbed::new()
                            .title("Workflow Failed")
                            .description(error)
                            .field("Workflow", &workflow_id, true)
                            .field("Run", run_id.chars().take(8).collect::<String>(), true)
                            .colour(EMBED_COLOR_ERROR);

                        if let Err(e) = ChannelId::new(cid)
                            .send_message(&http, CreateMessage::new().embed(embed))
                            .await
                        {
                            warn!("Discord: Failed to send workflow failure embed: {}", e);
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    async fn interaction_create(
        &self,
        ctx: Context,
        interaction: serenity::model::application::Interaction,
    ) {
        if let Some(command) = interaction.as_autocomplete() {
            if command.data.name == "workspace" {
                self.handle_workspace_autocomplete(&ctx, command).await;
            } else if command.data.name == "persona" {
                self.handle_persona_autocomplete(&ctx, command).await;
            } else if command.data.name == "workflow" {
                self.handle_workflow_autocomplete(&ctx, command).await;
            }
            return;
        }

        if let Some(component) = interaction.as_message_component() {
            self.handle_workflow_component(&ctx, component).await;
            return;
        }

        if let Some(command) = interaction.as_command() {
            let command_name = command.data.name.as_str();

            match command_name {
                "new" => self.handle_new_command(&ctx, command).await,
                "status" => self.handle_status_command(&ctx, command).await,
                "session" => {
                    let action = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "action")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("status");
                    self.handle_session_command(&ctx, command, action).await;
                }
                "reset" => {
                    let confirm = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "confirm")
                        .and_then(|o| o.value.as_bool())
                        .unwrap_or(false);
                    self.handle_reset_command(&ctx, command, confirm).await;
                }
                "permissions" => {
                    let action = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "action")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("list");
                    let path = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "path")
                        .and_then(|o| o.value.as_str());
                    self.handle_permissions_command(&ctx, command, action, path)
                        .await;
                }
                "mode" => {
                    if let Some(mode) = command.data.options.first().and_then(|o| o.value.as_str())
                    {
                        self.handle_mode_command(&ctx, command, mode).await;
                    }
                }
                "model" => {
                    if let Some(model) = command.data.options.first().and_then(|o| o.value.as_str())
                    {
                        self.handle_model_command(&ctx, command, model).await;
                    }
                }
                "workspace" => {
                    let action = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "action")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("list");
                    let workspace_id = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "id")
                        .and_then(|o| o.value.as_str());
                    self.handle_workspace_command(&ctx, command, action, workspace_id)
                        .await;
                }
                "persona" => {
                    let action = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "action")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("list");
                    let persona_id = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "id")
                        .and_then(|o| o.value.as_str());
                    let character = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "character")
                        .and_then(|o| o.value.as_str());
                    self.handle_persona_command(&ctx, command, action, persona_id, character)
                        .await;
                }
                "subagent" => {
                    self.handle_subagent_command(&ctx, command).await;
                }
                "lsp" => {
                    self.handle_lsp_command(&ctx, command).await;
                }
                "settings" => self.handle_settings_command(&ctx, command).await,
                "help" => Self::handle_help_command(&ctx, command).await,
                "chat" => {
                    if let Some(message) =
                        command.data.options.first().and_then(|o| o.value.as_str())
                    {
                        self.handle_chat_command(&ctx, command, message).await;
                    }
                }
                "workflow" => {
                    let workflow_name = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "name")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("");
                    let input = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "input")
                        .and_then(|o| o.value.as_str());

                    self.handle_workflow_command(&ctx, command, workflow_name, input)
                        .await;
                }
                "answer" => {
                    if let Some(answer) =
                        command.data.options.first().and_then(|o| o.value.as_str())
                    {
                        self.handle_answer_command(&ctx, command, answer).await;
                    }
                }
                _ => {
                    warn!("Discord: Unknown command: {}", command_name);
                }
            }
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let user_id = msg.author.id.get();

        if !self.is_authorized(user_id).await {
            return;
        }

        let channel_id = msg.channel_id.get();
        set_last_discord_channel_id(channel_id).await;
        self.persist_discord_channel_id(channel_id).await;

        let content = msg.content.trim();

        if content.starts_with('!') || content.starts_with('/') {
            return;
        }

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(id) => id,
            Err(e) => {
                error!("Failed to get/create session: {}", e);
                let embed = CreateEmbed::new()
                    .title("Session Error")
                    .description("Failed to create session. Please try again.")
                    .colour(EMBED_COLOR_ERROR);
                let _ = msg
                    .channel_id
                    .send_message(&ctx.http, CreateMessage::new().embed(embed))
                    .await;
                return;
            }
        };

        info!(
            "Discord: Processing message from user {}: {}",
            user_id, content
        );

        let agent = self.agent.clone();
        let channel_lock = self.get_channel_lock(msg.channel_id.get());
        let ctx_clone = ctx.clone();
        let msg_clone = msg.clone();
        let session_clone = session_id.clone();
        let content_clone = content.to_string();

        tokio::spawn(async move {
            Self::process_channel_message(
                agent,
                channel_lock,
                ctx_clone,
                msg_clone,
                session_clone,
                content_clone,
                user_id,
            )
            .await;
        });
    }
}

async fn run_discord_bot(
    discord_config: DiscordConfig,
    config_path: PathBuf,
    agent: Arc<AgentRuntime>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<(), String> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = Handler::new(agent.clone(), config_path);

    if let Some(channel_id) = discord_config.last_channel_id {
        if channel_id != 0 {
            set_last_discord_channel_id(channel_id).await;
            info!("Restored last Discord channel ID: {}", channel_id);
        }
    }

    let mut client = Client::builder(&discord_config.token, intents)
        .event_handler(handler)
        .await
        .map_err(|e| format!("Error creating Discord client: {e}"))?;

    let shard_manager = client.shard_manager.clone();

    info!("Discord bot starting...");

    let mut client_task = tokio::spawn(async move { client.start().await });

    tokio::select! {
        result = &mut client_task => {
            match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(why)) => Err(format!("Discord bot error: {why:?}")),
                Err(err) => Err(format!("Discord bot task failed: {err}")),
            }
        }
        _ = &mut stop_rx => {
            info!("Discord bot stop requested");
            shard_manager.shutdown_all().await;

            match client_task.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(why)) => Err(format!("Discord bot error: {why:?}")),
                Err(err) => Err(format!("Discord bot task failed: {err}")),
            }
        }
    }
}

pub async fn is_discord_bot_running() -> bool {
    get_discord_bot_state().lock().await.running
}

pub async fn spawn_discord_bot(
    discord_config: DiscordConfig,
    config_path: PathBuf,
    agent: Arc<AgentRuntime>,
) -> Result<(), String> {
    if discord_config.token.trim().is_empty() {
        return Err("Discord bot token is not configured".to_string());
    }

    {
        let mut state = get_discord_bot_state().lock().await;
        if state.running {
            return Err("Discord bot is already running".to_string());
        }

        let (stop_tx, stop_rx) = oneshot::channel();
        state.running = true;
        state.stop_tx = Some(stop_tx);

        tokio::spawn(async move {
            if let Err(err) = run_discord_bot(discord_config, config_path, agent, stop_rx).await {
                error!("{err}");
            }

            let mut state = get_discord_bot_state().lock().await;
            state.running = false;
            state.stop_tx = None;
        });
    }

    Ok(())
}

pub async fn stop_discord_bot() -> bool {
    let stop_tx = {
        let mut state = get_discord_bot_state().lock().await;
        state.stop_tx.take()
    };

    let Some(stop_tx) = stop_tx else {
        return false;
    };

    let _ = stop_tx.send(());
    let _ = timeout(Duration::from_secs(5), async {
        loop {
            if !is_discord_bot_running().await {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    true
}
