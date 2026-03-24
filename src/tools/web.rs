use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::output::maybe_store_large_output;
use crate::tools::registry::Tool;
use crate::tools::web_search::SearchService;
use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE, RETRY_AFTER};
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::warn;

#[derive(Debug, Clone)]
enum Mode {
    Fetch,
    Explore,
    Extract,
}

#[derive(Debug, Clone)]
struct ExtractConfig {
    selectors: Vec<FieldSelector>,
}

#[derive(Debug, Clone)]
struct FieldSelector {
    name: String,
    selector: String,
    attribute: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DiscoveredField {
    name: String,
    selector: String,
    field_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectedContentKind {
    Json,
    Html,
    Xml,
    Feed,
    Text,
}

impl DetectedContentKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Xml => "xml",
            Self::Feed => "feed",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone)]
struct FetchResult {
    final_url: String,
    status: u16,
    content_type: Option<String>,
    kind: DetectedContentKind,
    body: String,
}

pub struct WebFetchTool {
    client: Client,
    workspace: std::path::PathBuf,
    writable: bool,
}

impl WebFetchTool {
    pub fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());

        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspace =
            std::path::PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());

        Self {
            client,
            workspace,
            writable,
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a known URL and normalize readable HTML, site-aware JSON, XML, feeds, or raw page content; also supports page exploration and CSS extraction"
    }

    fn when_to_use(&self) -> &str {
        "Use when you already have a URL and need readable page text, smart summaries for common JSON endpoints, feed/XML output, or CSS-based extraction"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for file operations, command execution, or when offline"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Fetch readable page text".to_string(),
                input: json!({
                    "url": "https://example.com/docs",
                    "format": "auto"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Fetch a JSON endpoint".to_string(),
                input: json!({
                    "url": "https://www.reddit.com/r/rust/search.json?q=async&restrict_sr=1",
                    "format": "auto",
                    "headers": {
                        "User-Agent": "OSAgent/0.1"
                    }
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Explore page structure".to_string(),
                input: json!({
                    "url": "https://example.com/product",
                    "mode": "explore"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Extract structured data".to_string(),
                input: json!({
                    "url": "https://example.com/product",
                    "mode": "extract",
                    "extract": {
                        "selectors": [
                            {"name": "title", "selector": "h1.product-name"},
                            {"name": "price", "selector": ".price"}
                        ]
                    }
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch"
                },
                "format": {
                    "type": "string",
                    "enum": ["auto", "text", "json", "html", "xml"],
                    "description": "Fetch mode format: auto detects content, text returns normalized readable output, json/html/xml force raw type handling"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs"
                },
                "mode": {
                    "type": "string",
                    "enum": ["fetch", "explore", "extract"],
                    "description": "Mode: fetch (default, raw content), explore (discover available fields), extract (structured data with CSS selectors)"
                },
                "extract": {
                    "type": "object",
                    "description": "CSS selectors for extract mode",
                    "properties": {
                        "selectors": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string", "description": "Field name in output"},
                                    "selector": {"type": "string", "description": "CSS selector"},
                                    "attribute": {"type": "string", "description": "Optional: extract attribute instead of text (e.g., 'src', 'href')"}
                                },
                                "required": ["name", "selector"]
                            }
                        }
                    }
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let url_str = args["url"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'url' parameter".to_string()))?;

        let url = if url_str.starts_with("http://") {
            url_str.replace("http://", "https://")
        } else {
            url_str.to_string()
        };

        let mode_str = args["mode"].as_str().unwrap_or("fetch");
        let mode = match mode_str {
            "explore" => Mode::Explore,
            "extract" => Mode::Extract,
            _ => Mode::Fetch,
        };

        match mode {
            Mode::Explore => self.explore_page(&url, args.get("headers")).await,
            Mode::Extract => {
                self.extract_data(&url, args.get("headers"), &args["extract"])
                    .await
            }
            Mode::Fetch => {
                let format = args["format"].as_str().unwrap_or("auto");
                let content = match self
                    .fetch_with_reqwest(&url, format, args.get("headers"))
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("reqwest failed, trying curl: {}", e);
                        self.fetch_with_curl(&url, format, args.get("headers"))?
                    }
                };

                Ok(maybe_store_large_output(
                    &self.workspace,
                    self.writable,
                    "web_fetch",
                    &content,
                ))
            }
        }
    }
}

impl WebFetchTool {
    fn normalize_reddit_url(url: &str) -> String {
        if !url.contains("reddit.com") {
            return url.to_string();
        }

        if url.contains(".json") || url.ends_with('/') && url.len() > 1 {
            return url.to_string();
        }

        let has_query = url.contains('?');
        if has_query {
            url.replace("?", ".json?")
        } else if url.ends_with('/') {
            format!("{}.json", url.trim_end_matches('/'))
        } else {
            format!("{}.json", url)
        }
    }

    async fn fetch_with_reqwest(
        &self,
        url: &str,
        format: &str,
        headers: Option<&Value>,
    ) -> Result<String> {
        let normalized_url = Self::normalize_reddit_url(url);
        let fetched = self
            .fetch_response(&normalized_url, headers, Self::accept_header(format))
            .await?;
        self.render_fetch_result(url, format, fetched)
    }

    fn fetch_with_curl(&self, url: &str, format: &str, headers: Option<&Value>) -> Result<String> {
        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS", // silent but show errors
            "-L",  // follow redirects
            "--max-time",
            "30",
            "-A",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        ]);

        cmd.args(["-H", &format!("Accept: {}", Self::accept_header(format))]);

        if let Some(obj) = headers.and_then(Value::as_object) {
            for (key, value) in obj {
                if let Some(val) = value.as_str() {
                    cmd.args(["-H", &format!("{}: {}", key, val)]);
                }
            }
        }

        cmd.arg(url);

        let output = cmd
            .output()
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to execute curl: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(OSAgentError::ToolExecution(format!(
                "curl failed: {}",
                stderr
            )));
        }
        let body = String::from_utf8(output.stdout).map_err(|e| {
            OSAgentError::ToolExecution(format!("curl output invalid UTF-8: {}", e))
        })?;
        let fetched = FetchResult {
            final_url: url.to_string(),
            status: 200,
            content_type: None,
            kind: Self::detect_content_kind(None, &body),
            body,
        };
        self.render_fetch_result(url, format, fetched)
    }

    async fn explore_page(&self, url: &str, headers: Option<&Value>) -> Result<String> {
        let html_content = self.fetch_html(url, headers).await?;
        let discovered_fields = self.discover_fields(&html_content);

        let result = json!({
            "url": url,
            "mode": "explore",
            "discovered_fields": discovered_fields,
            "suggestion": "Use mode='extract' with selectors above to pull structured data"
        });

        Ok(result.to_string())
    }

    async fn extract_data(
        &self,
        url: &str,
        headers: Option<&Value>,
        extract_config: &Value,
    ) -> Result<String> {
        let extract = self.parse_extract_config(extract_config)?;
        let html_content = self.fetch_html(url, headers).await?;
        let data = self.apply_selectors(&html_content, &extract.selectors)?;

        let result = json!({
            "url": url,
            "mode": "extract",
            "data": data
        });

        Ok(result.to_string())
    }

    async fn fetch_html(&self, url: &str, headers: Option<&Value>) -> Result<String> {
        Ok(self
            .fetch_response(url, headers, Self::accept_header("html"))
            .await?
            .body)
    }

    async fn fetch_response(
        &self,
        url: &str,
        headers: Option<&Value>,
        accept: &str,
    ) -> Result<FetchResult> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_SECS: u64 = 2;

        for attempt in 0..=MAX_RETRIES {
            let mut request = self.client.get(url).header(ACCEPT, accept);
            if let Some(headers_obj) = headers.and_then(Value::as_object) {
                for (key, value) in headers_obj {
                    if let Some(val_str) = value.as_str() {
                        request = request.header(key.as_str(), val_str);
                    }
                }
            }

            let response = request
                .send()
                .await
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to fetch URL: {}", e)))?;

            let status = response.status();
            let status_code = status.as_u16();

            if status_code == 429 {
                let retry_after = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(BASE_DELAY_SECS * (2_u64.pow(attempt)));

                let delay = Duration::from_secs(retry_after.min(60));

                if attempt < MAX_RETRIES {
                    warn!(
                        "Rate limited (429) fetching {}. Retrying in {:?}... (attempt {}/{})",
                        url,
                        delay,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(OSAgentError::ToolExecution(format!(
                    "HTTP 429 Too Many Requests after {} retries",
                    MAX_RETRIES
                )));
            }

            if !status.is_success() {
                return Err(OSAgentError::ToolExecution(format!(
                    "HTTP error: {}",
                    status
                )));
            }

            let final_url = response.url().to_string();
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let body = response.text().await.map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to read response: {}", e))
            })?;

            return Ok(FetchResult {
                final_url,
                status: status_code,
                kind: Self::detect_content_kind(content_type.as_deref(), &body),
                content_type,
                body,
            });
        }

        Err(OSAgentError::ToolExecution(
            "Max retries exceeded".to_string(),
        ))
    }

    fn render_fetch_result(
        &self,
        requested_url: &str,
        format: &str,
        fetched: FetchResult,
    ) -> Result<String> {
        let content = match format {
            "auto" => Self::render_auto_content(&fetched)?,
            "text" => Self::render_text_content(&fetched)?,
            "json" => Self::render_json_content(&fetched.body)?,
            "html" => fetched.body.clone(),
            "xml" => fetched.body.clone(),
            _ => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Unsupported format: {}",
                    format
                )))
            }
        };

        let output = [
            format!("URL: {}", requested_url),
            format!("Final-URL: {}", fetched.final_url),
            format!("Status: {}", fetched.status),
            format!(
                "Content-Type: {}",
                fetched.content_type.as_deref().unwrap_or("unknown")
            ),
            format!("Detected: {}", fetched.kind.as_str()),
            String::new(),
            content,
        ];

        Ok(output.join("\n"))
    }

    fn render_auto_content(fetched: &FetchResult) -> Result<String> {
        match fetched.kind {
            DetectedContentKind::Json => Self::render_smart_json_content(fetched),
            DetectedContentKind::Html => Ok(Self::extract_readable_html(&fetched.body)),
            DetectedContentKind::Feed | DetectedContentKind::Xml => {
                Ok(Self::extract_feed_text(&fetched.body))
            }
            DetectedContentKind::Text => Ok(fetched.body.clone()),
        }
    }

    fn render_text_content(fetched: &FetchResult) -> Result<String> {
        match fetched.kind {
            DetectedContentKind::Json => Self::render_smart_json_content(fetched),
            DetectedContentKind::Html => Ok(Self::extract_readable_html(&fetched.body)),
            DetectedContentKind::Feed | DetectedContentKind::Xml => {
                Ok(Self::extract_feed_text(&fetched.body))
            }
            DetectedContentKind::Text => Ok(fetched.body.clone()),
        }
    }

    fn render_json_content(body: &str) -> Result<String> {
        let json: Value = serde_json::from_str(body)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to parse JSON: {}", e)))?;
        serde_json::to_string_pretty(&json)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to format JSON: {}", e)))
    }

    fn render_smart_json_content(fetched: &FetchResult) -> Result<String> {
        let json: Value = serde_json::from_str(&fetched.body)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to parse JSON: {}", e)))?;

        if let Some(summary) = Self::summarize_known_json(&fetched.final_url, &json) {
            Ok(summary)
        } else {
            serde_json::to_string_pretty(&json)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to format JSON: {}", e)))
        }
    }

    fn summarize_known_json(url: &str, json: &Value) -> Option<String> {
        if Self::looks_like_reddit_json(url, json) {
            return Self::summarize_reddit_json(json);
        }
        if Self::looks_like_wikipedia_json(url, json) {
            return Self::summarize_wikipedia_json(json);
        }
        if Self::looks_like_github_json(url, json) {
            return Self::summarize_github_json(json);
        }
        None
    }

    fn looks_like_reddit_json(url: &str, json: &Value) -> bool {
        url.contains("reddit.com")
            && (url.contains(".json")
                || json.get("kind").is_some()
                || json.pointer("/data/children").is_some())
    }

    fn looks_like_wikipedia_json(url: &str, json: &Value) -> bool {
        url.contains("wikipedia.org")
            || json.get("query").is_some()
            || json.get("extract").is_some()
            || json.get("title").is_some() && json.get("pageid").is_some()
    }

    fn looks_like_github_json(url: &str, json: &Value) -> bool {
        url.contains("api.github.com")
            || json.get("full_name").is_some()
            || json.get("html_url").is_some() && json.get("stargazers_count").is_some()
            || json.get("items").is_some() && json.get("total_count").is_some()
    }

    fn summarize_reddit_json(json: &Value) -> Option<String> {
        let children = json.pointer("/data/children")?.as_array()?;
        let mut lines = Vec::new();
        for child in children.iter().take(10) {
            let post = child.get("data")?;
            let title = Self::clean_text_for_display(post.get("title")?.as_str()?);
            if title.is_empty() {
                continue;
            }
            let permalink = post
                .get("permalink")
                .and_then(Value::as_str)
                .map(|value| format!("https://www.reddit.com{}", value))
                .or_else(|| post.get("url").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            let subreddit = post
                .get("subreddit")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let score = post.get("score").and_then(Value::as_i64).unwrap_or(0);
            let comments = post
                .get("num_comments")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            lines.push(format!("- {}", title));
            lines.push(format!(
                "  r/{} | {} pts | {} comments",
                subreddit, score, comments
            ));
            if !permalink.is_empty() {
                lines.push(format!("  {}", permalink));
            }
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    fn summarize_wikipedia_json(json: &Value) -> Option<String> {
        if let Some(title) = json.get("title").and_then(Value::as_str) {
            let mut lines = vec![format!("Title: {}", Self::clean_text_for_display(title))];
            if let Some(extract) = json.get("extract").and_then(Value::as_str) {
                let cleaned = Self::clean_text_for_display(extract);
                if !cleaned.is_empty() {
                    lines.push(String::new());
                    lines.push(cleaned);
                }
            }
            if let Some(url) = json
                .pointer("/content_urls/desktop/page")
                .and_then(Value::as_str)
                .or_else(|| {
                    json.pointer("/content_urls/mobile/page")
                        .and_then(Value::as_str)
                })
            {
                lines.push(String::new());
                lines.push(url.to_string());
            }
            return Some(lines.join("\n"));
        }

        let pages = json.pointer("/query/pages")?.as_object()?;
        let mut lines = Vec::new();
        for page in pages.values().take(10) {
            let title = page
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let extract = page
                .get("extract")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            lines.push(format!("- {}", Self::clean_text_for_display(title)));
            let cleaned = Self::clean_text_for_display(extract);
            if !cleaned.is_empty() {
                lines.push(format!("  {}", cleaned));
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    fn summarize_github_json(json: &Value) -> Option<String> {
        if let Some(items) = json.get("items").and_then(Value::as_array) {
            let mut lines = Vec::new();
            if let Some(total_count) = json.get("total_count").and_then(Value::as_u64) {
                lines.push(format!("Total: {}", total_count));
                lines.push(String::new());
            }
            for item in items.iter().take(10) {
                let title = item
                    .get("full_name")
                    .or_else(|| item.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if title.is_empty() {
                    continue;
                }
                lines.push(format!("- {}", title));
                if let Some(description) = item.get("description").and_then(Value::as_str) {
                    let cleaned = Self::clean_text_for_display(description);
                    if !cleaned.is_empty() {
                        lines.push(format!("  {}", cleaned));
                    }
                }
                let stars = item
                    .get("stargazers_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let language = item
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                lines.push(format!("  stars: {} | language: {}", stars, language));
                if let Some(url) = item.get("html_url").and_then(Value::as_str) {
                    lines.push(format!("  {}", url));
                }
            }
            return if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            };
        }

        let full_name = json.get("full_name").and_then(Value::as_str)?;
        let mut lines = vec![format!("Repository: {}", full_name)];
        if let Some(description) = json.get("description").and_then(Value::as_str) {
            let cleaned = Self::clean_text_for_display(description);
            if !cleaned.is_empty() {
                lines.push(cleaned);
            }
        }
        let stars = json
            .get("stargazers_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let forks = json.get("forks_count").and_then(Value::as_u64).unwrap_or(0);
        let issues = json
            .get("open_issues_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let language = json
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        lines.push(format!(
            "stars: {} | forks: {} | open issues: {} | language: {}",
            stars, forks, issues, language
        ));
        if let Some(url) = json.get("html_url").and_then(Value::as_str) {
            lines.push(url.to_string());
        }
        Some(lines.join("\n"))
    }

    fn accept_header(format: &str) -> &'static str {
        match format {
            "json" => "application/json,text/json;q=0.9,*/*;q=0.1",
            "xml" => "application/rss+xml,application/atom+xml,application/xml,text/xml;q=0.9,*/*;q=0.1",
            _ => "text/html,application/xhtml+xml,application/json,application/rss+xml,application/atom+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.1",
        }
    }

    fn detect_content_kind(content_type: Option<&str>, body: &str) -> DetectedContentKind {
        let body_trimmed = body.trim_start();
        let lower_content_type = content_type.unwrap_or_default().to_ascii_lowercase();

        if lower_content_type.contains("json")
            || body_trimmed.starts_with('{')
            || body_trimmed.starts_with('[')
        {
            return DetectedContentKind::Json;
        }
        if lower_content_type.contains("rss")
            || lower_content_type.contains("atom")
            || body_trimmed.contains("<rss")
            || body_trimmed.contains("<feed")
        {
            return DetectedContentKind::Feed;
        }
        if lower_content_type.contains("html")
            || body_trimmed.contains("<html")
            || body_trimmed.contains("<!doctype html")
        {
            return DetectedContentKind::Html;
        }
        if lower_content_type.contains("xml") || body_trimmed.starts_with("<?xml") {
            return DetectedContentKind::Xml;
        }
        DetectedContentKind::Text
    }

    fn extract_readable_html(html_content: &str) -> String {
        let document = Html::parse_document(html_content);
        let title_selector = Selector::parse("title").ok();
        let meta_desc_selector = Selector::parse(
            "meta[name='description'], meta[property='og:description'], meta[name='twitter:description']",
        )
        .ok();

        let mut lines = Vec::new();
        if let Some(selector) = title_selector.as_ref() {
            if let Some(title) = document.select(selector).next() {
                let text = Self::clean_text_for_display(&title.text().collect::<String>());
                if !text.is_empty() {
                    lines.push(format!("Title: {}", text));
                }
            }
        }
        if let Some(selector) = meta_desc_selector.as_ref() {
            if let Some(meta) = document.select(selector).next() {
                if let Some(content) = meta.value().attr("content") {
                    let text = Self::clean_text_for_display(content);
                    if !text.is_empty() {
                        lines.push(format!("Description: {}", text));
                    }
                }
            }
        }

        let content_text = Self::extract_structured_text(&document);
        if !content_text.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(content_text);
        }

        if lines.is_empty() {
            Self::clean_text_for_display(
                &document.root_element().text().collect::<Vec<_>>().join(" "),
            )
        } else {
            lines.join("\n")
        }
    }

    fn extract_feed_text(feed_content: &str) -> String {
        let mut lines = Vec::new();
        for block in Self::extract_feed_blocks(feed_content).into_iter().take(10) {
            let title = Self::extract_tag_text(&block, "title").unwrap_or_default();
            let link = Self::extract_feed_link(&block).unwrap_or_default();
            let description = Self::extract_tag_text(&block, "description")
                .or_else(|| Self::extract_tag_text(&block, "summary"))
                .or_else(|| Self::extract_tag_text(&block, "content"))
                .unwrap_or_default();

            if !title.is_empty() {
                lines.push(format!("- {}", title));
                if !link.is_empty() {
                    lines.push(format!("  {}", link));
                }
                if !description.is_empty() {
                    lines.push(format!("  {}", description));
                }
            }
        }

        if lines.is_empty() {
            Self::clean_text_for_display(feed_content)
        } else {
            lines.join("\n")
        }
    }

    fn extract_structured_text(document: &Html) -> String {
        let section_selector = Selector::parse("main, article, body").ok();
        let block_selector = Selector::parse("h1, h2, h3, h4, p, li, pre, code, blockquote").ok();
        let Some(block_selector) = block_selector else {
            return String::new();
        };

        let roots = section_selector
            .as_ref()
            .map(|selector| document.select(selector).collect::<Vec<_>>())
            .unwrap_or_default();

        let mut lines = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let roots = if roots.is_empty() {
            vec![document.root_element()]
        } else {
            roots
        };

        for root in roots {
            for node in root.select(&block_selector) {
                let text = Self::clean_text_for_display(&node.text().collect::<String>());
                if text.len() < 2 || !seen.insert(text.clone()) {
                    continue;
                }
                lines.push(text);
                if lines.len() >= 80 {
                    return lines.join("\n");
                }
            }
        }

        lines.join("\n")
    }

    fn clean_text_for_display(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn extract_tag_text(fragment: &str, tag_name: &str) -> Option<String> {
        let open = format!("<{}>", tag_name);
        let close = format!("</{}>", tag_name);
        let start = fragment.find(&open)? + open.len();
        let rest = &fragment[start..];
        let end = rest.find(&close)?;
        let text = Self::clean_text_for_display(&rest[..end]);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn extract_feed_blocks(feed_content: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        for (open, close) in [("<item", "</item>"), ("<entry", "</entry>")] {
            let mut remaining = feed_content;
            while let Some(start) = remaining.find(open) {
                let after_start = &remaining[start..];
                let Some(tag_end) = after_start.find('>') else {
                    break;
                };
                let content = &after_start[tag_end + 1..];
                let Some(end) = content.find(close) else {
                    break;
                };
                blocks.push(content[..end].to_string());
                remaining = &content[end + close.len()..];
            }
            if !blocks.is_empty() {
                break;
            }
        }
        blocks
    }

    fn extract_feed_link(fragment: &str) -> Option<String> {
        if let Some(link) = Self::extract_tag_text(fragment, "link") {
            return Some(link);
        }

        let marker = "<link ";
        let start = fragment.find(marker)?;
        let rest = &fragment[start + marker.len()..];
        let href_marker = "href=\"";
        let href_start = rest.find(href_marker)? + href_marker.len();
        let href_rest = &rest[href_start..];
        let href_end = href_rest.find('"')?;
        Some(href_rest[..href_end].to_string())
    }

    fn discover_fields(&self, html_content: &str) -> Vec<DiscoveredField> {
        let document = Html::parse_document(html_content);
        let mut fields: Vec<DiscoveredField> = Vec::new();
        let mut seen_selectors: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        let patterns: Vec<(&str, &str, &str)> = vec![
            ("h1", "heading1", "text"),
            ("h2", "heading2", "text"),
            ("h3", "heading3", "text"),
            (".title", "title", "text"),
            (".product-title", "product_title", "text"),
            (".product-name", "product_name", "text"),
            ("[class*='title']", "title_variant", "text"),
            (".price", "price", "text"),
            ("#price", "price", "text"),
            ("[class*='price']", "price_variant", "text"),
            ("[data-price]", "price_data", "text"),
            (".description", "description", "text"),
            (".product-description", "product_description", "text"),
            ("[class*='description']", "description_variant", "text"),
            ("p", "paragraph", "text"),
            ("img", "image", "attribute"),
            ("img[class*='product']", "product_image", "attribute"),
            ("img[class*='main']", "main_image", "attribute"),
            (".rating", "rating", "text"),
            ("[class*='rating']", "rating_variant", "text"),
            (".stars", "stars", "text"),
            ("a[href]", "link", "attribute"),
            (".stock", "stock", "text"),
            ("[class*='stock']", "stock_variant", "text"),
            ("[class*='availability']", "availability", "text"),
            ("#availability", "availability", "text"),
        ];

        for (selector_str, name, field_type) in patterns {
            if let Ok(selector) = Selector::parse(selector_str) {
                let matches: Vec<_> = document.select(&selector).collect();
                if !matches.is_empty() && !seen_selectors.contains(selector_str) {
                    seen_selectors.insert(selector_str.to_string());
                    let sample_value = matches
                        .first()
                        .map(|el| {
                            if field_type == "attribute" {
                                el.value()
                                    .attr("src")
                                    .or_else(|| el.value().attr("href"))
                                    .unwrap_or("")
                                    .to_string()
                            } else {
                                el.text().collect::<String>().trim().to_string()
                            }
                        })
                        .unwrap_or_default();

                    if !sample_value.is_empty() || matches.len() > 1 {
                        fields.push(DiscoveredField {
                            name: name.to_string(),
                            selector: selector_str.to_string(),
                            field_type: field_type.to_string(),
                        });
                    }
                }
            }
        }

        fields.truncate(12);
        fields
    }

    fn parse_extract_config(&self, extract_config: &Value) -> Result<ExtractConfig> {
        let selectors_array = extract_config["selectors"].as_array().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'selectors' in extract config".to_string())
        })?;

        let mut selectors = Vec::new();
        for item in selectors_array {
            let name = item["name"]
                .as_str()
                .ok_or_else(|| OSAgentError::ToolExecution("Selector missing 'name'".to_string()))?
                .to_string();
            let selector = item["selector"]
                .as_str()
                .ok_or_else(|| {
                    OSAgentError::ToolExecution("Selector missing 'selector'".to_string())
                })?
                .to_string();
            let attribute = item["attribute"].as_str().map(String::from);

            selectors.push(FieldSelector {
                name,
                selector,
                attribute,
            });
        }

        Ok(ExtractConfig { selectors })
    }

    fn apply_selectors(
        &self,
        html_content: &str,
        selectors: &[FieldSelector],
    ) -> Result<serde_json::Value> {
        let document = Html::parse_document(html_content);
        let mut data = serde_json::Map::new();

        for field in selectors {
            match Selector::parse(&field.selector) {
                Ok(selector) => {
                    let value = document.select(&selector).next().map(|el| {
                        if let Some(attr) = &field.attribute {
                            el.value().attr(attr.as_str()).unwrap_or("").to_string()
                        } else {
                            el.text().collect::<String>().trim().to_string()
                        }
                    });

                    data.insert(field.name.clone(), json!(value));
                }
                Err(e) => {
                    data.insert(
                        field.name.clone(),
                        json!({"error": format!("Invalid selector: {:?}", e)}),
                    );
                }
            }
        }

        Ok(serde_json::Value::Object(data))
    }
}

pub struct WebSearchTool {
    service: SearchService,
}

impl WebSearchTool {
    pub fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            service: SearchService::new(client, config.search.clone()),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using public no-key search backends"
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to find information on the web but don't have a specific URL"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use when you already have a specific URL or when doing local operations"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Search for documentation".to_string(),
            input: json!({
                "query": "Rust async programming tutorial",
                "num_results": 5
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'query' parameter".to_string()))?;

        let num_results = args["num_results"].as_u64().unwrap_or(5) as usize;
        let response = self.service.search(query, num_results).await?;
        serde_json::to_string(&response).map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to encode search results: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DetectedContentKind, WebFetchTool};
    use serde_json::json;

    #[test]
    fn detects_json_and_feed_content() {
        assert_eq!(
            WebFetchTool::detect_content_kind(Some("application/json"), "{\"ok\":true}"),
            DetectedContentKind::Json
        );
        assert_eq!(
            WebFetchTool::detect_content_kind(
                Some("application/rss+xml"),
                "<rss><channel></channel></rss>"
            ),
            DetectedContentKind::Feed
        );
    }

    #[test]
    fn extracts_readable_html_text() {
        let html = r#"
            <html>
              <head>
                <title>Example Page</title>
                <meta name="description" content="A useful description">
              </head>
              <body>
                <main>
                  <h1>Hello</h1>
                  <p>World content</p>
                </main>
              </body>
            </html>
        "#;
        let rendered = WebFetchTool::extract_readable_html(html);
        assert!(rendered.contains("Title: Example Page"));
        assert!(rendered.contains("Description: A useful description"));
        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("World content"));
    }

    #[test]
    fn extracts_feed_items() {
        let feed = r#"
            <rss>
              <channel>
                <item>
                  <title>First item</title>
                  <link>https://example.com/1</link>
                  <description>Item description</description>
                </item>
              </channel>
            </rss>
        "#;
        let rendered = WebFetchTool::extract_feed_text(feed);
        assert!(rendered.contains("First item"));
        assert!(rendered.contains("https://example.com/1"));
        assert!(rendered.contains("Item description"));
    }

    #[test]
    fn summarizes_reddit_json() {
        let json = json!({
            "data": {
                "children": [
                    {
                        "data": {
                            "title": "Async Rust discussion",
                            "permalink": "/r/rust/comments/abc123/async_rust_discussion/",
                            "subreddit": "rust",
                            "score": 42,
                            "num_comments": 7
                        }
                    }
                ]
            }
        });
        let rendered = WebFetchTool::summarize_known_json(
            "https://www.reddit.com/r/rust/search.json?q=async",
            &json,
        )
        .expect("expected reddit summary");
        assert!(rendered.contains("Async Rust discussion"));
        assert!(rendered.contains("r/rust"));
        assert!(rendered.contains("42 pts"));
    }

    #[test]
    fn summarizes_wikipedia_json() {
        let json = json!({
            "title": "Rust",
            "extract": "Rust is a programming language.",
            "content_urls": {
                "desktop": {
                    "page": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
                }
            }
        });
        let rendered = WebFetchTool::summarize_known_json(
            "https://en.wikipedia.org/api/rest_v1/page/summary/Rust_(programming_language)",
            &json,
        )
        .expect("expected wikipedia summary");
        assert!(rendered.contains("Title: Rust"));
        assert!(rendered.contains("Rust is a programming language."));
        assert!(rendered.contains("wikipedia.org/wiki"));
    }

    #[test]
    fn summarizes_github_json() {
        let json = json!({
            "full_name": "rust-lang/rust",
            "description": "Empowering everyone to build reliable and efficient software.",
            "stargazers_count": 100,
            "forks_count": 20,
            "open_issues_count": 5,
            "language": "Rust",
            "html_url": "https://github.com/rust-lang/rust"
        });
        let rendered = WebFetchTool::summarize_known_json(
            "https://api.github.com/repos/rust-lang/rust",
            &json,
        )
        .expect("expected github summary");
        assert!(rendered.contains("Repository: rust-lang/rust"));
        assert!(rendered.contains("stars: 100"));
        assert!(rendered.contains("github.com/rust-lang/rust"));
    }
}
