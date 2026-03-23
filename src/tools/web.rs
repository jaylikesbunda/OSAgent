use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::output::maybe_store_large_output;
use crate::tools::registry::Tool;
use crate::tools::web_search::SearchService;
use async_trait::async_trait;
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
        "Fetch content from a URL. Supports 3 modes: fetch (raw content), explore (discover fields), extract (structured data with CSS selectors)"
    }

    fn when_to_use(&self) -> &str {
        "Use for fetching web content, scraping structured data, or exploring page structure to find extractable fields"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for file operations, command execution, or when offline"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Fetch raw content".to_string(),
                input: json!({
                    "url": "https://example.com/docs",
                    "format": "text"
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
                    "enum": ["text", "json", "html"],
                    "description": "Response format for fetch mode (default: text)"
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
                let format = args["format"].as_str().unwrap_or("text");
                let content = match self.fetch_with_reqwest(&url, format).await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("reqwest failed, trying curl: {}", e);
                        self.fetch_with_curl(&url, format)?
                    }
                };

                let output = format!("URL: {}\n\n{}", url, content);

                Ok(maybe_store_large_output(
                    &self.workspace,
                    self.writable,
                    "web_fetch",
                    &output,
                ))
            }
        }
    }
}

impl WebFetchTool {
    async fn fetch_with_reqwest(&self, url: &str, format: &str) -> Result<String> {
        let request = self.client.get(url);

        let response = request
            .send()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("reqwest failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "HTTP error: {}",
                status
            )));
        }

        match format {
            "json" => {
                let json: Value = response.json().await.map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to parse JSON: {}", e))
                })?;
                serde_json::to_string_pretty(&json).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to format JSON: {}", e))
                })
            }
            "html" | "text" => response.text().await.map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to read response: {}", e))
            }),
            _ => Err(OSAgentError::ToolExecution(format!(
                "Unsupported format: {}",
                format
            ))),
        }
    }

    fn fetch_with_curl(&self, url: &str, format: &str) -> Result<String> {
        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS", // silent but show errors
            "-L",  // follow redirects
            "--max-time",
            "30",
            "-A",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        ]);

        if format == "json" {
            cmd.args(["-H", "Accept: application/json"]);
        } else {
            cmd.args(["-H", "Accept: text/html,application/xhtml+xml"]);
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

        String::from_utf8(output.stdout)
            .map_err(|e| OSAgentError::ToolExecution(format!("curl output invalid UTF-8: {}", e)))
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
        let mut request = self.client.get(url);

        if let Some(headers_obj) = headers {
            if let Some(obj) = headers_obj.as_object() {
                for (key, value) in obj {
                    if let Some(val_str) = value.as_str() {
                        request = request.header(key.as_str(), val_str);
                    }
                }
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to fetch URL: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "HTTP error: {}",
                status
            )));
        }

        response
            .text()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read response: {}", e)))
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
    pub fn new(_config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            service: SearchService::new(client),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo (no API key required)"
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
