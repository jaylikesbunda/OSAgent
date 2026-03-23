mod backends;
mod cache;
mod normalize;
mod rank;
mod types;

use crate::error::{OSAgentError, Result};
use backends::ddg_html::DuckDuckGoHtmlBackend;
use backends::ddg_lite::DuckDuckGoLiteBackend;
use cache::SearchCache;
use dashmap::DashMap;
use normalize::{normalize_query, normalize_results};
use rank::rank_results;
use reqwest::Client;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use types::{BackendError, SearchBackend, SearchRequest, SearchResponse};

const DEFAULT_SEARCH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_RESULTS: usize = 10;
const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";

#[derive(Clone, Debug)]
struct BackendCooldown {
    until: Instant,
}

pub struct SearchService {
    client: Client,
    backends: Vec<Box<dyn SearchBackend>>,
    cache: SearchCache<SearchResponse>,
    cooldowns: DashMap<String, BackendCooldown>,
}

impl SearchService {
    pub fn new(client: Client) -> Self {
        Self::with_backends(
            client,
            vec![
                Box::new(DuckDuckGoLiteBackend),
                Box::new(DuckDuckGoHtmlBackend),
            ],
            DEFAULT_SEARCH_CACHE_TTL,
        )
    }

    fn with_backends(
        client: Client,
        backends: Vec<Box<dyn SearchBackend>>,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            client,
            backends,
            cache: SearchCache::new(cache_ttl),
            cooldowns: DashMap::new(),
        }
    }

    pub async fn search(&self, query: &str, num_results: usize) -> Result<SearchResponse> {
        let query = normalize_query(query);
        if query.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Search query cannot be empty".to_string(),
            ));
        }

        let request = SearchRequest {
            query: query.clone(),
            num_results: num_results.clamp(1, MAX_RESULTS),
        };
        let cache_key = format!(
            "{}::{}",
            request.query.to_ascii_lowercase(),
            request.num_results
        );
        if let Some(mut cached) = self.cache.get(&cache_key) {
            cached.cached = true;
            return Ok(cached);
        }

        let now = Instant::now();
        let ready = self
            .backends
            .iter()
            .filter(|backend| !self.is_on_cooldown(backend.id(), now))
            .map(|backend| backend.as_ref())
            .collect::<Vec<_>>();

        let candidates = if ready.is_empty() {
            self.backends
                .iter()
                .map(|backend| backend.as_ref())
                .collect::<Vec<_>>()
        } else {
            ready
        };

        let mut tried_backends = Vec::new();
        let mut errors = Vec::new();

        for backend in candidates {
            tried_backends.push(backend.id().to_string());
            match backend.search(&self.client, &request).await {
                Ok(results) => {
                    self.cooldowns.remove(backend.id());
                    let results = rank_results(normalize_results(results), request.num_results);
                    if results.is_empty() {
                        errors.push(format!("{}: no normalized results", backend.id()));
                        continue;
                    }

                    let response = SearchResponse {
                        query: request.query.clone(),
                        backend: backend.id().to_string(),
                        fallback_used: tried_backends.len() > 1,
                        cached: false,
                        tried_backends: tried_backends.clone(),
                        results,
                    };
                    self.cache.insert(cache_key, response.clone());
                    return Ok(response);
                }
                Err(error) => {
                    self.record_failure(backend.id(), &error, now);
                    errors.push(format!("{}: {}", backend.id(), error.message));
                }
            }
        }

        Err(OSAgentError::ToolExecution(format!(
            "No search results found. Tried backends: {}",
            errors.join(" | ")
        )))
    }

    fn is_on_cooldown(&self, backend_id: &str, now: Instant) -> bool {
        self.cooldowns
            .get(backend_id)
            .map(|cooldown| cooldown.until > now)
            .unwrap_or(false)
    }

    fn record_failure(&self, backend_id: &str, error: &BackendError, now: Instant) {
        let Some(duration) = error.cooldown_duration() else {
            return;
        };
        self.cooldowns.insert(
            backend_id.to_string(),
            BackendCooldown {
                until: now + duration,
            },
        );
    }
}

pub(crate) async fn fetch_search_page(
    client: &Client,
    url: &str,
    accept: &str,
) -> std::result::Result<String, BackendError> {
    let mut last_error = None;

    for attempt in 0..2 {
        let response = client
            .get(url)
            .header("Accept", accept)
            .header("Accept-Language", ACCEPT_LANGUAGE)
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status();
                if status.as_u16() == 403 || status.as_u16() == 429 {
                    return Err(BackendError::blocked(format!(
                        "search backend returned HTTP {}",
                        status
                    )));
                }
                if !status.is_success() {
                    return Err(BackendError::http_status(
                        status.as_u16(),
                        format!("search backend returned HTTP {}", status),
                    ));
                }

                return response.text().await.map_err(|error| {
                    if error.is_timeout() {
                        BackendError::timeout(format!("timed out reading search response: {error}"))
                    } else {
                        BackendError::network(format!("failed reading search response: {error}"))
                    }
                });
            }
            Err(error) => {
                let classified = if error.is_timeout() {
                    BackendError::timeout(format!("timed out fetching search results: {error}"))
                } else {
                    BackendError::network(format!("failed fetching search results: {error}"))
                };
                last_error = Some(classified.clone());
                if attempt == 0 && (error.is_timeout() || error.is_connect() || error.is_request())
                {
                    sleep(Duration::from_millis(250)).await;
                    continue;
                }
                return Err(classified);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| BackendError::network("unknown search error")))
}

pub(crate) fn is_probable_block_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "captcha",
        "verify you are human",
        "verify you're human",
        "unusual traffic",
        "automated requests",
        "challenge",
        "bot detection",
        "access denied",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn looks_like_no_results_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "no results",
        "no more results",
        "did not match any documents",
        "try asking differently",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::SearchService;
    use crate::tools::web_search::types::{
        BackendError, SearchBackend, SearchRequest, SearchResult,
    };
    use async_trait::async_trait;
    use reqwest::Client;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FakeBackend {
        id: &'static str,
        calls: Arc<Mutex<usize>>,
        result: std::result::Result<Vec<SearchResult>, BackendError>,
    }

    #[async_trait]
    impl SearchBackend for FakeBackend {
        fn id(&self) -> &'static str {
            self.id
        }

        async fn search(
            &self,
            _client: &Client,
            _request: &SearchRequest,
        ) -> std::result::Result<Vec<SearchResult>, BackendError> {
            *self.calls.lock().expect("lock poisoned") += 1;
            self.result.clone()
        }
    }

    fn sample_result(url: &str, source: &str) -> SearchResult {
        SearchResult {
            title: "Example Result".to_string(),
            url: url.to_string(),
            snippet: "Useful snippet".to_string(),
            source: source.to_string(),
            position: 1,
        }
    }

    #[tokio::test]
    async fn falls_back_to_second_backend() {
        let primary_calls = Arc::new(Mutex::new(0usize));
        let fallback_calls = Arc::new(Mutex::new(0usize));
        let service = SearchService::with_backends(
            Client::new(),
            vec![
                Box::new(FakeBackend {
                    id: "ddg_lite",
                    calls: primary_calls.clone(),
                    result: Err(BackendError::blocked("blocked")),
                }),
                Box::new(FakeBackend {
                    id: "ddg_html",
                    calls: fallback_calls.clone(),
                    result: Ok(vec![sample_result("https://example.com/docs", "ddg_html")]),
                }),
            ],
            Duration::from_secs(60),
        );

        let response = service
            .search("rust async", 5)
            .await
            .expect("expected response");
        assert_eq!(response.backend, "ddg_html");
        assert!(response.fallback_used);
        assert_eq!(*primary_calls.lock().expect("lock poisoned"), 1);
        assert_eq!(*fallback_calls.lock().expect("lock poisoned"), 1);
    }

    #[tokio::test]
    async fn reuses_cached_results() {
        let calls = Arc::new(Mutex::new(0usize));
        let service = SearchService::with_backends(
            Client::new(),
            vec![Box::new(FakeBackend {
                id: "ddg_lite",
                calls: calls.clone(),
                result: Ok(vec![sample_result("https://example.com/docs", "ddg_lite")]),
            })],
            Duration::from_secs(60),
        );

        let first = service
            .search("rust async", 5)
            .await
            .expect("expected response");
        let second = service
            .search("rust async", 5)
            .await
            .expect("expected cached response");

        assert!(!first.cached);
        assert!(second.cached);
        assert_eq!(*calls.lock().expect("lock poisoned"), 1);
    }
}
