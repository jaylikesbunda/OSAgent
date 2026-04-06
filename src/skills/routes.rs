use crate::error::OSAgentError;
use crate::skills::config::ConfigFieldType;
use crate::skills::service::SkillService;
use axum::{
    extract::{Extension, Json, Path},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillInfoResponse>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct SkillInfoResponse {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub emoji: Option<String>,
    pub icon_url: Option<String>,
    pub has_icon: bool,
    pub enabled: bool,
    pub has_config: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigFieldResponse {
    pub name: String,
    pub field_type: String,
    pub description: String,
    pub required: bool,
    pub default: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillDetailResponse {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub emoji: Option<String>,
    pub icon_url: Option<String>,
    pub has_icon: bool,
    pub enabled: bool,
    pub content: String,
    pub config: HashMap<String, MaskedValueResponse>,
    pub config_schema: Vec<ConfigFieldResponse>,
    pub has_authorize: bool,
}

#[derive(Debug, Serialize)]
pub struct MaskedValueResponse {
    pub masked: String,
    pub is_api_key: bool,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub settings: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct SetEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct InstallResponse {
    pub success: bool,
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

impl IntoResponse for OSAgentError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            OSAgentError::Unknown(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = Json(ErrorResponse { error: message });
        (status, body).into_response()
    }
}

pub fn create_skills_router(skill_service: Arc<SkillService>) -> Router {
    Router::new()
        .route("/api/skills", get(list_skills))
        .route("/api/skills/:name", get(get_skill))
        .route(
            "/api/skills/:name/config",
            get(get_skill_config).put(update_skill_config),
        )
        .route(
            "/api/skills/:name/config/:key",
            delete(delete_skill_config_key),
        )
        .route("/api/skills/:name/enabled", put(set_skill_enabled))
        .route("/api/skills/:name/test", post(test_skill))
        .route("/api/skills/:name/authorize", post(authorize_skill))
        .route("/api/skills/:name/export", get(export_skill))
        .route("/api/skills/install", post(install_skill))
        .route("/api/skills/uninstall", post(uninstall_skill))
        .route("/api/skills/reload", post(reload_skills))
        .layer(Extension(skill_service))
}

async fn list_skills(
    Extension(service): Extension<Arc<SkillService>>,
) -> Result<Json<SkillListResponse>, OSAgentError> {
    let skills = service.list_skills()?;
    let response = SkillListResponse {
        total: skills.len(),
        skills: skills
            .into_iter()
            .map(|s| SkillInfoResponse {
                name: s.name,
                description: s.description,
                version: s.version,
                author: s.author,
                emoji: s.emoji,
                icon_url: s.icon_url,
                has_icon: s.has_icon,
                enabled: s.enabled,
                has_config: s.has_config,
            })
            .collect(),
    };
    Ok(Json(response))
}

async fn get_skill(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
) -> Result<Json<SkillDetailResponse>, OSAgentError> {
    let details = service.get_skill_details(&name)?;
    let config: HashMap<String, MaskedValueResponse> = details
        .config
        .into_iter()
        .map(|(k, v)| {
            let is_api_key = v.is_api_key();
            (
                k,
                MaskedValueResponse {
                    masked: v.masked(),
                    is_api_key,
                    value: v.as_string(),
                },
            )
        })
        .collect();

    let config_schema: Vec<ConfigFieldResponse> = details
        .config_schema
        .into_iter()
        .map(|f| ConfigFieldResponse {
            name: f.name,
            field_type: match f.field_type {
                ConfigFieldType::ApiKey => "api_key".to_string(),
                ConfigFieldType::Password => "password".to_string(),
                ConfigFieldType::Number => "number".to_string(),
                ConfigFieldType::Boolean => "boolean".to_string(),
                ConfigFieldType::String => "string".to_string(),
            },
            description: f.description,
            required: f.required,
            default: f.default,
        })
        .collect();

    Ok(Json(SkillDetailResponse {
        name: details.info.name,
        description: details.info.description,
        version: details.info.version,
        author: details.info.author,
        emoji: details.info.emoji,
        icon_url: details.info.icon_url,
        has_icon: details.info.has_icon,
        enabled: details.info.enabled,
        content: details.content,
        config,
        config_schema,
        has_authorize: details.has_authorize,
    }))
}

async fn get_skill_config(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
) -> Result<Json<HashMap<String, MaskedValueResponse>>, OSAgentError> {
    let config = service.get_config(&name)?;
    let response: HashMap<String, MaskedValueResponse> = config
        .into_iter()
        .map(|(k, v)| {
            let is_api_key = v.is_api_key();
            (
                k,
                MaskedValueResponse {
                    masked: v.masked(),
                    is_api_key,
                    value: v.as_string(),
                },
            )
        })
        .collect();
    Ok(Json(response))
}

async fn update_skill_config(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
    Json(payload): Json<UpdateConfigRequest>,
) -> Result<StatusCode, OSAgentError> {
    service.save_config(&name, payload.settings)?;
    Ok(StatusCode::OK)
}

async fn delete_skill_config_key(
    Extension(service): Extension<Arc<SkillService>>,
    Path((name, key)): Path<(String, String)>,
) -> Result<StatusCode, OSAgentError> {
    service.delete_config_value(&name, &key)?;
    Ok(StatusCode::OK)
}

async fn set_skill_enabled(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
    Json(payload): Json<SetEnabledRequest>,
) -> Result<StatusCode, OSAgentError> {
    service.set_skill_enabled(&name, payload.enabled)?;
    Ok(StatusCode::OK)
}

async fn test_skill(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
) -> Result<Json<TestResult>, OSAgentError> {
    if !service.skill_exists(&name) {
        return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
    }

    let env = service.get_skill_env(&name)?;

    if env.is_empty() {
        return Ok(Json(TestResult {
            success: true,
            message: "Skill is enabled but has no configuration".to_string(),
        }));
    }

    Ok(Json(TestResult {
        success: true,
        message: format!(
            "Skill '{}' is configured with {} environment variables",
            name,
            env.len()
        ),
    }))
}

#[derive(Debug, Serialize)]
pub struct AuthorizeResult {
    pub message: String,
}

async fn authorize_skill(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
) -> Result<Json<AuthorizeResult>, OSAgentError> {
    if !service.skill_exists(&name) {
        return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
    }

    let output = service.authorize_skill(&name).await?;
    Ok(Json(AuthorizeResult { message: output }))
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub success: bool,
    pub message: String,
}

async fn export_skill(
    Extension(service): Extension<Arc<SkillService>>,
    Path(name): Path<String>,
) -> Result<(HeaderMap, Vec<u8>), OSAgentError> {
    let data = service.export_skill(&name)?;
    let filename = format!("{}.oskill", name);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );

    Ok((headers, data))
}

async fn install_skill(
    Extension(service): Extension<Arc<SkillService>>,
    body: bytes::Bytes,
) -> Result<Json<InstallResponse>, OSAgentError> {
    let data = body.to_vec();

    if data.is_empty() {
        return Err(OSAgentError::Unknown("No bundle data provided".to_string()));
    }

    if data.len() > 50 * 1024 * 1024 {
        return Err(OSAgentError::Unknown(
            "Bundle too large (max 50MB)".to_string(),
        ));
    }

    let result = service.install_skill(&data)?;

    Ok(Json(InstallResponse {
        success: true,
        name: result.name,
        version: result.version,
        description: result.description,
    }))
}

async fn uninstall_skill(
    Extension(service): Extension<Arc<SkillService>>,
    Json(payload): Json<UninstallRequest>,
) -> Result<StatusCode, OSAgentError> {
    service.uninstall_skill(&payload.name)?;
    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct UninstallRequest {
    pub name: String,
}

async fn reload_skills(
    Extension(service): Extension<Arc<SkillService>>,
) -> Result<Json<SkillListResponse>, OSAgentError> {
    let skills = service.reload_all()?;
    let response = SkillListResponse {
        total: skills.len(),
        skills: skills
            .into_iter()
            .map(|s| SkillInfoResponse {
                name: s.name,
                description: s.description,
                version: s.version,
                author: s.author,
                emoji: s.emoji,
                icon_url: s.icon_url,
                has_icon: s.has_icon,
                enabled: s.enabled,
                has_config: s.has_config,
            })
            .collect(),
    };
    Ok(Json(response))
}
