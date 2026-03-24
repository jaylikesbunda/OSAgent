mod backends;
mod cache;
mod normalize;
mod rank;
mod types;

use crate::config::SearchConfig;
use crate::error::{OSAgentError, Result};
use backends::brave::BraveBackend;
use backends::ddg_html::DuckDuckGoHtmlBackend;
use backends::ddg_lite::DuckDuckGoLiteBackend;
use backends::searxng::SearxngBackend;
use backends::startpage::StartpageBackend;
use cache::SearchCache;
use dashmap::DashMap;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use normalize::{normalize_query, normalize_results};
use rank::rank_results;
use reqwest::Client;
use std::cmp::Reverse;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout, Instant as TokioInstant};
use types::{
    BackendError, BackendErrorKind, SearchBackend, SearchRequest, SearchResponse, SearchResult,
};

const DEFAULT_SEARCH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_RESULTS: usize = 10;
const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";
const PRIMARY_WAVE_SIZE: usize = 2;
const RATE_LIMIT_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default)]
struct BackendState {
    cooldown_until: Option<Instant>,
    last_started_at: Option<Instant>,
    consecutive_failures: u32,
    successes: u32,
    last_error_kind: Option<BackendErrorKind>,
}

#[derive(Clone)]
struct ScheduledBackend {
    backend: Arc<dyn SearchBackend>,
    cooldown_until: Option<Instant>,
    rate_limit_until: Option<Instant>,
    consecutive_failures: u32,
    successes: u32,
    last_error_kind: Option<BackendErrorKind>,
}

impl ScheduledBackend {
    fn ready_at(&self, now: Instant) -> Instant {
        let mut ready_at = now;
        if let Some(until) = self.cooldown_until {
            ready_at = ready_at.max(until);
        }
        if let Some(until) = self.rate_limit_until {
            ready_at = ready_at.max(until);
        }
        ready_at
    }
}

type BackendTask = BoxFuture<
    'static,
    (
        String,
        std::result::Result<types::BackendResult<Vec<SearchResult>>, tokio::time::error::Elapsed>,
    ),
>;

pub struct SearchService {
    client: Client,
    backends: Vec<Arc<dyn SearchBackend>>,
    cache: SearchCache<SearchResponse>,
    backend_states: DashMap<String, BackendState>,
    config: SearchConfig,
}

impl SearchService {
    pub fn new(client: Client, config: SearchConfig) -> Self {
        let refresh_ttl = Duration::from_secs(config.searxng_instance_refresh_minutes.max(1) * 60);
        let searxng_timeout = Duration::from_millis(config.per_backend_timeout_ms.max(900));

        Self::with_backends(
            client,
            vec![
                Arc::new(BraveBackend),
                Arc::new(StartpageBackend),
                Arc::new(SearxngBackend::new(
                    refresh_ttl,
                    config.searxng_max_instances,
                    searxng_timeout,
                )),
                Arc::new(DuckDuckGoLiteBackend),
                Arc::new(DuckDuckGoHtmlBackend),
            ],
            DEFAULT_SEARCH_CACHE_TTL,
            config,
        )
    }

    fn with_backends(
        client: Client,
        backends: Vec<Arc<dyn SearchBackend>>,
        cache_ttl: Duration,
        config: SearchConfig,
    ) -> Self {
        Self {
            client,
            backends,
            cache: SearchCache::new(cache_ttl),
            backend_states: DashMap::new(),
            config,
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
            num_results: num_results.clamp(1, self.config.max_results.clamp(1, MAX_RESULTS)),
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
        let mut scheduled = self.schedule_backends(now);
        if scheduled.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "No search backends are configured".to_string(),
            ));
        }

        let global_deadline =
            TokioInstant::now() + Duration::from_millis(self.config.global_timeout_ms.max(500));
        let mut pending = FuturesUnordered::new();
        let mut launched = 0usize;
        let max_launches = self
            .config
            .max_parallel_backends
            .max(1)
            .min(scheduled.len());
        let primary_wave = PRIMARY_WAVE_SIZE.min(max_launches);
        let mut tried_backends = Vec::new();
        let mut merged_results = Vec::new();
        let mut successful_backends = Vec::new();
        let mut errors = Vec::new();

        launched += self
            .launch_backends(
                &mut scheduled,
                &mut pending,
                &request,
                &mut tried_backends,
                primary_wave,
                global_deadline,
            )
            .await;

        if launched == 0 {
            return Err(OSAgentError::ToolExecution(
                "All search backends are cooling down or rate limited".to_string(),
            ));
        }

        self.collect_pending_results(
            &mut pending,
            global_deadline,
            &mut merged_results,
            &mut successful_backends,
            &mut errors,
        )
        .await;

        while launched < max_launches
            && !scheduled.is_empty()
            && !self.has_enough_results(&merged_results, request.num_results)
            && global_deadline > TokioInstant::now()
        {
            let launched_now = self
                .launch_backends(
                    &mut scheduled,
                    &mut pending,
                    &request,
                    &mut tried_backends,
                    1,
                    global_deadline,
                )
                .await;
            if launched_now == 0 {
                break;
            }

            launched += launched_now;
            self.collect_pending_results(
                &mut pending,
                global_deadline,
                &mut merged_results,
                &mut successful_backends,
                &mut errors,
            )
            .await;
        }

        if merged_results.is_empty() {
            let backend_info = if errors.is_empty() {
                tried_backends.join(", ")
            } else {
                errors.join(" | ")
            };
            return Err(OSAgentError::ToolExecution(format!(
                "No search results found. Tried backends: {}. Consider using web_fetch with a direct URL or site-specific endpoint (e.g., reddit.com/.json, wikipedia.org/api) as an alternative.",
                backend_info
            )));
        }

        let results = rank_results(merged_results, request.num_results);
        let backend = if successful_backends.len() == 1 {
            successful_backends[0].clone()
        } else {
            "multi".to_string()
        };

        let response = SearchResponse {
            query: request.query.clone(),
            backend,
            fallback_used: tried_backends.len() > 1 || !errors.is_empty(),
            cached: false,
            tried_backends,
            results,
        };
        self.cache.insert(cache_key, response.clone());
        Ok(response)
    }

    fn schedule_backends(&self, now: Instant) -> VecDeque<ScheduledBackend> {
        let mut backends = self
            .backends
            .iter()
            .map(|backend| {
                let backend_id = backend.id().to_string();
                let state = self
                    .backend_states
                    .get(&backend_id)
                    .map(|entry| entry.clone())
                    .unwrap_or_default();

                ScheduledBackend {
                    backend: Arc::clone(backend),
                    cooldown_until: state.cooldown_until,
                    rate_limit_until: state
                        .last_started_at
                        .map(|started_at| started_at + backend.min_interval()),
                    consecutive_failures: state.consecutive_failures,
                    successes: state.successes,
                    last_error_kind: state.last_error_kind,
                }
            })
            .collect::<Vec<_>>();

        backends.sort_by_key(|candidate| {
            (
                candidate.ready_at(now),
                candidate.consecutive_failures,
                Reverse(candidate.successes),
                candidate.backend.priority(),
            )
        });

        VecDeque::from(backends)
    }

    async fn launch_backends(
        &self,
        scheduled: &mut VecDeque<ScheduledBackend>,
        pending: &mut FuturesUnordered<BackendTask>,
        request: &SearchRequest,
        tried_backends: &mut Vec<String>,
        count: usize,
        global_deadline: TokioInstant,
    ) -> usize {
        let mut launched = 0;
        while launched < count {
            let Some(candidate) = scheduled.pop_front() else {
                break;
            };

            let now = Instant::now();
            if let Some(cooldown_until) = candidate.cooldown_until {
                if cooldown_until > now {
                    let soft_cooldown = is_soft_cooldown(candidate.last_error_kind.as_ref());
                    if soft_cooldown && launched == 0 && pending.is_empty() {
                        // Allow a best-effort retry when every option is otherwise cooling down.
                    } else {
                        scheduled.push_front(candidate);
                        break;
                    }
                }
            }

            if let Some(rate_limit_until) = candidate.rate_limit_until {
                if rate_limit_until > now {
                    let wait = rate_limit_until.saturating_duration_since(now);
                    let remaining = global_deadline.saturating_duration_since(TokioInstant::now());

                    if wait <= RATE_LIMIT_GRACE && wait < remaining {
                        sleep(wait).await;
                    } else if launched > 0 || !pending.is_empty() {
                        scheduled.push_front(candidate);
                        break;
                    }
                }
            }

            let backend = candidate.backend;
            let backend_id = backend.id().to_string();
            tried_backends.push(backend_id.clone());
            self.mark_started(&backend_id, Instant::now());

            pending.push(Box::pin(spawn_backend_task(
                self.client.clone(),
                backend,
                request.clone(),
                self.backend_timeout(),
            )));
            launched += 1;
        }

        launched
    }

    async fn collect_pending_results(
        &self,
        pending: &mut FuturesUnordered<BackendTask>,
        global_deadline: TokioInstant,
        merged_results: &mut Vec<SearchResult>,
        successful_backends: &mut Vec<String>,
        errors: &mut Vec<String>,
    ) {
        while !pending.is_empty() {
            let remaining = global_deadline.saturating_duration_since(TokioInstant::now());
            if remaining.is_zero() {
                break;
            }

            match timeout(remaining, pending.next()).await {
                Ok(Some((backend_id, Ok(Ok(results))))) => {
                    self.record_success(&backend_id, Instant::now());
                    successful_backends.push(backend_id);
                    merged_results.extend(normalize_results(results));
                }
                Ok(Some((backend_id, Ok(Err(error))))) => {
                    self.record_failure(&backend_id, &error, Instant::now());
                    errors.push(format!("{}: {}", backend_id, error.message));
                }
                Ok(Some((backend_id, Err(_)))) => {
                    let error = BackendError::timeout(format!(
                        "timed out after {} ms",
                        self.backend_timeout().as_millis()
                    ));
                    self.record_failure(&backend_id, &error, Instant::now());
                    errors.push(format!("{}: {}", backend_id, error.message));
                }
                Ok(None) | Err(_) => break,
            }
        }
    }

    fn has_enough_results(&self, merged_results: &[SearchResult], requested: usize) -> bool {
        let unique_results = normalize_results(merged_results.to_vec());
        let threshold = requested.clamp(3, 4);
        unique_results.len() >= threshold
    }

    fn backend_timeout(&self) -> Duration {
        Duration::from_millis(self.config.per_backend_timeout_ms.max(250))
    }

    fn mark_started(&self, backend_id: &str, now: Instant) {
        self.backend_states
            .entry(backend_id.to_string())
            .and_modify(|state| state.last_started_at = Some(now))
            .or_insert_with(|| BackendState {
                last_started_at: Some(now),
                ..BackendState::default()
            });
    }

    fn record_success(&self, backend_id: &str, now: Instant) {
        self.backend_states
            .entry(backend_id.to_string())
            .and_modify(|state| {
                state.cooldown_until = None;
                state.last_started_at = Some(now);
                state.consecutive_failures = 0;
                state.successes = state.successes.saturating_add(1).min(100);
                state.last_error_kind = None;
            })
            .or_insert_with(|| BackendState {
                cooldown_until: None,
                last_started_at: Some(now),
                consecutive_failures: 0,
                successes: 1,
                last_error_kind: None,
            });
    }

    fn record_failure(&self, backend_id: &str, error: &BackendError, now: Instant) {
        self.backend_states
            .entry(backend_id.to_string())
            .and_modify(|state| {
                state.last_started_at = Some(now);
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                state.last_error_kind = Some(error.kind.clone());
                if let Some(duration) = error.cooldown_duration() {
                    state.cooldown_until =
                        Some(now + scale_duration(duration, state.consecutive_failures));
                }
            })
            .or_insert_with(|| {
                let mut state = BackendState {
                    cooldown_until: None,
                    last_started_at: Some(now),
                    consecutive_failures: 1,
                    successes: 0,
                    last_error_kind: Some(error.kind.clone()),
                };
                if let Some(duration) = error.cooldown_duration() {
                    state.cooldown_until =
                        Some(now + scale_duration(duration, state.consecutive_failures));
                }
                state
            });
    }
}

fn scale_duration(duration: Duration, failures: u32) -> Duration {
    let multiplier = failures.clamp(1, 4);
    let millis = duration.as_millis().saturating_mul(multiplier as u128);
    let capped = millis.min(Duration::from_secs(30 * 60).as_millis());
    Duration::from_millis(capped as u64)
}

fn is_soft_cooldown(kind: Option<&BackendErrorKind>) -> bool {
    match kind {
        Some(BackendErrorKind::Timeout) | Some(BackendErrorKind::Network) => true,
        Some(BackendErrorKind::HttpStatus(status)) if *status >= 500 => true,
        _ => false,
    }
}

async fn spawn_backend_task(
    client: Client,
    backend: Arc<dyn SearchBackend>,
    request: SearchRequest,
    default_timeout: Duration,
) -> (
    String,
    std::result::Result<types::BackendResult<Vec<SearchResult>>, tokio::time::error::Elapsed>,
) {
    let backend_id = backend.id().to_string();
    let timeout_duration = backend.timeout().max(default_timeout);
    let result = timeout(timeout_duration, backend.search(&client, &request)).await;
    (backend_id, result)
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
                if status.as_u16() == 429 {
                    return Err(BackendError::http_status(
                        429,
                        format!("search backend returned HTTP {}", status),
                    ));
                }
                if status.as_u16() == 403 {
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
    use crate::config::SearchConfig;
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
        priority: u8,
        min_interval: Duration,
    }

    #[async_trait]
    impl SearchBackend for FakeBackend {
        fn id(&self) -> &'static str {
            self.id
        }

        fn priority(&self) -> u8 {
            self.priority
        }

        fn min_interval(&self) -> Duration {
            self.min_interval
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

    fn config() -> SearchConfig {
        SearchConfig {
            enabled: true,
            index_on_startup: true,
            max_results: 20,
            global_timeout_ms: 3_500,
            per_backend_timeout_ms: 1_000,
            max_parallel_backends: 4,
            searxng_instance_refresh_minutes: 30,
            searxng_max_instances: 2,
        }
    }

    #[tokio::test]
    async fn aggregates_results_from_multiple_backends() {
        let primary_calls = Arc::new(Mutex::new(0usize));
        let fallback_calls = Arc::new(Mutex::new(0usize));
        let service = SearchService::with_backends(
            Client::new(),
            vec![
                Arc::new(FakeBackend {
                    id: "brave",
                    calls: primary_calls.clone(),
                    result: Ok(vec![sample_result("https://example.com/docs", "brave")]),
                    priority: 10,
                    min_interval: Duration::ZERO,
                }),
                Arc::new(FakeBackend {
                    id: "startpage",
                    calls: fallback_calls.clone(),
                    result: Ok(vec![sample_result("https://example.com/docs", "startpage")]),
                    priority: 20,
                    min_interval: Duration::ZERO,
                }),
            ],
            Duration::from_secs(60),
            config(),
        );

        let response = service
            .search("rust async", 5)
            .await
            .expect("expected response");
        assert_eq!(response.backend, "multi");
        assert!(response.fallback_used);
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].source, "brave,startpage");
        assert_eq!(*primary_calls.lock().expect("lock poisoned"), 1);
        assert_eq!(*fallback_calls.lock().expect("lock poisoned"), 1);
    }

    #[tokio::test]
    async fn reuses_cached_results() {
        let calls = Arc::new(Mutex::new(0usize));
        let service = SearchService::with_backends(
            Client::new(),
            vec![Arc::new(FakeBackend {
                id: "brave",
                calls: calls.clone(),
                result: Ok(vec![sample_result("https://example.com/docs", "brave")]),
                priority: 10,
                min_interval: Duration::ZERO,
            })],
            Duration::from_secs(60),
            config(),
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

    #[tokio::test]
    async fn respects_backend_cooldowns_between_queries() {
        let brave_calls = Arc::new(Mutex::new(0usize));
        let fallback_calls = Arc::new(Mutex::new(0usize));
        let service = SearchService::with_backends(
            Client::new(),
            vec![
                Arc::new(FakeBackend {
                    id: "brave",
                    calls: brave_calls.clone(),
                    result: Err(BackendError::http_status(429, "rate limited")),
                    priority: 10,
                    min_interval: Duration::ZERO,
                }),
                Arc::new(FakeBackend {
                    id: "startpage",
                    calls: fallback_calls.clone(),
                    result: Ok(vec![sample_result("https://example.com/docs", "startpage")]),
                    priority: 20,
                    min_interval: Duration::ZERO,
                }),
            ],
            Duration::ZERO,
            SearchConfig {
                global_timeout_ms: 3_500,
                per_backend_timeout_ms: 1_000,
                ..config()
            },
        );

        let first = service.search("query one", 5).await.expect("first search");
        let second = service.search("query two", 5).await.expect("second search");

        assert_eq!(first.backend, "startpage");
        assert_eq!(second.backend, "startpage");
        assert_eq!(*brave_calls.lock().expect("lock poisoned"), 1);
        assert_eq!(*fallback_calls.lock().expect("lock poisoned"), 2);
    }
}
