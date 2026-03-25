use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    tauri_build::build();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let root_dir = Path::new(&manifest_dir).parent().unwrap();
    let core_manifest_path = root_dir.join("Cargo.toml");
    let is_release = env::var("PROFILE").unwrap() == "release";
    let profile_dir = if is_release { "release" } else { "debug" };
    let core_binary_name = if cfg!(windows) {
        "osagent.exe"
    } else {
        "osagent"
    };

    let src_dir = Path::new(&manifest_dir).join("src");
    let embedded_file = src_dir.join("core.bin");
    let core_source = root_dir
        .join("target")
        .join(profile_dir)
        .join(core_binary_name);

    println!("cargo:rerun-if-changed={}", core_manifest_path.display());
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../src-tauri");
    println!("cargo:rerun-if-changed={}", core_source.display());

    if !core_source.exists() {
        println!("cargo:warning=Building osagent core...");
        let manifest_str = core_manifest_path.to_string_lossy().into_owned();
        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--manifest-path")
            .arg(&manifest_str)
            .arg("--features")
            .arg("discord");
        if is_release {
            cmd.arg("--release");
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let status = cmd.status().expect("Failed to build osagent core");
        if !status.success() {
            panic!("Failed to build osagent core");
        }
    }

    if core_source.exists() {
        println!("cargo:warning=Copying osagent core to src/core.bin");
        fs::copy(&core_source, &embedded_file).expect("Failed to copy osagent core");
    } else {
        println!(
            "cargo:warning=osagent core not found at {}",
            core_source.display()
        );
    }
}
