use crate::agent::provider_presets::{get_presets, ProviderPreset};
use crate::config::ProviderConfig;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub context_window: usize,
    pub input_limit: Option<usize>,
    pub output_limit: usize,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub category: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub description: String,
    pub connected: bool,
    pub api_key_source: String,
    pub oauth_supported: bool,
    pub api_key_url: Option<String>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogState {
    pub providers: Vec<ProviderInfo>,
    pub all_models: Vec<ModelInfo>,
}

pub struct ModelCatalog {
    pub custom_models: RwLock<Vec<CustomModelEntry>>,
    pub cached_catalog: RwLock<Option<(ModelsDevCatalog, Instant)>>,
}

impl Clone for ModelCatalog {
    fn clone(&self) -> Self {
        Self {
            custom_models: RwLock::new(Vec::new()),
            cached_catalog: RwLock::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModelEntry {
    pub provider_id: String,
    pub model_id: String,
    pub name: String,
    pub context_window: usize,
    pub supports_tools: bool,
    pub supports_vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub doc: String,
    pub env: Vec<String>,
    pub models: ModelsDevModels,
}

pub type ModelsDevModels = std::collections::BTreeMap<String, ModelsDevModel>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub modalities: ModelsDevModalities,
    #[serde(default)]
    pub cost: ModelsDevCost,
    #[serde(default)]
    pub limit: ModelsDevLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsDevModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsDevCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsDevLimit {
    #[serde(default)]
    pub context: usize,
    #[serde(default)]
    pub input: Option<usize>,
    #[serde(default)]
    pub output: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLimitInfo {
    pub context: usize,
    pub input: Option<usize>,
    pub output: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelReasoningMetadata {
    pub provider_id: String,
    pub model_id: String,
    pub family: String,
    pub reasoning: bool,
    pub release_date: String,
    pub output_limit: usize,
}

pub type ModelsDevCatalog = std::collections::BTreeMap<String, ModelsDevProvider>;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_TTL_SECS: u64 = 3600;

impl ModelsDevModel {
    fn limit(&self) -> ModelLimitInfo {
        let context = if self.limit.context > 0 {
            self.limit.context
        } else {
            match self.family.as_str() {
                "claude" => 200_000,
                "gpt" | "o1" | "o3" => 128_000,
                "gemini" => 1_000_000,
                "llama" => 131_072,
                "mistral" => 32_000,
                "deepseek" => 131_072,
                "grok" => 131_072,
                _ => 128_000,
            }
        };
        ModelLimitInfo {
            context,
            input: self.limit.input,
            output: self.limit.output,
        }
    }

    fn supports_vision(&self) -> bool {
        self.modalities.input.iter().any(|m| m == "image")
    }

    fn category(&self) -> String {
        if self.reasoning {
            return "reasoning".to_string();
        }
        let ctx = self.limit().context;
        if ctx >= 500_000 || self.name.to_lowercase().contains("pro") {
            return "recommended".to_string();
        }
        if ctx <= 32_000
            || self.name.to_lowercase().contains("mini")
            || self.name.to_lowercase().contains("haiku")
        {
            return "fast".to_string();
        }
        "popular".to_string()
    }
}

impl ModelCatalog {
    pub fn new() -> Self {
        Self {
            custom_models: RwLock::new(Vec::new()),
            cached_catalog: RwLock::new(None),
        }
    }

    async fn fetch_models_dev() -> Option<ModelsDevCatalog> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("OSAgent/1.0")
            .build()
            .ok()?;

        let resp = client.get(MODELS_DEV_URL).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<ModelsDevCatalog>().await.ok()
    }

    fn from_models_dev(
        catalog: &ModelsDevCatalog,
        connected_ids: &std::collections::HashSet<String>,
        env_detected: &[ProviderPreset],
    ) -> (Vec<ProviderInfo>, Vec<ModelInfo>) {
        let mut providers = Vec::new();
        let mut all_models = Vec::new();
        let preset_map: std::collections::HashMap<String, ProviderPreset> = get_presets()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut seen_provider_ids = std::collections::HashSet::new();

        for (prov_id, prov) in catalog {
            seen_provider_ids.insert(prov_id.clone());
            let connected =
                connected_ids.contains(prov_id) || env_detected.iter().any(|e| e.id == *prov_id);

            let api_key_source = if connected {
                if connected_ids.contains(prov_id) {
                    "config".to_string()
                } else {
                    "env".to_string()
                }
            } else {
                String::new()
            };

            let preset = preset_map.get(prov_id);
            let base_url = if !prov.api.is_empty() {
                prov.api.clone()
            } else if let Some(p) = preset {
                p.base_url.clone()
            } else {
                format!("https://api.{prov_id}.com/v1")
            };

            let name = if !prov.name.is_empty() {
                prov.name.clone()
            } else if let Some(p) = preset {
                p.name.clone()
            } else {
                prov_id.clone()
            };

            let mut models: Vec<ModelInfo> = prov
                .models
                .iter()
                .filter(|(_, m)| {
                    !m.family.contains(&"embedding".to_string())
                        && !m.family.contains(&"whisper".to_string())
                        && !m.name.to_lowercase().contains("embed")
                        && !m.name.to_lowercase().contains("whisper")
                })
                .map(|(model_id, m)| {
                    let limit = m.limit();
                    ModelInfo {
                        id: model_id.clone(),
                        name: m.name.clone(),
                        provider_id: prov_id.clone(),
                        provider_name: name.clone(),
                        context_window: limit.context,
                        input_limit: limit.input,
                        output_limit: limit.output,
                        supports_tools: m.tool_call,
                        supports_vision: m.supports_vision(),
                        category: m.category(),
                        available: connected,
                    }
                })
                .collect();

            if let Some(preset) = preset {
                let existing_ids: std::collections::HashSet<String> =
                    models.iter().map(|m| m.id.clone()).collect();
                for model in &preset.models {
                    if existing_ids.contains(&model.id) {
                        continue;
                    }
                    models.push(ModelInfo {
                        id: model.id.clone(),
                        name: model.name.clone(),
                        provider_id: prov_id.clone(),
                        provider_name: name.clone(),
                        context_window: model.context_window,
                        input_limit: None,
                        output_limit: 0,
                        supports_tools: model.supports_tools,
                        supports_vision: model.supports_vision,
                        category: model.category.clone(),
                        available: connected,
                    });
                }
            }

            all_models.extend(models.clone());

            providers.push(ProviderInfo {
                id: prov_id.clone(),
                name,
                base_url,
                description: if !prov.doc.is_empty() {
                    prov.doc.clone()
                } else {
                    preset.map(|p| p.description.clone()).unwrap_or_default()
                },
                connected,
                api_key_source,
                oauth_supported: crate::oauth::provider::is_oauth_provider(prov_id),
                api_key_url: preset.and_then(|p| p.api_key_url.clone()),
                models,
            });
        }

        for preset in preset_map.values() {
            if seen_provider_ids.contains(&preset.id) {
                continue;
            }

            let connected = connected_ids.contains(&preset.id)
                || env_detected.iter().any(|e| e.id == preset.id);
            let api_key_source = if connected {
                if connected_ids.contains(&preset.id) {
                    "config".to_string()
                } else {
                    "env".to_string()
                }
            } else {
                String::new()
            };

            let models: Vec<ModelInfo> = preset
                .models
                .iter()
                .map(|model| ModelInfo {
                    id: model.id.clone(),
                    name: model.name.clone(),
                    provider_id: preset.id.clone(),
                    provider_name: preset.name.clone(),
                    context_window: model.context_window,
                    input_limit: None,
                    output_limit: 0,
                    supports_tools: model.supports_tools,
                    supports_vision: model.supports_vision,
                    category: model.category.clone(),
                    available: connected,
                })
                .collect();

            all_models.extend(models.clone());
            providers.push(ProviderInfo {
                id: preset.id.clone(),
                name: preset.name.clone(),
                base_url: preset.base_url.clone(),
                description: preset.description.clone(),
                connected,
                api_key_source,
                oauth_supported: crate::oauth::provider::is_oauth_provider(&preset.id),
                api_key_url: preset.api_key_url.clone(),
                models,
            });
        }

        (providers, all_models)
    }

    fn from_presets(
        connected_ids: &std::collections::HashSet<String>,
        env_detected: &[ProviderPreset],
    ) -> (Vec<ProviderInfo>, Vec<ModelInfo>) {
        let presets = get_presets();
        let mut providers = Vec::new();
        let mut all_models = Vec::new();

        for preset in &presets {
            let connected = connected_ids.contains(&preset.id)
                || env_detected.iter().any(|e| e.id == preset.id);

            let api_key_source = if connected {
                if connected_ids.contains(&preset.id) {
                    "config".to_string()
                } else {
                    "env".to_string()
                }
            } else {
                String::new()
            };

            let models: Vec<ModelInfo> = preset
                .models
                .iter()
                .map(|m| ModelInfo {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    provider_id: preset.id.clone(),
                    provider_name: preset.name.clone(),
                    context_window: m.context_window,
                    input_limit: None,
                    output_limit: 0,
                    supports_tools: m.supports_tools,
                    supports_vision: m.supports_vision,
                    category: m.category.clone(),
                    available: connected,
                })
                .collect();

            all_models.extend(models.clone());

            providers.push(ProviderInfo {
                id: preset.id.clone(),
                name: preset.name.clone(),
                base_url: preset.base_url.clone(),
                description: preset.description.clone(),
                connected,
                api_key_source,
                oauth_supported: crate::oauth::provider::is_oauth_provider(&preset.id),
                api_key_url: preset.api_key_url.clone(),
                models,
            });
        }

        (providers, all_models)
    }

    pub async fn refresh_catalog(&self) {
        if let Some(catalog) = Self::fetch_models_dev().await {
            let mut cache = self.cached_catalog.write().unwrap();
            *cache = Some((catalog, Instant::now()));
        }
    }

    pub fn get_state(&self, configured_providers: &[ProviderConfig]) -> CatalogState {
        let connected_ids: std::collections::HashSet<String> = configured_providers
            .iter()
            .map(|p| p.provider_type.clone())
            .collect();

        let env_detected = super::provider_presets::detect_env_providers();

        let (providers, mut all_models) = {
            let cache = self.cached_catalog.read().unwrap();
            if let Some((catalog, timestamp)) = cache.as_ref() {
                if timestamp.elapsed().as_secs() < CACHE_TTL_SECS {
                    Self::from_models_dev(catalog, &connected_ids, &env_detected)
                } else {
                    Self::from_presets(&connected_ids, &env_detected)
                }
            } else {
                Self::from_presets(&connected_ids, &env_detected)
            }
        };

        for custom in self.custom_models.read().unwrap().iter() {
            let provider_name = providers
                .iter()
                .find(|p| p.id == custom.provider_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| custom.provider_id.clone());

            let connected = connected_ids.contains(&custom.provider_id)
                || env_detected.iter().any(|e| e.id == custom.provider_id);

            all_models.push(ModelInfo {
                id: custom.model_id.clone(),
                name: custom.name.clone(),
                provider_id: custom.provider_id.clone(),
                provider_name,
                context_window: custom.context_window,
                input_limit: None,
                output_limit: 0,
                supports_tools: custom.supports_tools,
                supports_vision: custom.supports_vision,
                category: "custom".to_string(),
                available: connected,
            });
        }

        CatalogState {
            providers,
            all_models,
        }
    }

    pub fn get_models_for_provider(&self, provider_id: &str) -> Vec<ModelInfo> {
        let presets = get_presets();
        if let Some(preset) = presets.iter().find(|p| p.id == provider_id) {
            return preset
                .models
                .iter()
                .map(|m| ModelInfo {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    provider_id: preset.id.clone(),
                    provider_name: preset.name.clone(),
                    context_window: m.context_window,
                    input_limit: None,
                    output_limit: 0,
                    supports_tools: m.supports_tools,
                    supports_vision: m.supports_vision,
                    category: m.category.clone(),
                    available: true,
                })
                .collect();
        }
        Vec::new()
    }

    pub fn lookup_context_window(&self, provider_id: &str, model_id: &str) -> Option<usize> {
        let presets = get_presets();
        if let Some(preset) = presets.iter().find(|p| p.id == provider_id) {
            if let Some(model) = preset.models.iter().find(|m| m.id == model_id) {
                return Some(model.context_window);
            }
        }
        self.custom_models
            .read()
            .unwrap()
            .iter()
            .find(|m| m.provider_id == provider_id && m.model_id == model_id)
            .map(|m| m.context_window)
    }

    pub fn lookup_model_limit(&self, provider_id: &str, model_id: &str) -> Option<ModelLimitInfo> {
        let configured_providers: Vec<ProviderConfig> = Vec::new();
        let state = self.get_state(&configured_providers);
        state
            .all_models
            .iter()
            .find(|m| m.provider_id == provider_id && m.id == model_id)
            .map(|m| ModelLimitInfo {
                context: m.context_window,
                input: m.input_limit,
                output: m.output_limit,
            })
    }

    pub fn lookup_reasoning_metadata(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<ModelReasoningMetadata> {
        {
            let cache = self.cached_catalog.read().unwrap();
            if let Some((catalog, timestamp)) = cache.as_ref() {
                if timestamp.elapsed().as_secs() < CACHE_TTL_SECS {
                    if let Some(provider) = catalog.get(provider_id) {
                        if let Some(model) = provider.models.get(model_id) {
                            return Some(ModelReasoningMetadata {
                                provider_id: provider_id.to_string(),
                                model_id: model_id.to_string(),
                                family: model.family.clone(),
                                reasoning: model.reasoning,
                                release_date: model.release_date.clone(),
                                output_limit: model.limit.output,
                            });
                        }
                    }
                }
            }
        }

        let presets = get_presets();
        presets
            .iter()
            .find(|preset| preset.id == provider_id)
            .and_then(|preset| preset.models.iter().find(|model| model.id == model_id))
            .map(|model| ModelReasoningMetadata {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                family: infer_family(model_id),
                reasoning: model.category == "reasoning"
                    || model.id.to_ascii_lowercase().contains("codex"),
                release_date: String::new(),
                output_limit: 0,
            })
            .or_else(|| {
                self.custom_models
                    .read()
                    .unwrap()
                    .iter()
                    .find(|model| model.provider_id == provider_id && model.model_id == model_id)
                    .map(|model| ModelReasoningMetadata {
                        provider_id: provider_id.to_string(),
                        model_id: model_id.to_string(),
                        family: infer_family(model_id),
                        reasoning: model.model_id.to_ascii_lowercase().contains("reason")
                            || model.model_id.to_ascii_lowercase().contains("codex"),
                        release_date: String::new(),
                        output_limit: 0,
                    })
            })
    }

    pub fn search_models(&self, query: &str) -> Vec<ModelInfo> {
        let query = query.to_lowercase();
        let configured_providers: Vec<ProviderConfig> = Vec::new();
        let state = self.get_state(&configured_providers);
        state
            .all_models
            .into_iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&query)
                    || m.name.to_lowercase().contains(&query)
                    || m.provider_id.to_lowercase().contains(&query)
                    || m.provider_name.to_lowercase().contains(&query)
            })
            .collect()
    }

    pub fn add_custom_model(&self, entry: CustomModelEntry) {
        let mut models = self.custom_models.write().unwrap();
        models.retain(|m| !(m.provider_id == entry.provider_id && m.model_id == entry.model_id));
        models.push(entry);
    }

    pub fn remove_custom_model(&self, provider_id: &str, model_id: &str) {
        let mut models = self.custom_models.write().unwrap();
        models.retain(|m| !(m.provider_id == provider_id && m.model_id == model_id));
    }
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self::new()
    }
}

fn infer_family(model_id: &str) -> String {
    let id = model_id.to_ascii_lowercase();
    if id.contains("claude") {
        return "claude".to_string();
    }
    if id.contains("gemini") {
        return "gemini".to_string();
    }
    if id.contains("grok") {
        return "grok".to_string();
    }
    if id.contains("mistral") {
        return "mistral".to_string();
    }
    if id.contains("llama") {
        return "llama".to_string();
    }
    if id.contains("deepseek") {
        return "deepseek".to_string();
    }
    if id.contains("qwen") {
        return "qwen".to_string();
    }
    if id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
        return "o".to_string();
    }
    if id.contains("gpt") || id.contains("codex") {
        return "gpt".to_string();
    }
    String::new()
}
