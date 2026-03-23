use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub hooks: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub path: PathBuf,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn description(&self) -> Option<&str>;

    async fn init(&self) -> Result<(), String>;

    async fn on_tool_before(
        &self,
        _tool_name: &str,
        _session_id: &str,
        _args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        None
    }

    async fn on_tool_after(
        &self,
        _tool_name: &str,
        _session_id: &str,
        _args: &serde_json::Value,
        _result: &Result<String, String>,
    ) -> Option<String> {
        None
    }

    async fn on_permission_ask(&self, _permission_type: &str, _resource: &str) -> Option<String> {
        None
    }

    async fn on_event(
        &self,
        _event_type: &str,
        _data: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        None
    }

    async fn on_message(&self, _session_id: &str, _message: &str) -> Option<String> {
        None
    }

    async fn on_config(&self, _config: &mut serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}

pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, LoadedPlugin>>>,
    plugin_instances: Arc<RwLock<HashMap<String, Box<dyn Plugin>>>>,
    plugin_dir: PathBuf,
    enabled: bool,
}

impl PluginManager {
    pub fn new(plugin_dir: PathBuf, enabled: bool) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_instances: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            enabled,
        }
    }

    pub async fn load_all(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        if !self.plugin_dir.exists() {
            std::fs::create_dir_all(&self.plugin_dir).map_err(|e| e.to_string())?;
            return Ok(());
        }

        let entries = std::fs::read_dir(&self.plugin_dir).map_err(|e| e.to_string())?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.yaml");
                if manifest_path.exists() {
                    if let Err(e) = self.load_plugin(&path).await {
                        tracing::warn!("Failed to load plugin at {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(())
    }

    async fn load_plugin(&self, path: &PathBuf) -> Result<(), String> {
        let manifest_path = path.join("plugin.yaml");
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let manifest: PluginManifest = serde_yaml::from_str(&content).map_err(|e| e.to_string())?;

        let plugin = LoadedPlugin {
            manifest: manifest.clone(),
            path: path.clone(),
            enabled: true,
        };

        let mut plugins = self.plugins.write().await;
        plugins.insert(manifest.name.clone(), plugin);

        Ok(())
    }

    pub async fn register_plugin(&self, plugin: Box<dyn Plugin>) {
        let mut instances = self.plugin_instances.write().await;
        instances.insert(plugin.name().to_string(), plugin);
    }

    pub async fn list_plugins(&self) -> Vec<LoadedPlugin> {
        let plugins = self.plugins.read().await;
        plugins.values().cloned().collect()
    }

    pub async fn get_plugin(&self, name: &str) -> Option<LoadedPlugin> {
        let plugins = self.plugins.read().await;
        plugins.get(name).cloned()
    }

    pub async fn enable_plugin(&self, name: &str) -> Result<(), String> {
        let mut plugins = self.plugins.write().await;
        if let Some(plugin) = plugins.get_mut(name) {
            plugin.enabled = true;
            return Ok(());
        }
        Err(format!("Plugin '{}' not found", name))
    }

    pub async fn disable_plugin(&self, name: &str) -> Result<(), String> {
        let mut plugins = self.plugins.write().await;
        if let Some(plugin) = plugins.get_mut(name) {
            plugin.enabled = false;
            return Ok(());
        }
        Err(format!("Plugin '{}' not found", name))
    }

    pub async fn trigger_tool_before(
        &self,
        tool_name: &str,
        session_id: &str,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let instances = self.plugin_instances.read().await;
        for plugin in instances.values() {
            if let Some(result) = plugin.on_tool_before(tool_name, session_id, args).await {
                return Some(result);
            }
        }
        None
    }

    pub async fn trigger_tool_after(
        &self,
        tool_name: &str,
        session_id: &str,
        args: &serde_json::Value,
        result: &Result<String, String>,
    ) -> Option<String> {
        let instances = self.plugin_instances.read().await;
        for plugin in instances.values() {
            if let Some(result) = plugin
                .on_tool_after(tool_name, session_id, args, result)
                .await
            {
                return Some(result);
            }
        }
        None
    }

    pub async fn trigger_permission_ask(
        &self,
        permission_type: &str,
        resource: &str,
    ) -> Option<String> {
        let instances = self.plugin_instances.read().await;
        for plugin in instances.values() {
            if let Some(result) = plugin.on_permission_ask(permission_type, resource).await {
                return Some(result);
            }
        }
        None
    }

    pub async fn trigger_event(
        &self,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let instances = self.plugin_instances.read().await;
        for plugin in instances.values() {
            if let Some(result) = plugin.on_event(event_type, data).await {
                return Some(result);
            }
        }
        None
    }

    pub async fn trigger_message(&self, session_id: &str, message: &str) -> Option<String> {
        let instances = self.plugin_instances.read().await;
        for plugin in instances.values() {
            if let Some(result) = plugin.on_message(session_id, message).await {
                return Some(result);
            }
        }
        None
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new(PathBuf::from("~/.osagent/plugins"), false)
    }
}
