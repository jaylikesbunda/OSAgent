//! `/settings` — one ephemeral, self-updating control panel for the things
//! people actually change: provider, model, persona, workspace.
//!
//! Every control is a select menu, so nothing has to be typed from memory, and
//! the panel re-renders from live state after each change rather than trusting
//! what it last drew.

use super::{ui, Handler};
use crate::agent::provider::Provider;
use serenity::builder::{
    CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption,
};
use serenity::model::application::{
    ButtonStyle, CommandInteraction, ComponentInteraction, ComponentInteractionDataKind,
};
use serenity::prelude::*;
use tracing::error;

pub(super) const PANEL_PREFIX: &str = "osa:";

const CLEAR_VALUE: &str = "__clear__";

/// A notice rendered at the top of the panel after an action.
struct Notice {
    text: String,
    failed: bool,
}

impl Notice {
    fn ok(text: impl Into<String>) -> Option<Self> {
        Some(Self {
            text: text.into(),
            failed: false,
        })
    }

    fn err(text: impl Into<String>) -> Option<Self> {
        Some(Self {
            text: text.into(),
            failed: true,
        })
    }
}

fn option(
    label: &str,
    value: &str,
    description: Option<&str>,
    selected: bool,
) -> CreateSelectMenuOption {
    let mut option = CreateSelectMenuOption::new(ui::truncate_chars(label, 100), value)
        .default_selection(selected);
    if let Some(description) = description {
        let description = description.trim();
        if !description.is_empty() {
            option = option.description(ui::truncate_chars(description, 100));
        }
    }
    option
}

fn select_row(
    custom_id: String,
    placeholder: &str,
    options: Vec<CreateSelectMenuOption>,
) -> Option<CreateActionRow> {
    // Discord rejects a select menu with no options outright.
    if options.is_empty() {
        return None;
    }
    Some(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(custom_id, CreateSelectMenuKind::String { options })
            .placeholder(ui::truncate_chars(placeholder, 150))
            .min_values(1)
            .max_values(1),
    ))
}

pub(super) fn format_context(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{}k ctx", tokens / 1000)
    } else if tokens > 0 {
        format!("{tokens} ctx")
    } else {
        "unknown ctx".to_string()
    }
}

impl Handler {
    /// Choose the model to land on when the provider changes.
    pub(super) async fn model_for_provider(
        &self,
        provider_id: &str,
        current_model: &str,
    ) -> Option<String> {
        let models = self
            .agent
            .get_provider_models(provider_id.to_string())
            .await;

        if models.iter().any(|model| model.id == current_model) {
            return Some(current_model.to_string());
        }

        let configured = self
            .agent
            .get_config()
            .await
            .providers
            .iter()
            .find(|provider| provider.provider_type == provider_id)
            .map(|provider| provider.model.clone())
            .filter(|model| !model.is_empty());

        if configured.is_some() {
            return configured;
        }

        models
            .iter()
            .find(|model| model.available)
            .or_else(|| models.first())
            .map(|model| model.id.clone())
    }

    /// Switch provider and/or model together, then persist.
    ///
    /// Going through `switch_provider_model` is what keeps the running provider
    /// and the config file in agreement — setting the model alone leaves them
    /// pointing at different providers.
    pub(super) async fn apply_model_switch(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<(), String> {
        self.agent
            .switch_provider_model(provider_id.to_string(), model_id.to_string())
            .await
            .map_err(|e| e.to_string())?;

        if let Err(e) = self.agent.save_config(&self.config_path).await {
            error!("Discord: failed to persist provider/model switch: {e}");
            return Err(format!("Switched, but the config could not be saved: {e}"));
        }

        Ok(())
    }

    async fn build_panel(
        &self,
        user_id: u64,
        notice: Option<Notice>,
    ) -> (CreateEmbed, Vec<CreateActionRow>) {
        let session_id = self.get_or_create_session(user_id).await.ok();

        let provider = self.agent.active_provider().await;
        let provider_id = provider.provider_type().to_string();
        let model_id = provider.current_model().await;

        let catalog = self.agent.get_catalog_state().await;
        let models = self.agent.get_provider_models(provider_id.clone()).await;
        let active_model = models.iter().find(|model| model.id == model_id);

        let provider_name = catalog
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| provider_id.clone());

        let persona = match &session_id {
            Some(session_id) => self
                .agent
                .get_session_persona(session_id)
                .await
                .ok()
                .flatten(),
            None => None,
        };

        let workspaces = self.agent.get_workspaces().await;
        let active_workspace = self.agent.get_active_workspace().await;

        // ---- embed -------------------------------------------------------
        let mut description = String::new();
        if let Some(notice) = &notice {
            let marker = if notice.failed { "✗" } else { "✓" };
            description.push_str(&format!("{marker} {}\n\n", notice.text));
        }

        let model_detail = match active_model {
            Some(model) => {
                let mut bits = vec![format_context(model.context_window)];
                if model.supports_tools {
                    bits.push("tools".to_string());
                }
                if model.supports_vision {
                    bits.push("vision".to_string());
                }
                format!("`{model_id}`\n-# {}", bits.join(" · "))
            }
            None => format!("`{model_id}`\n-# not in the catalog for this provider"),
        };

        let provider_connected = catalog
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .map(|p| p.connected)
            .unwrap_or(false);

        let mut embed = CreateEmbed::new()
            .title("Settings")
            .description(description)
            .colour(if notice.as_ref().is_some_and(|n| n.failed) {
                ui::COLOR_ERROR
            } else {
                ui::COLOR_PRIMARY
            })
            .field(
                "Provider",
                format!(
                    "`{provider_name}`\n-# {}",
                    if provider_connected {
                        "connected"
                    } else {
                        "no API key configured"
                    }
                ),
                true,
            )
            .field("Model", model_detail, true)
            .field(
                "Persona",
                match &persona {
                    Some(persona) => format!(
                        "`{}`\n-# {}",
                        persona.name,
                        ui::truncate_chars(&persona.summary, 60)
                    ),
                    None => "`default`\n-# no persona set".to_string(),
                },
                true,
            )
            .field(
                "Workspace",
                format!("`{}`\n-# {}", active_workspace.id, active_workspace.path),
                false,
            );

        if let Some(session_id) = &session_id {
            let messages = self
                .agent
                .get_session(session_id)
                .await
                .ok()
                .flatten()
                .map(|session| session.messages.len())
                .unwrap_or(0);
            embed = embed.field(
                "Session",
                format!(
                    "`{}`\n-# {messages} messages",
                    session_id.chars().take(8).collect::<String>()
                ),
                false,
            );
        }

        embed = embed.footer(CreateEmbedFooter::new(
            "Provider, model and workspace are global — they affect every channel and the desktop app. Persona applies to your session only.",
        ));

        // ---- components --------------------------------------------------
        let mut rows = Vec::new();

        let mut provider_options: Vec<CreateSelectMenuOption> = catalog
            .providers
            .iter()
            .filter(|p| p.connected || p.id == provider_id)
            .take(ui::SELECT_LIMIT)
            .map(|p| {
                option(
                    &p.name,
                    &p.id,
                    Some(if p.connected {
                        "connected"
                    } else {
                        "no API key configured"
                    }),
                    p.id == provider_id,
                )
            })
            .collect();

        if provider_options.is_empty() {
            provider_options.push(option(&provider_name, &provider_id, None, true));
        }

        if let Some(row) = select_row(
            format!("{PANEL_PREFIX}provider:{user_id}"),
            "Provider",
            provider_options,
        ) {
            rows.push(row);
        }

        // Keep the active model visible even when the catalog is longer than the
        // 25 options a select menu allows.
        let mut sorted_models = models.clone();
        sorted_models.sort_by(|a, b| {
            (b.id == model_id)
                .cmp(&(a.id == model_id))
                .then_with(|| b.available.cmp(&a.available))
                .then_with(|| a.name.cmp(&b.name))
        });

        let model_options: Vec<CreateSelectMenuOption> = sorted_models
            .iter()
            .filter(|model| provider_id.len() + model.id.len() < 100)
            .take(ui::SELECT_LIMIT)
            .map(|model| {
                let mut detail = format_context(model.context_window);
                if !model.available {
                    detail.push_str(" · unavailable");
                }
                option(
                    &model.name,
                    &format!("{provider_id}:{}", model.id),
                    Some(&detail),
                    model.id == model_id,
                )
            })
            .collect();

        if let Some(row) = select_row(
            format!("{PANEL_PREFIX}model:{user_id}"),
            if model_options.is_empty() {
                "No catalog models — use /model set"
            } else {
                "Model"
            },
            model_options,
        ) {
            rows.push(row);
        }

        let current_persona_id = persona.as_ref().map(|persona| persona.id.clone());
        let mut persona_options: Vec<CreateSelectMenuOption> = self
            .agent
            .list_personas()
            .into_iter()
            .take(ui::SELECT_LIMIT - 1)
            .map(|persona| {
                let selected = current_persona_id.as_deref() == Some(persona.id.as_str());
                option(&persona.name, &persona.id, Some(&persona.summary), selected)
            })
            .collect();
        persona_options.push(option(
            "No persona",
            CLEAR_VALUE,
            Some("Use the agent's default behaviour"),
            current_persona_id.is_none(),
        ));

        if let Some(row) = select_row(
            format!("{PANEL_PREFIX}persona:{user_id}"),
            "Persona",
            persona_options,
        ) {
            rows.push(row);
        }

        let workspace_options: Vec<CreateSelectMenuOption> = workspaces
            .iter()
            .filter(|workspace| workspace.id.len() <= 100)
            .take(ui::SELECT_LIMIT)
            .map(|workspace| {
                option(
                    &workspace.id,
                    &workspace.id,
                    Some(&workspace.path),
                    workspace.id == active_workspace.id,
                )
            })
            .collect();

        if let Some(row) = select_row(
            format!("{PANEL_PREFIX}workspace:{user_id}"),
            "Workspace",
            workspace_options,
        ) {
            rows.push(row);
        }

        rows.push(CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{PANEL_PREFIX}refresh:{user_id}"))
                .label("Refresh")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{PANEL_PREFIX}newsession:{user_id}"))
                .label("New session")
                .style(ButtonStyle::Secondary),
        ]));

        (embed, rows)
    }

    /// `/settings` — open a fresh panel, private to the caller.
    pub(super) async fn open_settings_panel(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();
        if !self.command_is_authorized(command).await {
            Self::send_unauthorized_response_command(ctx, command).await;
            return;
        }

        let (embed, components) = self.build_panel(user_id, None).await;

        if let Err(e) = command
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(components)
                        .ephemeral(true),
                ),
            )
            .await
        {
            error!("Discord: failed to open settings panel: {e}");
        }
    }

    /// Route a panel component. Returns false when the id is not ours.
    pub(super) async fn handle_panel_component(
        &self,
        ctx: &Context,
        component: &ComponentInteraction,
    ) -> bool {
        let Some(rest) = component.data.custom_id.strip_prefix(PANEL_PREFIX) else {
            return false;
        };
        // `osa:<action>:<owner-id>[:<payload>]`
        let mut parts = rest.splitn(3, ':');
        let action = parts.next().unwrap_or_default();
        let owner = parts.next().unwrap_or_default();
        let payload = parts.next().unwrap_or_default();

        let user_id = component.user.id.get();

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
                user_id,
                component.guild_id.map(|id| id.get()),
                component.channel_id.get(),
                &roles,
            )
            .await
        {
            self.reject_component(ctx, component, "You are not authorized to use this bot.")
                .await;
            return true;
        }

        // Ephemeral messages are per-user already, but the id check keeps a
        // shared panel from ever acting on someone else's behalf.
        if owner.parse::<u64>().ok() != Some(user_id) {
            self.reject_component(
                ctx,
                component,
                "This panel belongs to someone else. Run `/settings` to open your own.",
            )
            .await;
            return true;
        }

        let selected = match &component.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
            _ => None,
        };

        let notice = match action {
            "provider" => self.on_provider_selected(selected).await,
            "model" => self.on_model_selected(selected).await,
            "persona" => self.on_persona_selected(user_id, selected).await,
            "workspace" => self.on_workspace_selected(user_id, selected).await,
            "newsession" => match self.start_new_session(user_id).await {
                Ok(_) => Notice::ok("Started a new session."),
                Err(e) => Notice::err(e),
            },
            "deletesession" => match self.delete_current_session(user_id).await {
                Ok(true) => Notice::ok("Session deleted."),
                Ok(false) => Notice::err("There was no active session to delete."),
                Err(e) => Notice::err(e),
            },
            // `/model set` with an id the catalog does not know, confirmed once.
            "forcemodel" => match payload.split_once(':') {
                Some((provider_id, model_id)) => {
                    match self.apply_model_switch(provider_id, model_id).await {
                        Ok(()) => Notice::ok(format!(
                            "Forced `{model_id}` on `{provider_id}` — it is not in the catalog."
                        )),
                        Err(e) => Notice::err(e),
                    }
                }
                None => None,
            },
            _ => None,
        };

        let (embed, components) = self.build_panel(user_id, notice).await;

        if let Err(e) = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(components),
                ),
            )
            .await
        {
            error!("Discord: failed to update settings panel: {e}");
        }

        true
    }

    async fn reject_component(
        &self,
        ctx: &Context,
        component: &ComponentInteraction,
        message: &str,
    ) {
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(ui::embed("Not Allowed", message, ui::COLOR_ERROR))
                        .ephemeral(true),
                ),
            )
            .await;
    }

    async fn on_provider_selected(&self, selected: Option<String>) -> Option<Notice> {
        let provider_id = selected?;
        let current_model = self.agent.get_current_model().await;

        let Some(model) = self.model_for_provider(&provider_id, &current_model).await else {
            return Notice::err(format!(
                "`{provider_id}` has no models in the catalog. Add one in the web UI, or use `/model set` with an explicit id."
            ));
        };

        match self.apply_model_switch(&provider_id, &model).await {
            Ok(()) => Notice::ok(format!("Switched to `{provider_id}` on `{model}`.")),
            Err(e) => Notice::err(e),
        }
    }

    async fn on_model_selected(&self, selected: Option<String>) -> Option<Notice> {
        let value = selected?;
        let (provider_id, model_id) = value.split_once(':')?;

        match self.apply_model_switch(provider_id, model_id).await {
            Ok(()) => Notice::ok(format!("Model set to `{model_id}`.")),
            Err(e) => Notice::err(e),
        }
    }

    async fn on_persona_selected(&self, user_id: u64, selected: Option<String>) -> Option<Notice> {
        let persona_id = selected?;
        let session_id = match self.get_or_create_session(user_id).await {
            Ok(session_id) => session_id,
            Err(e) => return Notice::err(e),
        };

        if persona_id == CLEAR_VALUE {
            return match self.agent.reset_session_persona(&session_id).await {
                Ok(()) => Notice::ok("Persona cleared."),
                Err(e) => Notice::err(e.to_string()),
            };
        }

        match self
            .agent
            .set_session_persona(&session_id, persona_id, None)
            .await
        {
            Ok(persona) => Notice::ok(format!("Persona set to `{}`.", persona.name)),
            Err(e) => Notice::err(e.to_string()),
        }
    }

    async fn on_workspace_selected(
        &self,
        user_id: u64,
        selected: Option<String>,
    ) -> Option<Notice> {
        let workspace_id = selected?;

        match self.agent.set_active_workspace(&workspace_id).await {
            Ok(workspace) => {
                if let Err(e) = self.agent.save_config(&self.config_path).await {
                    error!("Discord: failed to persist workspace switch: {e}");
                }
                if let Ok(session_id) = self.get_or_create_session(user_id).await {
                    let _ = self
                        .agent
                        .set_session_workspace(&session_id, &workspace.id)
                        .await;
                }
                Notice::ok(format!("Workspace set to `{}`.", workspace.id))
            }
            Err(e) => Notice::err(e.to_string()),
        }
    }
}
