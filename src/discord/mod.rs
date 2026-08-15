//! Discord surface for the agent.
//!
//! * [`chat`] runs an agent turn and renders it in a channel.
//! * [`commands`] registers and dispatches slash commands.
//! * [`panel`] is the `/settings` control panel and every component it owns.
//! * [`ui`] holds the shared rendering helpers.

mod chat;
mod commands;
mod panel;
mod ui;

use crate::agent::events::AgentEvent;
use crate::agent::runtime::AgentRuntime;
use crate::config::DiscordConfig;
use crate::workflow::db::WorkflowDb;
use crate::workflow::executor::WorkflowExecutor;
use dashmap::DashMap;
use serenity::{
    async_trait,
    builder::{
        CreateActionRow, CreateButton, CreateEmbed, CreateInteractionResponse,
        CreateInteractionResponseMessage, CreateMessage,
    },
    model::{
        application::{ButtonStyle, ComponentInteraction, Interaction},
        channel::Message,
        gateway::Ready,
        id::ChannelId,
    },
    prelude::*,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tracing::{error, info, warn};

/// Channel a session's agent-initiated questions should be delivered to.
static SESSION_TO_CHANNEL: std::sync::OnceLock<tokio::sync::RwLock<HashMap<String, u64>>> =
    std::sync::OnceLock::new();

/// Fallback delivery target for notifications that carry no channel of their own.
static LAST_DISCORD_CHANNEL: std::sync::OnceLock<tokio::sync::RwLock<u64>> =
    std::sync::OnceLock::new();

/// Set once the gateway hands us the bot's identity; used for mention gating.
static BOT_USER_ID: AtomicU64 = AtomicU64::new(0);
static GITHUB_TRACKER_GENERATION: AtomicU64 = AtomicU64::new(0);

static DISCORD_BOT_STATE: std::sync::OnceLock<tokio::sync::Mutex<DiscordBotState>> =
    std::sync::OnceLock::new();

const MAX_TRACKED_CHANNEL_LOCKS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AccessLevel {
    Community,
    Trusted,
}

fn session_to_channel() -> &'static tokio::sync::RwLock<HashMap<String, u64>> {
    SESSION_TO_CHANNEL.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

fn last_channel() -> &'static tokio::sync::RwLock<u64> {
    LAST_DISCORD_CHANNEL.get_or_init(|| tokio::sync::RwLock::new(0))
}

/// Remember where a session is being talked to, so questions the agent asks
/// mid-turn land back in the same channel.
pub(crate) async fn register_session_channel(session_id: &str, channel_id: u64) {
    let mut map = session_to_channel().write().await;
    if map.get(session_id).copied() != Some(channel_id) {
        map.insert(session_id.to_string(), channel_id);
    }
}

pub async fn get_last_discord_channel_id() -> u64 {
    *last_channel().read().await
}

pub async fn set_last_discord_channel_id(channel_id: u64) {
    *last_channel().write().await = channel_id;
}

struct DiscordBotState {
    running: bool,
    stop_tx: Option<oneshot::Sender<()>>,
}

fn bot_state() -> &'static tokio::sync::Mutex<DiscordBotState> {
    DISCORD_BOT_STATE.get_or_init(|| {
        tokio::sync::Mutex::new(DiscordBotState {
            running: false,
            stop_tx: None,
        })
    })
}

#[derive(Debug, Clone)]
struct PendingQuestion {
    question_id: String,
    questions: Vec<crate::tools::question::Question>,
}

pub struct Handler {
    agent: Arc<AgentRuntime>,
    config_path: PathBuf,
    /// Discord user id -> session id.
    sessions: Arc<tokio::sync::RwLock<HashMap<u64, String>>>,
    /// One agent turn at a time per channel.
    channel_locks: Arc<DashMap<u64, Arc<Mutex<()>>>>,
    /// Session id -> the question that session is blocked on.
    pending_questions: Arc<tokio::sync::RwLock<HashMap<String, PendingQuestion>>>,
    /// `ready` fires again on every gateway resume; setup must not.
    initialised: Arc<AtomicBool>,
}

impl Handler {
    pub fn new(agent: Arc<AgentRuntime>, config_path: PathBuf) -> Self {
        Self {
            agent,
            config_path,
            sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            channel_locks: Arc::new(DashMap::new()),
            pending_questions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            initialised: Arc::new(AtomicBool::new(false)),
        }
    }

    // -----------------------------------------------------------------------
    // authorization
    // -----------------------------------------------------------------------

    /// User and role grants are additive. Guild and channel lists are optional
    /// location restrictions. With no user or role grants, access fails closed.
    pub(super) async fn access_level(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
        channel_id: u64,
        role_ids: &[u64],
    ) -> Option<AccessLevel> {
        let Some(discord) = self.agent.discord_config().await else {
            return None;
        };

        if !discord.community_mode {
            return discord
                .allowed_users
                .contains(&user_id)
                .then_some(AccessLevel::Trusted);
        }

        let in_location = |guilds: &[u64], channels: &[u64]| {
            guild_id.is_some_and(|guild_id| {
                (guilds.is_empty() || guilds.contains(&guild_id))
                    && (channels.is_empty() || channels.contains(&channel_id))
            })
        };
        let trusted_identity = discord.trusted_users.contains(&user_id)
            || role_ids
                .iter()
                .any(|role_id| discord.trusted_roles.contains(role_id));
        if trusted_identity
            && (guild_id.is_none() && discord.allow_dms && discord.trusted_users.contains(&user_id)
                || !discord.trusted_guilds.is_empty()
                    && in_location(&discord.trusted_guilds, &discord.trusted_channels))
        {
            return Some(AccessLevel::Trusted);
        }

        let identity_allowed = discord.allowed_users.contains(&user_id)
            || role_ids
                .iter()
                .any(|role_id| discord.allowed_roles.contains(role_id));
        if !identity_allowed {
            return None;
        }

        let Some(guild_id) = guild_id else {
            return (discord.allow_dms && discord.allowed_users.contains(&user_id))
                .then_some(AccessLevel::Community);
        };

        ((discord.allowed_guilds.is_empty() || discord.allowed_guilds.contains(&guild_id))
            && (discord.allowed_channels.is_empty()
                || discord.allowed_channels.contains(&channel_id)))
        .then_some(AccessLevel::Community)
    }

    pub(super) async fn has_explicit_trusted_user(&self, user_id: u64) -> bool {
        self.agent
            .discord_config()
            .await
            .is_some_and(|discord| discord.trusted_users.contains(&user_id))
    }

    async fn is_authorized(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
        channel_id: u64,
        role_ids: &[u64],
    ) -> bool {
        self.access_level(user_id, guild_id, channel_id, role_ids)
            .await
            == Some(AccessLevel::Trusted)
    }

    fn community_owner_key(user_id: u64, guild_id: Option<u64>) -> String {
        format!(
            "discord-community:{}:{user_id}",
            guild_id.unwrap_or_default()
        )
    }

    async fn get_active_community_session_id(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
    ) -> Option<String> {
        self.agent
            .get_session_id_for_user(&Self::community_owner_key(user_id, guild_id))
            .await
            .ok()
            .flatten()
    }

    pub(super) async fn archive_community_session(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
    ) -> Result<Option<String>, String> {
        let Some(session_id) = self
            .get_active_community_session_id(user_id, guild_id)
            .await
        else {
            return Ok(None);
        };
        self.agent
            .archive_session(&session_id)
            .await
            .map_err(|error| format!("Failed to archive session: {error}"))?;
        Ok(Some(session_id))
    }

    pub(super) async fn start_new_community_session(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
    ) -> Result<String, String> {
        self.archive_community_session(user_id, guild_id).await?;
        self.get_or_create_community_session(user_id, guild_id)
            .await
    }

    pub(super) async fn delete_community_session(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
    ) -> Result<bool, String> {
        let Some(session_id) = self
            .get_active_community_session_id(user_id, guild_id)
            .await
        else {
            return Ok(false);
        };
        self.agent
            .delete_session(&session_id)
            .await
            .map_err(|error| format!("Failed to delete session: {error}"))?;
        Ok(true)
    }

    async fn get_or_create_community_session(
        &self,
        user_id: u64,
        guild_id: Option<u64>,
    ) -> Result<String, String> {
        let owner = Self::community_owner_key(user_id, guild_id);
        let session_id = match self.agent.get_session_id_for_user(&owner).await {
            Ok(Some(session_id)) => session_id,
            Ok(None) => {
                self.agent
                    .create_session_for_user(&owner, "discord-community")
                    .await
                    .map_err(|e| format!("Failed to create community session: {e}"))?
                    .id
            }
            Err(e) => return Err(format!("Failed to load community session: {e}")),
        };
        let discord = self.agent.discord_config().await.unwrap_or_default();
        let context = format!(
            "{}\n\nYou are operating in Discord community support mode. Never claim to access the host machine or private data. Help with the project, use public web research when useful, and cite sources.{}",
            discord.community_context.trim(),
            if discord.docs_url.trim().is_empty() {
                String::new()
            } else {
                format!(" The canonical documentation starts at {}.", discord.docs_url.trim())
            }
        );
        self.agent
            .set_discord_community_profile(&session_id, context)
            .await
            .map_err(|e| format!("Failed to apply community safety profile: {e}"))?;
        Ok(session_id)
    }

    async fn send_unauthorized_response_command(
        ctx: &Context,
        command: &serenity::model::application::CommandInteraction,
    ) {
        let embed = ui::embed(
            "Access Denied",
            "You are not on this bot's allow-list. The owner can add your user id under `discord.allowed_users` in the config, or from the web UI.",
            ui::COLOR_ERROR,
        );

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
            error!("Discord: failed to send unauthorized response: {e}");
        }
    }

    // -----------------------------------------------------------------------
    // sessions
    // -----------------------------------------------------------------------

    fn session_is_archived(session: &crate::storage::Session) -> bool {
        session
            .metadata
            .get("archived")
            .and_then(|value| value.as_bool())
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

        let resolved = self
            .agent
            .get_session_id_for_user(&Self::owner_key(user_id))
            .await
            .ok()
            .flatten();

        if let Some(session_id) = resolved.as_ref() {
            self.sessions
                .write()
                .await
                .insert(user_id, session_id.clone());
        }

        resolved
    }

    fn owner_key(user_id: u64) -> String {
        format!("discord:{user_id}")
    }

    async fn get_or_create_session(&self, user_id: u64) -> Result<String, String> {
        if let Some(session_id) = self.get_active_session_id_for_user(user_id).await {
            return Ok(session_id);
        }

        match self
            .agent
            .create_session_for_user(&Self::owner_key(user_id), "discord")
            .await
        {
            Ok(session) => {
                self.sessions
                    .write()
                    .await
                    .insert(user_id, session.id.clone());
                Ok(session.id)
            }
            Err(e) => Err(format!("Failed to create session: {e}")),
        }
    }

    async fn archive_current_session_for_user(
        &self,
        user_id: u64,
    ) -> Result<Option<String>, String> {
        let Some(session_id) = self.get_active_session_id_for_user(user_id).await else {
            return Ok(None);
        };

        self.agent
            .archive_session(&session_id)
            .await
            .map_err(|e| format!("Failed to archive session: {e}"))?;

        self.forget_session(user_id, &session_id).await;
        Ok(Some(session_id))
    }

    async fn start_new_session(&self, user_id: u64) -> Result<String, String> {
        self.archive_current_session_for_user(user_id).await?;
        self.get_or_create_session(user_id).await
    }

    async fn delete_current_session(&self, user_id: u64) -> Result<bool, String> {
        let Some(session_id) = self.get_active_session_id_for_user(user_id).await else {
            return Ok(false);
        };

        self.agent
            .delete_session(&session_id)
            .await
            .map_err(|e| format!("Failed to delete session: {e}"))?;

        self.forget_session(user_id, &session_id).await;
        Ok(true)
    }

    async fn forget_session(&self, user_id: u64, session_id: &str) {
        self.sessions.write().await.remove(&user_id);
        self.pending_questions.write().await.remove(session_id);
        session_to_channel().write().await.remove(session_id);
    }

    // -----------------------------------------------------------------------
    // channels
    // -----------------------------------------------------------------------

    fn get_channel_lock(&self, channel_id: u64) -> Arc<Mutex<()>> {
        // Locks for channels nobody is using any more are dropped once the map
        // grows past a sensible size, so a long-lived bot does not accumulate
        // one entry per channel it has ever seen.
        if self.channel_locks.len() > MAX_TRACKED_CHANNEL_LOCKS {
            self.channel_locks
                .retain(|_, lock| Arc::strong_count(lock) > 1);
        }

        self.channel_locks
            .entry(channel_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Make this the fallback channel for notifications that carry no target.
    async fn remember_channel(&self, channel_id: u64) {
        set_last_discord_channel_id(channel_id).await;

        let config = self.agent.config();
        let mut cfg = config.write().await;
        let changed = match &mut cfg.discord {
            Some(discord) => {
                let changed = discord.last_channel_id != Some(channel_id);
                discord.last_channel_id = Some(channel_id);
                changed
            }
            None => {
                cfg.discord = Some(DiscordConfig {
                    last_channel_id: Some(channel_id),
                    ..Default::default()
                });
                true
            }
        };
        drop(cfg);

        if changed {
            if let Err(e) = self.agent.save_config(&self.config_path).await {
                warn!("Discord: failed to persist last channel id: {e}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // questions
    // -----------------------------------------------------------------------

    async fn present_question(
        &self,
        http: &serenity::http::Http,
        session_id: &str,
        question_id: &str,
        channel_id: u64,
        questions: &[crate::tools::question::Question],
    ) {
        for question in questions {
            let mut description = format!("**{}**\n\n", question.question);

            for (index, option) in question.options.iter().enumerate() {
                let label = if option.label.is_empty() {
                    format!("Option {}", index + 1)
                } else {
                    option.label.clone()
                };
                description.push_str(&format!("`{}` — {label}\n", index + 1));
            }

            description.push_str(if question.options.is_empty() {
                "\nReply with `/answer <your text>`"
            } else {
                "\nReply with `/answer <number>` or `/answer <your text>`"
            });

            let title = if question.header.is_empty() {
                "Question"
            } else {
                question.header.as_str()
            };

            if let Err(e) = ChannelId::new(channel_id)
                .send_message(
                    http,
                    CreateMessage::new().embed(ui::embed(title, description, ui::COLOR_WARNING)),
                )
                .await
            {
                error!("Discord: failed to send question: {e}");
            }
        }

        self.pending_questions.write().await.insert(
            session_id.to_string(),
            PendingQuestion {
                question_id: question_id.to_string(),
                questions: questions.to_vec(),
            },
        );
    }

    /// Resolve `/answer` against the question *this user's* session is blocked on.
    async fn submit_answer(&self, user_id: u64, answer: &str) -> Result<String, String> {
        let session_id = self
            .get_active_session_id_for_user(user_id)
            .await
            .ok_or("You have no active session.")?;

        let pending = self
            .pending_questions
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or("The agent is not waiting on an answer from you right now.")?;

        // `2` should mean the second option, not the literal string "2".
        let resolved = pending
            .questions
            .first()
            .and_then(|question| {
                answer
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|index| *index >= 1 && *index <= question.options.len())
                    .map(|index| question.options[index - 1].label.clone())
            })
            .unwrap_or_else(|| answer.trim().to_string());

        if resolved.is_empty() {
            return Err("Your answer was empty.".to_string());
        }

        let accepted = self
            .agent
            .answer_question(&pending.question_id, vec![vec![resolved.clone()]])
            .await;

        self.pending_questions.write().await.remove(&session_id);

        if accepted {
            Ok(resolved)
        } else {
            Err("That question has already been answered, or it expired.".to_string())
        }
    }

    // -----------------------------------------------------------------------
    // workflows
    // -----------------------------------------------------------------------

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
            .map_err(|e| format!("Failed to initialize workflow db: {e}"))?;

        let (executor, _event_rx) = WorkflowExecutor::new(
            workflow_db.clone(),
            self.agent.get_subagent_manager(),
            self.agent.event_bus().clone(),
        );
        Ok((workflow_db, Arc::new(executor)))
    }

    fn format_workflow_output(output: &serde_json::Value) -> String {
        let text = if let Some(text) = output.as_str() {
            text.to_string()
        } else if let Some(object) = output.as_object() {
            match object.get("output") {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(value) => value.to_string(),
                None => output.to_string(),
            }
        } else {
            output.to_string()
        };

        ui::truncate_chars(&text, 1800)
    }

    /// Approve/reject buttons posted by a workflow gate.
    async fn handle_workflow_component(&self, ctx: &Context, component: &ComponentInteraction) {
        let custom_id = component.data.custom_id.as_str();
        let (question_id, answer, approved) =
            if let Some(question_id) = custom_id.strip_prefix("wf_approve:") {
                (question_id, "Approve", true)
            } else if let Some(question_id) = custom_id.strip_prefix("wf_reject:") {
                (question_id, "Reject", false)
            } else {
                return;
            };

        // Anyone can see an approval prompt in a channel; only allow-listed
        // users may act on it.
        let roles = component
            .member
            .as_ref()
            .map(|member| {
                member
                    .roles
                    .iter()
                    .map(|role| role.get())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !self
            .is_authorized(
                component.user.id.get(),
                component.guild_id.map(|id| id.get()),
                component.channel_id.get(),
                &roles,
            )
            .await
        {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .embed(ui::embed(
                                "Not Allowed",
                                "You are not authorized to approve workflow steps.",
                                ui::COLOR_ERROR,
                            ))
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }

        let accepted = self
            .agent
            .answer_question(question_id, vec![vec![answer.to_string()]])
            .await;

        let label = if approved { "Approved" } else { "Rejected" };
        let text = if accepted {
            format!("{label} by {}", component.user.name)
        } else {
            "This approval request is no longer active.".to_string()
        };

        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content(text)
                        .components(vec![CreateActionRow::Buttons(vec![CreateButton::new(
                            "workflow-approval-handled",
                        )
                        .label(label)
                        .style(if approved {
                            ButtonStyle::Success
                        } else {
                            ButtonStyle::Danger
                        })
                        .disabled(true)])]),
                ),
            )
            .await;
    }

    // -----------------------------------------------------------------------
    // notifications
    // -----------------------------------------------------------------------

    /// Deliver agent-initiated events (questions, schedules, workflows) to Discord.
    ///
    /// Spawned exactly once per bot; `ready` fires again on every gateway
    /// resume and must not start a second copy, or every notification is
    /// delivered once per reconnect.
    fn spawn_notification_task(&self, http: Arc<serenity::http::Http>) {
        let handler_agent = self.agent.clone();
        let pending_questions = self.pending_questions.clone();
        let agent = self.agent.clone();
        let config_path = self.config_path.clone();
        let sessions = self.sessions.clone();
        let channel_locks = self.channel_locks.clone();
        let initialised = self.initialised.clone();

        // A detached clone so the task can reuse the session/question helpers.
        let handler = Handler {
            agent: handler_agent,
            config_path,
            sessions,
            channel_locks,
            pending_questions,
            initialised,
        };

        tokio::spawn(async move {
            let mut events = agent.subscribe_to_events();

            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Discord: notification stream lagged, {skipped} events dropped");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };

                match event {
                    AgentEvent::QuestionAsked {
                        question_id,
                        session_id,
                        questions,
                        ..
                    } => {
                        let channel_id =
                            session_to_channel().read().await.get(&session_id).copied();
                        let channel_id = match channel_id {
                            Some(channel_id) => channel_id,
                            None => get_last_discord_channel_id().await,
                        };

                        if channel_id == 0 {
                            warn!("Discord: question for session {session_id} has nowhere to go");
                            continue;
                        }

                        handler
                            .present_question(
                                &http,
                                &session_id,
                                &question_id,
                                channel_id,
                                &questions,
                            )
                            .await;
                    }
                    AgentEvent::ScheduledJobFired {
                        notify_channels,
                        message,
                        job_id,
                        job_type,
                        session_id,
                        discord_channel_id,
                        ..
                    } => {
                        if !notify_channels.iter().any(|channel| channel == "discord") {
                            continue;
                        }

                        let session_channel = match &session_id {
                            Some(session_id) => {
                                session_to_channel().read().await.get(session_id).copied()
                            }
                            None => None,
                        };

                        let Some(channel_id) =
                            resolve_channel(discord_channel_id.or(session_channel)).await
                        else {
                            info!("Discord: scheduled job {job_id} has no delivery channel");
                            continue;
                        };

                        let title = match job_type.as_str() {
                            "daily_briefing" => "Daily Briefing".to_string(),
                            other => other.replace('_', " "),
                        };

                        let embed =
                            ui::embed(&format!("Scheduled {title}"), &message, ui::COLOR_INFO)
                                .field("Job", job_id.chars().take(8).collect::<String>(), true)
                                .field("Type", &job_type, true);

                        send_notification(&http, channel_id, embed, None).await;
                    }
                    AgentEvent::WorkflowApprovalRequested {
                        notify_channels,
                        discord_channel_id,
                        prompt,
                        approve_label,
                        reject_label,
                        question_id,
                        workflow_id,
                        run_id,
                        ..
                    } => {
                        if !notify_channels.iter().any(|channel| channel == "discord") {
                            continue;
                        }
                        let Some(channel_id) = resolve_channel(discord_channel_id).await else {
                            continue;
                        };

                        let embed =
                            ui::embed("Workflow Approval Required", prompt, ui::COLOR_WARNING)
                                .field("Workflow", &workflow_id, true)
                                .field("Run", run_id.chars().take(8).collect::<String>(), true);

                        let buttons = CreateActionRow::Buttons(vec![
                            CreateButton::new(format!("wf_approve:{question_id}"))
                                .label(approve_label)
                                .style(ButtonStyle::Success),
                            CreateButton::new(format!("wf_reject:{question_id}"))
                                .label(reject_label)
                                .style(ButtonStyle::Danger),
                        ]);

                        send_notification(&http, channel_id, embed, Some(buttons)).await;
                    }
                    AgentEvent::WorkflowCompleted {
                        notify_channels,
                        discord_channel_id,
                        output,
                        workflow_id,
                        run_id,
                        ..
                    } => {
                        if !notify_channels.iter().any(|channel| channel == "discord") {
                            continue;
                        }
                        let Some(channel_id) = resolve_channel(discord_channel_id).await else {
                            continue;
                        };

                        let embed = ui::embed(
                            "Workflow Completed",
                            output
                                .as_ref()
                                .map(Handler::format_workflow_output)
                                .unwrap_or_else(|| "Workflow completed.".to_string()),
                            ui::COLOR_SUCCESS,
                        )
                        .field("Workflow", &workflow_id, true)
                        .field(
                            "Run",
                            run_id.chars().take(8).collect::<String>(),
                            true,
                        );

                        send_notification(&http, channel_id, embed, None).await;
                    }
                    AgentEvent::WorkflowFailed {
                        notify_channels,
                        discord_channel_id,
                        error,
                        workflow_id,
                        run_id,
                        ..
                    } => {
                        if !notify_channels.iter().any(|channel| channel == "discord") {
                            continue;
                        }
                        let Some(channel_id) = resolve_channel(discord_channel_id).await else {
                            continue;
                        };

                        let embed = ui::embed("Workflow Failed", error, ui::COLOR_ERROR)
                            .field("Workflow", &workflow_id, true)
                            .field("Run", run_id.chars().take(8).collect::<String>(), true);

                        send_notification(&http, channel_id, embed, None).await;
                    }
                    _ => {}
                }
            }
        });
    }

    fn spawn_github_tracking_task(&self, http: Arc<serenity::http::Http>) {
        let agent = self.agent.clone();
        let generation = GITHUB_TRACKER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::spawn(async move {
            let client = match reqwest::Client::builder()
                .user_agent("OSAgent Discord GitHub tracker")
                .timeout(Duration::from_secs(20))
                .build()
            {
                Ok(client) => client,
                Err(error) => {
                    warn!("Discord: could not create GitHub tracking client: {error}");
                    return;
                }
            };
            let mut active_repo = String::new();
            let mut seen = HashSet::new();
            let mut initialized = false;

            loop {
                if GITHUB_TRACKER_GENERATION.load(Ordering::SeqCst) != generation {
                    return;
                }
                let discord = agent.discord_config().await.unwrap_or_default();
                let repo = discord.github_repo.trim();
                let channel = discord.github_tracking_channel;
                if repo != active_repo {
                    active_repo = repo.to_string();
                    seen.clear();
                    initialized = false;
                }

                let valid_repo = repo.split_once('/').is_some_and(|(owner, name)| {
                    !owner.is_empty()
                        && !name.is_empty()
                        && owner
                            .chars()
                            .chain(name.chars())
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                });
                if valid_repo {
                    if let Some(channel_id) = channel {
                        let url = format!(
                            "https://api.github.com/repos/{repo}/issues?state=all&sort=created&direction=desc&per_page=30"
                        );
                        let mut request = client
                            .get(url)
                            .header("Accept", "application/vnd.github+json");
                        if !discord.github_token.trim().is_empty() {
                            request = request.bearer_auth(discord.github_token.trim());
                        }
                        match request.send().await {
                            Ok(response) if response.status().is_success() => {
                                match response.json::<Vec<serde_json::Value>>().await {
                                    Ok(items) => {
                                        let current = items
                                            .iter()
                                            .filter_map(|item| {
                                                item.get("id").and_then(|id| id.as_u64())
                                            })
                                            .collect::<HashSet<_>>();
                                        if initialized {
                                            for item in items.iter().rev() {
                                                let Some(id) =
                                                    item.get("id").and_then(|id| id.as_u64())
                                                else {
                                                    continue;
                                                };
                                                if seen.contains(&id) {
                                                    continue;
                                                }
                                                let is_pr = item.get("pull_request").is_some();
                                                let kind =
                                                    if is_pr { "Pull Request" } else { "Issue" };
                                                let number = item
                                                    .get("number")
                                                    .and_then(|value| value.as_u64())
                                                    .unwrap_or_default();
                                                let title = item
                                                    .get("title")
                                                    .and_then(|value| value.as_str())
                                                    .unwrap_or("Untitled");
                                                let author = item
                                                    .pointer("/user/login")
                                                    .and_then(|value| value.as_str())
                                                    .unwrap_or("unknown");
                                                let link = item
                                                    .get("html_url")
                                                    .and_then(|value| value.as_str())
                                                    .unwrap_or("https://github.com");
                                                let embed = CreateEmbed::new()
                                                    .title(format!("New {kind} #{number}"))
                                                    .description(ui::truncate_chars(title, 1000))
                                                    .url(link)
                                                    .field("Repository", repo, true)
                                                    .field("Author", author, true)
                                                    .colour(ui::COLOR_INFO);
                                                send_notification(&http, channel_id, embed, None)
                                                    .await;
                                            }
                                        }
                                        seen = current;
                                        initialized = true;
                                    }
                                    Err(error) => {
                                        warn!("Discord: invalid GitHub tracking response: {error}")
                                    }
                                }
                            }
                            Ok(response) => {
                                warn!("Discord: GitHub tracking returned {}", response.status())
                            }
                            Err(error) => warn!("Discord: GitHub tracking request failed: {error}"),
                        }
                    }
                }

                sleep(Duration::from_secs(
                    discord.github_poll_seconds.clamp(60, 3600),
                ))
                .await;
            }
        });
    }
}

async fn resolve_channel(preferred: Option<u64>) -> Option<u64> {
    match preferred {
        Some(channel_id) if channel_id != 0 => Some(channel_id),
        _ => match get_last_discord_channel_id().await {
            0 => None,
            channel_id => Some(channel_id),
        },
    }
}

async fn send_notification(
    http: &serenity::http::Http,
    channel_id: u64,
    embed: CreateEmbed,
    components: Option<CreateActionRow>,
) {
    let mut message = CreateMessage::new().embed(embed);
    if let Some(row) = components {
        message = message.components(vec![row]);
    }

    if let Err(e) = ChannelId::new(channel_id).send_message(http, message).await {
        warn!("Discord: failed to deliver notification to {channel_id}: {e}");
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        BOT_USER_ID.store(ready.user.id.get(), Ordering::Relaxed);

        if self.initialised.swap(true, Ordering::SeqCst) {
            info!("Discord: gateway resumed as {}", ready.user.name);
            return;
        }

        info!("Discord: connected as {}", ready.user.name);
        self.register_commands(&ctx.http).await;
        self.spawn_notification_task(ctx.http.clone());
        self.spawn_github_tracking_task(ctx.http.clone());
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Autocomplete(command) => {
                self.dispatch_autocomplete(&ctx, &command).await;
            }
            Interaction::Component(component) => {
                if self.handle_panel_component(&ctx, &component).await {
                    return;
                }
                self.handle_workflow_component(&ctx, &component).await;
            }
            Interaction::Command(command) => {
                self.dispatch_command(&ctx, &command).await;
            }
            _ => {}
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let user_id = msg.author.id.get();
        let bot_id = BOT_USER_ID.load(Ordering::Relaxed);
        let is_dm = msg.guild_id.is_none();

        // In a server, only speak when spoken to. Without this the bot answers
        // every message in every channel it can see.
        let mentioned = bot_id != 0 && msg.mentions.iter().any(|user| user.id.get() == bot_id);
        let replied_to_bot = msg
            .referenced_message
            .as_ref()
            .is_some_and(|referenced| referenced.author.id.get() == bot_id);

        if !is_dm && !mentioned && !replied_to_bot {
            return;
        }

        let roles = msg
            .member
            .as_ref()
            .map(|member| {
                member
                    .roles
                    .iter()
                    .map(|role| role.get())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let access = self
            .access_level(
                user_id,
                msg.guild_id.map(|id| id.get()),
                msg.channel_id.get(),
                &roles,
            )
            .await;
        let Some(access) = access else {
            return;
        };

        let content = strip_mention(&msg.content, bot_id);
        if content.is_empty() || content.starts_with('!') || content.starts_with('/') {
            return;
        }

        if access == AccessLevel::Trusted {
            self.remember_channel(msg.channel_id.get()).await;
        }

        let session_result = match access {
            AccessLevel::Trusted => self.get_or_create_session(user_id).await,
            AccessLevel::Community => {
                self.get_or_create_community_session(
                    user_id,
                    msg.guild_id.map(|guild_id| guild_id.get()),
                )
                .await
            }
        };
        let session_id = match session_result {
            Ok(session_id) => session_id,
            Err(e) => {
                error!("Discord: {e}");
                let _ = msg
                    .channel_id
                    .send_message(
                        &ctx.http,
                        CreateMessage::new().embed(ui::embed(
                            "Session Error",
                            "Could not open a session. Try again in a moment.",
                            ui::COLOR_ERROR,
                        )),
                    )
                    .await;
                return;
            }
        };

        info!("Discord: message from {user_id} ({} chars)", content.len());

        self.run_turn(
            &ctx,
            chat::Turn {
                channel_id: msg.channel_id,
                session_id,
                user_id,
                prompt: content,
            },
        )
        .await;
    }
}

fn strip_mention(content: &str, bot_id: u64) -> String {
    if bot_id == 0 {
        return content.trim().to_string();
    }
    content
        .replace(&format!("<@{bot_id}>"), " ")
        .replace(&format!("<@!{bot_id}>"), " ")
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// lifecycle
// ---------------------------------------------------------------------------

async fn run_discord_bot(
    discord_config: DiscordConfig,
    config_path: PathBuf,
    agent: Arc<AgentRuntime>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<(), String> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    if let Some(channel_id) = discord_config.last_channel_id {
        if channel_id != 0 {
            set_last_discord_channel_id(channel_id).await;
            info!("Discord: restored last channel {channel_id}");
        }
    }

    if discord_config.allowed_users.is_empty()
        && discord_config.allowed_roles.is_empty()
        && discord_config.trusted_users.is_empty()
        && discord_config.trusted_roles.is_empty()
    {
        warn!(
            "Discord: allowed_users and allowed_roles are empty — the bot will refuse every request."
        );
    }

    let mut client = Client::builder(&discord_config.token, intents)
        .event_handler(Handler::new(agent, config_path))
        .await
        .map_err(|e| format!("Error creating Discord client: {e}"))?;

    let shard_manager = client.shard_manager.clone();
    let mut client_task = tokio::spawn(async move { client.start().await });

    info!("Discord: bot starting");

    tokio::select! {
        result = &mut client_task => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(why)) => Err(format!("Discord bot error: {why:?}")),
            Err(err) => Err(format!("Discord bot task failed: {err}")),
        },
        _ = &mut stop_rx => {
            info!("Discord: stop requested");
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
    bot_state().lock().await.running
}

pub async fn spawn_discord_bot(
    discord_config: DiscordConfig,
    config_path: PathBuf,
    agent: Arc<AgentRuntime>,
) -> Result<(), String> {
    if discord_config.token.trim().is_empty() {
        return Err("Discord bot token is not configured".to_string());
    }

    let mut state = bot_state().lock().await;
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

        let mut state = bot_state().lock().await;
        state.running = false;
        state.stop_tx = None;
    });

    Ok(())
}

pub async fn stop_discord_bot() -> bool {
    GITHUB_TRACKER_GENERATION.fetch_add(1, Ordering::SeqCst);
    let stop_tx = bot_state().lock().await.stop_tx.take();

    let Some(stop_tx) = stop_tx else {
        return false;
    };

    let _ = stop_tx.send(());
    let _ = timeout(Duration::from_secs(5), async {
        while is_discord_bot_running().await {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_is_stripped_from_content() {
        assert_eq!(strip_mention("<@42> hello there", 42), "hello there");
        assert_eq!(strip_mention("<@!42> hello", 42), "hello");
        assert_eq!(strip_mention("hello <@42>", 42), "hello");
    }

    #[test]
    fn unknown_bot_id_leaves_content_alone() {
        assert_eq!(strip_mention("  hello <@42>  ", 0), "hello <@42>");
    }
}
