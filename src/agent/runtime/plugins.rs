//! Plugin management facades
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn get_plugins(&self) -> Vec<crate::plugin::LoadedPlugin> {
        self.plugin_manager.list_plugins().await
    }

    pub async fn list_plugins(&self) -> Vec<crate::plugin::LoadedPlugin> {
        self.plugin_manager.list_plugins().await
    }

    pub async fn enable_plugin(&self, name: &str) -> std::result::Result<(), String> {
        self.plugin_manager.enable_plugin(name).await
    }

    pub async fn disable_plugin(&self, name: &str) -> std::result::Result<(), String> {
        self.plugin_manager.disable_plugin(name).await
    }

    pub async fn reload_plugins(&self) -> std::result::Result<(), String> {
        self.plugin_manager.load_all().await
    }
}
