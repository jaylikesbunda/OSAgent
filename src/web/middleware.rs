use axum::{
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::{Layer, Service};

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
}

impl AuthLayer {
    #[allow(dead_code)]
    pub fn new(secret: String) -> Self {
        Self { secret }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            secret: self.secret.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    secret: String,
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
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if request.method() == axum::http::Method::OPTIONS {
                return inner.call(request).await;
            }

            let auth_header = request
                .headers()
                .get("Authorization")
                .and_then(|h| h.to_str().ok());

            if let Some(auth) = auth_header {
                if let Some(token) = auth.strip_prefix("Bearer ") {
                    if crate::web::auth::verify_token(token, &secret).is_ok() {
                        return inner.call(request).await;
                    }
                }
            }

            if let Some(query) = request.uri().query() {
                if let Some(token) = token_from_query(query) {
                    if crate::web::auth::verify_token(&token, &secret).is_ok() {
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
        let mut service = AuthLayer::new("test-secret".to_string()).layer(OkService);

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
        let mut service = AuthLayer::new(secret).layer(OkService);

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
        let mut service = AuthLayer::new("test-secret".to_string()).layer(OkService);

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
        let mut service = AuthLayer::new(secret).layer(OkService);

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
}
