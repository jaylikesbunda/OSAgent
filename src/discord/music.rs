//! Discord voice music — YouTube + HTTP audio via `songbird`.
//! Simple, robust, excellent UX.

use super::{ui, Handler};
use serenity::{
    builder::{
        CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
        CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse,
    },
    model::{
        application::{CommandDataOptionValue, CommandInteraction},
        id::{ChannelId, GuildId},
    },
    prelude::*,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

// ── helpers — always compiled ─────────────────────────────────────────────────

fn opt_str<'a>(
    options: &'a [serenity::model::application::CommandDataOption],
    name: &str,
) -> Option<&'a str> {
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.value.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

pub fn extract_youtube_id(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(pos) = url.find("youtu.be/") {
        let id = url[pos + "youtu.be/".len()..]
            .split(['?', '&', '/', '#'])
            .next()
            .unwrap_or("")
            .trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    for pat in ["v=", "/embed/", "/v/", "/shorts/"] {
        if let Some(pos) = url.find(pat) {
            let id = url[pos + pat.len()..]
                .split(['?', '&', '/', '#', '"', '\''])
                .next()
                .unwrap_or("")
                .trim();
            if !id.is_empty() && id.len() <= 20 {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub fn format_duration(dur: Option<Duration>) -> String {
    match dur {
        Some(d) => {
            let s = d.as_secs();
            let m = s / 60;
            let h = m / 60;
            if h > 0 {
                format!("{h}:{:02}:{:02}", m % 60, s % 60)
            } else {
                format!("{m}:{:02}", s % 60)
            }
        }
        None => "—".to_string(),
    }
}

fn music_disabled_embed() -> CreateEmbed {
    ui::embed("Music Not Enabled",
        "Enable in web UI: **Discord → music_enabled = true** and restart.\n`yt-dlp` is auto-downloaded on first `/play`. Built with `--features discord-voice`.",
        ui::COLOR_WARNING)
}

#[cfg(feature = "discord-voice")]
fn auto_bin_dir() -> std::path::PathBuf {
    let base = std::env::var("OSAGENT_DATA_DIR")
        .or_else(|_| std::env::var("OSAGENT_WORKSPACE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_next::data_local_dir().unwrap_or_else(|| {
                std::path::PathBuf::from(shellexpand::tilde("~/.osagent").to_string())
            })
        });
    base.join("bin")
}

#[cfg(feature = "discord-voice")]
fn auto_yt_dlp_path() -> std::path::PathBuf {
    let mut p = auto_bin_dir();
    if cfg!(windows) {
        p.push("yt-dlp.exe");
    } else {
        p.push("yt-dlp");
    }
    p
}

#[cfg(feature = "discord-voice")]
pub(crate) async fn ensure_yt_dlp_auto() -> Result<std::path::PathBuf, String> {
    let dest = auto_yt_dlp_path();
    if dest.exists() {
        // if older than 7 days, try update in background (best-effort)
        if let Ok(meta) = std::fs::metadata(&dest) {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() > 7 * 24 * 3600 {
                        let dest2 = dest.clone();
                        tokio::spawn(async move {
                            let _ = update_yt_dlp(&dest2).await;
                        });
                    }
                }
            }
        }
        return Ok(dest);
    }
    download_yt_dlp(&dest).await?;
    Ok(dest)
}

#[cfg(feature = "discord-voice")]
async fn download_yt_dlp(dest: &std::path::Path) -> Result<(), String> {
    let url = if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"
    };
    info!("Music: downloading yt-dlp from {url} to {}", dest.display());
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Could not create bin dir: {e}"))?;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("yt-dlp download failed: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read failed: {e}"))?;
    tokio::fs::write(dest, &bytes)
        .await
        .map_err(|e| format!("Write failed: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dest)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dest, perms).map_err(|e| e.to_string())?;
    }
    info!("Music: yt-dlp installed at {}", dest.display());
    Ok(())
}

#[cfg(feature = "discord-voice")]
async fn update_yt_dlp(dest: &std::path::Path) -> Result<(), String> {
    // best-effort: re-download latest
    let _ = download_yt_dlp(dest).await;
    Ok(())
}

#[cfg(feature = "discord-voice")]
fn resolve_yt_dlp_program(cfg: &crate::config::DiscordConfig) -> String {
    if !cfg.yt_dlp_path.trim().is_empty() && cfg.yt_dlp_path.trim() != "yt-dlp" {
        let p = shellexpand::tilde(&cfg.yt_dlp_path).to_string();
        if std::path::Path::new(&p).exists() || which::which(&p).is_ok() {
            return p;
        }
        // fall back to auto
    }
    // check PATH first
    if which::which("yt-dlp").is_ok() || which::which("yt-dlp.exe").is_ok() {
        return "yt-dlp".to_string();
    }
    auto_yt_dlp_path().to_string_lossy().to_string()
}

// ── stub when discord-voice not enabled ───────────────────────────────────────

#[cfg(not(feature = "discord-voice"))]
impl Handler {
    pub(super) async fn handle_music_play(&self, ctx: &Context, cmd: &CommandInteraction) {
        let _ = cmd
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(music_disabled_embed())
                        .ephemeral(true),
                ),
            )
            .await;
    }
    pub(super) async fn handle_music_skip(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
    pub(super) async fn handle_music_stop(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
    pub(super) async fn handle_music_queue(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
    pub(super) async fn handle_music_leave(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
    pub(super) async fn handle_music_join(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
    pub(super) async fn handle_music_nowplaying(&self, ctx: &Context, cmd: &CommandInteraction) {
        self.handle_music_play(ctx, cmd).await
    }
}

// ── real impl ─────────────────────────────────────────────────────────────────
#[cfg(feature = "discord-voice")]
use songbird::{
    events::{Event, EventContext, EventHandler as VoiceEventHandler},
    input::{Compose, HttpRequest, YoutubeDl},
    SerenityInit, TrackEvent,
};

#[cfg(feature = "discord-voice")]
#[derive(Clone, Debug)]
struct QueuedMeta {
    title: String,
    url: String,
    author: Option<String>,
    duration: Option<Duration>,
    thumb: Option<String>,
    requester: u64,
}

#[cfg(feature = "discord-voice")]
struct TrackErrorNotifier {
    http: Arc<serenity::http::Http>,
    channel_id: ChannelId,
    guild_id: GuildId,
    title: String,
}

#[cfg(feature = "discord-voice")]
#[async_trait::async_trait]
impl VoiceEventHandler for TrackErrorNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let state = match ctx {
            EventContext::Track(tracks) => tracks.first().map(|(state, _)| *state),
            _ => None,
        };
        warn!(
            "Music: track '{}' failed in guild {}: {:?}",
            self.title,
            self.guild_id.get(),
            state
        );
        let detail = state
            .map(|state| format!("{:?}", state.playing))
            .unwrap_or_else(|| "unknown playback error".to_string());
        let _ = self.channel_id.send_message(
            &self.http,
            CreateMessage::new().embed(ui::embed(
                "Playback Failed",
                format!("Could not play **{}**.\n`{detail}`\n\nTry `/play` again or use a different source.", self.title),
                ui::COLOR_ERROR,
            )),
        ).await;
        None
    }
}

#[cfg(feature = "discord-voice")]
impl Handler {
    pub(super) async fn handle_music_play(&self, ctx: &Context, cmd: &CommandInteraction) {
        let query = {
            let (_, opts) = Self::music_subcommand_opts(cmd);
            opt_str(opts, "query")
                .or_else(|| opt_str(&cmd.data.options, "query"))
                .map(|s| s.to_string())
        };
        let Some(query) = query else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Missing Query",
                    "Usage: `/play <YouTube URL or search>`",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Server Only",
                    "Play in a server voice channel, not DMs.",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        self.remember_channel(cmd.channel_id.get()).await;
        let voice_channel = match self.user_voice_channel(ctx, guild_id, cmd.user.id).await {
            Some(ch) => ch,
            None => {
                self.reply_music(ctx, cmd, ui::embed("Join a Voice Channel", "Join a voice channel first, then `/play <song>`.\nThe bot will auto-join and queue.", ui::COLOR_WARNING)).await;
                return;
            }
        };
        // defer: yt-dlp can take seconds
        let _ = cmd
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new()),
            )
            .await;
        if let Err(e) = self.ensure_joined(ctx, guild_id, voice_channel).await {
            self.edit_reply_music(ctx, cmd, ui::embed("Could Not Join", e, ui::COLOR_ERROR))
                .await;
            return;
        }
        // build source
        let source = match self.build_source(&query).await {
            Ok(v) => v,
            Err(e) => {
                self.edit_reply_music(ctx, cmd, ui::embed("Could Not Play", e, ui::COLOR_ERROR))
                    .await;
                return;
            }
        };
        let (input, meta) = source;
        let raw_title = meta.title.clone();
        // enqueue
        let manager = match songbird::get(ctx).await {
            Some(m) => m.clone(),
            None => {
                self.edit_reply_music(
                    ctx,
                    cmd,
                    ui::embed(
                        "Voice Not Ready",
                        "Songbird not initialised.",
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };
        let handler_lock = match manager.get(guild_id) {
            Some(h) => h,
            None => {
                self.edit_reply_music(
                    ctx,
                    cmd,
                    ui::embed(
                        "Not Connected",
                        "Could not get voice handler.",
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };
        let mut handler = handler_lock.lock().await;
        if let Err(e) = handler.deafen(true).await {
            warn!("Music: could not deafen in guild {}: {e}", guild_id.get());
        }
        if let Err(e) = handler.mute(false).await {
            warn!(
                "Music: could not clear self-mute in guild {}: {e}",
                guild_id.get()
            );
        }
        let queue_len_before = handler.queue().len();
        let track = songbird::tracks::Track::new_with_data(input, Arc::new(meta.clone()));
        let track_handle = handler.enqueue(track).await;
        if let Err(e) = track_handle.add_event(
            Event::Track(TrackEvent::Error),
            TrackErrorNotifier {
                http: ctx.http.clone(),
                channel_id: cmd.channel_id,
                guild_id,
                title: raw_title.clone(),
            },
        ) {
            warn!("Music: could not register track error listener: {e}");
        }
        if queue_len_before == 0 {
            if let Err(e) = track_handle.play() {
                warn!(
                    "Music: could not start '{}' in guild {}: {e}",
                    raw_title,
                    guild_id.get()
                );
            }
        }
        let position = queue_len_before + 1;
        let embed = self.queued_embed(
            &query,
            &meta,
            position,
            handler.queue().len(),
            voice_channel,
        );
        drop(handler);
        self.edit_reply_music(ctx, cmd, embed).await;
        self.spawn_auto_leave(ctx.clone(), guild_id, cmd.channel_id)
            .await;
        info!(
            "Music: enqueued '{}' in guild {} by {}",
            raw_title,
            guild_id.get(),
            cmd.user.id.get()
        );
        let state_handle = track_handle.clone();
        let state_title = raw_title.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            match state_handle.get_info().await {
                Ok(state) => info!(
                    "Music: track state after startup: title='{}' guild={} playing={:?} ready={:?} position={:?}",
                    state_title,
                    guild_id.get(),
                    state.playing,
                    state.ready,
                    state.position,
                ),
                Err(e) => warn!("Music: could not inspect track '{}' in guild {}: {e}", state_title, guild_id.get()),
            }
        });
    }

    pub(super) async fn handle_music_skip(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let Some(manager) = songbird::get(ctx).await else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Voice Not Ready", "Not initialised.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        let Some(handler_lock) = manager.get(guild_id) else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Nothing Playing", "Not in a voice channel.", ui::COLOR_INFO),
            )
            .await;
            return;
        };
        let handler = handler_lock.lock().await;
        let len = handler.queue().len();
        if len == 0 {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Empty Queue", "Nothing to skip.", ui::COLOR_INFO),
            )
            .await;
            return;
        }
        let _ = handler.queue().skip();
        self.reply_music(
            ctx,
            cmd,
            ui::embed(
                "Skipped",
                format!("Skipped. `{}` left.", len.saturating_sub(1)),
                ui::COLOR_SUCCESS,
            ),
        )
        .await;
    }

    pub(super) async fn handle_music_stop(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let Some(manager) = songbird::get(ctx).await else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Voice Not Ready", "Not initialised.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        let Some(handler_lock) = manager.get(guild_id) else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Nothing Playing", "Not in a voice channel.", ui::COLOR_INFO),
            )
            .await;
            return;
        };
        {
            let h = handler_lock.lock().await;
            h.queue().stop();
        }
        self.reply_music(
            ctx,
            cmd,
            ui::embed(
                "Stopped",
                "Cleared queue. Use `/leave` to disconnect.",
                ui::COLOR_SUCCESS,
            ),
        )
        .await;
    }

    pub(super) async fn handle_music_queue(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let Some(manager) = songbird::get(ctx).await else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Voice Not Ready", "Not initialised.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        let Some(handler_lock) = manager.get(guild_id) else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Queue Empty",
                    "Not in voice. `/play <song>` to start.",
                    ui::COLOR_INFO,
                ),
            )
            .await;
            return;
        };
        let handler = handler_lock.lock().await;
        if handler.queue().is_empty() && handler.queue().current().is_none() {
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Queue Empty",
                    "No tracks. Add with `/play`.",
                    ui::COLOR_INFO,
                ),
            )
            .await;
            return;
        }
        // Build the queue embed from metadata attached to each track.
        let mut desc = String::new();
        // we need mutable to modify_queue; we already hold lock, so we can just use handler.queue() immutably?
        // To read queue entries we use modify_queue with a clone
        let mut lines: Vec<String> = Vec::new();
        let mut current_title: Option<String> = None;
        // We need to drop immutable borrow before modify_queue (needs &mut). Use a trick: clone queue handles via modify_queue
        drop(handler);
        let mut handler_mut = handler_lock.lock().await;
        // current
        if let Some(cur) = handler_mut.queue().current() {
            let meta = cur.data::<QueuedMeta>();
            current_title = Some(format!(
                "**Now Playing:** {} — `{}`\n",
                meta.title,
                format_duration(meta.duration)
            ));
        }
        handler_mut.queue().modify_queue(|q| {
            for (i, h) in q.iter().skip(1).take(10).enumerate() {
                let meta = h.data::<QueuedMeta>();
                let title_line = format!(
                    "`{}.` {} — `{}`",
                    i + 1,
                    meta.title,
                    format_duration(meta.duration)
                );
                lines.push(title_line);
            }
        });
        let queue_len = handler_mut.queue().len();
        drop(handler_mut);
        if let Some(ct) = &current_title {
            desc.push_str(ct);
        }
        if !lines.is_empty() {
            desc.push_str("\n**Up Next:**\n");
            desc.push_str(&lines.join("\n"));
            if queue_len > 11 {
                desc.push_str(&format!("\n… and {} more", queue_len - 11));
            }
        } else if current_title.is_some() {
            desc.push_str("\n_Queue empty after current._\n");
        }
        desc.push_str("\n\n-# `/skip` · `/stop` · `/leave` · `/nowplaying`");
        self.reply_music(
            ctx,
            cmd,
            CreateEmbed::new()
                .title("Queue")
                .description(desc)
                .colour(ui::COLOR_INFO),
        )
        .await;
    }

    pub(super) async fn handle_music_leave(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let Some(manager) = songbird::get(ctx).await else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Voice Not Ready", "Not initialised.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if manager.get(guild_id).is_some() {
            let _ = manager.remove(guild_id).await;
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Left",
                    "Disconnected. Thanks for listening!",
                    ui::COLOR_SUCCESS,
                ),
            )
            .await;
        } else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Not Connected", "Not in a voice channel.", ui::COLOR_INFO),
            )
            .await;
        }
    }

    pub(super) async fn handle_music_join(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let voice_channel = match self.user_voice_channel(ctx, guild_id, cmd.user.id).await {
            Some(ch) => ch,
            None => {
                self.reply_music(
                    ctx,
                    cmd,
                    ui::embed(
                        "Join a Voice Channel",
                        "Join a voice channel first.",
                        ui::COLOR_WARNING,
                    ),
                )
                .await;
                return;
            }
        };
        match self.ensure_joined(ctx, guild_id, voice_channel).await {
            Ok(_) => {
                self.reply_music(
                    ctx,
                    cmd,
                    ui::embed(
                        "Joined",
                        format!("Joined <#{voice_channel}>. `/play <song>` to start."),
                        ui::COLOR_SUCCESS,
                    ),
                )
                .await
            }
            Err(e) => {
                self.reply_music(ctx, cmd, ui::embed("Could Not Join", e, ui::COLOR_ERROR))
                    .await
            }
        }
    }

    pub(super) async fn handle_music_nowplaying(&self, ctx: &Context, cmd: &CommandInteraction) {
        if !self.ensure_music_enabled(ctx, cmd).await {
            return;
        }
        let Some(guild_id) = cmd.guild_id else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Server Only", "Use in a server.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        if self.command_access_level(cmd).await.is_none() {
            Self::send_unauthorized_response_command(ctx, cmd).await;
            return;
        }
        let Some(manager) = songbird::get(ctx).await else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Voice Not Ready", "Not initialised.", ui::COLOR_ERROR),
            )
            .await;
            return;
        };
        let Some(handler_lock) = manager.get(guild_id) else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed(
                    "Nothing Playing",
                    "Not in voice. `/play` to start.",
                    ui::COLOR_INFO,
                ),
            )
            .await;
            return;
        };
        let handler = handler_lock.lock().await;
        let Some(cur) = handler.queue().current() else {
            self.reply_music(
                ctx,
                cmd,
                ui::embed("Nothing Playing", "Queue empty.", ui::COLOR_INFO),
            )
            .await;
            return;
        };
        let meta = cur.data::<QueuedMeta>();
        let info = cur.get_info().await.ok();
        let pos = info
            .as_ref()
            .map(|s| format_duration(Some(s.position)))
            .unwrap_or_else(|| "—".to_string());
        let dur = format_duration(meta.duration);
        let mut embed = CreateEmbed::new()
            .title(meta.title.clone())
            .description(format!(
                "by `{}`\n`{pos} / {dur}`",
                meta.author.as_deref().unwrap_or("Unknown")
            ))
            .field(
                "Channel",
                format!(
                    "<#{}>",
                    handler.current_channel().map(|c| c.0.get()).unwrap_or(0)
                ),
                true,
            )
            .colour(ui::COLOR_PRIMARY);
        if let Some(thumb) = &meta.thumb {
            embed = embed.thumbnail(thumb);
        }
        if meta.url.starts_with("http") {
            embed = embed.url(&meta.url);
        }
        self.reply_music(ctx, cmd, embed).await;
    }

    // ── internals ───────────────────────────────────────────────────────────────

    async fn ensure_music_enabled(&self, ctx: &Context, cmd: &CommandInteraction) -> bool {
        let cfg = self.agent.get_config().await;
        let enabled = cfg
            .discord
            .as_ref()
            .map(|d| d.music_enabled)
            .unwrap_or(false);
        if !enabled {
            self.reply_music(ctx, cmd, ui::embed("Music Disabled", "Enable in web UI: **Discord → music_enabled = true** and restart. `yt-dlp` auto-downloads. Build with `--features discord-voice`.", ui::COLOR_WARNING)).await;
            return false;
        }
        true
    }

    async fn user_voice_channel(
        &self,
        ctx: &Context,
        guild_id: GuildId,
        user_id: serenity::model::id::UserId,
    ) -> Option<ChannelId> {
        // Requires `cache` feature — we enabled it. Fallback via HTTP guild fetch if cache missing.
        if let Some(guild) = ctx.cache.guild(guild_id) {
            return guild
                .voice_states
                .get(&user_id)
                .and_then(|vs| vs.channel_id);
        }
        // HTTP fallback: fetch guild via REST (voice_states not included via REST, so this rarely helps)
        // Instead try to get voice state via `ctx.http.get_guild(guild_id)` — voice_states empty via REST, so we just return None.
        // This keeps compilation even if cache disabled.
        None
    }

    async fn ensure_joined(
        &self,
        ctx: &Context,
        guild_id: GuildId,
        channel_id: ChannelId,
    ) -> Result<(), String> {
        let manager = songbird::get(ctx)
            .await
            .ok_or("Songbird not registered")?
            .clone();
        // A failed join leaves a Call whose current_channel() is still the
        // requested channel. Rejoin to verify the driver instead of accepting
        // that stale target as a live voice connection.
        match manager.join(guild_id, channel_id).await {
            Ok(_) => Ok(()),
            Err(first) => {
                warn!(
                    "Music: first voice join failed in guild {} channel {}: {first:?}; retrying",
                    guild_id.get(),
                    channel_id.get()
                );
                let _ = manager.remove(guild_id).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                manager
                    .join(guild_id, channel_id)
                    .await
                    .map(|_| ())
                    .map_err(|second| {
                        warn!(
                            "Music: voice join retry failed in guild {} channel {}: {second:?}",
                            guild_id.get(),
                            channel_id.get()
                        );
                        format!(
                            "Could not connect to <#{channel_id}> after retry: {second}\nCheck **Connect** + **Speak** permissions and that Discord voice traffic is not blocked by a firewall or VPN."
                        )
                    })
            }
        }
    }

    async fn build_source(
        &self,
        query: &str,
    ) -> Result<(songbird::input::Input, QueuedMeta), String> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Err("Empty query.".into());
        }
        let is_http_audio = query.starts_with("http") && {
            let lower = query.to_lowercase();
            lower.ends_with(".mp3")
                || lower.ends_with(".ogg")
                || lower.ends_with(".m4a")
                || lower.ends_with(".opus")
                || lower.ends_with(".flac")
                || lower.ends_with(".wav")
                || lower.contains("googlevideo.com")
        };
        if is_http_audio {
            let client = self.music_http_client().await;
            let req = HttpRequest::new(client, query.clone());
            let meta = QueuedMeta {
                title: query
                    .split('/')
                    .last()
                    .unwrap_or("Direct audio")
                    .to_string(),
                url: query.clone(),
                author: None,
                duration: None,
                thumb: None,
                requester: 0,
            };
            let input: songbird::input::Input = req.into();
            return Ok((input, meta));
        }
        // primary: yt-dlp (auto-download if needed)
        let is_search = !query.starts_with("http");
        let client = self.music_http_client().await;
        let cfg = self.agent.get_config().await;
        let discord_cfg = cfg.discord.clone().unwrap_or_default();
        let prog_str = resolve_yt_dlp_program(&discord_cfg);
        // ensure auto binary exists if we are using it or PATH has no yt-dlp
        let prog_is_auto = prog_str == auto_yt_dlp_path().to_string_lossy().to_string();
        let needs_auto = prog_is_auto
            || (which::which("yt-dlp").is_err()
                && which::which("yt-dlp.exe").is_err()
                && discord_cfg.yt_dlp_path.trim().is_empty());
        let final_prog = if needs_auto {
            match ensure_yt_dlp_auto().await {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(e) => {
                    warn!("Music: auto yt-dlp failed: {e}, trying PATH yt-dlp");
                    prog_str
                }
            }
        } else {
            prog_str
        };
        let extra_args: Vec<String> = discord_cfg
            .yt_dlp_extra_args
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        // leak prog for 'static required by new_ytdl_like
        let prog_static: &'static str = if final_prog == "yt-dlp" {
            "yt-dlp"
        } else {
            Box::leak(final_prog.clone().into_boxed_str())
        };
        let mut ytdl = if is_search {
            YoutubeDl::new_search_ytdl_like(prog_static, client.clone(), query.clone())
        } else {
            YoutubeDl::new_ytdl_like(prog_static, client.clone(), query.clone())
        };
        if !extra_args.is_empty() {
            ytdl = ytdl.user_args(extra_args);
        }
        // try aux metadata first to get title Thumb fast — but don't fail if it errors; fallback to piped
        let aux = match tokio::time::timeout(Duration::from_secs(8), ytdl.aux_metadata()).await {
            Ok(Ok(m)) => Some(m),
            Ok(Err(e)) => {
                warn!("Music ytdl aux failed for `{query}`: {e}");
                // fallback to piped
                if let Some((input, meta)) = self.try_piped(&query).await {
                    return Ok((input, meta));
                }
                return Err(format!("`{e}`\n\n**Fix:** `yt-dlp` auto-updates weekly (or `yt-dlp -U`). If **Sign in**/**429**: add cookies (`yt_dlp_extra_args = --cookies /path/cookies.txt`) or wait. Piped fallback also failed."));
            }
            Err(_) => {
                warn!("Music ytdl aux timeout for `{query}`");
                if let Some((input, meta)) = self.try_piped(&query).await {
                    return Ok((input, meta));
                }
                return Err(
                    "YouTube lookup timed out. Try again, update `yt-dlp`, or use a direct URL."
                        .into(),
                );
            }
        };
        let (title, author, dur, thumb, url) = if let Some(m) = aux {
            (
                m.title.unwrap_or_else(|| query.clone()),
                m.artist,
                m.duration,
                m.thumbnail,
                m.source_url.unwrap_or_else(|| query.clone()),
            )
        } else {
            (query.clone(), None, None, None, query.clone())
        };
        let meta = QueuedMeta {
            title: title.clone(),
            url: url.clone(),
            author,
            duration: dur,
            thumb,
            requester: 0,
        };
        let input: songbird::input::Input = ytdl.into();
        Ok((input, meta))
    }

    async fn try_piped(&self, query: &str) -> Option<(songbird::input::Input, QueuedMeta)> {
        let cfg = self.agent.get_config().await;
        let instances = cfg
            .discord
            .as_ref()
            .map(|d| d.piped_instances.clone())
            .unwrap_or_default();
        let instances = if instances.is_empty() {
            vec![
                "https://pipedapi.kavin.rocks".to_string(),
                "https://pipedapi.adminforge.de".to_string(),
            ]
        } else {
            instances
        };
        let vid = if query.starts_with("http") {
            extract_youtube_id(query)
        } else {
            self.piped_search(query, &instances).await
        };
        let Some(vid) = vid else {
            return None;
        };
        for inst in &instances {
            if let Some((audio_url, title, thumb, dur)) = self.piped_streams(inst, &vid).await {
                let client = self.music_http_client().await;
                let req = HttpRequest::new(client, audio_url.clone());
                let meta = QueuedMeta {
                    title,
                    url: format!("https://youtube.com/watch?v={vid}"),
                    author: None,
                    duration: dur,
                    thumb,
                    requester: 0,
                };
                let input: songbird::input::Input = req.into();
                return Some((input, meta));
            }
        }
        None
    }

    async fn piped_search(&self, query: &str, instances: &[String]) -> Option<String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        for inst in instances {
            let url = format!(
                "{}/search?q={}&filter=videos",
                inst,
                urlencoding::encode(query)
            );
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(arr) = json.get("items").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                                if let Some(id) = u
                                    .split("watch?v=")
                                    .last()
                                    .map(|s| s.split('&').next().unwrap().to_string())
                                {
                                    if !id.is_empty() {
                                        return Some(id);
                                    }
                                }
                            }
                            if let Some(id) = item
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                            {
                                if !id.is_empty() {
                                    return Some(id);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    async fn piped_streams(
        &self,
        instance: &str,
        vid: &str,
    ) -> Option<(String, String, Option<String>, Option<Duration>)> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .ok()?;
        let url = format!("{instance}/streams/{vid}");
        let resp = client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        let title = json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Piped audio")
            .to_string();
        let thumb = json
            .pointer("/thumbnailUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                json.get("thumbnail")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });
        let dur = json
            .get("duration")
            .and_then(|v| v.as_u64())
            .map(Duration::from_secs);
        let audio_url = json
            .get("audioStreams")
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .find_map(|s| s.get("url").and_then(|u| u.as_str()).map(|s| s.to_string()))
            })
            .or_else(|| {
                json.get("hls")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })?;
        Some((audio_url, title, thumb, dur))
    }

    async fn music_http_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent("OSAgent-music/1.0")
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    fn queued_embed(
        &self,
        query: &str,
        meta: &QueuedMeta,
        position: usize,
        queue_len: usize,
        voice_channel: ChannelId,
    ) -> CreateEmbed {
        let dur = format_duration(meta.duration);
        let author = meta.author.as_deref().unwrap_or("YouTube");
        let mut desc = format!("**{}**\nby `{author}` · `{dur}`\n", meta.title);
        if position == 1 {
            desc.push_str(&format!("\n▶️ **Now Playing** in <#{voice_channel}>"));
            if queue_len > 1 {
                desc.push_str(&format!(" — `{}` queued", queue_len - 1));
            }
        } else {
            desc.push_str(&format!(
                "\n📃 Queued at `#{position}` — `{}` ahead",
                position - 1
            ));
        }
        desc.push_str("\n\n-# `/queue` · `/skip` · `/stop` · `/leave` · `/nowplaying`");
        let mut e = CreateEmbed::new()
            .title(if position == 1 {
                "Now Playing"
            } else {
                "Added to Queue"
            })
            .description(desc)
            .colour(if meta.title.contains("Piped") {
                ui::COLOR_WARNING
            } else {
                ui::COLOR_SUCCESS
            })
            .field(
                "Source",
                format!("`{}`", ui::truncate_chars(query, 80)),
                false,
            )
            .footer(CreateEmbedFooter::new(format!(
                "{queue_len} track(s) in queue"
            )));
        if let Some(t) = &meta.thumb {
            e = e.thumbnail(t);
        }
        if meta.url.starts_with("http") {
            e = e.url(&meta.url);
        }
        e
    }

    async fn spawn_auto_leave(&self, ctx: Context, guild_id: GuildId, channel_id: ChannelId) {
        let Some(manager) = songbird::get(&ctx).await.map(|m| m.clone()) else {
            return;
        };
        let cfg = self.agent.get_config().await;
        let secs = cfg
            .discord
            .as_ref()
            .map(|d| d.music_auto_leave_secs)
            .unwrap_or(300);
        if secs == 0 {
            return;
        }
        let http = ctx.http.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(secs)).await;
                let Some(h) = manager.get(guild_id) else {
                    break;
                };
                let empty = {
                    let q = h.lock().await;
                    q.queue().is_empty() && q.queue().current().is_none()
                };
                if empty {
                    let _ = manager.remove(guild_id).await;
                    let _ = ChannelId::new(channel_id.get())
                        .send_message(
                            &http,
                            serenity::builder::CreateMessage::new().embed(ui::embed(
                                "Left Voice",
                                format!("Idle {secs}s — left. `/play` to start again."),
                                ui::COLOR_INFO,
                            )),
                        )
                        .await;
                    break;
                }
            }
        });
    }

    fn music_subcommand_opts(
        cmd: &CommandInteraction,
    ) -> (&str, &[serenity::model::application::CommandDataOption]) {
        for opt in &cmd.data.options {
            if let CommandDataOptionValue::SubCommand(opts) = &opt.value {
                return (opt.name.as_str(), opts.as_slice());
            }
        }
        ("", &[])
    }

    async fn reply_music(&self, ctx: &Context, cmd: &CommandInteraction, embed: CreateEmbed) {
        // Try create; if already deferred (e.g. play uses Defer), fallback to edit
        if cmd
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embed.clone())
                        .ephemeral(false),
                ),
            )
            .await
            .is_err()
        {
            let _ = cmd
                .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                .await;
        }
    }
    async fn edit_reply_music(&self, ctx: &Context, cmd: &CommandInteraction, embed: CreateEmbed) {
        let _ = cmd
            .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
            .await;
    }
}
