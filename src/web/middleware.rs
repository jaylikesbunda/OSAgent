use axum::{
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::{Layer, Service};

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
            if request.uri().path() == "/api/auth/login" {
                return inner.call(request).await;
            }

            let auth_header = request
                .headers()
                .get("Authorization")
                .and_then(|h| h.to_str().ok());

            if let Some(auth) = auth_header {
                if auth.starts_with("Bearer ") {
                    let token = &auth[7..];

                    if crate::web::auth::verify_token(token, &secret).is_ok() {
                        return inner.call(request).await;
                    }
                }
            }

            Ok(StatusCode::UNAUTHORIZED.into_response())
        })
    }
}
