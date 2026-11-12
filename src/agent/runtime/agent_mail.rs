//! Routing for the `agents` tool: structured messages between agents in one
//! conversation tree (a chat and its subagents) and between Global chat and
//! regular chats. Delivery itself happens in the recipient's run; see
//! [`crate::agent::mailbox`].
use super::*;
use crate::agent::mailbox::{self, KIND_FOLLOWUP, KIND_MESSAGE};
use crate::storage::AgentMail;

/// Undelivered messages one recipient may hold before senders are refused.
const MAX_PENDING_PER_RECIPIENT: usize = 50;
/// Unread messages one sender may have waiting for one recipient.
const MAX_UNREAD_FROM_ONE_SENDER: usize = 2;
/// How many recent messages to one recipient are checked for repeats.
const RECENT_REPEAT_WINDOW: usize = 8;

/// Prompt for a subagent run that a follow-up started; the task itself is the
/// delivered `<agent_message>` just before it.
pub(crate) const FOLLOWUP_RUN_PROMPT: &str = "Complete the follow-up task in the agent message above using your existing context. Your final answer is delivered to your parent.";

fn is_messageable_root(session: &Session) -> bool {
    session.parent_id.is_none()
        && session.agent_type == "primary"
        && session.metadata["discord_community"] != true
        && session.metadata["source"] != "discord-community"
}

fn agent_label(session: &Session) -> String {
    if session.is_global_coordinator() {
        return "Global chat".into();
    }
    let name = session.metadata["name"]
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    match (session.parent_id.is_some(), name) {
        (true, Some(name)) => name,
        (true, None) => format!("{} agent", session.agent_type),
        (false, Some(name)) => name,
        (false, None) => "Chat".into(),
    }
}

impl AgentRuntime {
    /// `[session, parent, ..., root]`, bounded so a corrupt parent cycle
    /// cannot hang the caller.
    fn ancestry(&self, session: &Session) -> Result<Vec<Session>> {
        let mut chain = vec![session.clone()];
        while let Some(parent_id) = chain.last().and_then(|s| s.parent_id.clone()) {
            if chain.len() > 32 || chain.iter().any(|s| s.id == parent_id) {
                break;
            }
            match self.storage.get_session(&parent_id)? {
                Some(parent) => chain.push(parent),
                None => break,
            }
        }
        Ok(chain)
    }

    /// Every session in the tree under `root`, root first.
    fn tree_members(&self, root: &Session) -> Result<Vec<Session>> {
        let mut out = vec![root.clone()];
        let mut index = 0;
        while index < out.len() && out.len() < 256 {
            let id = out[index].id.clone();
            for child in self.storage.get_child_sessions(&id)? {
                if !out.iter().any(|s| s.id == child.id) {
                    out.push(child);
                }
            }
            index += 1;
        }
        Ok(out)
    }

    fn agent_is_running(&self, session: &Session) -> bool {
        if session.parent_id.is_some() {
            self.subagent_manager.is_subagent_running(&session.id)
        } else {
            self.is_session_busy(&session.id)
        }
    }

    fn agent_state(&self, session: &Session) -> &'static str {
        if self.agent_is_running(session) {
            "running"
        } else if session.parent_id.is_none() && self.stopped_sessions.contains_key(&session.id) {
            "stopped"
        } else if session.parent_id.is_some() {
            match session.task_status.as_str() {
                "completed" => "completed",
                "failed" => "failed",
                "cancelled" => "stopped",
                "partial" => "partial",
                _ => "idle",
            }
        } else {
            "idle"
        }
    }

    /// Entry point for an `agents` tool call made by `source` (a primary chat
    /// or a subagent).
    pub async fn handle_agents_tool(
        &self,
        source: &Session,
        tool_call_id: &str,
        args: &serde_json::Value,
    ) -> Result<ToolResult> {
        if !is_messageable_root(
            &self
                .ancestry(source)?
                .last()
                .cloned()
                .unwrap_or_else(|| source.clone()),
        ) {
            return Err(OSAgentError::ToolExecution(
                "Agent messaging is not available in this conversation.".into(),
            ));
        }
        match args["action"].as_str().unwrap_or("list") {
            "list" => self.list_agent_tree(source),
            "send" => {
                self.send_agent_mail(source, tool_call_id, args, KIND_MESSAGE)
                    .await
            }
            "followup" => {
                self.send_agent_mail(source, tool_call_id, args, KIND_FOLLOWUP)
                    .await
            }
            "start_chat" => self.start_chat(source, tool_call_id, args).await,
            other => Err(OSAgentError::ToolExecution(format!(
                "Unknown action '{}'. Expected list, send, followup, or start_chat.",
                other
            ))),
        }
    }

    /// Global chat opens a new regular chat (in a configured workspace) and
    /// hands it a task. The chat is a separate conversation the user can open
    /// from the sidebar; it starts working immediately.
    async fn start_chat(
        &self,
        source: &Session,
        tool_call_id: &str,
        args: &serde_json::Value,
    ) -> Result<ToolResult> {
        if !source.is_global_coordinator() || source.parent_id.is_some() {
            return Err(OSAgentError::ToolExecution(
                "Only Global chat can start new chats. For work inside this chat, start a subagent instead.".into(),
            ));
        }
        let body = args["message"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                OSAgentError::ToolExecution("message (the new chat's task) cannot be empty.".into())
            })?;
        if body.chars().count() > mailbox::MAX_MESSAGE_CHARS {
            return Err(OSAgentError::ToolExecution(format!(
                "Agent messages are limited to {} characters.",
                mailbox::MAX_MESSAGE_CHARS
            )));
        }
        let workspace = match args["workspace_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => Some(self.config.read().await.get_workspace(id).ok_or_else(|| {
                OSAgentError::ToolExecution(format!(
                    "Workspace '{}' not found. Use an id from the available workspaces.",
                    id
                ))
            })?),
            None => None,
        };
        let chain = mailbox::session_chain(source) + 1;
        let cap = self
            .agent_settings
            .read()
            .await
            .subagent_auto_resume_max_turns
            .max(1) as u32;
        if chain > cap {
            return Err(OSAgentError::ToolExecution(format!(
                "Agent hand-offs since the user's last message have reached the limit ({}).",
                cap
            )));
        }

        let created = self.create_session().await?;
        if let Some(workspace) = &workspace {
            self.set_session_workspace(&created.id, &workspace.id)
                .await?;
        }
        let mut chat = self
            .storage
            .get_session(&created.id)?
            .ok_or_else(|| OSAgentError::Session("New chat not found".into()))?;
        let title = args["title"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(80).collect::<String>());
        if let Some(title) = &title {
            chat.metadata["name"] = serde_json::json!(title);
        }
        chat.metadata["started_by_session_id"] = serde_json::json!(source.id);
        self.storage.update_session(&chat)?;
        if let Some(title) = &title {
            self.update_session_title(&chat.id, title).await?;
        }

        let report_back = args["report_back"].as_bool().unwrap_or(true);
        let content = if report_back {
            format!(
                "{}\n\nWhen you finish, send a short result to Global chat with the agents tool (action send, target global).",
                body
            )
        } else {
            body.to_string()
        };
        let mail = AgentMail {
            id: Uuid::new_v4().to_string(),
            recipient_session_id: chat.id.clone(),
            sender_session_id: source.id.clone(),
            sender_label: agent_label(source),
            relation: "chat".into(),
            kind: KIND_FOLLOWUP.into(),
            content,
            chain,
            created_at: Utc::now(),
            delivered_at: None,
        };
        self.storage
            .enqueue_agent_mail(&mail, &format!("{}:{}", source.id, tool_call_id))?;
        self.notify_background_task_finished(chat.id.clone());

        let workspace_id = chat.metadata["workspace_id"]
            .as_str()
            .unwrap_or("default")
            .to_string();
        Ok(ToolResult::new(format!(
            "Started chat \"{}\" ({}) in workspace {}. It is working on the task now and appears in the sidebar.{}",
            agent_label(&chat),
            chat.id,
            workspace_id,
            if report_back {
                " It will send its result back here; do not wait or poll for it."
            } else {
                ""
            }
        )))
    }

    fn list_agent_tree(&self, source: &Session) -> Result<ToolResult> {
        let ancestry = self.ancestry(source)?;
        let root = ancestry.last().cloned().unwrap_or_else(|| source.clone());
        let mut lines = vec![format!("You are {} ({}).", agent_label(source), source.id)];
        for member in self.tree_members(&root)? {
            if member.id == source.id {
                continue;
            }
            let relation = if Some(member.id.as_str()) == source.parent_id.as_deref() {
                "parent"
            } else if member.parent_id.as_deref() == Some(source.id.as_str()) {
                "child"
            } else if member.parent_id.is_some() && member.parent_id == source.parent_id {
                "sibling"
            } else if ancestry.iter().any(|a| a.id == member.id) {
                "ancestor"
            } else {
                "relative"
            };
            let workspace = member.metadata["workspace_id"]
                .as_str()
                .map(|w| format!(" workspace={}", w))
                .unwrap_or_default();
            lines.push(format!(
                "- {} {} \"{}\" state={}{}",
                member.id,
                relation,
                agent_label(&member),
                self.agent_state(&member),
                workspace
            ));
        }
        if source.parent_id.is_none() && !source.is_global_coordinator() {
            lines.push(
                "- global: Global chat (another conversation; session-access permissions apply)"
                    .into(),
            );
        }
        if lines.len() == 1 {
            lines.push("No other agents in this conversation.".into());
        }
        Ok(ToolResult::new(lines.join("\n")))
    }

    async fn resolve_agent_target(
        &self,
        source: &Session,
        ancestry: &[Session],
        target: &str,
    ) -> Result<Session> {
        let root = ancestry.last().cloned().unwrap_or_else(|| source.clone());
        let not_found = || {
            OSAgentError::ToolExecution(format!(
                "Agent '{}' not found. Use action=list for ids.",
                target
            ))
        };
        match target {
            "parent" => {
                return ancestry.get(1).cloned().ok_or_else(|| {
                    OSAgentError::ToolExecution("This chat has no parent agent.".into())
                })
            }
            "root" => return Ok(root),
            "global" => {
                if root.is_global_coordinator() {
                    return Ok(root);
                }
                return self.get_or_create_global_session().await;
            }
            _ => {}
        }
        if let Some(session) = self.storage.get_session(target)? {
            return Ok(session);
        }
        if target.len() >= 6 {
            let members = self.tree_members(&root)?;
            let mut matches = members.into_iter().filter(|s| s.id.starts_with(target));
            if let Some(found) = matches.next() {
                if matches.next().is_some() {
                    return Err(OSAgentError::ToolExecution(format!(
                        "Agent id prefix '{}' is ambiguous; use the full id.",
                        target
                    )));
                }
                return Ok(found);
            }
        }
        Err(not_found())
    }

    async fn send_agent_mail(
        &self,
        source: &Session,
        tool_call_id: &str,
        args: &serde_json::Value,
        kind: &str,
    ) -> Result<ToolResult> {
        let body = args["message"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| OSAgentError::ToolExecution("message cannot be empty.".into()))?;
        if body.chars().count() > mailbox::MAX_MESSAGE_CHARS {
            return Err(OSAgentError::ToolExecution(format!(
                "Agent messages are limited to {} characters.",
                mailbox::MAX_MESSAGE_CHARS
            )));
        }
        let target_ref = args["target"]
            .as_str()
            .or_else(|| args["target_session_id"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                OSAgentError::ToolExecution(
                    "Specify target (an id from list, parent, root, or global).".into(),
                )
            })?;

        let source_ancestry = self.ancestry(source)?;
        let target = self
            .resolve_agent_target(source, &source_ancestry, target_ref)
            .await?;
        if target.id == source.id {
            return Err(OSAgentError::ToolExecution(
                "Cannot send a message to yourself.".into(),
            ));
        }
        let target_ancestry = self.ancestry(&target)?;
        let source_root = source_ancestry.last().map(|s| s.id.clone());
        let target_root = target_ancestry.last().map(|s| s.id.clone());

        // How the recipient sees the sender.
        let relation = if source_root == target_root {
            if target.parent_id.as_deref() == Some(source.id.as_str()) {
                "parent"
            } else if source.parent_id.as_deref() == Some(target.id.as_str()) {
                "child"
            } else if source.parent_id.is_some() && source.parent_id == target.parent_id {
                "sibling"
            } else if target_ancestry.iter().any(|a| a.id == source.id) {
                "ancestor"
            } else if source_ancestry.iter().any(|a| a.id == target.id) {
                "descendant"
            } else {
                "relative"
            }
        } else {
            // Across conversation trees only chats themselves may talk, and
            // only with Global chat; the session-access policy still applies.
            if source.parent_id.is_some() {
                return Err(OSAgentError::ToolExecution(
                    "Subagents can message agents in their own conversation only. Ask your parent to contact another chat.".into(),
                ));
            }
            if !is_messageable_root(&target) {
                return Err(OSAgentError::ToolExecution(
                    "Only another chat (not its subagents) can be messaged across conversations."
                        .into(),
                ));
            }
            if !(source.is_global_coordinator() || target.is_global_coordinator()) {
                return Err(OSAgentError::ToolExecution(
                    "Regular chats can message Global chat; Global chat can message regular chats."
                        .into(),
                ));
            }
            let auth_args = serde_json::json!({"action": "send", "target_session_id": target.id});
            self.authorize_session_access(&source.id, tool_call_id, &auth_args)
                .await?;
            "chat"
        };

        if kind == KIND_FOLLOWUP && source_ancestry.iter().any(|a| a.id == target.id) {
            return Err(OSAgentError::ToolExecution(
                "Follow-up tasks go to agents below you, siblings, or another chat. Report to your parent with send, or in your final answer.".into(),
            ));
        }

        let source_chain = mailbox::session_chain(source);
        let chain = if kind == KIND_FOLLOWUP {
            source_chain + 1
        } else {
            source_chain
        };
        let cap = self
            .agent_settings
            .read()
            .await
            .subagent_auto_resume_max_turns
            .max(1) as u32;
        if kind == KIND_FOLLOWUP && chain > cap {
            return Err(OSAgentError::ToolExecution(format!(
                "Agent hand-offs since the user's last message have reached the limit ({}). Finish with what you have, or use send to share information without starting another turn.",
                cap
            )));
        }
        let dedupe_key = format!("{}:{}", source.id, tool_call_id);
        if let Some(stored) = self
            .storage
            .find_agent_mail_by_key(&target.id, &dedupe_key)?
        {
            return Ok(ToolResult::new(format!(
                "Message {} to {} ({}) was already sent.",
                stored.id,
                agent_label(&target),
                target.id
            )));
        }
        // Agents cannot wait for replies, so a model that needs an input it
        // does not have tends to poll by re-sending. Refuse repeats and
        // piling onto unread messages; tell it to finish instead.
        let recent =
            self.storage
                .recent_agent_mail_between(&source.id, &target.id, RECENT_REPEAT_WINDOW)?;
        let normalized = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        if recent
            .iter()
            .any(|m| normalized(&m.content) == normalized(body))
        {
            return Err(OSAgentError::ToolExecution(format!(
                "You already sent this to {}. Do not repeat or poll: agents cannot wait for replies. Continue with what you have, or finish and say what is missing.",
                agent_label(&target)
            )));
        }
        let unread = recent.iter().filter(|m| m.delivered_at.is_none()).count();
        if unread >= MAX_UNREAD_FROM_ONE_SENDER {
            return Err(OSAgentError::ToolExecution(format!(
                "{} has not read your last {} messages yet. Do not send more or poll: agents cannot wait for replies. Continue with what you have, or finish and say what is missing.",
                agent_label(&target),
                unread
            )));
        }
        if self.storage.pending_agent_mail(&target.id)?.len() >= MAX_PENDING_PER_RECIPIENT {
            return Err(OSAgentError::ToolExecution(
                "The recipient has too many unread messages. Wait for it to catch up.".into(),
            ));
        }

        let mail = AgentMail {
            id: Uuid::new_v4().to_string(),
            recipient_session_id: target.id.clone(),
            sender_session_id: source.id.clone(),
            sender_label: agent_label(source),
            relation: relation.to_string(),
            kind: kind.to_string(),
            content: body.to_string(),
            chain,
            created_at: Utc::now(),
            delivered_at: None,
        };
        let (stored, created) = self.storage.enqueue_agent_mail(&mail, &dedupe_key)?;
        let label = agent_label(&target);
        if !created {
            return Ok(ToolResult::new(format!(
                "Message {} to {} ({}) was already sent.",
                stored.id, label, target.id
            )));
        }

        let status = self
            .trigger_agent_mail(&target, &target_ancestry, kind)
            .await;
        Ok(ToolResult::new(format!(
            "{} {} for {} ({}): {}.",
            if kind == KIND_FOLLOWUP {
                "Follow-up"
            } else {
                "Message"
            },
            stored.id,
            label,
            target.id,
            status
        )))
    }

    /// Decide what happens to a freshly stored message. Plain messages never
    /// start a turn; follow-ups start one when the recipient is idle.
    async fn trigger_agent_mail(
        &self,
        target: &Session,
        target_ancestry: &[Session],
        kind: &str,
    ) -> String {
        let root_stopped = target_ancestry
            .last()
            .is_some_and(|root| self.stopped_sessions.contains_key(&root.id));
        if self.agent_is_running(target) {
            if kind == KIND_FOLLOWUP && target.parent_id.is_none() {
                // Ensure a turn follows if the run ends before its next
                // model request picks the follow-up up.
                self.pending_wakes.insert(target.id.clone(), ());
            }
            return "recipient is running; it sees this before its next model request".into();
        }
        if kind == KIND_MESSAGE {
            return "recipient is idle; it sees this at the start of its next turn".into();
        }
        if root_stopped {
            return "recipient's chat is stopped; queued until the user resumes it".into();
        }
        if target.parent_id.is_none() {
            // Same path as a background-result wake: run guard, stop
            // handling and the consecutive-turn limit all apply.
            self.notify_background_task_finished(target.id.clone());
            return "recipient was idle; starting its turn".into();
        }
        let parent_id = target.parent_id.clone().unwrap_or_default();
        let model_override = target.metadata["model_override"]
            .as_str()
            .map(str::to_string);
        match self
            .subagent_manager
            .resume_subagent(
                parent_id,
                target.id.clone(),
                format!("Follow-up for {}", agent_label(target)),
                FOLLOWUP_RUN_PROMPT.into(),
                target.agent_type.clone(),
                true,
                model_override,
            )
            .await
        {
            Ok(_) => {
                "recipient was idle; resumed in the background, its final answer goes to its parent"
                    .into()
            }
            Err(error) if self.subagent_manager.is_subagent_running(&target.id) => {
                let _ = error;
                "recipient is running; it sees this before its next model request".into()
            }
            Err(error) => format!("queued, but the recipient could not be resumed: {}", error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::{
        Provider, ProviderResponse, StreamEvent, StreamToolCallDelta, ToolDefinition,
    };
    use crate::storage::ToolCall;
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tempfile::{tempdir, TempDir};

    /// Scripted model. A run whose user text contains `SUB_BLOCK` (subagent)
    /// or `PRIMARY_BLOCK` (chat) waits on `gate` once, then calls a tool.
    /// Every other reply reports the agent messages the model was shown.
    struct MailProvider {
        gate: tokio::sync::Semaphore,
        entered: AtomicUsize,
        calls: std::sync::Mutex<Vec<Vec<Message>>>,
    }

    impl MailProvider {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                gate: tokio::sync::Semaphore::new(0),
                entered: AtomicUsize::new(0),
                calls: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn first_pass(messages: &[Message], marker: &str) -> bool {
            messages
                .iter()
                .any(|m| m.role == "user" && m.content.contains(marker))
                && !messages.iter().any(|m| m.role == "tool")
        }

        async fn block(&self) {
            self.entered.fetch_add(1, Ordering::SeqCst);
            self.gate.acquire().await.unwrap().forget();
        }

        fn report(messages: &[Message]) -> String {
            let seen = messages
                .iter()
                .filter(|m| m.content.starts_with("<agent_message"))
                .filter_map(|m| m.content.lines().nth(1))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("DONE seen=[{}]", seen)
        }
    }

    #[async_trait::async_trait]
    impl Provider for MailProvider {
        async fn complete(
            &self,
            _: Option<&str>,
            messages: &[Message],
            _: &[ToolDefinition],
        ) -> Result<ProviderResponse> {
            self.calls.lock().unwrap().push(messages.to_vec());
            let blocked = Self::first_pass(messages, "SUB_BLOCK");
            if blocked {
                self.block().await;
            }
            Ok(ProviderResponse {
                content: Some(if blocked {
                    String::new()
                } else {
                    Self::report(messages)
                }),
                thinking: None,
                tool_calls: blocked.then(|| {
                    vec![ToolCall {
                        id: "list-1".into(),
                        name: "agents".into(),
                        arguments: json!({"action": "list"}),
                    }]
                }),
                finish_reason: if blocked { "tool_calls" } else { "stop" }.into(),
                retry_count: 0,
                context_compressed: false,
                compressed_summary: None,
                usage: None,
            })
        }

        async fn complete_stream(
            &self,
            _: Option<&str>,
            messages: &[Message],
            _: &[ToolDefinition],
        ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent>>> {
            self.calls.lock().unwrap().push(messages.to_vec());
            let blocked = Self::first_pass(messages, "PRIMARY_BLOCK");
            if blocked {
                self.block().await;
            }
            let event = StreamEvent {
                event_type: "content".into(),
                content: (!blocked).then(|| Self::report(messages)),
                thinking: None,
                tool_call_deltas: if blocked {
                    vec![StreamToolCallDelta {
                        index: 0,
                        id: Some("todo-1".into()),
                        name: Some("todoread".into()),
                        arguments: Some("{}".into()),
                    }]
                } else {
                    vec![]
                },
                finish_reason: Some(if blocked { "tool_calls" } else { "stop" }.into()),
                usage: None,
                done: true,
            };
            Ok(futures::stream::iter(vec![Ok(event)]).boxed())
        }

        async fn model_context_window(&self) -> Option<usize> {
            Some(128_000)
        }
        async fn current_model(&self) -> String {
            "test-model".into()
        }
        async fn set_model(&self, _: String) {}
        fn provider_type(&self) -> &str {
            "test"
        }
    }

    fn test_config(temp: &TempDir) -> Config {
        let mut config = Config::default_config();
        config.search.enabled = false;
        config.agent.checkpoint_enabled = false;
        config.agent.session_access_default_action = crate::permission::PermissionAction::Allow;
        config.agent.workspace = temp.path().to_string_lossy().into_owned();
        config.agent.workspaces[0].paths = vec![crate::config::WorkspacePath {
            path: config.agent.workspace.clone(),
            permission: crate::config::WorkspacePermission::ReadWrite,
            description: None,
        }];
        config.storage.database = temp.path().join("mail.db").to_string_lossy().into_owned();
        config
    }

    async fn runtime_with(config: Config, provider: Arc<MailProvider>) -> Arc<AgentRuntime> {
        let agent = AgentRuntime::new(config).unwrap();
        *agent.provider.write().await = provider.clone();
        agent
            .get_subagent_manager()
            .override_provider_for_tests(provider);
        agent
    }

    async fn until(what: &str, mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {}", what));
    }

    fn session(agent: &AgentRuntime, id: &str) -> Session {
        agent.storage.get_session(id).unwrap().unwrap()
    }

    fn agent_messages(session: &Session) -> Vec<&Message> {
        session
            .messages
            .iter()
            .filter(|m| m.metadata["kind"] == mailbox::MESSAGE_KIND)
            .collect()
    }

    fn completions(session: &Session) -> Vec<&Message> {
        session
            .messages
            .iter()
            .filter(|m| m.metadata["kind"] == "subagent_completed")
            .collect()
    }

    async fn call(
        agent: &AgentRuntime,
        source: &Session,
        id: &str,
        args: serde_json::Value,
    ) -> Result<String> {
        agent
            .handle_agents_tool(source, id, &args)
            .await
            .map(|r| r.output)
    }

    #[tokio::test]
    async fn tree_routing_attributes_relations_and_enforces_rules() {
        let temp = tempdir().unwrap();
        let agent = runtime_with(test_config(&temp), MailProvider::new()).await;
        let root = agent.create_session().await.unwrap();
        let child = |parent: &str| {
            agent
                .storage
                .create_subagent_session(parent.into(), "m".into(), "p".into(), "general".into())
                .unwrap()
        };
        let a = child(&root.id);
        let b = child(&root.id);
        let grandchild = child(&a.id);

        // Sibling message: stored, attributed, idempotent per tool call, and
        // it does not start the idle recipient.
        let send = json!({"action": "send", "target": b.id, "message": "Skip auth.rs"});
        let out = call(&agent, &a, "c1", send.clone()).await.unwrap();
        assert!(out.contains("idle"), "{out}");
        let out = call(&agent, &a, "c1", send).await.unwrap();
        assert!(out.contains("already sent"), "{out}");
        let pending = agent.storage.pending_agent_mail(&b.id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].relation, "sibling");
        assert_eq!(pending[0].sender_session_id, a.id);
        assert!(!agent.get_subagent_manager().is_subagent_running(&b.id));

        // Polling is refused: a repeat (even reformatted) and a third unread
        // message to the same recipient.
        let again = json!({"action": "send", "target": b.id, "message": "skip  AUTH.rs"});
        let err = call(&agent, &a, "c1b", again).await.unwrap_err();
        assert!(err.to_string().contains("already sent this"), "{err}");
        let second = json!({"action": "send", "target": b.id, "message": "Also skip db.rs"});
        call(&agent, &a, "c1c", second).await.unwrap();
        let third = json!({"action": "send", "target": b.id, "message": "Still waiting?"});
        let err = call(&agent, &a, "c1d", third).await.unwrap_err();
        assert!(
            err.to_string().contains("has not read your last 2"),
            "{err}"
        );

        // Relations as the recipient sees them; aliases and id prefixes.
        let up = json!({"action": "send", "target": "parent", "message": "up"});
        call(&agent, &a, "c2", up).await.unwrap();
        let down = json!({"action": "send", "target": &grandchild.id[..8], "message": "down"});
        call(&agent, &root, "c3", down).await.unwrap();
        let to_root = json!({"action": "send", "target": "root", "message": "to root"});
        call(&agent, &grandchild, "c4", to_root).await.unwrap();
        let root_mail = agent.storage.pending_agent_mail(&root.id).unwrap();
        assert_eq!(
            root_mail
                .iter()
                .map(|m| m.relation.as_str())
                .collect::<Vec<_>>(),
            vec!["child", "descendant"]
        );
        assert_eq!(
            agent.storage.pending_agent_mail(&grandchild.id).unwrap()[0].relation,
            "ancestor"
        );

        // Follow-ups never go up the tree; subagents never leave it.
        for (source, target) in [(&a, "parent"), (&grandchild, "root")] {
            let args = json!({"action": "followup", "target": target, "message": "x"});
            let err = call(&agent, source, "f", args).await.unwrap_err();
            assert!(err.to_string().contains("Follow-up tasks go to"), "{err}");
        }
        let args = json!({"action": "send", "target": "global", "message": "x"});
        let err = call(&agent, &a, "g", args).await.unwrap_err();
        assert!(err.to_string().contains("own conversation"), "{err}");
        let args = json!({"action": "send", "target": a.id, "message": "x"});
        assert!(call(&agent, &a, "s", args).await.is_err());
        let args = json!({"action": "send", "target": b.id, "message": "  "});
        assert!(call(&agent, &a, "e", args).await.is_err());
        let args = json!({"action": "send", "target": "no-such-agent", "message": "x"});
        assert!(call(&agent, &a, "n", args).await.is_err());

        // A chain of agent hand-offs is bounded; plain messages still flow.
        let mut tired = b.clone();
        tired.metadata["agent_chain"] = json!(5);
        let args = json!({"action": "followup", "target": a.id, "message": "more"});
        let err = call(&agent, &tired, "cap", args).await.unwrap_err();
        assert!(err.to_string().contains("limit (5)"), "{err}");
        let args = json!({"action": "send", "target": a.id, "message": "fyi"});
        call(&agent, &tired, "cap2", args).await.unwrap();

        let listing = call(&agent, &a, "l", json!({"action": "list"}))
            .await
            .unwrap();
        for expected in [
            format!("{} parent", root.id),
            format!("{} sibling", b.id),
            format!("{} child", grandchild.id),
        ] {
            assert!(listing.contains(&expected), "{listing}");
        }

        // Delivery appends visible, attributed messages exactly once.
        let mut recipient = session(&agent, &b.id);
        let delivery = mailbox::deliver_pending(&agent.storage, &mut recipient).unwrap();
        assert_eq!(delivery.delivered, 2);
        assert!(agent.storage.pending_agent_mail(&b.id).unwrap().is_empty());
        let stored = session(&agent, &b.id);
        let delivered = agent_messages(&stored);
        assert_eq!(delivered.len(), 2);
        assert!(delivered[0].content.contains("relation=\"sibling\""));
        assert!(delivered[0].content.contains("Skip auth.rs"));
    }

    #[tokio::test]
    async fn followup_resumes_an_idle_subagent_and_its_answer_reaches_the_parent() {
        let temp = tempdir().unwrap();
        let agent = runtime_with(test_config(&temp), MailProvider::new()).await;
        let manager = agent.get_subagent_manager();
        let root = agent.create_session().await.unwrap();
        let child = manager
            .spawn_subagent(
                root.id.clone(),
                "audit".into(),
                "Initial task".into(),
                "general".into(),
                true,
                None,
            )
            .await
            .unwrap();
        until("first completion", || {
            !manager.is_subagent_running(&child)
                && !agent.is_session_busy(&root.id)
                && completions(&session(&agent, &root.id)).len() == 1
        })
        .await;

        let args = json!({"action": "followup", "target": child, "message": "Check token expiry"});
        let out = call(&agent, &session(&agent, &root.id), "f1", args)
            .await
            .unwrap();
        assert!(out.contains("resumed"), "{out}");
        until("follow-up completion", || {
            !manager.is_subagent_running(&child)
                && !agent.is_session_busy(&root.id)
                && completions(&session(&agent, &root.id)).len() == 2
        })
        .await;

        let child_session = session(&agent, &child);
        let name = child_session.metadata["agent_name"].as_str().unwrap();
        assert_eq!(child_session.metadata["name"], format!("{} · audit", name));
        assert!(child_session.messages.iter().any(|m| m.role == "system"
            && m.content
                .contains(&format!("You are {}, a subagent working on: audit", name))));
        let delivered = agent_messages(&child_session);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].metadata["agent_message_type"], "followup");
        assert_eq!(delivered[0].metadata["relation"], "parent");
        let mail_at = child_session
            .messages
            .iter()
            .position(|m| m.metadata["kind"] == mailbox::MESSAGE_KIND)
            .unwrap();
        let prompt_at = child_session
            .messages
            .iter()
            .position(|m| m.content == FOLLOWUP_RUN_PROMPT)
            .unwrap();
        assert!(mail_at < prompt_at, "the task precedes the run prompt");
        assert_eq!(mailbox::session_chain(&child_session), 1);
        let root_session = session(&agent, &root.id);
        assert!(completions(&root_session)[1]
            .content
            .contains("Check token expiry"));
    }

    #[tokio::test]
    async fn running_subagent_gets_messages_at_its_next_boundary_without_a_new_turn() {
        let temp = tempdir().unwrap();
        let provider = MailProvider::new();
        let agent = runtime_with(test_config(&temp), provider.clone()).await;
        let manager = agent.get_subagent_manager();
        let root = agent.create_session().await.unwrap();
        let child = manager
            .spawn_subagent(
                root.id.clone(),
                "work".into(),
                "SUB_BLOCK work".into(),
                "general".into(),
                true,
                None,
            )
            .await
            .unwrap();
        until("model request", || {
            provider.entered.load(Ordering::SeqCst) == 1
        })
        .await;

        let args = json!({"action": "send", "target": child, "message": "Use cache dir /tmp/x"});
        let out = call(&agent, &root, "m1", args).await.unwrap();
        assert!(out.contains("running"), "{out}");
        provider.gate.add_permits(1);
        until("completion", || {
            !manager.is_subagent_running(&child)
                && !agent.is_session_busy(&root.id)
                && completions(&session(&agent, &root.id)).len() == 1
        })
        .await;

        let child_session = session(&agent, &child);
        let tool_at = child_session
            .messages
            .iter()
            .position(|m| m.role == "tool")
            .unwrap();
        assert!(child_session.messages[tool_at].content.contains("parent"));
        let mail_at = child_session
            .messages
            .iter()
            .position(|m| m.metadata["kind"] == mailbox::MESSAGE_KIND)
            .unwrap();
        assert!(
            mail_at > tool_at,
            "delivered at the boundary after the tool call"
        );
        assert!(child_session
            .messages
            .last()
            .unwrap()
            .content
            .contains("Use cache dir /tmp/x"));
        assert_eq!(
            agent.storage.list_subagent_tasks(&root.id).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn chats_get_mid_turn_messages_and_followups_wake_idle_or_busy_chats() {
        let temp = tempdir().unwrap();
        let provider = MailProvider::new();
        let agent = runtime_with(test_config(&temp), provider.clone()).await;
        let root = agent.create_session().await.unwrap();
        let helper = agent
            .storage
            .create_subagent_session(root.id.clone(), "m".into(), "p".into(), "general".into())
            .unwrap();

        // A busy chat sees a child's message before its next model request.
        agent
            .clone()
            .spawn_message_run(
                root.id.clone(),
                "PRIMARY_BLOCK please".into(),
                "test".into(),
            )
            .await
            .unwrap();
        until("chat model request", || {
            provider.entered.load(Ordering::SeqCst) == 1
        })
        .await;
        let args = json!({"action": "send", "target": "parent", "message": "Found it in auth.rs"});
        let out = call(&agent, &helper, "u1", args).await.unwrap();
        assert!(out.contains("running"), "{out}");
        provider.gate.add_permits(1);
        until("chat turn end", || !agent.is_session_busy(&root.id)).await;
        {
            let calls = provider.calls.lock().unwrap();
            let last = calls.last().unwrap();
            let tool_at = last.iter().position(|m| m.role == "tool").unwrap();
            let mail_at = last
                .iter()
                .position(|m| m.content.contains("Found it in auth.rs"))
                .expect("message reached the model mid-turn");
            assert!(mail_at > tool_at);
        }
        assert_eq!(
            agent_messages(&session(&agent, &root.id))[0].metadata["relation"],
            "child"
        );

        // A follow-up starts an idle Global chat.
        let global = agent.get_or_create_global_session().await.unwrap();
        let args = json!({"action": "followup", "target": "global", "message": "Summarize status"});
        let out = call(&agent, &session(&agent, &root.id), "g1", args)
            .await
            .unwrap();
        assert!(out.contains("starting its turn"), "{out}");
        let replied = |text: &str| {
            !agent.is_session_busy(&global.id)
                && session(&agent, &global.id)
                    .messages
                    .last()
                    .is_some_and(|m| m.role == "assistant" && m.content.contains(text))
        };
        until("global turn", || replied("Summarize status")).await;
        let g = session(&agent, &global.id);
        assert_eq!(agent_messages(&g)[0].metadata["relation"], "chat");
        assert_eq!(mailbox::session_chain(&g), 1);

        // A busy chat gets the follow-up after its current run ends.
        let guard = agent.try_start_run(&global.id, "test").unwrap();
        let args = json!({"action": "followup", "target": global.id, "message": "And the tests"});
        let out = call(&agent, &session(&agent, &root.id), "g2", args)
            .await
            .unwrap();
        assert!(out.contains("running"), "{out}");
        drop(guard);
        agent.check_pending_wake(&global.id);
        until("deferred follow-up", || {
            agent
                .storage
                .pending_agent_mail(&global.id)
                .unwrap()
                .is_empty()
                && replied("And the tests")
        })
        .await;
    }

    #[tokio::test]
    async fn global_chat_starts_a_new_chat_that_works_on_the_task() {
        let temp = tempdir().unwrap();
        let config = test_config(&temp);
        let workspace_id = config.get_active_workspace().id;
        let agent = runtime_with(config, MailProvider::new()).await;
        let global = agent.get_or_create_global_session().await.unwrap();
        let regular = agent.create_session().await.unwrap();

        // Only Global chat opens chats; unknown workspaces are refused.
        let args = json!({"action": "start_chat", "message": "Check the weather"});
        let err = call(&agent, &regular, "r1", args).await.unwrap_err();
        assert!(err.to_string().contains("Only Global chat"), "{err}");
        let args = json!({"action": "start_chat", "message": "x", "workspace_id": "missing"});
        assert!(call(&agent, &global, "w1", args).await.is_err());

        let before = agent.list_session_summaries().await.unwrap().len();
        let args = json!({
            "action": "start_chat",
            "message": "Check the weather in Canning Vale",
            "title": "Weather check",
            "workspace_id": workspace_id,
        });
        let out = call(&agent, &global, "s1", args).await.unwrap();
        assert!(out.contains("Started chat \"Weather check\""), "{out}");
        let summaries = agent.list_session_summaries().await.unwrap();
        assert_eq!(summaries.len(), before + 1);
        let chat_id = out
            .split(['(', ')'])
            .nth(1)
            .expect("new chat id in the result")
            .to_string();

        until("new chat turn", || {
            !agent.is_session_busy(&chat_id)
                && session(&agent, &chat_id)
                    .messages
                    .last()
                    .is_some_and(|m| m.role == "assistant" && m.content.contains("Canning Vale"))
        })
        .await;
        let chat = session(&agent, &chat_id);
        assert!(chat.parent_id.is_none());
        assert_eq!(chat.agent_type, "primary");
        assert_eq!(chat.metadata["workspace_id"], workspace_id);
        assert_eq!(chat.metadata["started_by_session_id"], global.id);
        let task = agent_messages(&chat);
        assert_eq!(task.len(), 1);
        assert_eq!(task[0].metadata["agent_message_type"], "followup");
        assert_eq!(task[0].metadata["relation"], "chat");
        assert!(task[0].content.contains("target global"));

        // The new chat can report back to Global chat.
        let args = json!({"action": "send", "target": "global", "message": "Sunny, 26C"});
        call(&agent, &chat, "back", args).await.unwrap();
        let back = agent.storage.pending_agent_mail(&global.id).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].sender_session_id, chat_id);
    }

    #[tokio::test]
    async fn stopped_and_restarted_chats_keep_mail_and_access_policy_applies() {
        let temp = tempdir().unwrap();
        let config = test_config(&temp);
        let agent = runtime_with(config.clone(), MailProvider::new()).await;
        let regular = agent.create_session().await.unwrap();
        let global = agent.get_or_create_global_session().await.unwrap();

        // Regular chats reach only Global chat across conversations.
        let other = agent.create_session().await.unwrap();
        let args = json!({"action": "send", "target": other.id, "message": "hi"});
        assert!(call(&agent, &regular, "x", args).await.is_err());

        agent.stop_session(&regular.id).await.unwrap();
        let args =
            json!({"action": "followup", "target": regular.id, "message": "Resume the migration"});
        let out = call(&agent, &global, "s1", args).await.unwrap();
        assert!(out.contains("stopped"), "{out}");
        assert!(!agent.is_session_busy(&regular.id));
        agent
            .config
            .write()
            .await
            .agent
            .session_access_default_action = crate::permission::PermissionAction::Deny;
        let args = json!({"action": "send", "target": regular.id, "message": "denied"});
        assert!(call(&agent, &global, "s2", args).await.is_err());
        assert_eq!(
            agent.storage.pending_agent_mail(&regular.id).unwrap().len(),
            1
        );
        drop(agent);

        let reopened = runtime_with(config, MailProvider::new()).await;
        assert_eq!(
            reopened
                .storage
                .pending_agent_mail(&regular.id)
                .unwrap()
                .len(),
            1
        );
        reopened
            .process_message(&regular.id, "hello".into(), "test".into())
            .await
            .unwrap();
        assert!(reopened
            .storage
            .pending_agent_mail(&regular.id)
            .unwrap()
            .is_empty());
        let s = session(&reopened, &regular.id);
        let mail_at = s
            .messages
            .iter()
            .position(|m| m.metadata["kind"] == mailbox::MESSAGE_KIND)
            .unwrap();
        let user_at = s
            .messages
            .iter()
            .position(|m| m.content == "hello")
            .unwrap();
        assert!(mail_at < user_at);
        assert!(s
            .messages
            .last()
            .unwrap()
            .content
            .contains("Resume the migration"));
        assert_eq!(
            mailbox::session_chain(&s),
            0,
            "a person's turn resets the chain"
        );
    }
}
