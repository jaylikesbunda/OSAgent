use crate::storage::SqliteStorage;
use axum::{
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// How stale a device's `last_seen_at` must be before an authenticated request
/// refreshes it. Keeps the pairing list current without a write per request.
const DEVICE_TOUCH_INTERVAL_SECS: i64 = 300;

fn token_from_query(query: &str) -> Option<String> {
    for part in query.split('&') {
        let (key, value) = match part.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };

        if key != "token" && key != "access_token" {
            continue;
        }

        let decoded = urlencoding::decode(value).ok()?.to_string();
        if !decoded.trim().is_empty() {
            return Some(decoded);
        }
    }
    None
}

#[derive(Clone)]
pub struct AuthLayer {
    pub secret: String,
    pub storage: Arc<SqliteStorage>,
}

impl AuthLayer {
    pub fn new(secret: String, storage: Arc<SqliteStorage>) -> Self {
        Self { secret, storage }
    }

    /// Is this token valid, and (when it is a paired-device token) is the
    /// device still paired?
    fn token_is_authorized(&self, token: &str) -> bool {
        match crate::web::auth::verify_token(token, &self.secret) {
            Ok(claims) => match claims.device_id.as_deref() {
                Some(device_id) => self
                    .storage
                    .check_paired_device(device_id, DEVICE_TOUCH_INTERVAL_SECS)
                    .unwrap_or(false),
                // Password-issued tokens are stateless.
                None => true,
            },
            Err(_) => false,
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            secret: self.secret.clone(),
            storage: self.storage.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    secret: String,
    storage: Arc<SqliteStorage>,
}

impl<S, B> Service<Request<B>> for AuthMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let secret = self.secret.clone();
        let storage = self.storage.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if request.method() == axum::http::Method::OPTIONS {
                return inner.call(request).await;
            }

            let layer = AuthLayer { secret, storage };

            let auth_header = request
                .headers()
                .get("Authorization")
                .and_then(|h| h.to_str().ok());

            if let Some(auth) = auth_header {
                if let Some(token) = auth.strip_prefix("Bearer ") {
                    if layer.token_is_authorized(token) {
                        return inner.call(request).await;
                    }
                }
            }

            if let Some(query) = request.uri().query() {
                if let Some(token) = token_from_query(query) {
                    if layer.token_is_authorized(&token) {
                        return inner.call(request).await;
                    }
                }
            }

            Ok(StatusCode::UNAUTHORIZED.into_response())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AuthLayer;
    use crate::web::auth;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
        response::{IntoResponse, Response},
    };
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tower::{Layer, Service};

    #[derive(Clone)]
    struct OkService;

    impl Service<Request<Body>> for OkService {
        type Response = Response;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: Request<Body>) -> Self::Future {
            std::future::ready(Ok(StatusCode::OK.into_response()))
        }
    }

    #[tokio::test]
    async fn rejects_missing_bearer_token() {
        let mut service = AuthLayer::new(
            "test-secret".to_string(),
            Arc::new(crate::storage::SqliteStorage::new_in_memory().unwrap()),
        )
        .layer(OkService);

        let response = service
            .call(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_valid_bearer_token() {
        let secret = "test-secret".to_string();
        let token = auth::generate_token("user", &secret).unwrap();
        let mut service = AuthLayer::new(
            secret,
            Arc::new(crate::storage::SqliteStorage::new_in_memory().unwrap()),
        )
        .layer(OkService);

        let response = service
            .call(
                Request::builder()
                    .uri("/api/config")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn allows_options_without_token() {
        let mut service = AuthLayer::new(
            "test-secret".to_string(),
            Arc::new(crate::storage::SqliteStorage::new_in_memory().unwrap()),
        )
        .layer(OkService);

        let response = service
            .call(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn accepts_valid_query_token() {
        let secret = "test-secret".to_string();
        let token = auth::generate_token("user", &secret).unwrap();
        let mut service = AuthLayer::new(
            secret,
            Arc::new(crate::storage::SqliteStorage::new_in_memory().unwrap()),
        )
        .layer(OkService);

        let response = service
            .call(
                Request::builder()
                    .uri(format!("/api/sessions/abc/events?token={}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn device_token_requires_an_active_paired_device() {
        let storage = Arc::new(crate::storage::SqliteStorage::new_in_memory().unwrap());
        let secret = "test-secret".to_string();
        let device = crate::storage::PairedDevice {
            id: "dev-1".to_string(),
            label: "Phone".to_string(),
            user_agent: None,
            created_at: chrono::Utc::now(),
            last_seen_at: None,
        };
        let token = auth::generate_device_token(&device.id, &secret, 3600).unwrap();

        let call = |service: &mut crate::web::middleware::AuthMiddleware<OkService>| {
            service.call(
                Request::builder()
                    .uri("/api/config")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
        };

        // A device token with no matching device row is rejected.
        let mut service = AuthLayer::new(secret.clone(), storage.clone()).layer(OkService);
        assert_eq!(
            call(&mut service).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // Once paired, the same token is accepted.
        storage.create_paired_device(&device).unwrap();
        let mut service = AuthLayer::new(secret.clone(), storage.clone()).layer(OkService);
        assert_eq!(call(&mut service).await.unwrap().status(), StatusCode::OK);

        // Revoking the device invalidates the still-signed token.
        storage.delete_paired_device(&device.id).unwrap();
        let mut service = AuthLayer::new(secret.clone(), storage.clone()).layer(OkService);
        assert_eq!(
            call(&mut service).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
