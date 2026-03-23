use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct ArtifactStore {
    base_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactInfo {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub created_at: String,
}

impl ArtifactStore {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    pub fn init(&self) -> std::io::Result<()> {
        let workflows_dir = self.base_path.join("workflows");
        let runs_dir = self.base_path.join("runs");

        std::fs::create_dir_all(workflows_dir)?;
        std::fs::create_dir_all(runs_dir)?;

        Ok(())
    }

    pub fn store_workflow_version(
        &self,
        workflow_id: &str,
        version: i32,
        data: &[u8],
    ) -> std::io::Result<PathBuf> {
        let path = self
            .base_path
            .join("workflows")
            .join(workflow_id)
            .join(format!("v{}_graph.json", version));

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, data)?;
        Ok(path)
    }

    pub fn get_workflow_version(
        &self,
        workflow_id: &str,
        version: i32,
    ) -> std::io::Result<Option<Vec<u8>>> {
        let path = self
            .base_path
            .join("workflows")
            .join(workflow_id)
            .join(format!("v{}_graph.json", version));

        if path.exists() {
            Ok(Some(std::fs::read(path)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_workflow_versions(&self, workflow_id: &str) -> std::io::Result<Vec<ArtifactInfo>> {
        let dir = self.base_path.join("workflows").join(workflow_id);

        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut versions = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            if name.starts_with("v") && name.ends_with("_graph.json") {
                let metadata = entry.metadata()?;
                versions.push(ArtifactInfo {
                    name,
                    path: entry.path().to_string_lossy().to_string(),
                    size_bytes: metadata.len(),
                    created_at: format!(
                        "{:?}",
                        metadata
                            .created()
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    ),
                });
            }
        }

        Ok(versions)
    }

    pub fn store_run_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        filename: &str,
        data: &[u8],
    ) -> std::io::Result<PathBuf> {
        let path = self
            .base_path
            .join("runs")
            .join(run_id)
            .join("artifacts")
            .join(node_id)
            .join(filename);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, data)?;
        Ok(path)
    }

    pub fn store_run_metadata(&self, run_id: &str, metadata: &[u8]) -> std::io::Result<PathBuf> {
        let path = self
            .base_path
            .join("runs")
            .join(run_id)
            .join("metadata.json");

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, metadata)?;
        Ok(path)
    }

    pub fn store_node_output(
        &self,
        run_id: &str,
        node_id: &str,
        output: &[u8],
    ) -> std::io::Result<PathBuf> {
        let path = self
            .base_path
            .join("runs")
            .join(run_id)
            .join("node_outputs")
            .join(format!("{}.json", node_id));

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, output)?;
        Ok(path)
    }

    pub fn get_node_output(&self, run_id: &str, node_id: &str) -> std::io::Result<Option<Vec<u8>>> {
        let path = self
            .base_path
            .join("runs")
            .join(run_id)
            .join("node_outputs")
            .join(format!("{}.json", node_id));

        if path.exists() {
            Ok(Some(std::fs::read(path)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_run_artifacts(&self, run_id: &str) -> std::io::Result<Vec<ArtifactInfo>> {
        let dir = self.base_path.join("runs").join(run_id);

        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut artifacts = Vec::new();
        self.collect_artifacts_recursive(&dir, &mut artifacts)?;

        Ok(artifacts)
    }

    fn collect_artifacts_recursive(
        &self,
        dir: &PathBuf,
        artifacts: &mut Vec<ArtifactInfo>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.collect_artifacts_recursive(&path, artifacts)?;
            } else {
                let metadata = entry.metadata()?;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                artifacts.push(ArtifactInfo {
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes: metadata.len(),
                    created_at: format!(
                        "{:?}",
                        metadata
                            .created()
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    ),
                });
            }
        }

        Ok(())
    }

    pub fn cleanup_old_runs(&self, max_age_days: u64) -> std::io::Result<usize> {
        let runs_dir = self.base_path.join("runs");

        if !runs_dir.exists() {
            return Ok(0);
        }

        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(max_age_days * 24 * 60 * 60);

        let mut removed_count = 0;

        for entry in std::fs::read_dir(&runs_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(created) = metadata.created() {
                        if created < cutoff {
                            std::fs::remove_dir_all(&path)?;
                            removed_count += 1;
                        }
                    }
                }
            }
        }

        Ok(removed_count)
    }
}
