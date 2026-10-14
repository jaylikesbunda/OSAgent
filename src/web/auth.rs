use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
    /// Set for tokens minted by QR pairing. The middleware checks the device is
    /// still active so a paired phone can be revoked server-side even though
    /// the token itself is a stateless JWT. Password logins leave this unset.
    #[serde(default)]
    pub device_id: Option<String>,
}

pub fn hash_password(password: &str) -> crate::error::Result<String> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> crate::error::Result<bool> {
    bcrypt::verify(password, hash).map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

fn token_with_claims(claims: Claims, secret: &str) -> crate::error::Result<String> {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))
}

pub fn generate_token(user_id: &str, secret: &str) -> crate::error::Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))?;

    let claims = Claims {
        sub: user_id.to_string(),
        exp: (now.as_secs() + 86400) as usize,
        iat: now.as_secs() as usize,
        device_id: None,
    };

    token_with_claims(claims, secret)
}

/// Mint a long-lived token bound to a paired device. `ttl_seconds` controls how
/// long it stays valid; revoking the device invalidates it sooner.
pub fn generate_device_token(
    device_id: &str,
    secret: &str,
    ttl_seconds: u64,
) -> crate::error::Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))?;

    let claims = Claims {
        sub: format!("device:{device_id}"),
        exp: (now.as_secs() + ttl_seconds) as usize,
        iat: now.as_secs() as usize,
        device_id: Some(device_id.to_string()),
    };

    token_with_claims(claims, secret)
}

pub fn verify_token(token: &str, secret: &str) -> crate::error::Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|e| crate::error::OSAgentError::Auth(e.to_string()))?;

    Ok(token_data.claims)
}
