use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn first_existing_path(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.exists())
}

fn ensure_embedded_binary(
    label: &str,
    source: Option<&Path>,
    destination: &Path,
    placeholder: &[u8],
) {
    match source {
        Some(path) => {
            fs::copy(path, destination).unwrap_or_else(|_| panic!("Failed to copy {label}"));
            println!("cargo:warning=Embedded {label} from {}", path.display());
        }
        None => {
            fs::write(destination, placeholder)
                .unwrap_or_else(|_| panic!("Failed to write placeholder for {label}"));
            println!(
                "cargo:warning={label} not found, wrote placeholder to {}",
                destination.display()
            );
        }
    }
}

fn main() {
    tauri_build::build();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let root_dir = Path::new(&manifest_dir).parent().unwrap();
    let src_dir = Path::new(&manifest_dir).join("src");
    let profile_dir = env::var("PROFILE").unwrap();
    let target_triple = env::var("TARGET").unwrap();

    let core_name = if cfg!(windows) {
        "osagent.exe"
    } else {
        "osagent"
    };
    let updater_name = if cfg!(windows) {
        "osagent-updater.exe"
    } else {
        "osagent-updater"
    };

    let core_source = env::var_os("OSAGENT_CORE_SOURCE")
        .map(PathBuf::from)
        .or_else(|| {
            first_existing_path([
                root_dir
                    .join("target")
                    .join(&target_triple)
                    .join(&profile_dir)
                    .join(core_name),
                root_dir.join("target").join(&profile_dir).join(core_name),
                root_dir.join("target").join("release").join(core_name),
            ])
        });

    let updater_source = env::var_os("OSAGENT_UPDATER_SOURCE")
        .map(PathBuf::from)
        .or_else(|| {
            first_existing_path([
                root_dir
                    .join("updater")
                    .join("target")
                    .join(&target_triple)
                    .join(&profile_dir)
                    .join(updater_name),
                root_dir
                    .join("updater")
                    .join("target")
                    .join(&profile_dir)
                    .join(updater_name),
                root_dir
                    .join("updater")
                    .join("target")
                    .join("release")
                    .join(updater_name),
            ])
        });

    if let Some(path) = core_source.as_ref() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    if let Some(path) = updater_source.as_ref() {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let embedded_core = src_dir.join("core.bin");
    ensure_embedded_binary(
        "osagent core",
        core_source.as_deref(),
        &embedded_core,
        b"placeholder",
    );

    let embedded_updater = src_dir.join("updater.bin");
    ensure_embedded_binary(
        "osagent-updater",
        updater_source.as_deref(),
        &embedded_updater,
        b"placeholder",
    );
}
