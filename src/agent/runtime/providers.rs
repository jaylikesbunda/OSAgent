//! Provider/model registry: current model, add/remove/switch, catalog
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn get_current_model(&self) -> String {
        self.active_provider().await.current_model().await
    }

    pub async fn set_current_model(&self, model: String) {
        self.active_provider().await.set_model(model).await;
    }

    pub async fn switch_provider_model(&self, provider_id: String, model: String) -> Result<()> {
        let providers = self.providers.read().await;
        let new_provider = providers
            .iter()
            .find(|(id, _)| id == &provider_id)
            .map(|(_, p)| p.clone())
            .ok_or_else(|| OSAgentError::Config(format!("Provider '{}' not found", provider_id)))?;
        drop(providers);

        new_provider.set_model(model.clone()).await;

        let mut provider_lock = self.providers.write().await;
        *provider_lock = provider_lock
            .iter()
            .map(|(id, p)| {
                if id == &provider_id {
                    (id.clone(), new_provider.clone())
                } else {
                    (id.clone(), p.clone())
                }
            })
            .collect();
        drop(provider_lock);

        let mut cfg = self.config.write().await;
        cfg.set_active_provider_model(&provider_id, &model);
        drop(cfg);

        let mut active_provider = self.provider.write().await;
        *active_provider = new_provider;
        drop(active_provider);

        info!("Switched to provider={}, model={}", provider_id, model);
        Ok(())
    }

    pub async fn get_catalog_state(&self) -> crate::agent::model_catalog::CatalogState {
        let cfg = self.config.read().await;
        let catalog = self.catalog.clone();
        tokio::spawn(async move {
            catalog.refresh_catalog().await;
        });
        self.catalog.get_state(&cfg.providers)
    }

    pub async fn get_provider_models(
        &self,
        provider_id: String,
    ) -> Vec<crate::agent::model_catalog::ModelInfo> {
        self.catalog.get_models_for_provider(&provider_id)
    }

    pub async fn search_catalog_models(
        &self,
        query: String,
    ) -> Vec<crate::agent::model_catalog::ModelInfo> {
        self.catalog.search_models(&query)
    }

    pub async fn add_provider(&self, provider_config: crate::config::ProviderConfig) -> Result<()> {
        let catalog = self.catalog.clone();
        // Runtime map is keyed by entry id, so two entries can share one
        // provider_type (two keys, a custom endpoint next to stock).
        let entry_id = provider_config.effective_id().to_string();
        let oauth_dir = PathBuf::from(
            shellexpand::tilde(&self.config.read().await.storage.database).to_string(),
        )
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
        let provider = Arc::new(
            OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                provider_config.clone(),
                Some(catalog),
                Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                self.agent_settings.clone(),
            )?,
        );
        provider.set_retry_listener(Some(Self::provider_retry_listener(
            Arc::new(self.event_bus.clone()),
            self.storage.clone(),
        )));

        let default_provider = self.config.read().await.default_provider.clone();
        let should_activate = default_provider.is_empty() || default_provider == entry_id;

        let mut providers = self.providers.write().await;
        providers.retain(|(id, _)| id != &entry_id);
        providers.push((entry_id.clone(), provider));
        drop(providers);

        let mut cfg = self.config.write().await;
        cfg.providers.retain(|p| p.effective_id() != entry_id);
        cfg.providers.push(provider_config.clone());
        if cfg.default_provider.is_empty() {
            cfg.default_provider = entry_id.clone();
        }
        if cfg.default_provider == entry_id {
            cfg.default_model = provider_config.model.clone();
        }
        drop(cfg);

        if should_activate {
            let active = self
                .providers
                .read()
                .await
                .iter()
                .find(|(id, _)| id == &entry_id)
                .map(|(_, p)| p.clone())
                .ok_or_else(|| {
                    OSAgentError::Config(format!("Provider '{}' not found", entry_id))
                })?;
            let mut active_provider = self.provider.write().await;
            *active_provider = active;
        }

        Ok(())
    }

    pub async fn remove_provider(&self, provider_id: String) -> Result<()> {
        let mut providers = self.providers.write().await;
        let was_active = providers.iter().any(|(id, _)| id == &provider_id);
        providers.retain(|(id, _)| id != &provider_id);
        if providers.is_empty() {
            drop(providers);
            return Err(OSAgentError::Config(
                "Cannot remove the last provider".to_string(),
            ));
        }
        let first_provider_id = providers.first().map(|(id, _)| id.clone());
        drop(providers);

        let mut cfg = self.config.write().await;
        cfg.providers.retain(|p| p.effective_id() != provider_id);
        if cfg.default_provider == provider_id {
            if let Some(ref first_id) = first_provider_id {
                cfg.default_provider = first_id.clone();
            }
            if let Some(first) = cfg.providers.first() {
                cfg.default_model = first.model.clone();
            }
        }
        drop(cfg);

        // Drop stored keys for the removed entry so secrets do not linger.
        let auth_dir = self.config.read().await.config_dir();
        let mut stored = crate::auth_store::load_auth_keys(&auth_dir);
        if stored.remove(&provider_id).is_some() {
            let _ = crate::auth_store::save_auth_keys(&auth_dir, &stored);
        }

        if was_active {
            let provider_guard = self.providers.read().await;
            if let Some((_, first)) = provider_guard.first() {
                let mut active_provider = self.provider.write().await;
                *active_provider = first.clone();
            }
        }

        Ok(())
    }
}
