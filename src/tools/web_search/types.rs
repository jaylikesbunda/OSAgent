use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: String,
    pub num_results: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
    #[serde(skip_serializing)]
    pub position: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SearchResponse {
    pub query: String,
    pub backend: String,
    pub fallback_used: bool,
    pub cached: bool,
    pub tried_backends: Vec<String>,
    pub results: Vec<SearchResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendErrorKind {
    Blocked,
    Empty,
    Parse,
    Network,
    Timeout,
    HttpStatus(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
}

impl BackendError {
    pub fn blocked(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Blocked,
            message: message.into(),
        }
    }

    pub fn empty(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Empty,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Parse,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Network,
            message: message.into(),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Timeout,
            message: message.into(),
        }
    }

    pub fn http_status(status: u16, message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::HttpStatus(status),
            message: message.into(),
        }
    }

    pub fn cooldown_duration(&self) -> Option<Duration> {
        match self.kind {
            BackendErrorKind::Blocked => Some(Duration::from_secs(10 * 60)),
            BackendErrorKind::Parse => Some(Duration::from_secs(3 * 60)),
            BackendErrorKind::Network | BackendErrorKind::Timeout => Some(Duration::from_secs(90)),
            BackendErrorKind::HttpStatus(status) if status >= 500 => Some(Duration::from_secs(90)),
            BackendErrorKind::HttpStatus(429) => Some(Duration::from_secs(5 * 60)),
            BackendErrorKind::HttpStatus(_) | BackendErrorKind::Empty => None,
        }
    }
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

#[async_trait]
pub trait SearchBackend: Send + Sync {
    fn id(&self) -> &'static str;
    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>>;
}
