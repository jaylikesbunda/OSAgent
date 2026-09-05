//! The single agent-turn runner shared by channel messages, `/chat`, and the
//! command shortcuts that phrase themselves as prompts (`/lsp`, `/subagent`).
//!
//! Every turn renders one status message that is edited in place while the
//! agent works, then replaced by the response. That keeps a channel readable
//! even when a turn fires a dozen tools.

use super::{ui, Handler};
use crate::agent::events::AgentEvent;
use crate::agent::provider::Provider;
use serenity::builder::{CreateEmbed, CreateMessage, EditMessage};
use serenity::http::Http;
use serenity::model::id::{ChannelId, MessageId};
use serenity::prelude::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify};
use tracing::{error, info};

const TURN_TIMEOUT_SECS: u64 = 3600;
const STATUS_EDIT_INTERVAL_MS: u64 = 2500;
const TYPING_PULSE_SECS: u64 = 8;
const TASK_JOIN_TIMEOUT_MS: u64 = 250;

pub(super) struct Turn {
    pub channel_id: ChannelId,
    pub session_id: String,
    pub user_id: u64,
    pub prompt: String,
    pub community: bool,
}

#[derive(Default)]
struct TurnState {
    tools_ok: usize,
    tools_failed: usize,
    current_tool: Option<String>,
    failures: Vec<String>,
    retries: usize,
    compactions: usize,
}

impl TurnState {
    fn tool_summary(&self) -> String {
        let total = self.tools_ok + self.tools_failed;
        match (total, self.tools_failed) {
            (0, _) => "no tools".to_string(),
            (1, 0) => "1 tool".to_string(),
            (n, 0) => format!("{n} tools"),
            (n, f) => format!("{n} tools · {f} failed"),
        }
    }
}

fn status_embed(
    state: &TurnState,
    elapsed: Duration,
    finished: bool,
    community: bool,
) -> CreateEmbed {
    let elapsed_label = ui::humanize_duration(elapsed.as_millis() as u64);

    let headline = if finished {
        "Wrapping up…".to_string()
    } else {
        match &state.current_tool {
            Some(tool) => format!("Running `{tool}`…"),
            None => "Thinking…".to_string(),
        }
    };

    let mut description = format!("{headline}\n-# {} · {elapsed_label}", state.tool_summary());

    if state.retries > 0 {
        description.push_str(&format!(" · {} retries", state.retries));
    }
    if state.compactions > 0 {
        description.push_str(" · context compacted");
    }

    for failure in state.failures.iter().rev().take(3) {
        if community {
            description.push_str("\n✗ A tool failed");
        } else {
            description.push_str(&format!("\n✗ {failure}"));
        }
    }

    CreateEmbed::new()
        .description(description)
        .colour(ui::COLOR_INFO)
}

async fn typing_loop(http: Arc<Http>, channel_id: ChannelId, done: Arc<Notify>) {
    let _ = channel_id.broadcast_typing(&http).await;
    loop {
        tokio::select! {
            _ = done.notified() => break,
            _ = tokio::time::sleep(Duration::from_secs(TYPING_PULSE_SECS)) => {
                let _ = channel_id.broadcast_typing(&http).await;
            }
        }
    }
}

/// Consume agent events for one session, keeping the status message current.
///
/// Edits are throttled so a tool-heavy turn cannot burn the channel's rate limit.
#[allow(clippy::too_many_arguments)]
async fn status_loop(
    http: Arc<Http>,
    channel_id: ChannelId,
    status_id: Option<MessageId>,
    session_id: String,
    mut events: broadcast::Receiver<AgentEvent>,
    state: Arc<AsyncMutex<TurnState>>,
    done: Arc<Notify>,
    started: Instant,
    community: bool,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(STATUS_EDIT_INTERVAL_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // the first tick completes immediately

    let mut dirty = false;

    loop {
        tokio::select! {
            _ = done.notified() => break,
            event = events.recv() => {
                match event {
                    Ok(AgentEvent::ToolStart { session_id: sid, tool_name, .. }) => {
                        if sid != session_id { continue; }
                        state.lock().await.current_tool = Some(tool_name);
                        dirty = true;
                    }
                    Ok(AgentEvent::ToolComplete { session_id: sid, tool_name, success, output, .. }) => {
                        if sid != session_id { continue; }
                        let mut state = state.lock().await;
                        state.current_tool = None;
                        if success {
                            state.tools_ok += 1;
                        } else {
                            state.tools_failed += 1;
                            let detail = output
                                .lines()
                                .map(str::trim)
                                .find(|line| !line.is_empty())
                                .unwrap_or("failed");
                            state.failures.push(format!(
                                "`{tool_name}`: {}",
                                ui::truncate_chars(detail, 120)
                            ));
                        }
                        dirty = true;
                    }
                    Ok(AgentEvent::Retry { session_id: sid, .. }) => {
                        if sid != session_id { continue; }
                        state.lock().await.retries += 1;
                        dirty = true;
                    }
                    Ok(AgentEvent::Compaction { session_id: sid, .. }) => {
                        if sid != session_id { continue; }
                        state.lock().await.compactions += 1;
                        dirty = true;
                    }
                    Ok(AgentEvent::ResponseComplete { session_id: sid, .. })
                    | Ok(AgentEvent::Error { session_id: sid, .. }) => {
                        if sid == session_id { break; }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = ticker.tick() => {
                let Some(status_id) = status_id else { continue };
                if !dirty { continue; }
                dirty = false;

                let embed = {
                    let state = state.lock().await;
                    status_embed(&state, started.elapsed(), false, community)
                };
                if let Err(e) = channel_id
                    .edit_message(&http, status_id, EditMessage::new().embed(embed))
                    .await
                {
                    // A deleted status message (or a lost permission) should not
                    // take the turn down with it.
                    error!("Discord: failed to update status message: {e}");
                    return;
                }
            }
        }
    }
}

impl Handler {
    async fn turn_response_segments(
        &self,
        session_id: &str,
        from_message_index: usize,
        fallback: String,
    ) -> Vec<String> {
        let mut segments = self
            .agent
            .get_session(session_id)
            .await
            .ok()
            .flatten()
            .map(|session| {
                session
                    .messages
                    .into_iter()
                    .skip(from_message_index)
                    .filter(|message| {
                        message.role == "assistant" && !message.content.trim().is_empty()
                    })
                    .filter(|message| {
                        message
                            .tool_calls
                            .as_ref()
                            .map(|calls| calls.is_empty())
                            .unwrap_or(true)
                    })
                    .map(|message| message.content.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        segments.dedup();
        if segments.is_empty() && !fallback.trim().is_empty() {
            segments.push(fallback);
        }
        segments
    }

    /// Footer shown under every response: what answered, and how much it took.
    async fn turn_footer(&self, session_id: &str, elapsed: Duration, state: &TurnState) -> String {
        let provider = self.agent.active_provider().await;
        let model = provider.current_model().await;
        let provider_id = provider.provider_type().to_string();

        let persona = self
            .agent
            .get_session_persona(session_id)
            .await
            .ok()
            .flatten()
            .map(|persona| persona.name)
            .unwrap_or_else(|| "default".to_string());

        format!(
            "{model} · {provider_id} · {persona} · {} · {}",
            state.tool_summary(),
            ui::humanize_duration(elapsed.as_millis() as u64)
        )
    }

    /// Run one agent turn end to end, serialised per channel.
    pub(super) async fn run_turn(&self, ctx: &Context, turn: Turn) {
        let Turn {
            channel_id,
            session_id,
            user_id,
            prompt,
            community,
        } = turn;

        // Route agent-initiated questions for this session back to this channel.
        super::register_session_channel(&session_id, channel_id.get()).await;

        let lock = self.get_channel_lock(channel_id.get());
        let queued_notice = match lock.try_lock() {
            Ok(_) => None,
            Err(_) => channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().embed(ui::embed(
                        "Queued",
                        "Another request is running in this channel. Yours starts as soon as it finishes.",
                        ui::COLOR_INFO,
                    )),
                )
                .await
                .ok()
                .map(|message| message.id),
        };

        let _guard = lock.lock().await;

        if let Some(notice) = queued_notice {
            let _ = channel_id.delete_message(&ctx.http, notice).await;
        }

        let started = Instant::now();
        let state = Arc::new(AsyncMutex::new(TurnState::default()));

        let status_id = channel_id
            .send_message(
                &ctx.http,
                CreateMessage::new().embed(status_embed(
                    &TurnState::default(),
                    Duration::ZERO,
                    false,
                    community,
                )),
            )
            .await
            .ok()
            .map(|message| message.id);

        let typing_done = Arc::new(Notify::new());
        let mut typing_task = tokio::spawn(typing_loop(
            ctx.http.clone(),
            channel_id,
            typing_done.clone(),
        ));

        let status_done = Arc::new(Notify::new());
        let mut status_task = tokio::spawn(status_loop(
            ctx.http.clone(),
            channel_id,
            status_id,
            session_id.clone(),
            self.agent.subscribe_to_events(),
            state.clone(),
            status_done.clone(),
            started,
            community,
        ));

        let initial_message_count = self
            .agent
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .map(|session| session.messages.len())
            .unwrap_or_default();

        let result = tokio::time::timeout(
            Duration::from_secs(TURN_TIMEOUT_SECS),
            self.agent
                .process_message(&session_id, prompt, format!("discord:{user_id}")),
        )
        .await;

        typing_done.notify_one();
        status_done.notify_one();

        for task in [&mut typing_task, &mut status_task] {
            let _ =
                tokio::time::timeout(Duration::from_millis(TASK_JOIN_TIMEOUT_MS), &mut *task).await;
            if !task.is_finished() {
                task.abort();
            }
        }

        let final_state = state.lock().await;
        let elapsed = started.elapsed();

        // The status message has done its job; the response carries the summary.
        let clear_status = |embed: Option<CreateEmbed>| async {
            let Some(status_id) = status_id else { return };
            match embed {
                Some(embed) => {
                    let _ = channel_id
                        .edit_message(&ctx.http, status_id, EditMessage::new().embed(embed))
                        .await;
                }
                None => {
                    let _ = channel_id.delete_message(&ctx.http, status_id).await;
                }
            }
        };

        match result {
            Ok(Ok(response)) if response.trim().is_empty() => {
                let workspace = self
                    .agent
                    .get_session_workspace(&session_id)
                    .await
                    .map(|workspace| format!("Workspace: `{}`", workspace.path))
                    .unwrap_or_else(|_| "Workspace: current active".to_string());

                clear_status(Some(ui::embed(
                    "Done",
                    format!(
                        "The agent finished without a text reply.\n{workspace}\n-# {}",
                        self.turn_footer(&session_id, elapsed, &final_state).await
                    ),
                    ui::COLOR_SUCCESS,
                )))
                .await;
            }
            Ok(Ok(response)) => {
                clear_status(None).await;
                let footer = self.turn_footer(&session_id, elapsed, &final_state).await;
                let segments = self
                    .turn_response_segments(&session_id, initial_message_count, response)
                    .await;
                let last = segments.len().saturating_sub(1);
                for (index, segment) in segments.iter().enumerate() {
                    ui::send_chunks(
                        &ctx.http,
                        channel_id,
                        segment,
                        (index == last).then_some(footer.as_str()),
                    )
                    .await;
                }
                info!(
                    "Discord: delivered {} response segment(s) for session {}",
                    segments.len(),
                    session_id
                );
                info!("Discord: turn complete for session {session_id} in {elapsed:?}");
            }
            Ok(Err(e)) => {
                error!("Discord: turn failed for session {session_id}: {e}");
                let (title, description) = if community {
                    ui::describe_community_error(&e.to_string())
                } else {
                    ui::describe_error(&e.to_string())
                };
                clear_status(Some(ui::embed(&title, description, ui::COLOR_ERROR))).await;
            }
            Err(_) => {
                error!("Discord: turn timed out for session {session_id}");
                clear_status(Some(ui::embed(
                    "Timed Out",
                    "The agent did not finish within 60 minutes. Try a narrower request, or check the desktop app for what it was doing.",
                    ui::COLOR_ERROR,
                )))
                .await;
            }
        }
    }
}
