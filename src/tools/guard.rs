use crate::error::{OSAgentError, Result};
use std::path::Path;

pub const BACKUP_DIR_NAME: &str = ".osagent_backups";

pub fn relative_path_touches_backups(path: &str) -> bool {
    path.replace('\\', "/")
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(BACKUP_DIR_NAME))
}

pub fn path_touches_backups(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(BACKUP_DIR_NAME)
    })
}

pub fn ensure_relative_path_not_backups(path: &str) -> Result<()> {
    if relative_path_touches_backups(path) {
        return Err(OSAgentError::ToolExecution(
            "Access to backup files and .osagent_backups is blocked".to_string(),
        ));
    }

    Ok(())
}

pub fn command_touches_backups(command: &str) -> bool {
    command
        .to_ascii_lowercase()
        .contains(&BACKUP_DIR_NAME.to_ascii_lowercase())
}
