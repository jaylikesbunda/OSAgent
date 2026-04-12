use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::warn;

struct FeedSource {
    name: &'static str,
    url: &'static str,
    category: &'static str,
}

const FEED_SOURCES: &[FeedSource] = &[
    FeedSource {
        name: "BBC World",
        url: "https://feeds.bbci.co.uk/news/world/rss.xml",
        category: "general",
    },
    FeedSource {
        name: "BBC Tech",
        url: "https://feeds.bbci.co.uk/news/technology/rss.xml",
        category: "technology",
    },
    FeedSource {
        name: "BBC Science",
        url: "https://feeds.bbci.co.uk/news/science_and_environment/rss.xml",
        category: "science",
    },
    FeedSource {
        name: "BBC Business",
        url: "https://feeds.bbci.co.uk/news/business/rss.xml",
        category: "business",
    },
    FeedSource {
        name: "Al Jazeera",
        url: "https://www.aljazeera.com/xml/rss/all.xml",
        category: "general",
    },
    FeedSource {
        name: "NPR",
        url: "https://feeds.npr.org/1001/rss.xml",
        category: "general",
    },
    FeedSource {
        name: "Google News",
        url: "https://news.google.com/rss",
        category: "general",
    },
];

struct Article {
    title: String,
    link: String,
    description: String,
    source: String,
    pub_date: Option<DateTime<Utc>>,
}

pub struct NewsTool {
    client: Client,
}

impl NewsTool {
    pub fn new(_config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("OSAgent/0.1")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    fn sources_for_category(category: &str) -> Vec<&FeedSource> {
        let cat_lower = category.to_lowercase();
        if cat_lower == "all" || cat_lower.is_empty() {
            return FEED_SOURCES.iter().collect();
        }
        let matched: Vec<&FeedSource> = FEED_SOURCES
            .iter()
            .filter(|s| s.category == cat_lower || s.category == "general")
            .collect();
        if matched.is_empty() {
            FEED_SOURCES.iter().collect()
        } else {
            matched
        }
    }

    fn google_news_url(topic: &str) -> String {
        if topic.is_empty() || topic == "*" {
            "https://news.google.com/rss".to_string()
        } else {
            format!(
                "https://news.google.com/rss/search?q={}",
                urlencoding::encode(topic)
            )
        }
    }

    fn extract_tag_text(fragment: &str, tag_name: &str) -> Option<String> {
        let open = format!("<{}>", tag_name);
        let close = format!("</{}>", tag_name);
        let start = fragment.find(&open)? + open.len();
        let rest = &fragment[start..];
        let end = rest.find(&close)?;
        let text: String = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
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

    fn parse_date(text: &str) -> Option<DateTime<Utc>> {
        let formats = [
            "%a, %d %b %Y %H:%M:%S %z",
            "%a, %d %b %Y %H:%M:%S GMT",
            "%Y-%m-%dT%H:%M:%S%:z",
            "%Y-%m-%dT%H:%M:%SZ",
            "%Y-%m-%dT%H:%M:%S%#z",
        ];
        for fmt in &formats {
            if let Ok(dt) = DateTime::parse_from_str(text.trim(), fmt) {
                return Some(dt.to_utc());
            }
        }
        None
    }

    fn parse_articles(feed_content: &str, source_name: &str) -> Vec<Article> {
        Self::extract_feed_blocks(feed_content)
            .into_iter()
            .filter_map(|block| {
                let title = Self::extract_tag_text(&block, "title")?;
                let link = Self::extract_feed_link(&block).unwrap_or_default();
                let description = Self::extract_tag_text(&block, "description")
                    .or_else(|| Self::extract_tag_text(&block, "summary"))
                    .or_else(|| Self::extract_tag_text(&block, "content"))
                    .unwrap_or_default();
                let pub_date = Self::extract_tag_text(&block, "pubDate")
                    .or_else(|| Self::extract_tag_text(&block, "published"))
                    .or_else(|| Self::extract_tag_text(&block, "updated"))
                    .and_then(|d| Self::parse_date(&d));
                Some(Article {
                    title,
                    link,
                    description,
                    source: source_name.to_string(),
                    pub_date,
                })
            })
            .collect()
    }

    fn matches_topic(article: &Article, topic: &str) -> bool {
        if topic.is_empty() || topic == "*" {
            return true;
        }
        let topic_lower = topic.to_lowercase();
        let search_text = format!("{} {}", article.title, article.description).to_lowercase();
        topic_lower
            .split_whitespace()
            .all(|word| search_text.contains(word))
    }

    fn format_time_ago(dt: &DateTime<Utc>) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(*dt);
        let minutes = diff.num_minutes();
        if minutes < 0 {
            return "just now".to_string();
        }
        if minutes < 60 {
            return format!("{}m ago", minutes);
        }
        let hours = diff.num_hours();
        if hours < 24 {
            return format!("{}h ago", hours);
        }
        let days = diff.num_days();
        format!("{}d ago", days)
    }

    fn render_articles(articles: &[Article], topic: &str) -> String {
        if articles.is_empty() {
            if topic.is_empty() || topic == "*" {
                return "No headlines found. The RSS feeds may be temporarily unavailable."
                    .to_string();
            }
            return format!(
                "No articles found matching '{}'. Try a broader topic or omit the topic parameter.",
                topic
            );
        }

        let now: DateTime<Local> = Local::now();
        let mut lines = vec![
            format!("Latest Headlines ({})", now.format("%Y-%m-%d %H:%M")),
            "─".repeat(50),
        ];

        if !topic.is_empty() && topic != "*" {
            lines.push(format!("Topic filter: {}", topic));
            lines.push(String::new());
        }

        for (i, article) in articles.iter().enumerate() {
            let time_str = article
                .pub_date
                .as_ref()
                .map(Self::format_time_ago)
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!("{}. [{}] {}", i + 1, article.source, article.title));
            if !article.link.is_empty() {
                lines.push(format!("   {} | {}", article.link, time_str));
            } else {
                lines.push(format!("   {}", time_str));
            }
            if !article.description.is_empty() {
                let desc: String = article.description.chars().take(200).collect();
                lines.push(format!("   {}", desc));
            }
            lines.push(String::new());
        }

        lines.push(format!(
            "Showing {} articles from {} sources",
            articles.len(),
            {
                let mut sources: Vec<&str> = articles.iter().map(|a| a.source.as_str()).collect();
                sources.sort();
                sources.dedup();
                sources.len()
            }
        ));

        lines.join("\n")
    }

    async fn fetch_feed(&self, source: &FeedSource) -> Result<Vec<Article>> {
        let response = self.client.get(source.url).send().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to fetch {} feed: {}", source.name, e))
        })?;

        if !response.status().is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "{} returned status {}",
                source.name,
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to read {} response: {}", source.name, e))
        })?;

        Ok(Self::parse_articles(&body, source.name))
    }

    async fn fetch_google_topic(&self, topic: &str) -> Result<Vec<Article>> {
        let url = Self::google_news_url(topic);
        let response = self.client.get(&url).send().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to fetch Google News feed: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "Google News returned status {}",
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to read Google News response: {}", e))
        })?;

        Ok(Self::parse_articles(&body, "Google News"))
    }
}

#[async_trait]
impl Tool for NewsTool {
    fn name(&self) -> &str {
        "news"
    }

    fn description(&self) -> &str {
        "Fetch latest news headlines from multiple RSS sources (BBC, Al Jazeera, NPR, Google News). Supports topic filtering and category selection."
    }

    fn when_to_use(&self) -> &str {
        "Use when the user asks for news, headlines, current events, or what's happening in the world. Prefer over web_search for general news queries."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for specific article lookups (use web_fetch), fact-checking, or non-news web searches (use web_search)."
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Get latest headlines".to_string(),
                input: json!({}),
            },
            ToolExample {
                description: "Get news about a specific topic".to_string(),
                input: json!({"topic": "AI"}),
            },
            ToolExample {
                description: "Get technology news".to_string(),
                input: json!({"category": "technology"}),
            },
            ToolExample {
                description: "Get news with more results".to_string(),
                input: json!({"topic": "climate", "max_results": 20}),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "Filter articles by topic/keyword (e.g. 'AI', 'climate change', 'sports'). Omit for all latest headlines."
                },
                "category": {
                    "type": "string",
                    "enum": ["general", "technology", "science", "business", "all"],
                    "description": "News category to fetch from. Default: 'general'. Use 'all' for all categories."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Maximum number of articles to return (default: 15)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let topic = args["topic"].as_str().unwrap_or("").trim().to_string();
        let category = args["category"]
            .as_str()
            .unwrap_or("general")
            .trim()
            .to_lowercase();
        let max_results = args["max_results"].as_u64().unwrap_or(15).min(50) as usize;

        let mut all_articles = Vec::new();

        if !topic.is_empty() && topic != "*" {
            match self.fetch_google_topic(&topic).await {
                Ok(articles) => all_articles.extend(articles),
                Err(e) => {
                    warn!("Google News topic fetch failed: {}", e);
                }
            }
        }

        if all_articles.len() < max_results {
            let sources = Self::sources_for_category(&category);
            let fetch_tasks: Vec<_> = sources
                .into_iter()
                .map(|source| self.fetch_feed(source))
                .collect();

            let results = futures::future::join_all(fetch_tasks).await;
            for result in results {
                match result {
                    Ok(articles) => all_articles.extend(articles),
                    Err(e) => {
                        warn!("Feed fetch failed: {}", e);
                    }
                }
            }
        }

        if !topic.is_empty() && topic != "*" {
            all_articles.retain(|a| Self::matches_topic(a, &topic));
        }

        all_articles.sort_by(|a, b| {
            let a_time = a.pub_date.unwrap_or(DateTime::UNIX_EPOCH);
            let b_time = b.pub_date.unwrap_or(DateTime::UNIX_EPOCH);
            b_time.cmp(&a_time)
        });

        let mut seen_titles = std::collections::HashSet::new();
        all_articles.retain(|a| {
            let normalized: String = a
                .title
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            seen_titles.insert(normalized)
        });

        all_articles.truncate(max_results);

        Ok(Self::render_articles(&all_articles, &topic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_articles() {
        let feed = r#"<?xml version="1.0"?>
        <rss><channel><title>Test</title>
        <item>
            <title>Breaking: Something happened</title>
            <link>https://example.com/1</link>
            <description>A description of the event</description>
            <pubDate>Mon, 07 Apr 2026 12:00:00 GMT</pubDate>
        </item>
        <item>
            <title>Another story</title>
            <link>https://example.com/2</link>
            <description>More news here</description>
        </item>
        </channel></rss>"#;

        let articles = NewsTool::parse_articles(feed, "Test Source");
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "Breaking: Something happened");
        assert_eq!(articles[0].source, "Test Source");
        assert!(articles[0].pub_date.is_some());
        assert!(articles[1].pub_date.is_none());
    }

    #[test]
    fn test_topic_matching() {
        let article = Article {
            title: "AI advances in healthcare".to_string(),
            link: String::new(),
            description: "New machine learning models help doctors".to_string(),
            source: "Test".to_string(),
            pub_date: None,
        };
        assert!(NewsTool::matches_topic(&article, "AI"));
        assert!(NewsTool::matches_topic(&article, "healthcare AI"));
        assert!(!NewsTool::matches_topic(&article, "sports"));
    }

    #[test]
    fn test_google_news_url() {
        assert_eq!(
            NewsTool::google_news_url("AI"),
            "https://news.google.com/rss/search?q=AI"
        );
        assert_eq!(NewsTool::google_news_url(""), "https://news.google.com/rss");
        assert_eq!(
            NewsTool::google_news_url("*"),
            "https://news.google.com/rss"
        );
    }

    #[test]
    fn test_sources_for_category() {
        let tech = NewsTool::sources_for_category("technology");
        assert!(tech.iter().any(|s| s.name == "BBC Tech"));

        let all = NewsTool::sources_for_category("all");
        assert_eq!(all.len(), FEED_SOURCES.len());
    }

    #[test]
    fn test_render_empty() {
        let rendered = NewsTool::render_articles(&[], "");
        assert!(rendered.contains("No headlines found"));
    }

    #[test]
    fn test_deduplication() {
        let articles = vec![
            Article {
                title: "Test Title".to_string(),
                link: "https://a.com".to_string(),
                description: String::new(),
                source: "A".to_string(),
                pub_date: None,
            },
            Article {
                title: "test title".to_string(),
                link: "https://b.com".to_string(),
                description: String::new(),
                source: "B".to_string(),
                pub_date: None,
            },
            Article {
                title: "Different".to_string(),
                link: "https://c.com".to_string(),
                description: String::new(),
                source: "C".to_string(),
                pub_date: None,
            },
        ];

        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<Article> = articles
            .into_iter()
            .filter(|a| {
                let normalized: String = a
                    .title
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect();
                seen.insert(normalized)
            })
            .collect();

        assert_eq!(deduped.len(), 2);
    }
}
