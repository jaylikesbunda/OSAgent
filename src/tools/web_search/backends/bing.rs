use crate::tools::web_search::types::{
    BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult,
};
use crate::tools::web_search::{
    fetch_search_page, is_probable_block_page, looks_like_no_results_page,
};
use async_trait::async_trait;
use base64::engine::{general_purpose::URL_SAFE, Engine};
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};

pub struct BingBackend;

#[async_trait]
impl SearchBackend for BingBackend {
    fn id(&self) -> &'static str {
        "bing"
    }

    fn priority(&self) -> u8 {
        15
    }

    fn min_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(2_500)
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>> {
        let url = format!(
            "https://www.bing.com/search?q={}&count={}&setlang=en",
            urlencoding::encode(&request.query),
            request.num_results.clamp(1, 20),
        );
        let html = fetch_search_page(
            client,
            &url,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1",
        )
        .await?;
        parse_bing_results(&html, request.num_results, self.id())
    }
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bing wraps result links as `/ck/a?...&u=a1<base64url>` where the payload
/// decodes straight to the destination URL. Plain `http(s)` hrefs pass
/// through; anything else (assets, javascript:) is dropped.
fn decode_bing_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        if host == "bing.com" || host.ends_with(".bing.com") {
            if parsed.path() == "/ck/a" {
                for (key, value) in parsed.query_pairs() {
                    if key == "u" {
                        let payload = value.strip_prefix("a1").unwrap_or(value.as_ref());
                        let padded = format!("{payload:=<width$}", width = payload.len().next_multiple_of(4));
                        if let Ok(decoded) = URL_SAFE.decode(padded.as_bytes()) {
                            if let Ok(url) = String::from_utf8(decoded) {
                                let url = url.trim().to_string();
                                if url.starts_with("http://") || url.starts_with("https://") {
                                    return Some(url);
                                }
                            }
                        }
                        return None;
                    }
                }
                return None;
            }
            return None;
        }
        return match parsed.scheme() {
            "http" | "https" => Some(trimmed.to_string()),
            _ => None,
        };
    }
    None
}

fn element_text(element: Option<ElementRef<'_>>) -> String {
    element
        .map(|node| clean_text(&node.text().collect::<String>()))
        .unwrap_or_default()
}

pub(crate) fn parse_bing_results(
    html: &str,
    max_results: usize,
    source: &str,
) -> BackendResult<Vec<SearchResult>> {
    if is_probable_block_page(html) {
        return Err(BackendError::blocked(
            "Bing returned a challenge or blocked page",
        ));
    }

    let document = Html::parse_document(html);
    let result_selector = Selector::parse("li.b_algo")
        .map_err(|e| BackendError::parse(format!("invalid result selector: {e:?}")))?;
    let link_selector = Selector::parse("h2 a[href]")
        .map_err(|e| BackendError::parse(format!("invalid link selector: {e:?}")))?;
    let snippet_selector = Selector::parse("div.b_caption p, div.b_caption, p")
        .map_err(|e| BackendError::parse(format!("invalid snippet selector: {e:?}")))?;

    let mut results = Vec::new();
    for element in document.select(&result_selector) {
        if results.len() >= max_results {
            break;
        }

        let Some(link) = element.select(&link_selector).next() else {
            continue;
        };
        let raw_url = link.value().attr("href").unwrap_or_default();
        let Some(url) = decode_bing_url(raw_url) else {
            continue;
        };

        let title = clean_text(&link.text().collect::<String>());
        if title.is_empty() {
            continue;
        }

        let snippet = element
            .select(&snippet_selector)
            .map(|node| clean_text(&node.text().collect::<String>()))
            .map(|text| {
                // `div.b_caption` text starts with the snippet paragraph; the
                // bare-`p` fallback can catch footer noise, so drop fragments
                // that merely repeat the title.
                if text == title {
                    String::new()
                } else {
                    text
                }
            })
            .find(|text| !text.is_empty())
            .unwrap_or_default();

        results.push(SearchResult {
            title,
            url,
            snippet,
            source: source.to_string(),
            position: results.len() + 1,
        });
    }

    if results.is_empty() {
        if looks_like_no_results_page(html) {
            return Err(BackendError::empty("Bing returned no results"));
        }
        return Err(BackendError::parse(
            "Bing markup did not contain recognizable results",
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::{decode_bing_url, parse_bing_results};

    #[test]
    fn decodes_ck_redirects() {
        assert_eq!(
            decode_bing_url("https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9kb2Nz"),
            Some("https://example.com/docs".to_string()),
        );
        assert_eq!(
            decode_bing_url("https://rust-lang.org/learn/async"),
            Some("https://rust-lang.org/learn/async".to_string()),
        );
        assert_eq!(decode_bing_url("https://www.bing.com/ck/a?!&&p=abc"), None);
        assert_eq!(
            decode_bing_url("https://r.bing.com/rp/abc123.gz.css"),
            None,
        );
    }

    #[test]
    fn parses_bing_results() {
        let html = r#"
        <ol id="b_results">
          <li class="b_algo">
            <h2><a href="https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9kb2Nz">Example Docs</a></h2>
            <div class="b_caption"><p>Read the docs to learn more.</p></div>
          </li>
          <li class="b_algo">
            <h2><a href="https://rust-lang.org/learn/async">Rust Async</a></h2>
            <div class="b_caption"><p>Async programming guide.</p></div>
          </li>
          <li class="b_ad">sponsored noise without h2 link</li>
        </ol>
        "#;

        let results = parse_bing_results(html, 5, "bing").expect("expected results");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example Docs");
        assert_eq!(results[0].url, "https://example.com/docs");
        assert_eq!(results[0].snippet, "Read the docs to learn more.");
        assert_eq!(results[1].url, "https://rust-lang.org/learn/async");
    }
}
