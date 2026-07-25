use globset::Glob;
use globset::GlobMatcher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PermissionAction {
    Allow,
    Deny,
    #[default]
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalDirectoryRule {
    pub pattern: String,
    pub action: PermissionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalPermissionConfig {
    pub default_action: PermissionAction,
    pub whitelist: Vec<String>,
    pub rules: Vec<ExternalDirectoryRule>,
    pub prompt_timeout_seconds: u64,
}

impl Default for ExternalPermissionConfig {
    fn default() -> Self {
        Self {
            default_action: PermissionAction::Ask,
            whitelist: vec![],
            rules: vec![],
            prompt_timeout_seconds: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPrompt {
    pub id: String,
    pub session_id: String,
    pub source: String,
    pub path: String,
    pub path_type: String,
    pub patterns: Vec<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPromptResponse {
    pub prompt_id: String,
    pub allowed: bool,
    pub always: bool,
}

pub struct ExternalDirectoryManager {
    config: ExternalPermissionConfig,
    path_matchers: Vec<(GlobMatcher, PermissionAction)>,
    whitelist_matchers: Vec<(GlobMatcher, PermissionAction)>,
    pending_prompts: Arc<RwLock<HashMap<String, PermissionPrompt>>>,
    granted_permissions: Arc<RwLock<HashMap<String, chrono::DateTime<chrono::Utc>>>>,
    pending_responses: Arc<RwLock<HashMap<String, oneshot::Sender<bool>>>>,
}

impl ExternalDirectoryManager {
    pub fn new(config: ExternalPermissionConfig) -> Self {
        let path_matchers: Vec<(GlobMatcher, PermissionAction)> = config
            .rules
            .iter()
            .filter_map(|p| {
                Glob::new(&p.pattern)
                    .ok()
                    .map(|g| (g.compile_matcher(), p.action.clone()))
            })
            .collect();

        let whitelist_matchers: Vec<(GlobMatcher, PermissionAction)> = config
            .whitelist
            .iter()
            .filter_map(|p| {
                Glob::new(p)
                    .ok()
                    .map(|g| (g.compile_matcher(), PermissionAction::Allow))
            })
            .collect();

        Self {
            config,
            path_matchers,
            whitelist_matchers,
            pending_prompts: Arc::new(RwLock::new(HashMap::new())),
            granted_permissions: Arc::new(RwLock::new(HashMap::new())),
            pending_responses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn evaluate(&self, path: &str, workspace_path: &str) -> PermissionAction {
        let normalized_path = Path::new(path);
        let normalized_workspace = Path::new(workspace_path);

        if let Ok(path_canonical) = normalized_path.canonicalize() {
            if let Ok(workspace_canonical) = normalized_workspace.canonicalize() {
                if path_canonical.starts_with(&workspace_canonical) {
                    return PermissionAction::Allow;
                }
            }
        }

        for (matcher, action) in &self.whitelist_matchers {
            if matcher.is_match(path) {
                return action.clone();
            }
        }

        for (matcher, action) in &self.path_matchers {
            if matcher.is_match(path) {
                return action.clone();
            }
        }

        self.config.default_action.clone()
    }

    pub fn is_inside_workspace(&self, path: &str, workspace_path: &str) -> bool {
        let normalized_path = Path::new(path);
        let normalized_workspace = Path::new(workspace_path);

        if let Ok(path_canonical) = normalized_path.canonicalize() {
            if let Ok(workspace_canonical) = normalized_workspace.canonicalize() {
                return path_canonical.starts_with(&workspace_canonical);
            }
        }

        let ws = workspace_path.replace('\\', "/");
        path.replace('\\', "/").starts_with(&ws)
    }

    pub async fn create_prompt(
        &self,
        session_id: String,
        source: String,
        path: String,
        path_type: String,
        patterns: Vec<String>,
    ) -> PermissionPrompt {
        let prompt = PermissionPrompt {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            source,
            path: path.clone(),
            path_type,
            patterns,
            timestamp: chrono::Utc::now(),
        };

        let mut prompts = self.pending_prompts.write().await;
        prompts.insert(prompt.id.clone(), prompt.clone());

        prompt
    }

    pub async fn create_waiting_prompt(
        &self,
        session_id: String,
        source: String,
        path: String,
        path_type: String,
        patterns: Vec<String>,
    ) -> (PermissionPrompt, oneshot::Receiver<bool>) {
        let prompt = self
            .create_prompt(session_id, source, path, path_type, patterns)
            .await;
        let (sender, receiver) = oneshot::channel();
        self.pending_responses
            .write()
            .await
            .insert(prompt.id.clone(), sender);
        (prompt, receiver)
    }

    pub fn prompt_timeout_seconds(&self) -> u64 {
        self.config.prompt_timeout_seconds
    }

    pub async fn expire_prompt(&self, prompt_id: &str) {
        self.pending_prompts.write().await.remove(prompt_id);
        self.pending_responses.write().await.remove(prompt_id);
    }

    pub async fn respond_to_prompt(
        &self,
        prompt_id: &str,
        allowed: bool,
        always: bool,
    ) -> Option<PermissionPrompt> {
        let mut prompts = self.pending_prompts.write().await;
        let prompt = prompts.remove(prompt_id)?;

        if allowed && always {
            let mut permissions = self.granted_permissions.write().await;
            permissions.insert(prompt.path.clone(), chrono::Utc::now());
        }

        if let Some(sender) = self.pending_responses.write().await.remove(prompt_id) {
            let _ = sender.send(allowed);
        }

        Some(prompt)
    }

    pub async fn get_pending_prompts(&self) -> Vec<PermissionPrompt> {
        let prompts = self.pending_prompts.read().await;
        let mut prompts = prompts.values().cloned().collect::<Vec<_>>();
        prompts.sort_by_key(|prompt| prompt.timestamp);
        prompts
    }

    pub async fn get_session_prompts(&self, session_id: &str) -> Vec<PermissionPrompt> {
        let prompts = self.pending_prompts.read().await;
        prompts
            .values()
            .filter(|p| p.session_id == session_id)
            .cloned()
            .collect()
    }

    pub async fn has_granted_permission(&self, path: &str) -> bool {
        let permissions = self.granted_permissions.read().await;
        permissions.contains_key(path)
    }

    pub async fn clear_expired_permissions(&self, ttl_hours: i64) {
        let mut permissions = self.granted_permissions.write().await;
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(ttl_hours);
        permissions.retain(|_, v| *v > cutoff);
    }

    pub fn evaluate_glob_patterns(&self, path: &str, patterns: &[String]) -> bool {
        for pattern in patterns {
            if let Ok(glob) = Glob::new(pattern) {
                let matcher = glob.compile_matcher();
                if matcher.is_match(path) {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for ExternalDirectoryManager {
    fn default() -> Self {
        Self::new(ExternalPermissionConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waiting_prompt_resumes_with_user_response() {
        let manager = ExternalDirectoryManager::default();
        let (prompt, response) = manager
            .create_waiting_prompt(
                "session".to_string(),
                "read_file:call".to_string(),
                "/outside/file.txt".to_string(),
                "read".to_string(),
                vec!["/outside/**".to_string()],
            )
            .await;

        assert_eq!(manager.get_pending_prompts().await.len(), 1);
        manager
            .respond_to_prompt(&prompt.id, true, false)
            .await
            .expect("pending prompt");

        assert!(response.await.expect("permission response"));
        assert!(manager.get_pending_prompts().await.is_empty());
    }

    #[tokio::test]
    async fn always_response_remembers_exact_path() {
        let manager = ExternalDirectoryManager::default();
        let (prompt, _response) = manager
            .create_waiting_prompt(
                "session".to_string(),
                "write_file:call".to_string(),
                "/outside/file.txt".to_string(),
                "write".to_string(),
                Vec::new(),
            )
            .await;

        manager
            .respond_to_prompt(&prompt.id, true, true)
            .await
            .expect("pending prompt");

        assert!(manager.has_granted_permission("/outside/file.txt").await);
    }
}
