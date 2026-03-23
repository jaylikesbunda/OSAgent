use crate::tools::web_search::normalize::decode_duckduckgo_redirect;
use crate::tools::web_search::types::{BackendError, BackendResult, SearchBackend, SearchRequest, SearchResult};
use crate::tools::web_search::{fetch_search_page, is_probable_block_page, looks_like_no_results_page};
use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};

pub struct DuckDuckGoHtmlBackend;

#[async_trait]
impl SearchBackend for DuckDuckGoHtmlBackend {
    fn id(&self) -> &'static str {
        "ddg_html"
    }

    async fn search(
        &self,
        client: &Client,
        request: &SearchRequest,
    ) -> BackendResult<Vec<SearchResult>> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&request.query)
        );
        let html = fetch_search_page(client, &url, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1").await?;
        parse_html_results(&html, request.num_results, self.id())
    }
}

pub(crate) fn parse_html_results(
    html: &str,
    max_results: usize,
    source: &str,
) -> BackendResult<Vec<SearchResult>> {
    if is_probable_block_page(html) {
        return Err(BackendError::blocked(
            "DuckDuckGo HTML returned a challenge or blocked page",
        ));
    }

    let document = Html::parse_document(html);
    let result_selector = Selector::parse(".result, .results_links_deep")
        .map_err(|e| BackendError::parse(format!("invalid result selector: {e:?}")))?;
    let link_selector = Selector::parse(".result__a, a.result__a")
        .map_err(|e| BackendError::parse(format!("invalid link selector: {e:?}")))?;
    let snippet_selector = Selector::parse(".result__snippet, .result__body")
        .map_err(|e| BackendError::parse(format!("invalid snippet selector: {e:?}")))?;

    let mut results = Vec::new();
    for element in document.select(&result_selector) {
        if results.len() >= max_results {
            break;
        }

        let Some(link) = element.select(&link_selector).next() else {
            continue;
        };

        let title = link.text().collect::<String>().trim().to_string();
        let raw_url = link.value().attr("href").unwrap_or_default();
        let url = decode_duckduckgo_redirect(raw_url);
        let snippet = element
            .select(&snippet_selector)
            .next()
            .map(|snippet| snippet.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if title.is_empty() || url.is_empty() {
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
            return Err(BackendError::empty("DuckDuckGo HTML returned no results"));
        }
        return Err(BackendError::parse(
            "DuckDuckGo HTML markup did not contain recognizable results",
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::parse_html_results;

    #[test]
    fn parses_html_results() {
        let html = r#"
        <div class="result">
          <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs">Example Docs</a>
          <a class="result__snippet">Read the docs</a>
        </div>
        <div class="result">
          <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2Flearn">Rust Learn</a>
          <a class="result__snippet">Language docs</a>
        </div>
        "#;

        let results = parse_html_results(html, 5, "ddg_html").expect("expected results");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example Docs");
        assert_eq!(results[0].url, "https://example.com/docs");
        assert_eq!(results[0].snippet, "Read the docs");
    }
}
