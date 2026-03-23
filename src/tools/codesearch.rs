use crate::error::{OSAgentError, Result};
use crate::indexer::{CodeIndexer, SearchResult};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

pub struct CodeSearchTool {
    indexer: Arc<CodeIndexer>,
}

impl CodeSearchTool {
    pub fn new(indexer: Arc<CodeIndexer>) -> Self {
        Self { indexer }
    }
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "codesearch"
    }

    fn description(&self) -> &str {
        "Semantic code search using BM25. Searches across all indexed code files in the workspace. Better than grep for finding code by meaning/intent, function names, variable names, and concepts."
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to find code by concept, function name patterns (camelCase, snake_case), or when grep is too literal. Good for 'find authentication code' or 'where is error handling'"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for exact literal string matches (use grep instead), or when you need to search non-code files"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Search for authentication-related code".to_string(),
                input: json!({
                    "query": "authentication login user session",
                    "limit": 10
                }),
            },
            ToolExample {
                description: "Search for specific function in Python files".to_string(),
                input: json!({
                    "query": "process_data transform",
                    "language": "python",
                    "limit": 5
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Can include function names, variable names, concepts, or keywords. camelCase and snake_case are automatically handled."
                },
                "language": {
                    "type": "string",
                    "description": "Filter by programming language (e.g., 'rust', 'python', 'javascript', 'typescript')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 50)",
                    "default": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'query' parameter".to_string()))?;

        let limit = args["limit"].as_u64().unwrap_or(10).min(50) as usize;
        let language = args["language"].as_str();

        let stats = self.indexer.get_stats().await;
        let is_indexed = stats
            .as_ref()
            .map(|s| s.number_of_documents > 0)
            .unwrap_or(false);
        let is_indexing = stats.as_ref().map(|s| s.is_indexing).unwrap_or(false);

        if !is_indexed && !is_indexing {
            return Ok("Code search index is empty or MeiliSearch is not running. \
                The workspace may not have been indexed yet, or MeiliSearch failed to start. \
                Check that MeiliSearch is installed and the search feature is enabled in config."
                .to_string());
        }

        if is_indexing {
            return Ok("Indexing in progress. Please wait a moment and try again.".to_string());
        }

        if let Err(e) = self.indexer.ensure_index_ready().await {
            warn!("Index not ready: {}", e);
            return Ok(
                "Index is still being prepared. Please wait a moment and try again.".to_string(),
            );
        }

        let results: Vec<SearchResult> = if let Some(lang) = language {
            self.indexer
                .search_with_language(query, lang, limit)
                .await?
        } else {
            self.indexer.search(query, limit).await?
        };

        if results.is_empty() {
            return Ok("No results found. Try different keywords or check if the files contain your search terms.".to_string());
        }

        let mut output = String::new();
        output.push_str(&format!("Found {} results:\n\n", results.len()));

        for result in results {
            output.push_str(&format!(
                "**{}** ({}:{}-{}) [{}]\n",
                result.file_path,
                result.file_path,
                result.start_line,
                result.end_line,
                result.language
            ));

            if let Some(score) = result.score {
                output.push_str(&format!("Score: {:.2}\n", score));
            }

            let content_preview = result
                .content
                .lines()
                .take(10)
                .collect::<Vec<_>>()
                .join("\n");
            output.push_str(&format!("```\n{}\n```\n\n", content_preview));
        }

        Ok(output)
    }
}
