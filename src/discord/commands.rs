//! Slash command registration and dispatch.
//!
//! Commands are grouped into subcommands (`/model set`, `/session new`) so the
//! Discord client itself documents them, and every id the user has to supply is
//! backed by autocomplete.

use super::chat::Turn;
use super::panel::{format_context, PANEL_PREFIX};
use super::{ui, Handler};
use crate::agent::provider::Provider;
use serenity::builder::{
    CreateActionRow, CreateAutocompleteResponse, CreateButton, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse,
};
use serenity::http::Http;
use serenity::model::application::{
    ButtonStyle, Command, CommandDataOption, CommandDataOptionValue, CommandInteraction,
    CommandOptionType,
};
use serenity::prelude::*;
use std::collections::HashMap;
use tracing::{error, warn};

const CHOICE_LIMIT: usize = 25;

// ---------------------------------------------------------------------------
// option helpers
// ---------------------------------------------------------------------------

fn sub(name: &str, description: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::SubCommand, name, description)
}

fn str_opt(name: &str, description: &str, required: bool) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::String, name, description).required(required)
}

fn int_opt(name: &str, description: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::Integer, name, description).required(false)
}

/// The invoked subcommand and its own options, or `("", &[])` for flat commands.
fn subcommand(command: &CommandInteraction) -> (&str, &[CommandDataOption]) {
    for option in &command.data.options {
        if let CommandDataOptionValue::SubCommand(options) = &option.value {
            return (option.name.as_str(), options.as_slice());
        }
    }
    ("", &[])
}

fn opt_str<'a>(options: &'a [CommandDataOption], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| option.value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn opt_i64(options: &[CommandDataOption], name: &str) -> Option<i64> {
    options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| option.value.as_i64())
}

/// Find the option the user is currently typing in, at any subcommand depth.
fn focused(options: &[CommandDataOption]) -> Option<(String, String)> {
    for option in options {
        match &option.value {
            CommandDataOptionValue::Autocomplete { value, .. } => {
                return Some((option.name.clone(), value.to_lowercase()))
            }
            CommandDataOptionValue::SubCommand(nested)
            | CommandDataOptionValue::SubCommandGroup(nested) => {
                if let Some(found) = focused(nested) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

impl Handler {
    // -----------------------------------------------------------------------
    // registration
    // -----------------------------------------------------------------------

    pub(super) async fn register_commands(&self, http: &Http) {
        let commands = vec![
            CreateCommand::new("chat")
                .description("Send a message to the agent")
                .add_option(str_opt("message", "What you want the agent to do", true)),
            CreateCommand::new("settings")
                .description("Open the settings panel: provider, model, persona, workspace"),
            CreateCommand::new("session")
                .description("Manage your session")
                .add_option(sub("status", "Show the current session"))
                .add_option(sub(
                    "new",
                    "Archive the current session and start a fresh one",
                ))
                .add_option(sub("archive", "Archive the current session"))
                .add_option(sub("delete", "Permanently delete the current session")),
            CreateCommand::new("model")
                .description("Show or change the active model")
                .add_option(sub("show", "Show the active model and provider"))
                .add_option(
                    sub("set", "Switch to another model").add_sub_option(
                        str_opt("id", "Model — autocompletes from the catalog", true)
                            .set_autocomplete(true),
                    ),
                ),
            CreateCommand::new("provider")
                .description("Show or change the active provider")
                .add_option(sub("list", "List configured providers"))
                .add_option(
                    sub("use", "Switch to another provider")
                        .add_sub_option(str_opt("id", "Provider id", true).set_autocomplete(true)),
                ),
            CreateCommand::new("persona")
                .description("Manage the persona for your session")
                .add_option(sub("list", "List available personas"))
                .add_option(
                    sub("set", "Activate a persona")
                        .add_sub_option(str_opt("id", "Persona id", true).set_autocomplete(true))
                        .add_sub_option(str_opt(
                            "character",
                            "Custom character — use with id: custom",
                            false,
                        )),
                )
                .add_option(sub("clear", "Revert to default behaviour")),
            CreateCommand::new("workspace")
                .description("List or switch the active workspace")
                .add_option(sub("list", "List configured workspaces"))
                .add_option(
                    sub("set", "Switch the active workspace")
                        .add_sub_option(str_opt("id", "Workspace id", true).set_autocomplete(true)),
                ),
            CreateCommand::new("permissions")
                .description("Manage external directory permissions")
                .add_option(sub("list", "Show pending permission requests"))
                .add_option(
                    sub("allow", "Grant access to a path").add_sub_option(str_opt(
                        "path",
                        "File or directory path",
                        true,
                    )),
                )
                .add_option(
                    sub("deny", "Refuse access to a path").add_sub_option(str_opt(
                        "path",
                        "File or directory path",
                        true,
                    )),
                ),
            CreateCommand::new("workflow")
                .description("Run a workflow")
                .add_option(str_opt("name", "Workflow name", true).set_autocomplete(true))
                .add_option(str_opt("input", "Optional trigger input", false)),
            CreateCommand::new("answer")
                .description("Answer the question the agent is waiting on")
                .add_option(str_opt("answer", "Option number, or free text", true)),
            CreateCommand::new("mode")
                .description("Set the mode hint passed to the agent (build or plan)")
                .add_option(
                    str_opt("mode", "The mode to request", true)
                        .add_string_choice("build", "build")
                        .add_string_choice("plan", "plan"),
                ),
            CreateCommand::new("lsp")
                .description("Ask the agent to run an LSP operation")
                .add_option(
                    str_opt("operation", "LSP operation", true)
                        .add_string_choice("goto definition", "goToDefinition")
                        .add_string_choice("find references", "findReferences")
                        .add_string_choice("hover", "hover")
                        .add_string_choice("document symbols", "documentSymbol"),
                )
                .add_option(str_opt("file", "File path", true))
                .add_option(int_opt("line", "Line number"))
                .add_option(int_opt("character", "Character position")),
            CreateCommand::new("subagent")
                .description("Run a task with a subagent")
                .add_option(
                    str_opt("type", "Subagent type", true)
                        .add_string_choice("general", "general")
                        .add_string_choice("explore", "explore"),
                )
                .add_option(str_opt("prompt", "What the subagent should do", true)),
            CreateCommand::new("help").description("Show available commands"),
        ];

        match Command::set_global_commands(http, commands).await {
            Ok(registered) => {
                tracing::info!("Discord: registered {} slash commands", registered.len())
            }
            Err(e) => error!("Discord: failed to register global commands: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // dispatch
    // -----------------------------------------------------------------------

    pub(super) async fn dispatch_command(&self, ctx: &Context, command: &CommandInteraction) {
        let name = command.data.name.as_str();

        if name == "help" {
            self.handle_help(ctx, command).await;
            return;
        }

        if !self.ensure_authorized(ctx, command).await {
            return;
        }

        // Anything that talks to a channel should make that channel the default
        // delivery target for scheduled jobs and workflow notifications.
        self.remember_channel(command.channel_id.get()).await;

        match name {
            "settings" => self.open_settings_panel(ctx, command).await,
            "chat" => self.handle_chat(ctx, command).await,
            "session" => self.handle_session(ctx, command).await,
            "model" => self.handle_model(ctx, command).await,
            "provider" => self.handle_provider(ctx, command).await,
            "persona" => self.handle_persona(ctx, command).await,
            "workspace" => self.handle_workspace(ctx, command).await,
            "permissions" => self.handle_permissions(ctx, command).await,
            "workflow" => self.handle_workflow(ctx, command).await,
            "answer" => self.handle_answer(ctx, command).await,
            "mode" => self.handle_mode(ctx, command).await,
            "lsp" => self.handle_lsp(ctx, command).await,
            "subagent" => self.handle_subagent(ctx, command).await,
            other => warn!("Discord: unknown command: {other}"),
        }
    }

    pub(super) async fn dispatch_autocomplete(&self, ctx: &Context, command: &CommandInteraction) {
        if !self.is_authorized(command.user.id.get()).await {
            let _ = command
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Autocomplete(CreateAutocompleteResponse::new()),
                )
                .await;
            return;
        }

        let (_, query) = focused(&command.data.options).unwrap_or_default();

        let choices: Vec<(String, String)> = match command.data.name.as_str() {
            "model" => self.model_choices(&query).await,
            "provider" => self.provider_choices(&query).await,
            "persona" => self.persona_choices(&query),
            "workspace" => self.workspace_choices(&query).await,
            "workflow" => self.workflow_choices(&query),
            _ => Vec::new(),
        };

        let mut response = CreateAutocompleteResponse::new();
        for (label, value) in choices
            .into_iter()
            .filter(|(_, value)| !value.is_empty() && value.len() <= 100)
            .take(CHOICE_LIMIT)
        {
            response = response.add_string_choice(ui::truncate_chars(&label, 100), value);
        }

        if let Err(e) = command
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(response))
            .await
        {
            error!("Discord: failed to answer autocomplete: {e}");
        }
    }

    async fn ensure_authorized(&self, ctx: &Context, command: &CommandInteraction) -> bool {
        if self.is_authorized(command.user.id.get()).await {
            return true;
        }
        Self::send_unauthorized_response_command(ctx, command).await;
        false
    }

    // -----------------------------------------------------------------------
    // autocomplete sources
    // -----------------------------------------------------------------------

    async fn model_choices(&self, query: &str) -> Vec<(String, String)> {
        let models = if query.is_empty() {
            let provider = self.agent.active_provider().await;
            self.agent
                .get_provider_models(provider.provider_type().to_string())
                .await
        } else {
            self.agent.search_catalog_models(query.to_string()).await
        };

        models
            .into_iter()
            .map(|model| {
                (
                    format!(
                        "{} · {} · {}",
                        model.name,
                        model.provider_id,
                        format_context(model.context_window)
                    ),
                    format!("{}:{}", model.provider_id, model.id),
                )
            })
            .collect()
    }

    async fn provider_choices(&self, query: &str) -> Vec<(String, String)> {
        self.agent
            .get_catalog_state()
            .await
            .providers
            .into_iter()
            .filter(|provider| {
                query.is_empty()
                    || provider.id.to_lowercase().contains(query)
                    || provider.name.to_lowercase().contains(query)
            })
            .map(|provider| {
                (
                    format!(
                        "{} · {}",
                        provider.name,
                        if provider.connected {
                            "connected"
                        } else {
                            "no API key"
                        }
                    ),
                    provider.id,
                )
            })
            .collect()
    }

    fn persona_choices(&self, query: &str) -> Vec<(String, String)> {
        self.agent
            .list_personas()
            .into_iter()
            .filter(|persona| {
                query.is_empty()
                    || persona.id.to_lowercase().contains(query)
                    || persona.name.to_lowercase().contains(query)
            })
            .map(|persona| (persona.name, persona.id))
            .collect()
    }

    async fn workspace_choices(&self, query: &str) -> Vec<(String, String)> {
        self.agent
            .get_workspaces()
            .await
            .into_iter()
            .filter(|workspace| {
                query.is_empty()
                    || workspace.id.to_lowercase().contains(query)
                    || workspace.path.to_lowercase().contains(query)
            })
            .map(|workspace| {
                (
                    format!("{} — {}", workspace.id, workspace.path),
                    workspace.id,
                )
            })
            .collect()
    }

    fn workflow_choices(&self, query: &str) -> Vec<(String, String)> {
        let Ok((db, _)) = self.build_workflow_services() else {
            return Vec::new();
        };
        match db.list_workflows() {
            Ok(workflows) => workflows
                .into_iter()
                .filter(|workflow| {
                    query.is_empty()
                        || workflow.name.to_lowercase().contains(query)
                        || workflow.id.to_lowercase().contains(query)
                })
                .map(|workflow| (workflow.name.clone(), workflow.name))
                .collect(),
            Err(e) => {
                warn!("Discord: failed to list workflows for autocomplete: {e}");
                Vec::new()
            }
        }
    }

    // -----------------------------------------------------------------------
    // chat
    // -----------------------------------------------------------------------

    async fn handle_chat(&self, ctx: &Context, command: &CommandInteraction) {
        let Some(message) = opt_str(&command.data.options, "message") else {
            return;
        };
        let user_id = command.user.id.get();

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(session_id) => session_id,
            Err(e) => {
                self.reply(ctx, command, ui::embed("Session Error", e, ui::COLOR_ERROR))
                    .await;
                return;
            }
        };

        // Acknowledge inside the 3s interaction window, then work in the channel
        // where everyone can follow along.
        self.reply(
            ctx,
            command,
            ui::embed(
                "Sent",
                "Working on it — I'll post the reply in this channel.",
                ui::COLOR_INFO,
            ),
        )
        .await;

        self.run_turn(
            ctx,
            Turn {
                channel_id: command.channel_id,
                session_id,
                user_id,
                prompt: message.to_string(),
            },
        )
        .await;
    }

    /// `/lsp` and `/subagent` are phrasings of a normal agent turn.
    async fn run_prompt_command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        prompt: String,
    ) {
        let user_id = command.user.id.get();

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(session_id) => session_id,
            Err(e) => {
                self.reply(ctx, command, ui::embed("Session Error", e, ui::COLOR_ERROR))
                    .await;
                return;
            }
        };

        self.reply(
            ctx,
            command,
            ui::embed(
                "Sent",
                format!("Asked the agent:\n> {}", ui::truncate_chars(&prompt, 300)),
                ui::COLOR_INFO,
            ),
        )
        .await;

        self.run_turn(
            ctx,
            Turn {
                channel_id: command.channel_id,
                session_id,
                user_id,
                prompt,
            },
        )
        .await;
    }

    async fn handle_lsp(&self, ctx: &Context, command: &CommandInteraction) {
        let options = &command.data.options;
        let operation = opt_str(options, "operation").unwrap_or("hover");
        let Some(file) = opt_str(options, "file") else {
            return;
        };
        let line = opt_i64(options, "line").unwrap_or(1);
        let character = opt_i64(options, "character").unwrap_or(1);

        let prompt = format!(
            "Use the LSP `{operation}` operation on `{file}` at line {line}, character {character}, and summarise the result."
        );
        self.run_prompt_command(ctx, command, prompt).await;
    }

    async fn handle_subagent(&self, ctx: &Context, command: &CommandInteraction) {
        let options = &command.data.options;
        let kind = opt_str(options, "type").unwrap_or("general");
        let Some(task) = opt_str(options, "prompt") else {
            return;
        };

        let prompt =
            format!("Launch the `{kind}` subagent for this task and report back:\n\n{task}");
        self.run_prompt_command(ctx, command, prompt).await;
    }

    // -----------------------------------------------------------------------
    // session
    // -----------------------------------------------------------------------

    async fn handle_session(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();
        let (action, _) = subcommand(command);

        match action {
            "new" => {
                let embed = match self.start_new_session(user_id).await {
                    Ok(session_id) => ui::embed(
                        "New Session",
                        format!(
                            "Started a fresh session.\n`{}`",
                            session_id.chars().take(8).collect::<String>()
                        ),
                        ui::COLOR_SUCCESS,
                    ),
                    Err(e) => ui::embed("Session Error", e, ui::COLOR_ERROR),
                };
                self.reply(ctx, command, embed).await;
            }
            "archive" => {
                let embed = match self.archive_current_session_for_user(user_id).await {
                    Ok(Some(_)) => ui::embed(
                        "Session Archived",
                        "Your next message starts a new session.",
                        ui::COLOR_INFO,
                    ),
                    Ok(None) => ui::embed(
                        "No Active Session",
                        "There is nothing to archive.",
                        ui::COLOR_INFO,
                    ),
                    Err(e) => ui::embed("Archive Failed", e, ui::COLOR_ERROR),
                };
                self.reply(ctx, command, embed).await;
            }
            "delete" => {
                let custom_id = format!("{PANEL_PREFIX}deletesession:{user_id}");
                let embed = ui::embed(
                    "Delete This Session?",
                    "This permanently deletes the conversation and cannot be undone. Archiving keeps the history instead.",
                    ui::COLOR_WARNING,
                );
                let buttons = CreateActionRow::Buttons(vec![CreateButton::new(custom_id)
                    .label("Delete permanently")
                    .style(ButtonStyle::Danger)]);

                if let Err(e) = command
                    .create_response(
                        &ctx.http,
                        CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .embed(embed)
                                .components(vec![buttons])
                                .ephemeral(true),
                        ),
                    )
                    .await
                {
                    error!("Discord: failed to send delete confirmation: {e}");
                }
            }
            _ => {
                let embed = match self.get_active_session_id_for_user(user_id).await {
                    Some(session_id) => match self.agent.get_session(&session_id).await {
                        Ok(Some(session)) => {
                            let persona = self
                                .agent
                                .get_session_persona(&session_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|persona| persona.name)
                                .unwrap_or_else(|| "default".to_string());

                            CreateEmbed::new()
                                .title("Session")
                                .colour(ui::COLOR_PRIMARY)
                                .field("Id", format!("`{}`", session.id), false)
                                .field("Messages", format!("`{}`", session.messages.len()), true)
                                .field("Status", format!("`{}`", session.task_status), true)
                                .field("Persona", format!("`{persona}`"), true)
                                .footer(CreateEmbedFooter::new(
                                    "/settings opens the full control panel",
                                ))
                        }
                        _ => ui::embed(
                            "No Active Session",
                            "Send a message or run `/session new` to start one.",
                            ui::COLOR_INFO,
                        ),
                    },
                    None => ui::embed(
                        "No Active Session",
                        "Send a message or run `/session new` to start one.",
                        ui::COLOR_INFO,
                    ),
                };
                self.reply(ctx, command, embed).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // model / provider
    // -----------------------------------------------------------------------

    async fn handle_model(&self, ctx: &Context, command: &CommandInteraction) {
        let (action, options) = subcommand(command);

        if action != "set" {
            let provider = self.agent.active_provider().await;
            let provider_id = provider.provider_type().to_string();
            let model_id = provider.current_model().await;
            let catalog = self.agent.get_provider_models(provider_id.clone()).await;
            let known = catalog.iter().find(|model| model.id == model_id);

            let embed = CreateEmbed::new()
                .title("Model")
                .colour(ui::COLOR_PRIMARY)
                .field("Active", format!("`{model_id}`"), true)
                .field("Provider", format!("`{provider_id}`"), true)
                .field(
                    "Context",
                    match known {
                        Some(model) => format_context(model.context_window),
                        None => "not in the catalog".to_string(),
                    },
                    true,
                )
                .footer(CreateEmbedFooter::new(
                    "/model set switches model · /settings opens the full panel",
                ));
            self.reply(ctx, command, embed).await;
            return;
        }

        let Some(raw) = opt_str(options, "id") else {
            return;
        };

        let active_provider = self.agent.active_provider().await;
        let active_provider_id = active_provider.provider_type().to_string();

        // Autocomplete hands back `provider:model`; a hand-typed value is just a
        // model id on the current provider.
        let (provider_id, model_id) = match raw.split_once(':') {
            Some((provider, model)) if !provider.contains(' ') && !model.is_empty() => {
                (provider.to_string(), model.to_string())
            }
            _ => (active_provider_id, raw.to_string()),
        };

        let known = self
            .agent
            .get_provider_models(provider_id.clone())
            .await
            .into_iter()
            .any(|model| model.id == model_id);

        if !known {
            self.confirm_unknown_model(ctx, command, &provider_id, &model_id)
                .await;
            return;
        }

        let embed = match self.apply_model_switch(&provider_id, &model_id).await {
            Ok(()) => ui::embed(
                "Model Updated",
                format!("Now using `{model_id}` on `{provider_id}`."),
                ui::COLOR_SUCCESS,
            )
            .footer(CreateEmbedFooter::new(
                "Applies to every channel and the desktop app",
            )),
            Err(e) => ui::embed("Switch Failed", e, ui::COLOR_ERROR),
        };
        self.reply(ctx, command, embed).await;
    }

    /// Never silently accept a model the catalog does not know — offer the
    /// nearest matches and require one more click to force it.
    async fn confirm_unknown_model(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        provider_id: &str,
        model_id: &str,
    ) {
        let suggestions: Vec<String> = self
            .agent
            .search_catalog_models(model_id.to_string())
            .await
            .into_iter()
            .filter(|model| model.provider_id == provider_id)
            .take(3)
            .map(|model| format!("`{}`", model.id))
            .collect();

        let mut description =
            format!("`{model_id}` is not in the catalog for `{provider_id}`.\n\nIf that is a custom deployment it may still work — otherwise the next turn will fail with a model error.");
        if !suggestions.is_empty() {
            description.push_str(&format!("\n\nDid you mean {}?", suggestions.join(", ")));
        }

        let embed = ui::embed("Unknown Model", description, ui::COLOR_WARNING);
        let custom_id = format!(
            "{PANEL_PREFIX}forcemodel:{}:{provider_id}:{model_id}",
            command.user.id.get()
        );

        let mut response = CreateInteractionResponseMessage::new()
            .embed(embed)
            .ephemeral(true);

        // custom_id is capped at 100 characters; skip the button rather than
        // sending a payload Discord will reject.
        if custom_id.len() <= 100 {
            response =
                response.components(vec![CreateActionRow::Buttons(vec![CreateButton::new(
                    custom_id,
                )
                .label("Use anyway")
                .style(ButtonStyle::Secondary)])]);
        }

        if let Err(e) = command
            .create_response(&ctx.http, CreateInteractionResponse::Message(response))
            .await
        {
            error!("Discord: failed to send unknown-model prompt: {e}");
        }
    }

    async fn handle_provider(&self, ctx: &Context, command: &CommandInteraction) {
        let (action, options) = subcommand(command);
        let catalog = self.agent.get_catalog_state().await;
        let active = self.agent.active_provider().await;
        let active_id = active.provider_type().to_string();

        if action != "use" {
            let lines: Vec<String> = catalog
                .providers
                .iter()
                .filter(|provider| provider.connected || provider.id == active_id)
                .take(CHOICE_LIMIT)
                .map(|provider| {
                    format!(
                        "{} `{}` — {} model{}",
                        if provider.id == active_id { "▸" } else { " " },
                        provider.id,
                        provider.models.len(),
                        if provider.models.len() == 1 { "" } else { "s" }
                    )
                })
                .collect();

            let embed = ui::embed(
                "Providers",
                if lines.is_empty() {
                    "No providers are configured. Add one in the web UI.".to_string()
                } else {
                    lines.join("\n")
                },
                ui::COLOR_PRIMARY,
            )
            .footer(CreateEmbedFooter::new("▸ marks the active provider"));
            self.reply(ctx, command, embed).await;
            return;
        }

        let Some(provider_id) = opt_str(options, "id") else {
            return;
        };

        let configured = self.agent.get_config().await;
        let exists = configured
            .providers
            .iter()
            .any(|provider| provider.provider_type == provider_id)
            || catalog.providers.iter().any(|p| p.id == provider_id);

        if !exists {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Unknown Provider",
                    format!("`{provider_id}` is not configured. Add it in the web UI first."),
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        }

        let current_model = self.agent.get_current_model().await;
        let Some(model) = self.model_for_provider(provider_id, &current_model).await else {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "No Models",
                    format!("`{provider_id}` has no models in the catalog. Pick one explicitly with `/model set`."),
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };

        let embed = match self.apply_model_switch(provider_id, &model).await {
            Ok(()) => ui::embed(
                "Provider Updated",
                format!("Now using `{provider_id}` on `{model}`."),
                ui::COLOR_SUCCESS,
            ),
            Err(e) => ui::embed("Switch Failed", e, ui::COLOR_ERROR),
        };
        self.reply(ctx, command, embed).await;
    }

    // -----------------------------------------------------------------------
    // persona / workspace
    // -----------------------------------------------------------------------

    async fn handle_persona(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();
        let (action, options) = subcommand(command);

        if action == "list" || action.is_empty() {
            let lines: Vec<String> = self
                .agent
                .list_personas()
                .into_iter()
                .map(|persona| format!("`{}` — {}", persona.id, persona.summary))
                .collect();

            let embed = ui::embed(
                "Personas",
                if lines.is_empty() {
                    "No personas available.".to_string()
                } else {
                    lines.join("\n")
                },
                ui::COLOR_PRIMARY,
            )
            .footer(CreateEmbedFooter::new(
                "/persona set id:<id> — or pick one in /settings",
            ));
            self.reply(ctx, command, embed).await;
            return;
        }

        // Personas are per-session, so make sure there is one to attach to.
        let session_id = match self.get_or_create_session(user_id).await {
            Ok(session_id) => session_id,
            Err(e) => {
                self.reply(ctx, command, ui::embed("Session Error", e, ui::COLOR_ERROR))
                    .await;
                return;
            }
        };

        let embed = match action {
            "clear" => match self.agent.reset_session_persona(&session_id).await {
                Ok(()) => ui::embed(
                    "Persona Cleared",
                    "Reverted to default behaviour.",
                    ui::COLOR_SUCCESS,
                ),
                Err(e) => ui::embed("Clear Failed", e.to_string(), ui::COLOR_ERROR),
            },
            _ => {
                let Some(persona_id) = opt_str(options, "id") else {
                    return;
                };
                let character = opt_str(options, "character").map(str::to_string);

                match self
                    .agent
                    .set_session_persona(&session_id, persona_id.to_string(), character)
                    .await
                {
                    Ok(persona) => {
                        let mut description = format!("**{}**\n{}", persona.name, persona.summary);
                        if let Some(character) = &persona.roleplay_character {
                            description.push_str(&format!("\n\n_Roleplaying as: {character}_"));
                        }
                        ui::embed("Persona Activated", description, ui::COLOR_SUCCESS)
                    }
                    Err(e) => ui::embed("Persona Failed", e.to_string(), ui::COLOR_ERROR),
                }
            }
        };

        self.reply(ctx, command, embed).await;
    }

    async fn handle_workspace(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();
        let (action, options) = subcommand(command);

        if action != "set" {
            let workspaces = self.agent.get_workspaces().await;
            let active = self.agent.get_active_workspace().await;

            let lines: Vec<String> = workspaces
                .iter()
                .take(CHOICE_LIMIT)
                .map(|workspace| {
                    format!(
                        "{} `{}` → `{}`",
                        if workspace.id == active.id {
                            "▸"
                        } else {
                            " "
                        },
                        workspace.id,
                        workspace.path
                    )
                })
                .collect();

            let embed = ui::embed(
                "Workspaces",
                if lines.is_empty() {
                    "No workspaces configured.".to_string()
                } else {
                    lines.join("\n")
                },
                ui::COLOR_PRIMARY,
            )
            .footer(CreateEmbedFooter::new("▸ marks the active workspace"));
            self.reply(ctx, command, embed).await;
            return;
        }

        let Some(workspace_id) = opt_str(options, "id") else {
            return;
        };

        let embed = match self.agent.set_active_workspace(workspace_id).await {
            Ok(workspace) => {
                if let Err(e) = self.agent.save_config(&self.config_path).await {
                    error!("Discord: failed to persist workspace switch: {e}");
                }
                if let Ok(session_id) = self.get_or_create_session(user_id).await {
                    if let Err(e) = self
                        .agent
                        .set_session_workspace(&session_id, &workspace.id)
                        .await
                    {
                        warn!("Discord: failed to move session to new workspace: {e}");
                    }
                }
                ui::embed(
                    "Workspace Updated",
                    format!(
                        "Active workspace is `{}`\n`{}`",
                        workspace.id, workspace.path
                    ),
                    ui::COLOR_SUCCESS,
                )
            }
            Err(e) => ui::embed("Workspace Failed", e.to_string(), ui::COLOR_ERROR),
        };

        self.reply(ctx, command, embed).await;
    }

    // -----------------------------------------------------------------------
    // permissions / answer / mode
    // -----------------------------------------------------------------------

    async fn handle_permissions(&self, ctx: &Context, command: &CommandInteraction) {
        let (action, options) = subcommand(command);

        let embed = match action {
            "allow" | "deny" => {
                let Some(path) = opt_str(options, "path") else {
                    return;
                };
                let allowed = action == "allow";
                let _ = self
                    .agent
                    .respond_to_permission_prompt(path, allowed, false)
                    .await;

                ui::embed(
                    "Permission Updated",
                    format!(
                        "{} access to `{path}`",
                        if allowed { "Allowed" } else { "Denied" }
                    ),
                    if allowed {
                        ui::COLOR_SUCCESS
                    } else {
                        ui::COLOR_ERROR
                    },
                )
            }
            _ => {
                let prompts = self.agent.get_pending_permission_prompts().await;
                let lines: Vec<String> = prompts
                    .iter()
                    .take(10)
                    .map(|prompt| format!("`{}` — {}", prompt.path, prompt.source))
                    .collect();

                ui::embed(
                    "External Directory Permissions",
                    if lines.is_empty() {
                        "No pending permission requests.".to_string()
                    } else {
                        lines.join("\n")
                    },
                    ui::COLOR_PRIMARY,
                )
            }
        };

        self.reply(ctx, command, embed).await;
    }

    async fn handle_answer(&self, ctx: &Context, command: &CommandInteraction) {
        let Some(answer) = opt_str(&command.data.options, "answer") else {
            return;
        };

        let embed = match self.submit_answer(command.user.id.get(), answer).await {
            Ok(resolved) => ui::embed(
                "Answer Submitted",
                format!("Sent to the agent: **{resolved}**"),
                ui::COLOR_SUCCESS,
            ),
            Err(e) => ui::embed("No Pending Question", e, ui::COLOR_WARNING),
        };

        self.reply(ctx, command, embed).await;
    }

    async fn handle_mode(&self, ctx: &Context, command: &CommandInteraction) {
        let mode = opt_str(&command.data.options, "mode").unwrap_or("build");

        // The runtime has no mode switch yet — say so instead of implying the
        // agent's tool access just changed.
        let embed = ui::embed(
            "Mode",
            format!(
                "Requested `{mode}` mode.\n\nThis is a hint only: tool access is governed by the agent's permission settings, not by this command. Use `/permissions` to control filesystem access.",
            ),
            ui::COLOR_WARNING,
        );
        self.reply(ctx, command, embed).await;
    }

    // -----------------------------------------------------------------------
    // workflow
    // -----------------------------------------------------------------------

    async fn handle_workflow(&self, ctx: &Context, command: &CommandInteraction) {
        let user_id = command.user.id.get();
        let options = &command.data.options;
        let Some(workflow_name) = opt_str(options, "name") else {
            return;
        };
        let input = opt_str(options, "input");

        self.reply(
            ctx,
            command,
            ui::embed(
                "Workflow Starting",
                format!("Running `{workflow_name}`…"),
                ui::COLOR_INFO,
            ),
        )
        .await;

        let session_id = match self.get_or_create_session(user_id).await {
            Ok(session_id) => session_id,
            Err(e) => {
                self.edit_reply(ctx, command, ui::embed("Session Error", e, ui::COLOR_ERROR))
                    .await;
                return;
            }
        };

        let (workflow_db, executor) = match self.build_workflow_services() {
            Ok(services) => services,
            Err(e) => {
                self.edit_reply(
                    ctx,
                    command,
                    ui::embed("Workflow Service Error", e, ui::COLOR_ERROR),
                )
                .await;
                return;
            }
        };

        let workflow = match workflow_db.list_workflows().map(|workflows| {
            workflows
                .into_iter()
                .find(|workflow| workflow.name.eq_ignore_ascii_case(workflow_name))
        }) {
            Ok(Some(workflow)) => workflow,
            Ok(None) => {
                self.edit_reply(
                    ctx,
                    command,
                    ui::embed(
                        "Workflow Not Found",
                        format!("No workflow named `{workflow_name}`."),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
            Err(e) => {
                self.edit_reply(
                    ctx,
                    command,
                    ui::embed("Workflow Error", e.to_string(), ui::COLOR_ERROR),
                )
                .await;
                return;
            }
        };

        let version = match workflow_db.get_version(&workflow.id, workflow.current_version) {
            Ok(Some(version)) => version,
            Ok(None) => {
                self.edit_reply(
                    ctx,
                    command,
                    ui::embed(
                        "Workflow Version Missing",
                        "The current version of this workflow could not be loaded.",
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
            Err(e) => {
                self.edit_reply(
                    ctx,
                    command,
                    ui::embed("Workflow Error", e.to_string(), ui::COLOR_ERROR),
                )
                .await;
                return;
            }
        };

        let mut parameters = HashMap::new();
        if let Some(trigger_input) = input {
            parameters.insert(
                "trigger_input".to_string(),
                serde_json::Value::String(trigger_input.to_string()),
            );
        }

        let channel_id = command.channel_id.get();

        if let Some(workspace_id) = workflow.default_workspace_id.as_deref() {
            if let Err(e) = self
                .agent
                .set_session_workspace(&session_id, workspace_id)
                .await
            {
                warn!("Discord: failed to apply workflow workspace lock '{workspace_id}': {e}");
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
                Some(format!("discord:{user_id}")),
                vec!["discord".to_string()],
                Some(channel_id),
            )
            .await;

        let embed = match result {
            Ok(run) if run.status == "completed" => ui::embed(
                "Workflow Completed",
                run.output
                    .as_ref()
                    .map(Self::format_workflow_output)
                    .unwrap_or_else(|| "Workflow completed.".to_string()),
                ui::COLOR_SUCCESS,
            )
            .field("Workflow", &workflow.name, true)
            .field("Run", run.run_id.chars().take(8).collect::<String>(), true),
            Ok(run) => ui::embed(
                "Workflow Failed",
                run.error
                    .unwrap_or_else(|| "Unknown workflow failure".to_string()),
                ui::COLOR_ERROR,
            )
            .field("Workflow", &workflow.name, true)
            .field("Run", run.run_id.chars().take(8).collect::<String>(), true),
            Err(e) => ui::embed(
                "Workflow Execution Error",
                format!("```\n{e}\n```"),
                ui::COLOR_ERROR,
            ),
        };

        self.edit_reply(ctx, command, embed).await;
    }

    // -----------------------------------------------------------------------
    // help
    // -----------------------------------------------------------------------

    async fn handle_help(&self, ctx: &Context, command: &CommandInteraction) {
        let embed = CreateEmbed::new()
            .title("OSAgent")
            .description(
                "Talk to the agent by mentioning the bot in a channel, or by DMing it directly. Everything else is a slash command.",
            )
            .colour(ui::COLOR_PRIMARY)
            .field(
                "Chat",
                "`/chat` — send a message\n`/answer` — reply to a question the agent asked",
                false,
            )
            .field(
                "Configure",
                "`/settings` — provider, model, persona and workspace in one panel\n`/model set` · `/provider use` · `/persona set` · `/workspace set`",
                false,
            )
            .field(
                "Session",
                "`/session status` · `/session new` · `/session archive` · `/session delete`",
                false,
            )
            .field(
                "Run",
                "`/workflow` — run a saved workflow\n`/subagent` · `/lsp` — shortcuts that phrase a request for you",
                false,
            )
            .field(
                "Access",
                "`/permissions` — grant or refuse access to paths outside the workspace",
                false,
            )
            .footer(CreateEmbedFooter::new(
                "Responses show model · provider · persona in their footer",
            ));

        self.reply(ctx, command, embed).await;
    }

    // -----------------------------------------------------------------------
    // response helpers
    // -----------------------------------------------------------------------

    async fn reply(&self, ctx: &Context, command: &CommandInteraction, embed: CreateEmbed) {
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
            error!("Discord: failed to respond to command: {e}");
        }
    }

    async fn edit_reply(&self, ctx: &Context, command: &CommandInteraction, embed: CreateEmbed) {
        if let Err(e) = command
            .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
            .await
        {
            error!("Discord: failed to edit command response: {e}");
        }
    }
}
