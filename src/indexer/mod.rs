pub mod tokenizer;

use meilisearch_sdk::client::Client;
use meilisearch_sdk::indexes::Index;
use meilisearch_sdk::tasks::Task;
use serde::{Deserialize, Serialize};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};
use walkdir::WalkDir;

use crate::error::{OSAgentError, Result};

const INDEX_NAME: &str = "code_chunks";
const MAX_CHUNK_SIZE: usize = 2000;
const MEILISEARCH_PORT: u16 = 7700;
const MEILISEARCH_HOST: &str = "http://127.0.0.1:7700";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub id: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub tokens: String,
    pub language: String,
}

pub struct CodeIndexer {
    client: Client,
    index: Index,
    workspace: PathBuf,
    #[allow(dead_code)]
    meilisearch_process: Option<Arc<tokio::sync::Mutex<Child>>>,
}

impl CodeIndexer {
    pub async fn new(workspace: PathBuf) -> Result<Self> {
        let meilisearch_process = Self::spawn_meilisearch().await?;

        let client = Client::new(MEILISEARCH_HOST, None::<String>).map_err(|e| {
            OSAgentError::Config(format!("Failed to create MeiliSearch client: {}", e))
        })?;

        let index = client.index(INDEX_NAME);

        Self::wait_for_meilisearch(&client).await?;

        let settings_task = index
            .set_filterable_attributes(["file_path", "language"])
            .await
            .map_err(|e| {
                OSAgentError::Config(format!("Failed to set filterable attributes: {}", e))
            })?;

        let settings_result = settings_task
            .wait_for_completion(
                &client,
                Some(Duration::from_millis(100)),
                Some(Duration::from_secs(30)),
            )
            .await
            .map_err(|e| OSAgentError::Config(format!("Settings task did not complete: {}", e)))?;

        if let Task::Failed { content } = settings_result {
            return Err(OSAgentError::Config(format!(
                "Failed to configure index settings: {}",
                content.error.error_message
            )));
        }

        info!("MeiliSearch initialized at {}", MEILISEARCH_HOST);

        Ok(Self {
            client,
            index,
            workspace,
            meilisearch_process: meilisearch_process.map(|p| Arc::new(tokio::sync::Mutex::new(p))),
        })
    }

    async fn spawn_meilisearch() -> Result<Option<Child>> {
        if TcpStream::connect(format!("127.0.0.1:{}", MEILISEARCH_PORT)).is_ok() {
            info!(
                "MeiliSearch already running on port {}, reusing existing instance",
                MEILISEARCH_PORT
            );
            return Ok(None);
        }

        let meilisearch_path = Self::find_meilisearch_binary()?;

        let data_dir = if let Some(home) = std::env::var_os("USERPROFILE") {
            PathBuf::from(home)
                .join(".osagent")
                .join("search")
                .join("data")
        } else {
            std::env::temp_dir().join("osagent_meilisearch")
        };

        std::fs::create_dir_all(&data_dir).map_err(|e| {
            OSAgentError::Config(format!("Failed to create MeiliSearch data dir: {}", e))
        })?;

        info!("Spawning MeiliSearch with data dir: {:?}", data_dir);

        let child = Command::new(&meilisearch_path)
            .arg("--http-addr")
            .arg(format!("127.0.0.1:{}", MEILISEARCH_PORT))
            .arg("--db-path")
            .arg(&data_dir)
            .arg("--no-analytics")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| OSAgentError::Config(format!("Failed to spawn MeiliSearch: {}", e)))?;

        Ok(Some(child))
    }

    fn find_meilisearch_binary() -> Result<PathBuf> {
        let candidates = ["meilisearch", "meilisearch.exe"];

        for candidate in candidates {
            if let Ok(path) = which::which(candidate) {
                return Ok(path);
            }
        }

        if let Some(home) = std::env::var_os("USERPROFILE") {
            let osagent_search = PathBuf::from(home)
                .join(".osagent")
                .join("search")
                .join("meilisearch.exe");
            if osagent_search.exists() {
                return Ok(osagent_search);
            }
        }

        if let Some(home) = std::env::var_os("HOME") {
            let osagent_search = PathBuf::from(home)
                .join(".osagent")
                .join("search")
                .join("meilisearch");
            if osagent_search.exists() {
                return Ok(osagent_search);
            }
        }

        let common_paths = [
            "/usr/local/bin/meilisearch",
            "/usr/bin/meilisearch",
            "/opt/homebrew/bin/meilisearch",
            "C:\\Program Files\\MeiliSearch\\meilisearch.exe",
        ];

        for path in common_paths {
            if Path::new(path).exists() {
                return Ok(PathBuf::from(path));
            }
        }

        Err(OSAgentError::Config(
            "MeiliSearch binary not found. Please install MeiliSearch and ensure it's in PATH."
                .to_string(),
        ))
    }

    async fn wait_for_meilisearch(client: &Client) -> Result<()> {
        for attempt in 0..30 {
            match client.get_stats().await {
                Ok(_) => {
                    info!("MeiliSearch is ready");
                    return Ok(());
                }
                Err(_) => {
                    if attempt < 29 {
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        Err(OSAgentError::Config(
            "MeiliSearch did not start within 15 seconds".to_string(),
        ))
    }

    pub async fn index_workspace(&self) -> Result<usize> {
        if let Ok(stats) = self.index.get_stats().await {
            if !stats.is_indexing && stats.number_of_documents > 0 {
                info!(
                    "Index already has {} documents, skipping re-indexing",
                    stats.number_of_documents
                );
                return Ok(stats.number_of_documents);
            }
        }

        info!("Indexing workspace: {:?}", self.workspace);

        let mut chunks: Vec<CodeChunk> = Vec::new();
        let mut chunk_id = 0u64;

        for entry in WalkDir::new(&self.workspace)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();

            if !Self::should_index(path) {
                continue;
            }

            let relative_path = path
                .strip_prefix(&self.workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let language = Self::detect_language(path);

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let file_chunks = Self::chunk_file(&content, &relative_path, &language, &mut chunk_id);
            chunks.extend(file_chunks);

            if chunks.len() >= 1000 {
                self.index_chunks(&chunks).await?;
                chunks.clear();
            }
        }

        if !chunks.is_empty() {
            self.index_chunks(&chunks).await?;
        }

        info!("Indexed {} chunks", chunk_id);
        Ok(chunk_id as usize)
    }

    fn should_index(path: &Path) -> bool {
        let skip_dirs = [
            "node_modules",
            "target",
            "build",
            "dist",
            ".git",
            ".idea",
            ".vscode",
            "__pycache__",
            ".cache",
            "vendor",
            ".venv",
            "venv",
        ];

        for component in path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if skip_dirs.contains(&name) {
                    return false;
                }
            }
        }

        let extensions = [
            "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "kt", "swift", "c", "cpp", "h",
            "hpp", "cs", "rb", "php", "scala", "lua", "vim", "sh", "bash", "zsh", "fish", "ps1",
            "bat", "json", "yaml", "yml", "toml", "xml", "ini", "cfg", "conf", "md", "rst", "txt",
            "org", "html", "css", "scss", "sass", "less", "vue", "svelte", "sql", "graphql",
        ];

        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            extensions.contains(&ext)
        } else {
            false
        }
    }

    fn detect_language(path: &Path) -> String {
        let ext_to_lang = [
            ("rs", "rust"),
            ("py", "python"),
            ("js", "javascript"),
            ("ts", "typescript"),
            ("jsx", "javascript"),
            ("tsx", "typescript"),
            ("go", "go"),
            ("java", "java"),
            ("kt", "kotlin"),
            ("swift", "swift"),
            ("c", "c"),
            ("cpp", "cpp"),
            ("h", "c"),
            ("hpp", "cpp"),
            ("cs", "csharp"),
            ("rb", "ruby"),
            ("php", "php"),
            ("scala", "scala"),
            ("sh", "bash"),
            ("bash", "bash"),
            ("zsh", "zsh"),
            ("html", "html"),
            ("css", "css"),
            ("scss", "scss"),
            ("json", "json"),
            ("yaml", "yaml"),
            ("yml", "yaml"),
            ("toml", "toml"),
            ("md", "markdown"),
            ("sql", "sql"),
        ];

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        ext_to_lang
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, lang)| lang.to_string())
            .unwrap_or_else(|| ext.to_string())
    }

    fn sanitize_id(file_path: &str, chunk_id: &u64) -> String {
        let sanitized = file_path.replace(['\\', '/', '.'], "_");
        format!("{}-{}", sanitized, chunk_id)
    }

    fn chunk_file(
        content: &str,
        file_path: &str,
        language: &str,
        chunk_id: &mut u64,
    ) -> Vec<CodeChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut current_start = 0;
        let mut current_content = String::new();

        for (i, line) in lines.iter().enumerate() {
            if current_content.len() + line.len() > MAX_CHUNK_SIZE && !current_content.is_empty() {
                let tokens = tokenizer::tokenize_code(&current_content);

                chunks.push(CodeChunk {
                    id: Self::sanitize_id(file_path, chunk_id),
                    file_path: file_path.to_string(),
                    start_line: current_start + 1,
                    end_line: i,
                    content: current_content.clone(),
                    tokens: tokens.join(" "),
                    language: language.to_string(),
                });

                *chunk_id += 1;
                current_start = i;
                current_content.clear();
            }

            current_content.push_str(line);
            current_content.push('\n');
        }

        if !current_content.is_empty() {
            let tokens = tokenizer::tokenize_code(&current_content);

            chunks.push(CodeChunk {
                id: Self::sanitize_id(file_path, chunk_id),
                file_path: file_path.to_string(),
                start_line: current_start + 1,
                end_line: lines.len(),
                content: current_content.clone(),
                tokens: tokens.join(" "),
                language: language.to_string(),
            });

            *chunk_id += 1;
        }

        chunks
    }

    async fn index_chunks(&self, chunks: &[CodeChunk]) -> Result<()> {
        let task_info = self
            .index
            .add_documents(chunks, None)
            .await
            .map_err(|e| OSAgentError::Config(format!("Failed to index chunks: {}", e)))?;

        let completed = task_info
            .wait_for_completion(
                &self.client,
                Some(Duration::from_millis(100)),
                Some(Duration::from_secs(30)),
            )
            .await
            .map_err(|e| OSAgentError::Config(format!("Task did not complete: {}", e)))?;

        if let Task::Failed { content } = completed {
            return Err(OSAgentError::Config(format!(
                "Indexing task failed: {}",
                content.error.error_message
            )));
        }

        Ok(())
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let tokenized_query = tokenizer::tokenize_query(query);

        let results = self
            .index
            .search()
            .with_query(&tokenized_query)
            .with_limit(limit)
            .build()
            .execute::<CodeChunk>()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("Search failed: {}", e)))?;

        let search_results: Vec<SearchResult> = results
            .hits
            .into_iter()
            .map(|hit| SearchResult {
                file_path: hit.result.file_path,
                start_line: hit.result.start_line,
                end_line: hit.result.end_line,
                content: hit.result.content,
                language: hit.result.language,
                score: hit.ranking_score.map(|s| s),
            })
            .collect();

        Ok(search_results)
    }

    pub async fn search_with_language(
        &self,
        query: &str,
        language: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let tokenized_query = tokenizer::tokenize_query(query);

        let results = self
            .index
            .search()
            .with_query(&tokenized_query)
            .with_filter(&format!("language = \"{}\"", language))
            .with_limit(limit)
            .build()
            .execute::<CodeChunk>()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("Search failed: {}", e)))?;

        let search_results: Vec<SearchResult> = results
            .hits
            .into_iter()
            .map(|hit| SearchResult {
                file_path: hit.result.file_path,
                start_line: hit.result.start_line,
                end_line: hit.result.end_line,
                content: hit.result.content,
                language: hit.result.language,
                score: hit.ranking_score.map(|s| s),
            })
            .collect();

        Ok(search_results)
    }

    #[allow(dead_code)]
    pub async fn stop(&self) -> Result<()> {
        if let Some(ref process) = self.meilisearch_process {
            let mut proc = process.lock().await;
            if let Err(e) = proc.kill() {
                warn!("Failed to kill MeiliSearch process: {}", e);
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn get_stats(&self) -> Result<IndexStats> {
        let stats = self.index.get_stats().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to get index stats: {}", e))
        })?;

        info!(
            "MeiliSearch index stats: {} documents, is_indexing: {}",
            stats.number_of_documents, stats.is_indexing
        );

        Ok(IndexStats {
            number_of_documents: stats.number_of_documents as u64,
            is_indexing: stats.is_indexing,
        })
    }

    pub async fn ensure_index_ready(&self) -> Result<()> {
        for attempt in 0..30 {
            match self.index.get_stats().await {
                Ok(stats) => {
                    if !stats.is_indexing && stats.number_of_documents > 0 {
                        info!(
                            "Index is ready with {} documents",
                            stats.number_of_documents
                        );
                        return Ok(());
                    }
                    if stats.is_indexing {
                        info!(
                            "Index still indexing (attempt {}/30), waiting...",
                            attempt + 1
                        );
                    } else if stats.number_of_documents == 0 {
                        info!(
                            "Index has 0 documents (attempt {}/30), waiting...",
                            attempt + 1
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Error getting index stats (attempt {}/30): {}",
                        attempt + 1,
                        e
                    );
                }
            }
            sleep(Duration::from_secs(1)).await;
        }
        Err(OSAgentError::ToolExecution(
            "Index did not become ready in time".to_string(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub language: String,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub number_of_documents: u64,
    pub is_indexing: bool,
}
