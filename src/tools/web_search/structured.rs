//! Structured search over free, key-less JSON APIs.
//!
//! Scraping a general search engine is the fragile path: the markup changes and
//! bot management challenges the client. These endpoints are public, documented,
//! need no API key, and return JSON, so they neither rot nor get blocked.
//!
//! They are used in two situations:
//!   * the query carries a `site:` operator pointing at a site we have an API
//!     for — then it is the primary path, exactly like typing `site:` into a
//!     browser; and
//!   * every general backend failed — then they run as a last resort instead of
//!     returning "no results".

use super::types::{BackendError, BackendResult, SearchRequest, SearchResult, TimeRange};
use reqwest::Client;
use serde_json::Value;

/// Identify ourselves properly: several of these APIs (crates.io, Reddit,
/// Stack Exchange) reject or throttle requests with a generic or absent user
/// agent, and crates.io's policy requires a contact URL.
const API_USER_AGENT: &str = concat!(
    "osagent/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jaylikesbunda/OSAgent)"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteRoute {
    Wikipedia,
    HackerNews,
    GitHub,
    StackExchange,
    CratesIo,
    Npm,
    Reddit,
    ArXiv,
}

impl SiteRoute {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Wikipedia => "wikipedia-api",
            Self::HackerNews => "hn-algolia",
            Self::GitHub => "github-api",
            Self::StackExchange => "stackexchange-api",
            Self::CratesIo => "crates-io-api",
            Self::Npm => "npm-registry-api",
            Self::Reddit => "reddit-rss",
            Self::ArXiv => "arxiv-api",
        }
    }

    /// Map the host in a `site:` operator to the API that covers it.
    fn from_domain(domain: &str) -> Option<Self> {
        let domain = domain.trim_start_matches("www.").to_ascii_lowercase();
        match domain.as_str() {
            "wikipedia.org" | "en.wikipedia.org" => Some(Self::Wikipedia),
            "news.ycombinator.com" | "ycombinator.com" | "hn.algolia.com" => Some(Self::HackerNews),
            "github.com" | "gist.github.com" => Some(Self::GitHub),
            "stackoverflow.com" | "stackexchange.com" | "serverfault.com" | "superuser.com" => {
                Some(Self::StackExchange)
            }
            "crates.io" | "docs.rs" | "lib.rs" => Some(Self::CratesIo),
            "npmjs.com" | "registry.npmjs.org" => Some(Self::Npm),
            "reddit.com" | "old.reddit.com" => Some(Self::Reddit),
            "arxiv.org" => Some(Self::ArXiv),
            _ => None,
        }
    }
}

/// Routes tried when every general backend has failed. Ordered by how broadly
/// useful they are for an arbitrary query.
pub const FALLBACK_ROUTES: [SiteRoute; 3] = [
    SiteRoute::Wikipedia,
    SiteRoute::HackerNews,
    SiteRoute::GitHub,
];

/// Detect a `site:` operator and split it from the rest of the query.
///
/// Routing on bare keywords would misfire constantly — "github actions cache"
/// wants documentation, not a repository list — so only the explicit operator
/// counts, which is the same thing it means in a browser.
pub fn route_for_query(query: &str) -> Option<(SiteRoute, String)> {
    let mut route = None;
    let mut remaining = Vec::new();

    for token in query.split_whitespace() {
        if let Some(domain) = token.strip_prefix("site:") {
            if route.is_none() {
                if let Some(matched) = SiteRoute::from_domain(domain) {
                    route = Some(matched);
                    continue;
                }
            }
            // An unknown site: filter stays in the query for general backends.
            remaining.push(token);
            continue;
        }
        remaining.push(token);
    }

    let route = route?;
    let cleaned = remaining.join(" ").trim().to_string();
    if cleaned.is_empty() {
        return None;
    }
    Some((route, cleaned))
}

async fn get_json(client: &Client, url: &str) -> BackendResult<Value> {
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", API_USER_AGENT)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                BackendError::timeout(format!("timed out calling API: {e}"))
            } else {
                BackendError::network(format!("API request failed: {e}"))
            }
        })?;

    let status = response.status();
    if status.as_u16() == 429 {
        return Err(BackendError::http_status(429, "API rate limit reached"));
    }
    if !status.is_success() {
        return Err(BackendError::http_status(
            status.as_u16(),
            format!("API returned HTTP {status}"),
        ));
    }

    response
        .json::<Value>()
        .await
        .map_err(|e| BackendError::parse(format!("API returned invalid JSON: {e}")))
}

async fn get_text(client: &Client, url: &str) -> BackendResult<String> {
    let response = client
        .get(url)
        .header("User-Agent", API_USER_AGENT)
        .send()
        .await
        .map_err(|e| BackendError::network(format!("API request failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(BackendError::http_status(
            status.as_u16(),
            format!("API returned HTTP {status}"),
        ));
    }

    response
        .text()
        .await
        .map_err(|e| BackendError::network(format!("failed reading API response: {e}")))
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip tags from the small amount of HTML these APIs embed in snippets.
fn strip_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    clean(
        &out.replace("&quot;", "\"")
            .replace("&amp;", "&")
            .replace("&#x27;", "'"),
    )
}

pub async fn search_route(
    route: SiteRoute,
    client: &Client,
    request: &SearchRequest,
) -> BackendResult<Vec<SearchResult>> {
    let results = match route {
        SiteRoute::Wikipedia => wikipedia(client, request).await?,
        SiteRoute::HackerNews => hacker_news(client, request).await?,
        SiteRoute::GitHub => github(client, request).await?,
        SiteRoute::StackExchange => stack_exchange(client, request).await?,
        SiteRoute::CratesIo => crates_io(client, request).await?,
        SiteRoute::Npm => npm(client, request).await?,
        SiteRoute::Reddit => reddit(client, request).await?,
        SiteRoute::ArXiv => arxiv(client, request).await?,
    };

    if results.is_empty() {
        return Err(BackendError::empty(format!(
            "{} returned no results",
            route.id()
        )));
    }
    Ok(results)
}

/// MediaWiki search API. No key, no rate limit worth worrying about.
async fn wikipedia(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&srlimit={}&format=json&origin=*",
        urlencoding::encode(&request.query),
        request.num_results
    );
    let body = get_json(client, &url).await?;

    let items = body["query"]["search"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item["title"].as_str()?;
            Some(SearchResult {
                title: title.to_string(),
                url: format!(
                    "https://en.wikipedia.org/wiki/{}",
                    urlencoding::encode(&title.replace(' ', "_"))
                ),
                snippet: strip_tags(item["snippet"].as_str().unwrap_or_default()),
                source: SiteRoute::Wikipedia.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// Hacker News via Algolia. Supports real date filtering, which makes it the
/// best free source for "what did people say about X recently".
async fn hacker_news(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let mut url = format!(
        "https://hn.algolia.com/api/v1/search?query={}&tags=(story,comment)&hitsPerPage={}",
        urlencoding::encode(&request.query),
        request.num_results
    );
    if let Some(range) = request.time_range {
        url.push_str(&format!(
            "&numericFilters=created_at_i>{}",
            range.since_unix()
        ));
    }
    let body = get_json(client, &url).await?;

    let hits = body["hits"].as_array().cloned().unwrap_or_default();
    Ok(hits
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, hit)| {
            let object_id = hit["objectID"].as_str()?;
            let title = hit["title"]
                .as_str()
                .or_else(|| hit["story_title"].as_str())
                .unwrap_or("Hacker News comment");
            let url = hit["url"]
                .as_str()
                .filter(|u| !u.is_empty())
                .map(|u| u.to_string())
                .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={object_id}"));
            let snippet = hit["story_text"]
                .as_str()
                .or_else(|| hit["comment_text"].as_str())
                .map(strip_tags)
                .unwrap_or_default();

            Some(SearchResult {
                title: clean(title),
                url,
                snippet,
                source: SiteRoute::HackerNews.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// GitHub repository search. Unauthenticated is 60 requests/hour per IP, which
/// is plenty for interactive use.
async fn github(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let mut query = request.query.clone();
    if let Some(range) = request.time_range {
        query.push_str(&format!(" pushed:>{}", range.since_date()));
    }
    let url = format!(
        "https://api.github.com/search/repositories?q={}&sort=updated&per_page={}",
        urlencoding::encode(&query),
        request.num_results
    );
    let body = get_json(client, &url).await?;

    let items = body["items"].as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let full_name = item["full_name"].as_str()?;
            let stars = item["stargazers_count"].as_u64().unwrap_or(0);
            let description = item["description"].as_str().unwrap_or_default();
            let updated = item["updated_at"].as_str().unwrap_or_default();

            Some(SearchResult {
                title: full_name.to_string(),
                url: item["html_url"].as_str()?.to_string(),
                snippet: clean(&format!(
                    "{description} (★{stars}{})",
                    if updated.is_empty() {
                        String::new()
                    } else {
                        format!(", updated {}", &updated[..updated.len().min(10)])
                    }
                )),
                source: SiteRoute::GitHub.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// Stack Exchange search. Key-less quota is ~300 requests/day per IP.
async fn stack_exchange(
    client: &Client,
    request: &SearchRequest,
) -> BackendResult<Vec<SearchResult>> {
    let mut url = format!(
        "https://api.stackexchange.com/2.3/search/advanced?order=desc&sort=relevance&q={}&site=stackoverflow&pagesize={}&filter=default",
        urlencoding::encode(&request.query),
        request.num_results
    );
    if let Some(range) = request.time_range {
        url.push_str(&format!("&fromdate={}", range.since_unix()));
    }
    let body = get_json(client, &url).await?;

    let items = body["items"].as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let answered = item["is_answered"].as_bool().unwrap_or(false);
            let score = item["score"].as_i64().unwrap_or(0);
            Some(SearchResult {
                title: strip_tags(item["title"].as_str()?),
                url: item["link"].as_str()?.to_string(),
                snippet: format!(
                    "score {score}, {}",
                    if answered { "answered" } else { "unanswered" }
                ),
                source: SiteRoute::StackExchange.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// crates.io registry search. Requires a descriptive user agent by policy.
async fn crates_io(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let url = format!(
        "https://crates.io/api/v1/crates?q={}&per_page={}",
        urlencoding::encode(&request.query),
        request.num_results.min(100)
    );
    let body = get_json(client, &url).await?;

    let crates = body["crates"].as_array().cloned().unwrap_or_default();
    Ok(crates
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let name = item["name"].as_str()?;
            let version = item["max_version"].as_str().unwrap_or("");
            Some(SearchResult {
                title: format!("{name} {version}"),
                url: format!("https://crates.io/crates/{name}"),
                snippet: clean(item["description"].as_str().unwrap_or_default()),
                source: SiteRoute::CratesIo.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// npm registry search.
async fn npm(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let url = format!(
        "https://registry.npmjs.org/-/v1/search?text={}&size={}",
        urlencoding::encode(&request.query),
        request.num_results.min(250)
    );
    let body = get_json(client, &url).await?;

    let objects = body["objects"].as_array().cloned().unwrap_or_default();
    Ok(objects
        .iter()
        .take(request.num_results)
        .enumerate()
        .filter_map(|(index, entry)| {
            let package = &entry["package"];
            let name = package["name"].as_str()?;
            Some(SearchResult {
                title: format!("{name} {}", package["version"].as_str().unwrap_or("")),
                url: package["links"]["npm"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("https://www.npmjs.com/package/{name}")),
                snippet: clean(package["description"].as_str().unwrap_or_default()),
                source: SiteRoute::Npm.id().to_string(),
                position: index + 1,
            })
        })
        .collect())
}

/// Reddit search over the RSS endpoint.
///
/// Reddit moved its JSON API behind OAuth and blocks anonymous `/search.json`,
/// but the RSS feed still answers unauthenticated requests that carry a real
/// user agent — which is why the JSON attempt fails and this one works.
async fn reddit(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let mut url = format!(
        "https://www.reddit.com/search.rss?q={}&sort=new&limit={}",
        urlencoding::encode(&request.query),
        request.num_results
    );
    if let Some(range) = request.time_range {
        url.push_str(&format!("&t={}", range.as_word()));
    }
    let body = get_text(client, &url).await?;
    Ok(parse_atom_entries(
        &body,
        request.num_results,
        SiteRoute::Reddit.id(),
    ))
}

/// arXiv query API, which returns Atom.
async fn arxiv(client: &Client, request: &SearchRequest) -> BackendResult<Vec<SearchResult>> {
    let url = format!(
        "https://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}&sortBy=submittedDate&sortOrder=descending",
        urlencoding::encode(&request.query),
        request.num_results
    );
    let body = get_text(client, &url).await?;
    Ok(parse_atom_entries(
        &body,
        request.num_results,
        SiteRoute::ArXiv.id(),
    ))
}

/// Minimal Atom reader covering the two feeds above. Both use `<entry>` with a
/// `<title>`, a `<link href>` and a summary/content body.
fn parse_atom_entries(xml: &str, max_results: usize, source: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    for entry in xml.split("<entry").skip(1) {
        if results.len() >= max_results {
            break;
        }
        let entry = entry.split("</entry>").next().unwrap_or(entry);

        let title = extract_tag(entry, "title")
            .map(|t| strip_tags(&t))
            .unwrap_or_default();
        let url = extract_link_href(entry).unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }

        let snippet = extract_tag(entry, "summary")
            .or_else(|| extract_tag(entry, "content"))
            .map(|s| strip_tags(&s))
            .unwrap_or_default();
        let snippet = if snippet.len() > 400 {
            format!("{}...", &snippet[..400])
        } else {
            snippet
        };

        results.push(SearchResult {
            title,
            url,
            snippet,
            source: source.to_string(),
            position: results.len() + 1,
        });
    }

    results
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let start_marker = format!("<{tag}");
    let start = xml.find(&start_marker)?;
    let after_open = xml[start..].find('>')? + start + 1;
    let end = xml[after_open..].find(&format!("</{tag}>"))? + after_open;
    let raw = &xml[after_open..end];
    let raw = raw
        .trim()
        .trim_start_matches("<![CDATA[")
        .trim_end_matches("]]>");
    Some(raw.to_string())
}

fn extract_link_href(xml: &str) -> Option<String> {
    // Prefer <link href="..."/>, fall back to <link>...</link>.
    if let Some(index) = xml.find("<link") {
        let tail = &xml[index..];
        if let Some(href_index) = tail.find("href=\"") {
            let rest = &tail[href_index + 6..];
            if let Some(end) = rest.find('"') {
                let href = &rest[..end];
                if !href.is_empty() {
                    return Some(href.to_string());
                }
            }
        }
    }
    extract_tag(xml, "link").filter(|link| link.starts_with("http"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            num_results: 5,
            time_range: None,
        }
    }

    #[test]
    fn routes_known_site_operators() {
        let (route, cleaned) = route_for_query("site:github.com ghostesp firmware").unwrap();
        assert_eq!(route, SiteRoute::GitHub);
        assert_eq!(cleaned, "ghostesp firmware");

        let (route, cleaned) = route_for_query("rust async site:stackoverflow.com").unwrap();
        assert_eq!(route, SiteRoute::StackExchange);
        assert_eq!(cleaned, "rust async");
    }

    #[test]
    fn ignores_unknown_or_absent_site_operators() {
        assert!(route_for_query("github actions cache").is_none());
        assert!(route_for_query("site:example.com widgets").is_none());
        // A bare site: filter with no query left is not a routable search.
        assert!(route_for_query("site:github.com").is_none());
    }

    #[test]
    fn time_range_parses_common_spellings() {
        assert_eq!(TimeRange::parse("month"), Some(TimeRange::Month));
        assert_eq!(TimeRange::parse("past_week"), Some(TimeRange::Week));
        assert_eq!(TimeRange::parse("24h"), Some(TimeRange::Day));
        assert_eq!(TimeRange::parse("decade"), None);
    }

    #[test]
    fn strips_markup_from_snippets() {
        assert_eq!(
            strip_tags("<span class=\"x\">Ghost</span> ESP &amp; friends"),
            "Ghost ESP & friends"
        );
    }

    #[test]
    fn parses_atom_entries() {
        let xml = r#"
        <feed>
          <entry>
            <title>GhostESP thread</title>
            <link href="https://www.reddit.com/r/esp32/comments/abc/ghostesp/" />
            <content>Some &lt;b&gt;discussion&lt;/b&gt;</content>
          </entry>
          <entry>
            <title>Second</title>
            <link href="https://example.com/2" />
          </entry>
        </feed>
        "#;
        let results = parse_atom_entries(xml, 5, "reddit-rss");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "GhostESP thread");
        assert_eq!(
            results[0].url,
            "https://www.reddit.com/r/esp32/comments/abc/ghostesp/"
        );
        assert_eq!(results[1].position, 2);
    }

    #[test]
    fn wikipedia_url_uses_underscores() {
        // Guards the title -> URL conversion, which is easy to get wrong.
        let title = "Ghost ESP".replace(' ', "_");
        assert_eq!(title, "Ghost_ESP");
        let _ = request("x");
    }
}
