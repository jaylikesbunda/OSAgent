use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

pub struct SkillBundle;

impl SkillBundle {
    pub fn unpack(bundle_data: &[u8], target_dir: &PathBuf) -> std::io::Result<BundleManifest> {
        let reader = std::io::Cursor::new(bundle_data);
        let mut archive = ZipArchive::new(reader)?;

        let mut manifest: Option<BundleManifest> = None;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let raw_path = file.mangled_name();
            let outpath = target_dir.join(&raw_path);

            if file.name() == "manifest.toml" {
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                manifest = Some(
                    toml::from_str(&content)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
                );
            } else if outpath
                .parent()
                .map(|p| !p.as_os_str().is_empty())
                .unwrap_or(false)
            {
                let comment = file.comment();
                if !comment.is_empty() && comment.starts_with("icon:") {
                    continue;
                }

                if file.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else {
                    if let Some(parent) = outpath.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let mut outfile = fs::File::create(&outpath)?;
                    std::io::copy(&mut file, &mut outfile)?;
                }
            }
        }

        manifest.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No manifest.toml found in bundle",
            )
        })
    }

    pub fn pack(skill_dir: &PathBuf, output_path: &PathBuf) -> std::io::Result<BundleManifest> {
        let manifest_path = skill_dir.join("manifest.toml");
        if !manifest_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "manifest.toml",
            ));
        }

        let manifest_content = fs::read_to_string(&manifest_path)?;
        let manifest: BundleManifest = toml::from_str(&manifest_content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let file = fs::File::create(output_path)?;
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

        Self::add_dir_to_zip(&mut zip, skill_dir, skill_dir, &options)?;

        if let Some(icon_name) = &manifest.icon {
            let icon_path = skill_dir.join(icon_name);
            if icon_path.exists() {
                zip.start_file(icon_name, options)?;
                let icon_data = fs::read(&icon_path)?;
                zip.write_all(&icon_data)?;
            }
        }

        zip.finish()?;
        Ok(manifest)
    }

    fn add_dir_to_zip<W: Write + std::io::Seek>(
        zip: &mut ZipWriter<W>,
        base_path: &PathBuf,
        current_path: &PathBuf,
        options: &FileOptions,
    ) -> std::io::Result<()> {
        let entries = fs::read_dir(current_path)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(base_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

            if path.is_dir() {
                let dir_name = relative.to_string_lossy().to_string();
                if dir_name == ".git"
                    || dir_name.starts_with("target")
                    || dir_name == "node_modules"
                {
                    continue;
                }
                zip.add_directory(&dir_name, *options)?;
                Self::add_dir_to_zip(zip, base_path, &path, options)?;
            } else {
                let file_name = relative.to_string_lossy().replace('\\', "/");
                zip.start_file(&file_name, *options)?;
                let file_data = fs::read(&path)?;
                zip.write_all(&file_data)?;
            }
        }

        Ok(())
    }

    pub fn read_manifest(bundle_data: &[u8]) -> std::io::Result<BundleManifest> {
        let reader = std::io::Cursor::new(bundle_data);
        let mut archive = ZipArchive::new(reader)?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            if file.name() == "manifest.toml" {
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                return Ok(toml::from_str(&content)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?);
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No manifest.toml found in bundle",
        ))
    }

    pub fn validate(bundle_data: &[u8]) -> std::io::Result<()> {
        let manifest = Self::read_manifest(bundle_data)?;

        if manifest.name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Manifest name is required",
            ));
        }

        if manifest
            .name
            .chars()
            .any(|c| !c.is_alphanumeric() && c != '-' && c != '_')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Skill name can only contain alphanumeric characters, hyphens, and underscores",
            ));
        }

        Ok(())
    }
}

pub fn get_skills_base_dir() -> PathBuf {
    let data_dir = std::env::var("OSAGENT_DATA_DIR")
        .unwrap_or_else(|_| std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string()));
    PathBuf::from(data_dir).join("skills")
}

pub fn get_icons_base_dir() -> PathBuf {
    let data_dir = std::env::var("OSAGENT_DATA_DIR")
        .unwrap_or_else(|_| std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string()));
    PathBuf::from(data_dir).join("skills-icons")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_manifest_parsing() {
        let manifest = r#"
name = "spotify"
version = "1.0.0"
description = "Spotify controls"
author = "Test Author"
icon = "icon.png"
"#;
        let parsed: BundleManifest = toml::from_str(manifest).unwrap();
        assert_eq!(parsed.name, "spotify");
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.description, "Spotify controls");
    }
}
