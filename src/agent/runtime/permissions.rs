//! Path authorization, external access, permission prompts, rules
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub(super) fn requested_absolute_path(
        tool_name: &str,
        args: &serde_json::Value,
        workspace_root: Option<&Path>,
    ) -> Option<String> {
        if tool_name == "bash" || tool_name == "process" {
            if let Some(workdir) = args.get("workdir").and_then(|value| value.as_str()) {
                if let Some(resolved) = Self::resolve_key_path(workdir, workspace_root) {
                    return Some(resolved);
                }
            }

            let command = args.get("command").and_then(|value| value.as_str())?;
            for token in Self::extract_command_absolute_paths(command) {
                let is_inside = workspace_root
                    .map(|root| Self::is_within_workspace_path(Path::new(&token), root))
                    .unwrap_or(false);
                if !is_inside {
                    return Some(token);
                }
            }
            return None;
        }

        // Batch read_file `paths[]`: every entry must be in-workspace for
        // parallel fan-out; first outside-workspace path blocks it.
        if tool_name == "read_file" {
            if let Some(paths) = args.get("paths").and_then(|v| v.as_array()) {
                for entry in paths {
                    if let Some(key) = entry.as_str() {
                        if let Some(outside) = Self::resolve_key_path(key, workspace_root) {
                            // Absolute-outside or escaping `..` — block parallel.
                            // Confirm it is genuinely outside before blocking:
                            // resolve_key_path already returns None for
                            // in-workspace absolute paths.
                            return Some(outside);
                        }
                    }
                }
                return None;
            }
        }

        let key = match tool_name {
            "read_file" => args
                .get("filePath")
                .and_then(|value| value.as_str())
                .or_else(|| args.get("path").and_then(|value| value.as_str())),
            "write_file" | "edit_file" | "delete_file" | "list_files" | "grep" | "glob" => {
                args.get("path").and_then(|value| value.as_str())
            }
            _ => None,
        }?;

        Self::resolve_key_path(key, workspace_root)
    }

    pub(super) fn resolve_key_path(key: &str, workspace_root: Option<&Path>) -> Option<String> {
        let expanded = shellexpand::tilde(key).to_string();
        if Path::new(&expanded).is_absolute() {
            // Absolute but inside the workspace is fine for parallel fan-out
            // (Codex-style: gate on outside-workspace, not on absolute).
            if let Some(root) = workspace_root {
                if Self::is_within_workspace_path(Path::new(&expanded), root) {
                    return None;
                }
            }
            return Some(expanded);
        }

        if let Some(root) = workspace_root {
            if expanded.contains("..") {
                let resolved = root.join(&expanded);
                let canonical = resolved.canonicalize().unwrap_or(resolved);
                if canonical.is_absolute() && !canonical.starts_with(root) {
                    return Some(canonical.to_string_lossy().to_string());
                }
            }
        }

        None
    }

    /// Expands the environment-variable forms a shell would resolve *after*
    /// this check runs (`%VAR%`, `$env:VAR`, `$VAR`, `~`), so a path like
    /// `%USERPROFILE%\Documents` is recognised as the absolute path it will
    /// actually become. Unknown variables are left untouched.
    pub(super) fn expand_path_vars(token: &str) -> String {
        // %VAR% (cmd)
        let mut out = String::new();
        let mut rest = token;
        while let Some(start) = rest.find('%') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('%') else { break };
            let name = &after[..end];
            out.push_str(&rest[..start]);
            match std::env::var(name) {
                Ok(value) if !name.is_empty() => out.push_str(&value),
                _ => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);

        // $env:VAR (PowerShell) and $VAR (POSIX)
        let mut expanded = String::new();
        let mut rest = out.as_str();
        while let Some(start) = rest.find('$') {
            expanded.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let (body, prefix_len) = match after.strip_prefix("env:") {
                Some(stripped) => (stripped, "env:".len()),
                None => (after, 0),
            };
            let name_len = body
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(body.len());
            let name = &body[..name_len];
            match std::env::var(name) {
                Ok(value) if !name.is_empty() => expanded.push_str(&value),
                _ => expanded.push_str(&rest[start..start + 1 + prefix_len + name_len]),
            }
            rest = &body[name_len..];
        }
        expanded.push_str(rest);

        shellexpand::tilde(&expanded).to_string()
    }

    /// Heuristically scans a shell command string for tokens that look like
    /// absolute filesystem paths (Windows drive-letter, UNC, or POSIX-style),
    /// so outside-workspace approval can trigger even when the path is
    /// embedded in the command text rather than passed via `workdir`.
    /// Environment-variable forms are expanded first. Still best-effort: it
    /// cannot resolve globs or paths built up dynamically inside the command.
    pub(super) fn extract_command_absolute_paths(command: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;

        for ch in command.chars() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                c if c.is_whitespace() && !in_single && !in_double => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        tokens
            .into_iter()
            .map(|token| Self::expand_path_vars(&token))
            .filter(|token| Self::looks_like_absolute_path(token))
            .collect()
    }

    pub(super) fn looks_like_absolute_path(token: &str) -> bool {
        let bytes = token.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        if token.starts_with("\\\\") && token.len() > 2 {
            return true;
        }
        if cfg!(not(windows)) && token.starts_with('/') && token.len() > 1 {
            return true;
        }
        false
    }

    pub(super) fn path_is_inside_workspace(path: &Path, workspace: &WorkspaceConfig) -> bool {
        workspace.paths.iter().any(|entry| {
            let root = PathBuf::from(shellexpand::tilde(&entry.path).to_string());
            Self::is_within_workspace_path(path, &root)
        })
    }

    /// Collapse `.` and `..` components without touching the filesystem.
    fn lexically_normalized(path: &Path) -> PathBuf {
        use std::path::Component;
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    let last = out.components().next_back();
                    if matches!(last, Some(Component::Normal(_))) {
                        out.pop();
                    } else {
                        out.push("..");
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// Resolve the longest existing ancestor so symlinks are followed and
    /// not-yet-existing write targets still compare against the real root.
    fn resolve_for_containment(path: &Path) -> PathBuf {
        let normalized = Self::lexically_normalized(path);
        let mut probe = normalized.clone();
        let mut suffix: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if probe.exists() {
                if let Ok(canonical) = probe.canonicalize() {
                    let mut resolved = Self::lexically_normalized(&canonical);
                    for part in suffix.iter().rev() {
                        resolved.push(part);
                    }
                    return resolved;
                }
            }
            match probe.file_name() {
                Some(name) => {
                    suffix.push(name.to_os_string());
                    if !probe.pop() {
                        break;
                    }
                }
                None => break,
            }
        }
        normalized
    }

    #[cfg(windows)]
    fn components_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }

    #[cfg(not(windows))]
    fn components_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
        left == right
    }

    /// Containment on resolved paths, component-wise, with the platform's
    /// case rules. Never falls back to raw lexical comparison: a path that
    /// cannot be resolved is treated as outside the workspace.
    pub(super) fn is_within_workspace_path(path: &Path, root: &Path) -> bool {
        let resolved_path = Self::resolve_for_containment(path);
        let resolved_root = Self::resolve_for_containment(root);
        let mut path_components = resolved_path.components();
        for root_component in resolved_root.components() {
            match path_components.next() {
                Some(path_component)
                    if Self::components_equal(
                        path_component.as_os_str(),
                        root_component.as_os_str(),
                    ) =>
                {
                    continue;
                }
                _ => return false,
            }
        }
        true
    }

    pub(super) fn external_execution_root(tool_name: &str, path: &str) -> String {
        let path = Path::new(path);
        if matches!(
            tool_name,
            "list_files" | "grep" | "glob" | "bash" | "process"
        ) || path.is_dir()
        {
            return path.to_string_lossy().to_string();
        }
        path.parent().unwrap_or(path).to_string_lossy().to_string()
    }

    /// The composer's approval behavior (Codex-style `approvals` chip):
    /// resolves a would-be *Ask* to Allow/Deny without raising the prompt
    /// dock, or keeps Ask so the prompt appears. Explicit Allow/Deny rules
    /// are resolved before this — this only governs requests that would
    /// otherwise ask the user.
    async fn resolve_ask_by_approval_policy(&self) -> Result<crate::permission::PermissionAction> {
        Ok(self.config.read().await.agent.approval_policy.clone())
    }

    /// Policy gate for the `sessions` tool. Reading or searching
    /// conversations other than the caller's own (its own subagent children
    /// are exempt) follows `session_access` permission rules first, then
    /// `[agent] session_access_default_action`; Ask raises the same approval
    /// popup used for outside-workspace path access, addressed as
    /// `session://<id>` (or `session://*` for broad search).
    pub(super) async fn authorize_session_access(
        &self,
        caller_session_id: &str,
        tool_call_id: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        let action = args
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("read");
        let target = args
            .get("target_session_id")
            .and_then(|value| value.as_str());
        let scope = crate::tools::sessions::resolve_access_scope(caller_session_id, action, target);
        let resource = match scope {
            crate::tools::sessions::SessionAccessScope::Ungated
            | crate::tools::sessions::SessionAccessScope::Exempt => return Ok(()),
            crate::tools::sessions::SessionAccessScope::Resource(resource) => resource,
        };

        // A read of a session that does not exist needs no approval — the
        // tool answers with a clean not-found error instead of a popup.
        if let Some(id) = resource.strip_prefix("session://") {
            if id != "*" {
                let other = match self.storage.get_session(id)? {
                    Some(session) => Some(session),
                    // Short hex prefixes (as printed by `action=list` and
                    // accepted by the sessions tool) resolve to the same
                    // session; gate the resolved session, not the literal id.
                    None => self.resolve_session_by_prefix(id).await?,
                };
                match other {
                    None => return Ok(()),
                    Some(other) => {
                        // The caller's own subagent children are part of
                        // its task tree and need no prompt.
                        if other.parent_id.as_deref() == Some(caller_session_id) {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // The composer's approval behavior can be a hard session lockdown:
        // `deny` refuses everything outside the workspace before any grant,
        // whitelist, or rule can allow it.
        if let crate::permission::PermissionAction::Deny =
            self.config.read().await.agent.approval_policy
        {
            return Err(OSAgentError::ToolExecution(format!(
                "Access to conversation '{}' is denied by this session's approval policy (auto-deny)",
                resource
            )));
        }

        if self
            .external_manager
            .has_granted_permission(caller_session_id, &resource)
            .await
        {
            return Ok(());
        }

        let effective = {
            let config = self.config.read().await;
            config
                .evaluate_permission_rule("session_access", &resource)
                .unwrap_or_else(|| config.agent.session_access_default_action.clone())
        };
        // Note: `evaluate_permission_rule` returns the `permission` module's
        // action enum; the external-path flow uses the identical-looking
        // `external` module one.
        let effective = match effective {
            crate::permission::PermissionAction::Ask => {
                self.resolve_ask_by_approval_policy().await?
            }
            other => other,
        };
        match effective {
            crate::permission::PermissionAction::Allow => return Ok(()),
            crate::permission::PermissionAction::Deny => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Access to conversation '{}' is denied by policy",
                    resource
                )))
            }
            crate::permission::PermissionAction::Ask => {}
        }

        let (prompt, response) = self
            .external_manager
            .create_waiting_prompt(
                caller_session_id.to_string(),
                format!("sessions:{}", tool_call_id),
                resource.clone(),
                "session_read".to_string(),
                Vec::new(),
            )
            .await;

        let timeout =
            std::time::Duration::from_secs(self.external_manager.prompt_timeout_seconds().max(1));
        let response = tokio::select! {
            response = tokio::time::timeout(timeout, response) => response,
            _ = self.wait_for_cancellation(caller_session_id) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
        };
        match response {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(OSAgentError::ToolExecution(format!(
                "User denied access to conversation '{}'",
                resource
            ))),
            Ok(Err(_)) => Err(OSAgentError::ToolExecution(
                "Session access permission request was cancelled".to_string(),
            )),
            Err(_) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                Err(OSAgentError::ToolExecution(format!(
                    "Permission request for '{}' timed out after {}s. Approve the prompt when it appears, or allow the path in Settings so it is not requested again.",
                    resource,
                    self.external_manager.prompt_timeout_seconds()
                )))
            }
        }
    }

    pub(super) async fn authorize_external_paths(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_call_id: &str,
        args: &serde_json::Value,
        workspace: &WorkspaceConfig,
    ) -> Result<(Vec<String>, Option<String>)> {
        let workspace_root =
            PathBuf::from(shellexpand::tilde(&workspace.resolved_path()).to_string());
        let Some(path) = Self::requested_absolute_path(tool_name, args, Some(&workspace_root))
        else {
            return Ok((Vec::new(), None));
        };
        if Self::path_is_inside_workspace(Path::new(&path), workspace) {
            return Ok((Vec::new(), None));
        }
        let execution_root = Self::external_execution_root(tool_name, &path);

        // The composer's approval behavior can be a hard session lockdown:
        // `deny` refuses everything outside the workspace before any grant,
        // whitelist, or rule can allow it. (Fixes a real leak: an "Always
        // allow" pressed in one chat used to unlock the path in every chat,
        // silently bypassing a later chat's auto-deny.)
        if let crate::permission::PermissionAction::Deny =
            self.config.read().await.agent.approval_policy
        {
            return Err(OSAgentError::ToolExecution(format!(
                "Access to external path '{}' is denied by this session's approval policy (auto-deny)",
                path
            )));
        }

        // Explicit Deny rules still deny...
        let evaluated = self
            .external_manager
            .evaluate(&path, &workspace.resolved_path());
        if matches!(evaluated, PermissionAction::Deny) {
            return Err(OSAgentError::ToolExecution(format!(
                "Access to external path '{}' is denied by policy",
                path
            )));
        }

        // ...session-scoped prompt approvals cover the rest of this chat...
        if self
            .external_manager
            .has_granted_permission(session_id, &path)
            .await
        {
            return Ok((vec![execution_root], Some(path)));
        }

        // ...explicit Allow (whitelist / rules) and the auto-approve policy
        // skip the prompt; anything else asks.
        let action = match evaluated {
            PermissionAction::Allow => PermissionAction::Allow,
            PermissionAction::Deny => unreachable!("handled above"),
            PermissionAction::Ask => match self.resolve_ask_by_approval_policy().await? {
                crate::permission::PermissionAction::Allow => PermissionAction::Allow,
                crate::permission::PermissionAction::Deny => PermissionAction::Deny,
                crate::permission::PermissionAction::Ask => PermissionAction::Ask,
            },
        };
        match action {
            PermissionAction::Allow => return Ok((vec![execution_root], Some(path))),
            PermissionAction::Deny => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Access to external path '{}' is denied by policy",
                    path
                )))
            }
            PermissionAction::Ask => {}
        }

        let path_type = if matches!(tool_name, "write_file" | "edit_file" | "delete_file") {
            "write"
        } else if matches!(tool_name, "bash" | "process") {
            "execute"
        } else {
            "read"
        };
        let parent_pattern = {
            let root = execution_root.trim_end_matches(['/', '\\']);
            format!("{}{}**", root, std::path::MAIN_SEPARATOR)
        };
        let (prompt, response) = self
            .external_manager
            .create_waiting_prompt(
                session_id.to_string(),
                format!("{}:{}", tool_name, tool_call_id),
                path.clone(),
                path_type.to_string(),
                vec![parent_pattern],
            )
            .await;

        let timeout =
            std::time::Duration::from_secs(self.external_manager.prompt_timeout_seconds().max(1));
        let response = tokio::select! {
            response = tokio::time::timeout(timeout, response) => response,
            _ = self.wait_for_cancellation(session_id) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
        };
        match response {
            Ok(Ok(true)) => Ok((vec![execution_root], Some(path))),
            Ok(Ok(false)) => Err(OSAgentError::ToolExecution(format!(
                "User denied access to external path '{}'",
                path
            ))),
            Ok(Err(_)) => Err(OSAgentError::ToolExecution(
                "External path permission request was cancelled".to_string(),
            )),
            Err(_) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                Err(OSAgentError::ToolExecution(format!(
                    "Permission request for '{}' timed out after {}s. Approve the prompt when it appears, or add the path to a workspace or the external permission whitelist so it is allowed automatically.",
                    path,
                    self.external_manager.prompt_timeout_seconds()
                )))
            }
        }
    }

    pub async fn check_external_directory_permission(&self, path: &str) -> PermissionAction {
        let cfg = self.config.read().await;
        let workspace = cfg.get_active_workspace();
        drop(cfg);
        self.external_manager
            .evaluate(path, &workspace.resolved_path())
    }

    pub async fn create_permission_prompt(
        &self,
        session_id: String,
        source: String,
        path: String,
        path_type: String,
        patterns: Vec<String>,
    ) -> PermissionPrompt {
        self.external_manager
            .create_prompt(session_id, source, path, path_type, patterns)
            .await
    }

    pub async fn respond_to_permission_prompt(
        &self,
        prompt_id: &str,
        allowed: bool,
        always: bool,
    ) -> Option<PermissionPrompt> {
        self.external_manager
            .respond_to_prompt(prompt_id, allowed, always)
            .await
    }

    pub async fn get_pending_permission_prompts(&self) -> Vec<PermissionPrompt> {
        self.external_manager.get_pending_prompts().await
    }

    pub async fn get_session_permission_prompts(&self, session_id: &str) -> Vec<PermissionPrompt> {
        self.external_manager.get_session_prompts(session_id).await
    }

    pub async fn has_granted_external_permission(&self, session_id: &str, path: &str) -> bool {
        self.external_manager
            .has_granted_permission(session_id, path)
            .await
    }

    pub async fn get_permission_rules(&self) -> Vec<crate::permission::PermissionRule> {
        let cfg = self.config.read().await;
        cfg.get_permission_rules()
    }

    pub async fn add_permission_rule(&self, rule: crate::permission::PermissionRule) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.add_permission_rule(rule)
    }

    pub async fn remove_permission_rule(&self, rule_id: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_permission_rule(rule_id)
    }
}
