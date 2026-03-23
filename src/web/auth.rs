use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
}

#[allow(dead_code)]
pub fn hash_password(password: &str) -> crate::error::Result<String> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> crate::error::Result<bool> {
    bcrypt::verify(password, hash).map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

pub fn generate_token(user_id: &str, secret: &str) -> crate::error::Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))?;

    let claims = Claims {
        sub: user_id.to_string(),
        exp: (now.as_secs() + 86400) as usize,
        iat: now.as_secs() as usize,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

#[allow(dead_code)]
pub fn verify_token(token: &str, secret: &str) -> crate::error::Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))?;

    Ok(token_data.claims)
}
