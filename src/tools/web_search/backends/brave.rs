use crate::tools::web_search::types::{
    BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult,
};
use crate::tools::web_search::{
    fetch_search_page, is_probable_block_page, looks_like_no_results_page,
};
use async_trait::async_trait;
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};

pub struct BraveBackend;

#[async_trait]
impl SearchBackend for BraveBackend {
    fn id(&self) -> &'static str {
        "brave"
    }

    fn priority(&self) -> u8 {
        10
    }

    fn min_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(20)
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(1_500)
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>> {
        let url = format!(
            "https://search.brave.com/search?q={}&source=web",
            urlencoding::encode(&request.query)
        );
        let html = fetch_search_page(
            client,
            &url,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1",
        )
        .await?;
        parse_brave_results(&html, request.num_results, self.id())
    }
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn direct_url(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(raw.to_string()),
        _ => None,
    }
}

fn element_text(element: Option<ElementRef<'_>>) -> String {
    element
        .map(|node| clean_text(&node.text().collect::<String>()))
        .unwrap_or_default()
}

fn extract_snippet(result: &ElementRef<'_>, title: &str) -> String {
    let text = clean_text(&result.text().collect::<String>());
    if text.is_empty() {
        return String::new();
    }

    let mut snippet = text;
    if let Some(rest) = snippet.strip_prefix(title) {
        snippet = rest.trim().to_string();
    }
    if let Some(index) = snippet.find("Description") {
        snippet = snippet[index + "Description".len()..].trim().to_string();
    }
    snippet
}

pub(crate) fn parse_brave_results(
    html: &str,
    max_results: usize,
    source: &str,
) -> BackendResult<Vec<SearchResult>> {
    if is_probable_block_page(html) {
        return Err(BackendError::blocked(
            "Brave Search returned a challenge or blocked page",
        ));
    }

    let document = Html::parse_document(html);
    let result_selector = Selector::parse("div.snippet[data-type=\"web\"]")
        .map_err(|e| BackendError::parse(format!("invalid result selector: {e:?}")))?;
    let link_selector = Selector::parse("div.result-content a[href^='http']")
        .map_err(|e| BackendError::parse(format!("invalid link selector: {e:?}")))?;
    let title_selector = Selector::parse(".title.search-snippet-title")
        .map_err(|e| BackendError::parse(format!("invalid title selector: {e:?}")))?;
    let url_selector = Selector::parse("cite.snippet-url")
        .map_err(|e| BackendError::parse(format!("invalid url selector: {e:?}")))?;

    let mut results = Vec::new();
    for element in document.select(&result_selector) {
        if results.len() >= max_results {
            break;
        }

        let Some(link) = element.select(&link_selector).next() else {
            continue;
        };
        let Some(url) = link.value().attr("href").and_then(direct_url) else {
            continue;
        };

        let title = element_text(element.select(&title_selector).next());
        if title.is_empty() {
            continue;
        }

        let display_url = element_text(element.select(&url_selector).next());
        let mut snippet = extract_snippet(&element, &title);
        if !display_url.is_empty() && snippet.starts_with(&display_url) {
            snippet = snippet[display_url.len()..].trim().to_string();
        }

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
            return Err(BackendError::empty("Brave Search returned no results"));
        }
        return Err(BackendError::parse(
            "Brave Search markup did not contain recognizable results",
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::parse_brave_results;

    #[test]
    fn parses_brave_results() {
        let html = r#"
        <div class="snippet" data-type="web">
          <div class="result-wrapper">
            <div class="result-content">
              <a href="https://example.com/docs">
                <cite class="snippet-url">example.com docs</cite>
                <div class="title search-snippet-title">Example Docs</div>
              </a>
              <div>Description Learn async Rust quickly.</div>
            </div>
          </div>
        </div>
        "#;

        let results = parse_brave_results(html, 5, "brave").expect("expected results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Docs");
        assert_eq!(results[0].url, "https://example.com/docs");
        assert_eq!(results[0].snippet, "Learn async Rust quickly.");
    }
}
