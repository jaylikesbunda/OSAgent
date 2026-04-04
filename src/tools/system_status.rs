use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;
use sysinfo::{Disks, System};

pub struct SystemStatusTool;

impl SystemStatusTool {
    pub fn new(_config: Config) -> Self {
        Self
    }

    fn format_bytes(bytes: u64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"];
        let mut value = bytes as f64;
        let mut index = 0usize;
        while value >= 1024.0 && index < units.len() - 1 {
            value /= 1024.0;
            index += 1;
        }
        if index == 0 {
            format!("{} {}", bytes, units[index])
        } else {
            format!("{:.1} {}", value, units[index])
        }
    }

    fn format_duration(secs: u64) -> String {
        let days = secs / 86_400;
        let hours = (secs % 86_400) / 3_600;
        let minutes = (secs % 3_600) / 60;
        if days > 0 {
            format!("{}d {}h {}m", days, hours, minutes)
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }

    fn collect() -> String {
        let mut system = System::new_all();
        system.refresh_all();
        thread::sleep(Duration::from_millis(200));
        system.refresh_cpu_usage();
        system.refresh_memory();

        let host = System::host_name().unwrap_or_else(|| "Unknown".to_string());
        let os = System::long_os_version()
            .or_else(System::name)
            .unwrap_or_else(|| "Unknown OS".to_string());
        let kernel = System::kernel_version().unwrap_or_else(|| "Unknown".to_string());
        let uptime = System::uptime();
        let cpu_usage = if system.cpus().is_empty() {
            0.0
        } else {
            system.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / system.cpus().len() as f32
        };
        let cpu_model = system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().to_string())
            .unwrap_or_else(|| "Unknown CPU".to_string());

        let total_memory = system.total_memory();
        let used_memory = system.used_memory();
        let total_swap = system.total_swap();
        let used_swap = system.used_swap();

        let disks = Disks::new_with_refreshed_list();
        let total_disk = disks.iter().map(|disk| disk.total_space()).sum::<u64>();
        let available_disk = disks.iter().map(|disk| disk.available_space()).sum::<u64>();
        let used_disk = total_disk.saturating_sub(available_disk);

        let mut lines = vec![
            format!("Host: {}", host),
            format!("OS: {}", os),
            format!("Kernel: {}", kernel),
            format!("Uptime: {}", Self::format_duration(uptime)),
            format!("CPU: {} ({} cores, {:.1}% avg usage)", cpu_model, system.cpus().len(), cpu_usage),
            format!(
                "Memory: {} / {}",
                Self::format_bytes(used_memory),
                Self::format_bytes(total_memory)
            ),
        ];

        if total_swap > 0 {
            lines.push(format!(
                "Swap: {} / {}",
                Self::format_bytes(used_swap),
                Self::format_bytes(total_swap)
            ));
        }

        if total_disk > 0 {
            lines.push(format!(
                "Disk: {} / {} used",
                Self::format_bytes(used_disk),
                Self::format_bytes(total_disk)
            ));
        }

        if !disks.is_empty() {
            lines.push("Volumes:".to_string());
            for disk in disks.iter().take(5) {
                lines.push(format!(
                    "{}: {} free of {}",
                    disk.mount_point().display(),
                    Self::format_bytes(disk.available_space()),
                    Self::format_bytes(disk.total_space())
                ));
            }
        }

        lines.join("\n")
    }
}

#[async_trait]
impl Tool for SystemStatusTool {
    fn name(&self) -> &str {
        "system_status"
    }

    fn description(&self) -> &str {
        "Show current machine status including OS, uptime, CPU, memory, swap, and disk usage"
    }

    fn when_to_use(&self) -> &str {
        "Use when the user asks about machine health, resources, storage, uptime, or general computer status"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for process management or network/web lookups"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "Get a machine summary".to_string(),
            input: json!({}),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let report = tokio::task::spawn_blocking(Self::collect)
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to collect system status: {}", e)))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(SystemStatusTool::format_bytes(1024), "1.0 KB");
    }

    #[tokio::test]
    async fn returns_non_empty_report() {
        let tool = SystemStatusTool::new(Config::default());
        let report = tool.execute(json!({})).await.unwrap();
        assert!(report.contains("Host:"));
        assert!(report.contains("Memory:"));
    }
}
