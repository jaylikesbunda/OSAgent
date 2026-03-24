use crate::tools::web_search::types::{
    BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult,
};
use crate::tools::web_search::{
    fetch_search_page, is_probable_block_page, looks_like_no_results_page,
};
use async_trait::async_trait;
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};

pub struct StartpageBackend;

#[async_trait]
impl SearchBackend for StartpageBackend {
    fn id(&self) -> &'static str {
        "startpage"
    }

    fn priority(&self) -> u8 {
        20
    }

    fn min_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(12)
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
            "https://www.startpage.com/sp/search?query={}&cat=web&segment=startpage.udog&page=1",
            urlencoding::encode(&request.query)
        );
        let html = fetch_search_page(
            client,
            &url,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1",
        )
        .await?;
        parse_startpage_results(&html, request.num_results, self.id())
    }
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_direct_url(link: &str) -> Option<String> {
    let parsed = Url::parse(link).ok()?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return None,
    }

    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.contains("startpage.com") && parsed.path().contains("/av/proxy") {
        return None;
    }

    Some(link.to_string())
}

fn first_text(element: Option<ElementRef<'_>>) -> String {
    element
        .map(|node| clean_text(&node.text().collect::<String>()))
        .unwrap_or_default()
}

pub(crate) fn parse_startpage_results(
    html: &str,
    max_results: usize,
    source: &str,
) -> BackendResult<Vec<SearchResult>> {
    if is_probable_block_page(html) {
        return Err(BackendError::blocked(
            "Startpage returned a challenge or blocked page",
        ));
    }

    let document = Html::parse_document(html);
    let result_selector = Selector::parse("div.w-gl div.result, section#main div.result")
        .map_err(|e| BackendError::parse(format!("invalid result selector: {e:?}")))?;
    let title_selector = Selector::parse("a.result-title.result-link[href]")
        .map_err(|e| BackendError::parse(format!("invalid title selector: {e:?}")))?;
    let snippet_selector = Selector::parse("p.description")
        .map_err(|e| BackendError::parse(format!("invalid snippet selector: {e:?}")))?;

    let mut results = Vec::new();
    for element in document.select(&result_selector) {
        if results.len() >= max_results {
            break;
        }

        let Some(link) = element.select(&title_selector).next() else {
            continue;
        };
        let Some(url) = link.value().attr("href").and_then(extract_direct_url) else {
            continue;
        };

        let title = clean_text(&link.text().collect::<String>());
        let snippet = first_text(element.select(&snippet_selector).next());
        if title.is_empty() {
            continue;
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
            return Err(BackendError::empty("Startpage returned no results"));
        }
        return Err(BackendError::parse(
            "Startpage markup did not contain recognizable results",
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::parse_startpage_results;

    #[test]
    fn parses_startpage_results() {
        let html = r#"
        <div class="w-gl">
          <div class="result">
            <a class="result-title result-link" href="https://example.com/docs">
              <h2 class="wgl-title">Example Docs</h2>
            </a>
            <p class="description">Read the docs</p>
          </div>
          <div class="result">
            <a class="result-title result-link" href="https://us2-browse.startpage.com/av/proxy?u=https://example.com/anon">
              Anonymous View
            </a>
          </div>
        </div>
        "#;

        let results = parse_startpage_results(html, 5, "startpage").expect("expected results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Docs");
        assert_eq!(results[0].url, "https://example.com/docs");
        assert_eq!(results[0].snippet, "Read the docs");
    }
}
