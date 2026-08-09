use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use std::time::Duration;

/// Recency filter, mirroring the "past day/week/month/year" control every
/// search engine exposes in its UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "day" | "d" | "24h" | "today" | "past_day" => Some(Self::Day),
            "week" | "w" | "7d" | "past_week" => Some(Self::Week),
            "month" | "m" | "30d" | "past_month" => Some(Self::Month),
            "year" | "y" | "365d" | "past_year" => Some(Self::Year),
            _ => None,
        }
    }

    /// DuckDuckGo / Startpage single-letter form.
    pub fn as_letter(&self) -> &'static str {
        match self {
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "m",
            Self::Year => "y",
        }
    }

    /// SearXNG long form.
    pub fn as_word(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    pub fn as_duration(&self) -> Duration {
        match self {
            Self::Day => Duration::from_secs(24 * 60 * 60),
            Self::Week => Duration::from_secs(7 * 24 * 60 * 60),
            Self::Month => Duration::from_secs(30 * 24 * 60 * 60),
            Self::Year => Duration::from_secs(365 * 24 * 60 * 60),
        }
    }

    /// Unix timestamp of the start of the window, for APIs that filter by date.
    pub fn since_unix(&self) -> i64 {
        let now = chrono::Utc::now().timestamp();
        now - self.as_duration().as_secs() as i64
    }

    pub fn since_date(&self) -> String {
        let since =
            chrono::Utc::now() - chrono::Duration::seconds(self.as_duration().as_secs() as i64);
        since.format("%Y-%m-%d").to_string()
    }
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: String,
    pub num_results: usize,
    pub time_range: Option<TimeRange>,
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
            BackendErrorKind::Blocked => Some(Duration::from_secs(5 * 60)),
            BackendErrorKind::Parse => Some(Duration::from_secs(2 * 60)),
            BackendErrorKind::Network | BackendErrorKind::Timeout => Some(Duration::from_secs(15)),
            BackendErrorKind::HttpStatus(status) if status >= 500 => Some(Duration::from_secs(30)),
            BackendErrorKind::HttpStatus(429) => Some(Duration::from_secs(6 * 60)),
            BackendErrorKind::HttpStatus(_) | BackendErrorKind::Empty => None,
        }
    }
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

#[async_trait]
pub trait SearchBackend: Send + Sync {
    fn id(&self) -> &'static str;

    fn priority(&self) -> u8 {
        100
    }

    fn min_interval(&self) -> Duration {
        Duration::from_secs(8)
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(1_800)
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>>;
}
