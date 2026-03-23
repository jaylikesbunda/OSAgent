use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let snapshot_path = Path::new(&out_dir).join("models_snapshot.json");
    let target_path = Path::new("src/agent/models_snapshot.json");

    let snapshot_exists = if snapshot_path.exists() {
        let metadata = fs::metadata(&snapshot_path).unwrap();
        let age = metadata.modified().unwrap().elapsed().unwrap();
        age.as_secs() < 24 * 3600
    } else {
        false
    };

    if !snapshot_exists {
        let mut data = String::new();
        let fetch_success = Command::new("curl")
            .args([
                "-sL",
                "--connect-timeout",
                "10",
                "--max-time",
                "30",
                "https://models.dev/api.json",
            ])
            .output()
            .map(|o| {
                if o.status.success() {
                    let body = String::from_utf8_lossy(&o.stdout);
                    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&body);
                    if parsed.is_ok() {
                        data = body.to_string();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .unwrap_or(false);

        if fetch_success && !data.is_empty() {
            if let Err(e) = fs::write(&snapshot_path, &data) {
                eprintln!("build.rs: failed to write snapshot to OUT_DIR: {}", e);
            }
            if let Err(e) = fs::write(target_path, &data) {
                eprintln!("build.rs: failed to write snapshot to src: {}", e);
            }
        } else if target_path.exists() {
            eprintln!("build.rs: network fetch failed, using existing snapshot");
        } else {
            eprintln!("build.rs: network fetch failed and no existing snapshot found");
            let empty = "{}";
            fs::write(target_path, empty).ok();
            fs::write(&snapshot_path, empty).ok();
        }
    } else if let Ok(existing) = fs::read(&snapshot_path) {
        fs::write(target_path, &existing).ok();
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/agent/models_snapshot.json");
}
