use globset::Glob;
use globset::GlobMatcher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
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

        Some(prompt)
    }

    pub async fn get_pending_prompts(&self) -> Vec<PermissionPrompt> {
        let prompts = self.pending_prompts.read().await;
        prompts.values().cloned().collect()
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
