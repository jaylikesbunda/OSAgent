use crate::tools::web_search::cache::SearchCache;
use crate::tools::web_search::types::{
    BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult,
};
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use reqwest::{Client, Url};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

const INSTANCE_DISCOVERY_URL: &str = "https://searx.space/data/instances.json";

#[derive(Clone, Debug, PartialEq)]
struct InstanceCandidate {
    base_url: String,
    score: f64,
}

pub struct SearxngBackend {
    instances: SearchCache<Vec<InstanceCandidate>>,
    instance_limit: usize,
    instance_timeout: Duration,
}

impl SearxngBackend {
    pub fn new(refresh_ttl: Duration, instance_limit: usize, instance_timeout: Duration) -> Self {
        Self {
            instances: SearchCache::new(refresh_ttl),
            instance_limit: instance_limit.max(1),
            instance_timeout,
        }
    }

    async fn discover_instances(&self, client: &Client) -> BackendResult<Vec<InstanceCandidate>> {
        if let Some(cached) = self.instances.get("instances") {
            return Ok(cached);
        }

        let response = client
            .get(INSTANCE_DISCOVERY_URL)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| {
                BackendError::network(format!("failed to fetch SearXNG instances: {error}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(BackendError::http_status(
                status.as_u16(),
                format!("SearXNG discovery returned HTTP {status}"),
            ));
        }

        let payload: Value = response.json().await.map_err(|error| {
            BackendError::parse(format!("invalid SearXNG discovery payload: {error}"))
        })?;
        let instances = parse_instance_candidates(&payload);
        if instances.is_empty() {
            return Err(BackendError::empty(
                "SearXNG discovery did not yield any healthy public instances",
            ));
        }

        self.instances
            .insert("instances".to_string(), instances.clone());
        Ok(instances)
    }

    async fn search_instance(
        client: &Client,
        instance: InstanceCandidate,
        request: SearchRequest,
        timeout_duration: Duration,
    ) -> BackendResult<Vec<SearchResult>> {
        let mut url = format!(
            "{}search?q={}&format=json&language=en-US&pageno=1&safesearch=0",
            instance.base_url,
            urlencoding::encode(&request.query)
        );
        if request.num_results > 0 {
            url.push_str(&format!("&max_results={}", request.num_results));
        }

        let response = timeout(
            timeout_duration,
            client.get(&url).header("Accept", "application/json").send(),
        )
        .await
        .map_err(|_| BackendError::timeout("SearXNG instance timed out"))
        .and_then(|result| {
            result.map_err(|error| {
                BackendError::network(format!(
                    "failed querying SearXNG instance {}: {error}",
                    instance.base_url
                ))
            })
        })?;

        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err(BackendError::blocked(format!(
                "SearXNG instance {} returned HTTP {}",
                instance.base_url, status
            )));
        }
        if !status.is_success() {
            return Err(BackendError::http_status(
                status.as_u16(),
                format!(
                    "SearXNG instance {} returned HTTP {}",
                    instance.base_url, status
                ),
            ));
        }

        let payload: Value = response.json().await.map_err(|error| {
            BackendError::parse(format!(
                "invalid SearXNG JSON from {}: {error}",
                instance.base_url
            ))
        })?;
        let mut results = parse_search_results(&payload, &instance.base_url, request.num_results);
        if results.is_empty() {
            return Err(BackendError::empty(format!(
                "SearXNG instance {} returned no results",
                instance.base_url
            )));
        }

        for (index, result) in results.iter_mut().enumerate() {
            result.position = index + 1;
        }

        Ok(results)
    }
}

#[async_trait]
impl SearchBackend for SearxngBackend {
    fn id(&self) -> &'static str {
        "searxng"
    }

    fn priority(&self) -> u8 {
        30
    }

    fn min_interval(&self) -> Duration {
        Duration::from_secs(6)
    }

    fn timeout(&self) -> Duration {
        self.instance_timeout.max(Duration::from_millis(2_500))
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>> {
        let instances = self.discover_instances(client).await?;
        let mut searches = FuturesUnordered::new();
        let mut tried = Vec::new();

        for instance in instances.into_iter().take(self.instance_limit) {
            tried.push(instance.base_url.clone());
            searches.push(Self::search_instance(
                client,
                instance,
                request.clone(),
                self.instance_timeout,
            ));
        }

        let mut errors = Vec::new();
        while let Some(result) = searches.next().await {
            match result {
                Ok(results) => return Ok(results),
                Err(error) => errors.push(error.message),
            }
        }

        Err(BackendError::empty(format!(
            "No healthy SearXNG instance returned results (tried: {}){}",
            tried.join(", "),
            if errors.is_empty() {
                String::new()
            } else {
                format!(" | {}", errors.join(" | "))
            }
        )))
    }
}

fn normalize_base_url(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    Some(format!("{}/", parsed.as_str().trim_end_matches('/')))
}

fn score_instance(base_url: &str, data: &Value) -> Option<f64> {
    let status = data.pointer("/http/status_code")?.as_i64()?;
    if status != 200 {
        return None;
    }
    let http_error = data.pointer("/http/error");
    if let Some(error) = http_error {
        if !error.is_null() && !error.as_str().unwrap_or_default().is_empty() {
            return None;
        }
    }

    let generator = data
        .get("generator")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !generator.contains("searx") {
        return None;
    }

    let network_type = data
        .get("network_type")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    if network_type != "normal" {
        return None;
    }

    let initial_success = data
        .pointer("/timing/initial/success_percentage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let search_success = data
        .pointer("/timing/search/success_percentage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let latency = data
        .pointer("/timing/initial/all/value")
        .and_then(Value::as_f64)
        .unwrap_or(10.0);
    let uptime_day = data
        .pointer("/uptime/uptimeDay")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let uptime_week = data
        .pointer("/uptime/uptimeWeek")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    if initial_success < 80.0 || search_success < 70.0 || latency > 2.5 {
        return None;
    }

    normalize_base_url(base_url)?;
    let mut score =
        initial_success * 1.8 + search_success * 2.4 + uptime_day * 0.5 + uptime_week * 0.3;
    score -= latency * 25.0;
    if data.get("main").and_then(Value::as_bool).unwrap_or(false) {
        score += 15.0;
    }
    if data
        .pointer("/tls/grade")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .starts_with('A')
    {
        score += 5.0;
    }

    if score <= 0.0 {
        return None;
    }

    Some(score)
}

fn parse_instance_candidates(payload: &Value) -> Vec<InstanceCandidate> {
    let Some(instances) = payload.get("instances").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut candidates = instances
        .iter()
        .filter_map(|(base_url, data)| {
            let normalized = normalize_base_url(base_url)?;
            let score = score_instance(base_url, data)?;
            Some(InstanceCandidate {
                base_url: normalized,
                score,
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    candidates
}

fn parse_search_results(payload: &Value, base_url: &str, max_results: usize) -> Vec<SearchResult> {
    let host = Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "searxng".to_string());
    let Some(items) = payload.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let title = item
                .get("title")?
                .as_str()?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let url = item.get("url")?.as_str()?.to_string();
            if title.is_empty() || url.is_empty() {
                return None;
            }

            let snippet = item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            Some(SearchResult {
                title,
                url,
                snippet,
                source: format!("searxng:{host}"),
                position: 0,
            })
        })
        .take(max_results)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_instance_candidates, parse_search_results};
    use serde_json::json;

    #[test]
    fn filters_and_sorts_instances() {
        let payload = json!({
            "instances": {
                "https://slow.example/": {
                    "generator": "searxng",
                    "network_type": "normal",
                    "main": true,
                    "http": { "status_code": 200, "error": null },
                    "timing": {
                        "initial": { "success_percentage": 100.0, "all": { "value": 1.2 } },
                        "search": { "success_percentage": 90.0 }
                    },
                    "uptime": { "uptimeDay": 90.0, "uptimeWeek": 90.0 },
                    "tls": { "grade": "A+" }
                },
                "https://fast.example/": {
                    "generator": "searxng",
                    "network_type": "normal",
                    "main": true,
                    "http": { "status_code": 200, "error": null },
                    "timing": {
                        "initial": { "success_percentage": 100.0, "all": { "value": 0.2 } },
                        "search": { "success_percentage": 92.0 }
                    },
                    "uptime": { "uptimeDay": 95.0, "uptimeWeek": 94.0 },
                    "tls": { "grade": "A+" }
                },
                "http://nope.example/": {
                    "generator": "searxng",
                    "network_type": "normal",
                    "http": { "status_code": 200, "error": null }
                }
            }
        });

        let instances = parse_instance_candidates(&payload);
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].base_url, "https://fast.example/");
    }

    #[test]
    fn parses_json_results() {
        let payload = json!({
            "results": [
                {
                    "title": " Example Docs ",
                    "url": "https://example.com/docs",
                    "content": " Learn   async Rust "
                }
            ]
        });

        let results = parse_search_results(&payload, "https://search.example/", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Docs");
        assert_eq!(results[0].snippet, "Learn async Rust");
        assert_eq!(results[0].source, "searxng:search.example");
    }
}
