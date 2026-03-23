use crate::tools::web_search::normalize::decode_duckduckgo_redirect;
use crate::tools::web_search::types::{
    BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult,
};
use crate::tools::web_search::{
    fetch_search_page, is_probable_block_page, looks_like_no_results_page,
};
use async_trait::async_trait;
use reqwest::{Client, Url};
use scraper::{Html, Selector};

pub struct DuckDuckGoLiteBackend;

#[async_trait]
impl SearchBackend for DuckDuckGoLiteBackend {
    fn id(&self) -> &'static str {
        "ddg_lite"
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>> {
        let url = format!(
            "https://lite.duckduckgo.com/lite/?q={}",
            urlencoding::encode(&request.query)
        );
        let html = fetch_search_page(
            client,
            &url,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1",
        )
        .await?;
        parse_lite_results(&html, request.num_results, self.id())
    }
}

fn looks_like_result_link(raw_url: &str, title: &str) -> bool {
    if title.trim().len() < 3 {
        return false;
    }

    let lowered = title.trim().to_ascii_lowercase();
    if [
        "next page",
        "next",
        "previous",
        "feedback",
        "help",
        "settings",
    ]
    .contains(&lowered.as_str())
    {
        return false;
    }

    let decoded = decode_duckduckgo_redirect(raw_url);
    let Ok(url) = Url::parse(&decoded) else {
        return false;
    };
    match url.scheme() {
        "http" | "https" => {}
        _ => return false,
    }

    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    !(host.contains("duckduckgo.com") || host.contains("duck.co"))
}

pub(crate) fn parse_lite_results(
    html: &str,
    max_results: usize,
    source: &str,
) -> BackendResult<Vec<SearchResult>> {
    if is_probable_block_page(html) {
        return Err(BackendError::blocked(
            "DuckDuckGo Lite returned a challenge or blocked page",
        ));
    }

    let document = Html::parse_document(html);
    let link_selector = Selector::parse("a[href]")
        .map_err(|e| BackendError::parse(format!("invalid link selector: {e:?}")))?;

    let mut results = Vec::new();
    for link in document.select(&link_selector) {
        if results.len() >= max_results {
            break;
        }

        let title = link.text().collect::<String>().trim().to_string();
        let raw_url = link.value().attr("href").unwrap_or_default();
        if !looks_like_result_link(raw_url, &title) {
            continue;
        }

        let url = decode_duckduckgo_redirect(raw_url);
        results.push(SearchResult {
            title,
            url,
            snippet: String::new(),
            source: source.to_string(),
            position: results.len() + 1,
        });
    }

    if results.is_empty() {
        if looks_like_no_results_page(html) {
            return Err(BackendError::empty("DuckDuckGo Lite returned no results"));
        }
        return Err(BackendError::parse(
            "DuckDuckGo Lite markup did not contain recognizable results",
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::parse_lite_results;

    #[test]
    fn parses_lite_results() {
        let html = r#"
        <table>
          <tr><td><a href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs">Example Docs</a></td></tr>
          <tr><td><a href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2Flearn%2Fasync">Rust Async</a></td></tr>
          <tr><td><a href="/html/">Next Page</a></td></tr>
        </table>
        "#;

        let results = parse_lite_results(html, 5, "ddg_lite").expect("expected results");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/docs");
        assert_eq!(results[1].title, "Rust Async");
    }
}
