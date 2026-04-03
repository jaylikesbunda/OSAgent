use globset::Glob;
use globset::GlobMatcher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
pub struct PermissionRule {
    pub id: String,
    pub permission: String,
    pub pattern: String,
    pub action: PermissionAction,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl PermissionRule {
    pub fn new(permission: String, pattern: String, action: PermissionAction) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            permission,
            pattern,
            action,
            created_at: chrono::Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermission {
    pub name: String,
    pub action: PermissionAction,
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathPermission {
    pub pattern: String,
    pub action: PermissionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionConfig {
    pub default_action: PermissionAction,
    pub tools: HashMap<String, PermissionAction>,
    pub paths: Vec<PathPermission>,
    pub external: ExternalPermissionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalPermissionConfig {
    pub default_action: PermissionAction,
    pub whitelist: Vec<String>,
}

pub struct PermissionEvaluator {
    config: PermissionConfig,
    path_matchers: Vec<(GlobMatcher, PermissionAction)>,
    external_matchers: Vec<(GlobMatcher, PermissionAction)>,
}

impl PermissionEvaluator {
    pub fn new(config: PermissionConfig) -> Self {
        let path_matchers: Vec<(GlobMatcher, PermissionAction)> = config
            .paths
            .iter()
            .filter_map(|p| {
                Glob::new(&p.pattern)
                    .ok()
                    .map(|g| (g.compile_matcher(), p.action.clone()))
            })
            .collect();

        let external_matchers: Vec<(GlobMatcher, PermissionAction)> = config
            .external
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
            external_matchers,
        }
    }

    pub fn default_config() -> Self {
        let mut tools = HashMap::new();
        tools.insert("bash".to_string(), PermissionAction::Allow);
        tools.insert("read_file".to_string(), PermissionAction::Allow);
        tools.insert("write_file".to_string(), PermissionAction::Ask);
        tools.insert("delete_file".to_string(), PermissionAction::Deny);
        tools.insert("lsp".to_string(), PermissionAction::Allow);
        tools.insert("subagent".to_string(), PermissionAction::Ask);

        let config = PermissionConfig {
            default_action: PermissionAction::Ask,
            tools,
            paths: vec![
                PathPermission {
                    pattern: "*.env".to_string(),
                    action: PermissionAction::Deny,
                },
                PathPermission {
                    pattern: "*.secret*".to_string(),
                    action: PermissionAction::Deny,
                },
            ],
            external: ExternalPermissionConfig {
                default_action: PermissionAction::Ask,
                whitelist: vec![],
            },
        };

        Self::new(config)
    }

    pub fn evaluate_tool(&self, tool_name: &str) -> PermissionAction {
        self.config
            .tools
            .get(tool_name)
            .cloned()
            .unwrap_or(self.config.default_action.clone())
    }

    pub fn evaluate_path(&self, path: &str) -> PermissionAction {
        for (matcher, action) in &self.path_matchers {
            if matcher.is_match(path) {
                return action.clone();
            }
        }
        PermissionAction::Ask
    }

    pub fn evaluate_external(&self, path: &str) -> PermissionAction {
        for (matcher, action) in &self.external_matchers {
            if matcher.is_match(path) {
                return action.clone();
            }
        }
        self.config.external.default_action.clone()
    }

    pub fn is_allowed(&self, tool_name: &str, path: Option<&str>) -> PermissionAction {
        let tool_action = self.evaluate_tool(tool_name);

        if tool_action == PermissionAction::Allow {
            if let Some(p) = path {
                let path_action = self.evaluate_path(p);
                if path_action == PermissionAction::Deny {
                    return PermissionAction::Deny;
                }
            }
            return PermissionAction::Allow;
        }

        if tool_action == PermissionAction::Deny {
            return PermissionAction::Deny;
        }

        if let Some(p) = path {
            return self.evaluate_path(p);
        }

        PermissionAction::Ask
    }
}
