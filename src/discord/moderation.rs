//! Basic moderation tools for trusted operators.
//!
//! `/mod` covers the everyday community-server chores: warn, timeout, untimeout,
//! kick, ban, unban, purge, and slowmode. It is deliberately narrow:
//!
//! * Opt-in through `discord.moderation_enabled` (off by default) — `/mod` is
//!   only registered while the flag is on.
//! * Guild-only and gated on `trusted_users` alone — trusted *roles* and
//!   community grants never unlock it — so a trusted user keeps moderation
//!   reach inside the community server without that server becoming a trusted
//!   location for chat or agent access.
//! * Every action checks Discord-side permissions for the invoker *and* the
//!   bot, and refuses the server owner, other bots, protected trusted members,
//!   and anyone at or above either acting role position.

use super::commands::{int_opt, opt_i64, opt_str, str_opt, sub, subcommand};
use super::{ui, Handler};
use serenity::builder::{
    CreateCommand, CreateCommandOption, CreateMessage, EditChannel, EditMember, GetMessages,
};
use serenity::model::application::{
    CommandDataOption, CommandDataOptionValue, CommandInteraction, CommandOptionType,
};
use serenity::model::id::{RoleId, UserId};
use serenity::model::permissions::Permissions;
use serenity::model::Timestamp;
use serenity::prelude::*;
use std::sync::atomic::Ordering;
use tracing::warn;

/// Discord caps communication timeouts at 28 days.
const MAX_TIMEOUT_MINUTES: i64 = 40_320;
/// Discord caps bulk deletes at 100 messages.
const MAX_PURGE: i64 = 100;
/// Discord caps slowmode at 6 hours.
const MAX_SLOWMODE: i64 = 21_600;
/// Messages older than 14 days cannot be bulk deleted.
const BULK_DELETE_MAX_AGE_SECS: i64 = 14 * 24 * 60 * 60;

// ---------------------------------------------------------------------------
// option helpers
// ---------------------------------------------------------------------------

fn user_opt(name: &str, description: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::User, name, description).required(true)
}

fn req_int(name: &str, description: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::Integer, name, description).required(true)
}

fn opt_user(options: &[CommandDataOption], name: &str) -> Option<UserId> {
    options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| match &option.value {
            CommandDataOptionValue::User(id) => Some(*id),
            _ => None,
        })
}

/// The Discord permission each action needs, and its human name.
fn action_permission(action: &str) -> Option<(Permissions, &'static str)> {
    match action {
        "warn" | "timeout" | "untimeout" => {
            Some((Permissions::MODERATE_MEMBERS, "Moderate Members"))
        }
        "kick" => Some((Permissions::KICK_MEMBERS, "Kick Members")),
        "ban" | "unban" => Some((Permissions::BAN_MEMBERS, "Ban Members")),
        "purge" => Some((Permissions::MANAGE_MESSAGES, "Manage Messages")),
        "slowmode" => Some((Permissions::MANAGE_CHANNELS, "Manage Channels")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// safety checks (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Role facts the safety checks need — kept plain so the checks are testable
/// without building serenity structs.
#[derive(Debug, Clone, Copy)]
struct RoleInfo {
    id: u64,
    position: u16,
    permissions: Permissions,
}

/// Highest role position a member holds (`0` when they only have @everyone).
fn highest_role_position(member_roles: &[RoleId], roles: &[RoleInfo]) -> u16 {
    member_roles
        .iter()
        .map(|role_id| role_id.get())
        .filter_map(|role_id| roles.iter().find(|role| role.id == role_id))
        .map(|role| role.position)
        .max()
        .unwrap_or(0)
}

/// Guild-wide permissions for a member, straight from the role list.
fn member_permissions(
    roles: &[RoleInfo],
    member_roles: &[RoleId],
    everyone_role_id: u64,
    is_owner: bool,
) -> Permissions {
    if is_owner {
        return Permissions::all();
    }
    let mut permissions = roles
        .iter()
        .find(|role| role.id == everyone_role_id)
        .map(|role| role.permissions)
        .unwrap_or_default();
    for role_id in member_roles {
        let role_id = role_id.get();
        if let Some(role) = roles.iter().find(|role| role.id == role_id) {
            permissions |= role.permissions;
        }
    }
    if permissions.contains(Permissions::ADMINISTRATOR) {
        Permissions::all()
    } else {
        permissions
    }
}

#[derive(Debug, Clone, Copy)]
struct TargetFacts {
    id: u64,
    top_role: u16,
    is_owner: bool,
    is_bot: bool,
    is_trusted: bool,
}

#[derive(Debug, Clone, Copy)]
struct ActorFacts {
    id: u64,
    is_owner: bool,
    top_role: u16,
}

/// Why a moderation target is refused, or `None` when the action may proceed.
fn target_block_reason(
    target: &TargetFacts,
    actor: &ActorFacts,
    bot_top_role: u16,
) -> Option<&'static str> {
    if target.is_owner {
        return Some("the server owner cannot be moderated");
    }
    if target.id == actor.id {
        return Some("you cannot moderate yourself");
    }
    if target.is_trusted {
        return Some("trusted users and trusted roles are protected from moderation");
    }
    if target.is_bot {
        return Some("bot accounts are protected from these tools");
    }
    if !actor.is_owner && actor.top_role <= target.top_role {
        return Some("their highest role is at or above yours");
    }
    if bot_top_role <= target.top_role {
        return Some("their highest role is at or above the bot's highest role");
    }
    None
}

// ---------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------

/// `/mod` is only registered while `moderation_enabled` is on, so community
/// servers never see a moderation command they cannot use.
pub(super) fn moderation_commands() -> Vec<CreateCommand> {
    vec![CreateCommand::new("mod")
        .description("Moderation tools for trusted users")
        .add_option(
            sub("warn", "Warn a member in a direct message")
                .add_sub_option(user_opt("user", "Member to warn"))
                .add_sub_option(str_opt("reason", "Reason shown to the member", true)),
        )
        .add_option(
            sub("timeout", "Timeout a member (1 minute to 28 days)")
                .add_sub_option(user_opt("user", "Member to timeout"))
                .add_sub_option(req_int("minutes", "Timeout length in minutes"))
                .add_sub_option(str_opt("reason", "Reason for the timeout", false)),
        )
        .add_option(
            sub("untimeout", "Remove a member's timeout")
                .add_sub_option(user_opt("user", "Member to restore")),
        )
        .add_option(
            sub("kick", "Kick a member from this server")
                .add_sub_option(user_opt("user", "Member to kick"))
                .add_sub_option(str_opt("reason", "Reason for the kick", false)),
        )
        .add_option(
            sub("ban", "Ban a member from this server")
                .add_sub_option(user_opt("user", "Member to ban"))
                .add_sub_option(str_opt("reason", "Reason for the ban", false))
                .add_sub_option(int_opt(
                    "delete_days",
                    "Days of message history to delete (0-7)",
                )),
        )
        .add_option(
            sub("unban", "Remove a ban").add_sub_option(user_opt("user", "Banned user to unban")),
        )
        .add_option(
            sub("purge", "Bulk-delete recent messages in this channel")
                .add_sub_option(req_int("count", "Messages to delete (1-100)")),
        )
        .add_option(
            sub("slowmode", "Set this channel's slowmode (0 disables)")
                .add_sub_option(req_int("seconds", "Slowmode in seconds (0-21600)")),
        )]
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

impl Handler {
    pub(super) async fn handle_moderation(&self, ctx: &Context, command: &CommandInteraction) {
        let (action, options) = subcommand(command);

        let Some(guild_id) = command.guild_id else {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Server Required",
                    "Moderation tools only work inside a server.",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };

        let Some((required, permission_name)) = action_permission(action) else {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Unknown Action",
                    "That moderation action does not exist.",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };

        // Being trusted never grants Discord permissions nobody granted: the
        // invoker and the bot both need the matching permission.
        match command
            .member
            .as_ref()
            .and_then(|member| member.permissions)
        {
            Some(permissions)
                if permissions.contains(required)
                    || permissions.contains(Permissions::ADMINISTRATOR) => {}
            Some(_) => {
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Missing Permission",
                        format!(
                            "You need the **{permission_name}** permission in this server to do that."
                        ),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
            None => {
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Permissions Unavailable",
                        "Discord did not report your permissions for this interaction.",
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        }

        let roles = match guild_id.roles(&ctx.http).await {
            Ok(roles) => roles,
            Err(error) => {
                warn!(
                    "Discord: moderation could not read roles in guild {}: {error}",
                    guild_id.get()
                );
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Moderation Failed",
                        format!("Could not read this server's roles: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };
        let guild = match guild_id.to_partial_guild(&ctx.http).await {
            Ok(guild) => guild,
            Err(error) => {
                warn!(
                    "Discord: moderation could not read guild {}: {error}",
                    guild_id.get()
                );
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Moderation Failed",
                        format!("Could not read this server: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };
        let bot_id = UserId::new(super::BOT_USER_ID.load(Ordering::Relaxed));
        if bot_id.get() == 0 {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Bot Starting Up",
                    "The bot has not finished starting yet — try again in a moment.",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        }
        let bot_member = match guild_id.member(&ctx.http, bot_id).await {
            Ok(member) => member,
            Err(error) => {
                warn!(
                    "Discord: moderation could not read the bot member in guild {}: {error}",
                    guild_id.get()
                );
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Moderation Failed",
                        format!("Could not read my own member entry: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };

        let role_infos: Vec<RoleInfo> = roles
            .values()
            .map(|role| RoleInfo {
                id: role.id.get(),
                position: role.position,
                permissions: role.permissions,
            })
            .collect();
        let bot_permissions = member_permissions(
            &role_infos,
            &bot_member.roles,
            guild_id.get(),
            guild.owner_id == bot_id,
        );
        if !bot_permissions.contains(required) {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Missing Bot Permission",
                    format!(
                        "I need the **{permission_name}** permission in this server, with my highest role above the target, to do that."
                    ),
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        }
        let bot_top_role = highest_role_position(&bot_member.roles, &role_infos);

        if action == "purge" {
            self.mod_purge(ctx, command, options).await;
            return;
        }
        if action == "slowmode" {
            self.mod_slowmode(ctx, command, options).await;
            return;
        }

        // Member actions resolve their target first and run the shared safety
        // checks. `unban` only needs the id: unbanning never harms anyone, and
        // the user is usually not a member anymore.
        let Some(target_id) = opt_user(options, "user") else {
            self.reply(
                ctx,
                command,
                ui::embed(
                    "Missing User",
                    "Name the member to act on.",
                    ui::COLOR_ERROR,
                ),
            )
            .await;
            return;
        };

        if matches!(action, "warn" | "timeout" | "untimeout" | "kick" | "ban") {
            let member = match guild_id.member(&ctx.http, target_id).await {
                Ok(member) => member,
                Err(_) => {
                    self.reply(
                        ctx,
                        command,
                        ui::embed(
                            "Unknown Member",
                            "That user is not a member of this server.",
                            ui::COLOR_ERROR,
                        ),
                    )
                    .await;
                    return;
                }
            };

            let discord = self.agent.discord_config().await.unwrap_or_default();
            let target_is_trusted = discord.trusted_users.contains(&target_id.get())
                || member
                    .roles
                    .iter()
                    .any(|role_id| discord.trusted_roles.contains(&role_id.get()));
            let actor_roles = command
                .member
                .as_ref()
                .map(|member| member.roles.clone())
                .unwrap_or_default();
            let target = TargetFacts {
                id: target_id.get(),
                top_role: highest_role_position(&member.roles, &role_infos),
                is_owner: guild.owner_id == target_id,
                is_bot: member.user.bot,
                is_trusted: target_is_trusted,
            };
            let actor = ActorFacts {
                id: command.user.id.get(),
                is_owner: guild.owner_id == command.user.id,
                top_role: highest_role_position(&actor_roles, &role_infos),
            };
            if let Some(blocked) = target_block_reason(&target, &actor, bot_top_role) {
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Target Protected",
                        format!("That member was not moderated: {blocked}."),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        }

        let reason = ui::truncate_chars(
            opt_str(options, "reason").unwrap_or("No reason provided"),
            512,
        );
        let mention = format!("<@{}>", target_id.get());
        let result: Result<String, String> = match action {
            "warn" => {
                let message = format!("**Warning from {}**\nReason: {reason}", guild.name);
                match target_id
                    .direct_message(&ctx.http, CreateMessage::new().content(message))
                    .await
                {
                    Ok(_) => Ok(format!(
                        "{mention} was warned in a direct message. Reason: {reason}"
                    )),
                    Err(error) => Ok(format!(
                        "{mention} was warned, but the direct message failed ({error}). Reason: {reason}"
                    )),
                }
            }
            "timeout" => {
                let minutes = opt_i64(options, "minutes")
                    .unwrap_or(0)
                    .clamp(1, MAX_TIMEOUT_MINUTES);
                let until = Timestamp::now().unix_timestamp() + minutes.saturating_mul(60);
                match Timestamp::from_unix_timestamp(until) {
                    Ok(until) => guild_id
                        .edit_member(
                            &ctx.http,
                            target_id,
                            EditMember::new()
                                .disable_communication_until_datetime(until)
                                .audit_log_reason(&reason),
                        )
                        .await
                        .map(|_| {
                            format!(
                                "{mention} is timed out for {minutes} minute(s). Reason: {reason}"
                            )
                        })
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                }
            }
            "untimeout" => guild_id
                .edit_member(
                    &ctx.http,
                    target_id,
                    EditMember::new()
                        .enable_communication()
                        .audit_log_reason(&reason),
                )
                .await
                .map(|_| format!("{mention} can talk again."))
                .map_err(|error| error.to_string()),
            "kick" => guild_id
                .kick_with_reason(&ctx.http, target_id, &reason)
                .await
                .map(|_| format!("{mention} was kicked. Reason: {reason}"))
                .map_err(|error| error.to_string()),
            "ban" => {
                let delete_days = opt_i64(options, "delete_days").unwrap_or(0).clamp(0, 7) as u8;
                guild_id
                    .ban_with_reason(&ctx.http, target_id, delete_days, &reason)
                    .await
                    .map(|_| format!("{mention} was banned. Reason: {reason}"))
                    .map_err(|error| error.to_string())
            }
            "unban" => guild_id
                .unban(&ctx.http, target_id)
                .await
                .map(|_| format!("The ban on {mention} was removed."))
                .map_err(|error| error.to_string()),
            other => Err(format!("Unknown moderation action: {other}")),
        };

        let (title, colour) = match action {
            "warn" => ("Member Warned", ui::COLOR_WARNING),
            "timeout" => ("Member Timed Out", ui::COLOR_SUCCESS),
            "untimeout" => ("Timeout Removed", ui::COLOR_SUCCESS),
            "kick" => ("Member Kicked", ui::COLOR_SUCCESS),
            "ban" => ("Member Banned", ui::COLOR_SUCCESS),
            "unban" => ("Ban Removed", ui::COLOR_SUCCESS),
            _ => ("Moderation", ui::COLOR_INFO),
        };
        match result {
            Ok(description) => {
                self.reply(ctx, command, ui::embed(title, description, colour))
                    .await;
            }
            Err(error) => {
                warn!(
                    "Discord: moderation action '{action}' failed in guild {}: {error}",
                    guild_id.get()
                );
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Moderation Failed",
                        ui::truncate_chars(&error, 512),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
            }
        }
    }

    /// Bulk-delete recent messages. Discord refuses bulk deletes older than 14
    /// days, so stale messages are skipped and reported instead of failing the
    /// whole purge.
    async fn mod_purge(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        options: &[CommandDataOption],
    ) {
        let count = opt_i64(options, "count").unwrap_or(0).clamp(1, MAX_PURGE) as u8;
        let messages = match command
            .channel_id
            .messages(&ctx.http, GetMessages::new().limit(count))
            .await
        {
            Ok(messages) => messages,
            Err(error) => {
                warn!("Discord: purge could not read messages: {error}");
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Purge Failed",
                        format!("Could not read this channel's messages: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
                return;
            }
        };

        let cutoff = Timestamp::now().unix_timestamp() - BULK_DELETE_MAX_AGE_SECS;
        let (fresh, stale) = messages
            .into_iter()
            .partition::<Vec<_>, _>(|message| message.timestamp.unix_timestamp() >= cutoff);
        let deleted = fresh.len();

        let outcome = if fresh.len() == 1 {
            command
                .channel_id
                .delete_message(&ctx.http, fresh[0].id)
                .await
                .map(|_| ())
        } else if fresh.len() > 1 {
            command
                .channel_id
                .delete_messages(&ctx.http, fresh.iter().map(|message| message.id))
                .await
                .map(|_| ())
        } else {
            Ok(())
        };

        match outcome {
            Ok(()) => {
                let mut description = format!("Deleted {deleted} message(s).");
                if !stale.is_empty() {
                    description.push_str(&format!(
                        " Skipped {} older than 14 days — Discord refuses bulk deletes that old.",
                        stale.len()
                    ));
                }
                self.reply(
                    ctx,
                    command,
                    ui::embed("Purge Complete", description, ui::COLOR_SUCCESS),
                )
                .await;
            }
            Err(error) => {
                warn!("Discord: purge failed: {error}");
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Purge Failed",
                        format!("Could not delete those messages: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
            }
        }
    }

    async fn mod_slowmode(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
        options: &[CommandDataOption],
    ) {
        let seconds = opt_i64(options, "seconds")
            .unwrap_or(0)
            .clamp(0, MAX_SLOWMODE) as u16;
        match command
            .channel_id
            .edit(&ctx.http, EditChannel::new().rate_limit_per_user(seconds))
            .await
        {
            Ok(_) => {
                let description = if seconds == 0 {
                    "Slowmode is off.".to_string()
                } else {
                    format!("Slowmode is now {seconds} second(s).")
                };
                self.reply(
                    ctx,
                    command,
                    ui::embed("Slowmode Updated", description, ui::COLOR_SUCCESS),
                )
                .await;
            }
            Err(error) => {
                warn!("Discord: slowmode change failed: {error}");
                self.reply(
                    ctx,
                    command,
                    ui::embed(
                        "Slowmode Failed",
                        format!("Could not change this channel's slowmode: {error}"),
                        ui::COLOR_ERROR,
                    ),
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: u64, position: u16, permissions: Permissions) -> RoleInfo {
        RoleInfo {
            id,
            position,
            permissions,
        }
    }

    #[test]
    fn highest_role_position_ignores_unknown_roles() {
        let roles = vec![
            role(1, 0, Permissions::empty()),
            role(2, 3, Permissions::empty()),
            role(3, 9, Permissions::empty()),
        ];
        assert_eq!(highest_role_position(&[], &roles), 0);
        assert_eq!(highest_role_position(&[RoleId::new(2)], &roles), 3);
        assert_eq!(
            highest_role_position(&[RoleId::new(2), RoleId::new(3), RoleId::new(42)], &roles),
            9
        );
    }

    #[test]
    fn member_permissions_add_up_roles_and_honor_admin() {
        let roles = vec![
            role(1, 0, Permissions::KICK_MEMBERS),
            role(2, 3, Permissions::BAN_MEMBERS),
            role(3, 9, Permissions::MANAGE_MESSAGES),
        ];

        let permissions = member_permissions(&roles, &[RoleId::new(2)], 1, false);
        assert!(permissions.contains(Permissions::KICK_MEMBERS));
        assert!(permissions.contains(Permissions::BAN_MEMBERS));
        assert!(!permissions.contains(Permissions::MANAGE_MESSAGES));

        // The server owner and administrators get everything.
        assert!(member_permissions(&roles, &[], 1, true).contains(Permissions::MANAGE_CHANNELS));
        let admin = vec![role(4, 1, Permissions::ADMINISTRATOR)];
        assert!(member_permissions(&admin, &[RoleId::new(4)], 1, false)
            .contains(Permissions::BAN_MEMBERS));
    }

    fn actor() -> ActorFacts {
        ActorFacts {
            id: 100,
            is_owner: false,
            top_role: 5,
        }
    }

    fn target() -> TargetFacts {
        TargetFacts {
            id: 200,
            top_role: 2,
            is_owner: false,
            is_bot: false,
            is_trusted: false,
        }
    }

    #[test]
    fn protected_targets_are_never_moderated() {
        // Ordinary target below both role positions: allowed.
        assert_eq!(target_block_reason(&target(), &actor(), 8), None);

        let mut owner = target();
        owner.is_owner = true;
        assert!(target_block_reason(&owner, &actor(), 8).is_some());

        let mut myself = target();
        myself.id = actor().id;
        assert!(target_block_reason(&myself, &actor(), 8).is_some());

        let mut trusted = target();
        trusted.is_trusted = true;
        assert!(target_block_reason(&trusted, &actor(), 8).is_some());

        let mut bot = target();
        bot.is_bot = true;
        assert!(target_block_reason(&bot, &actor(), 8).is_some());
    }

    #[test]
    fn hierarchy_beats_trust_but_not_the_owner() {
        // Equal or higher roles than the invoker are refused...
        let mut high = target();
        high.top_role = 5;
        assert!(target_block_reason(&high, &actor(), 8).is_some());

        // ...but the owner bypasses their own role position...
        let mut owner_actor = actor();
        owner_actor.is_owner = true;
        assert_eq!(target_block_reason(&high, &owner_actor, 8), None);

        // ...while nobody acts above the bot's highest role.
        let mut above_bot = target();
        above_bot.top_role = 8;
        assert!(target_block_reason(&above_bot, &owner_actor, 8).is_some());
    }
}
