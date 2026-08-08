#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_strip)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::map_identity)]
#![allow(clippy::vec_init_then_push)]
#![allow(clippy::match_like_matches_macro)]

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{error, info, Level};
use tracing_subscriber::EnvFilter;

mod agent;
mod config;
mod error;
mod external;
mod indexer;
mod lsp;
mod oauth;
mod permission;
mod plugin;
mod scheduler;
mod skills;
mod storage;
mod tools;
mod update;
mod voice;
mod web;
mod workflow;

#[cfg(feature = "discord")]
mod discord;

#[derive(Parser)]
#[command(name = "osagent")]
#[command(about = "Secure local AI agent", long_about = None)]
struct Cli {
    /// Defaults to `start` so that running the bare binary does something
    /// useful instead of printing a usage error.
    #[command(subcommand)]
    command: Option<Commands>,
}

fn default_config_path() -> PathBuf {
    PathBuf::from("~/.osagent/config.toml")
}

/// Path to the restart sentinel. Deliberately absolute: this used to be a
/// relative path, which dropped a dotfile into whatever directory the user
/// happened to launch from, and silently broke restart-from-settings when the
/// service ran with a read-only working directory.
fn restart_flag_path() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~/.osagent/restart_flag").to_string())
}

#[derive(Subcommand)]
enum Commands {
    /// Start the agent server
    Start {
        /// Configuration file path
        #[arg(short, long, default_value = "~/.osagent/config.toml")]
        config: PathBuf,
        /// Custom workspace directory (overrides config)
        #[arg(short = 'w', long)]
        workspace: Option<String>,
        /// Port for the web UI (overrides config and OSAGENT_PORT)
        #[arg(short = 'p', long)]
        port: Option<u16>,
        /// Enable verbose logging
        #[arg(short, long)]
        verbose: bool,
    },
    /// Interactive setup wizard
    Setup {
        /// Configuration file path
        #[arg(short, long, default_value = "~/.osagent/config.toml")]
        config: PathBuf,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Install as system service
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    /// Check for updates
    Update {
        /// Only check, don't show full details
        #[arg(short, long)]
        check: bool,
        /// Update channel (stable/beta/dev)
        #[arg(short = 'c', long)]
        channel: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Edit configuration
    Edit,
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Install as system service
    Install,
    /// Uninstall system service
    Uninstall,
    /// Start service
    Start,
    /// Stop service
    Stop,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let command = cli.command.unwrap_or(Commands::Start {
        config: default_config_path(),
        workspace: None,
        port: None,
        verbose: false,
    });

    // The filter has to be built after parsing: initialising it first meant
    // -v was read long after the level was already fixed, so the flag's only
    // observable effect was logging that it had been enabled.
    let verbose = matches!(
        &command,
        Commands::Start {
            verbose: true,
            ..
        }
    );
    let default_level = if verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(default_level.into())
                .from_env_lossy(),
        )
        .init();

    match command {
        Commands::Start {
            config,
            workspace,
            port,
            verbose,
        } => {
            let config_path = shellexpand::tilde(&config.to_string_lossy()).to_string();
            let mut config = config::Config::load(&config_path)?;

            // Override workspace in order of precedence:
            // 1. Command line flag (-w/--workspace)
            // 2. Environment variable (OSAGENT_WORKSPACE)
            // 3. Config file
            if let Some(custom_workspace) = workspace {
                info!(
                    "Using custom workspace from command line: {}",
                    custom_workspace
                );
                config.agent.workspace = custom_workspace.clone();
                config.agent.active_workspace = Some("default".to_string());
                if let Some(default_ws) = config
                    .agent
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.id == "default")
                {
                    default_ws.path = custom_workspace;
                }
            } else if let Ok(env_workspace) = std::env::var("OSAGENT_WORKSPACE") {
                info!("Using workspace from environment: {}", env_workspace);
                config.agent.workspace = env_workspace.clone();
                config.agent.active_workspace = Some("default".to_string());
                if let Some(default_ws) = config
                    .agent
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.id == "default")
                {
                    default_ws.path = env_workspace;
                }
            }

            // Port precedence mirrors the workspace chain above:
            // flag > environment > config file.
            let port_override = port.or_else(|| {
                std::env::var("OSAGENT_PORT")
                    .ok()
                    .and_then(|p| p.parse::<u16>().ok())
            });
            if let Some(p) = port_override {
                info!("Using port override: {}", p);
                config.server.port = p;
            }

            config.ensure_workspace_defaults();

            if verbose {
                info!("Verbose logging enabled");
            }

            info!("Starting OSA v{}", update::build_version());
            info!("Config: {}", config_path);

            for workspace in config.list_workspaces() {
                let workspace_path = shellexpand::tilde(&workspace.resolved_path()).to_string();
                std::fs::create_dir_all(&workspace_path).map_err(crate::error::OSAgentError::Io)?;
            }
            let active_workspace = config.get_active_workspace();
            info!(
                "Active workspace: {} ({})",
                active_workspace.id,
                active_workspace.resolved_path()
            );

            let config_path_buf = std::path::PathBuf::from(&config_path);

            // Helper function to check for restart flag
            let check_restart_flag = || -> bool {
                std::fs::read_to_string(restart_flag_path())
                    .map(|s| s.trim() == "1")
                    .unwrap_or(false)
            };

            // Helper function to clear restart flag
            let clear_restart_flag = || {
                let _ = std::fs::remove_file(restart_flag_path());
            };

            clear_restart_flag();

            // A previous run killed hard (launcher, crash, Task Manager) can
            // leave whisper-server alive; on Windows it can hold this process's
            // listening socket open and make the bind below fail.
            voice::whisper_server::kill_stale_servers();

            // Main server loop - allows restart
            loop {
                let mut config = config::Config::load(&config_path)?;
                // Re-apply the override: the restart path reloads from disk and
                // would otherwise silently drop back to the configured port.
                if let Some(p) = port_override {
                    config.server.port = p;
                }
                config.ensure_workspace_defaults();
                let discord_enabled = cfg!(feature = "discord")
                    && config.discord.as_ref().map(|d| d.enabled).unwrap_or(false);

                let runtime_start = Instant::now();
                let agent = std::sync::Arc::new(agent::AgentRuntime::new(config.clone())?);
                info!(
                    target: "osagent::startup",
                    "phase=main_agent_runtime_construct elapsed_ms={:.2}",
                    runtime_start.elapsed().as_secs_f64() * 1000.0
                );
                let shutdown_rx = agent.subscribe_shutdown();

                let scheduler_start = Instant::now();
                if let Err(e) = agent.start_scheduler().await {
                    tracing::warn!("Failed to start scheduler: {}", e);
                }
                info!(
                    target: "osagent::startup",
                    "phase=main_scheduler_start elapsed_ms={:.2}",
                    scheduler_start.elapsed().as_secs_f64() * 1000.0
                );

                if discord_enabled {
                    #[cfg(feature = "discord")]
                    if let Some(discord_config) = &config.discord {
                        if discord_config.enabled {
                            info!("Discord bot enabled");
                            if let Err(err) = discord::spawn_discord_bot(
                                discord_config.clone(),
                                config_path_buf.clone(),
                                agent.clone(),
                            )
                            .await
                            {
                                error!("Failed to start Discord bot: {}", err);
                            }
                        }
                    }

                    web::server::run_with_agent(
                        config,
                        agent,
                        config_path_buf.clone(),
                        Some(shutdown_rx),
                    )
                    .await?;
                    info!("Web server stopped");

                    #[cfg(feature = "discord")]
                    if discord::stop_discord_bot().await {
                        info!("Discord bot stopped");
                    }
                } else {
                    // Just web server
                    web::server::run_with_agent(
                        config,
                        agent,
                        config_path_buf.clone(),
                        Some(shutdown_rx),
                    )
                    .await?;
                }

                // The resident Whisper model is hundreds of megabytes; do not
                // leave it running across a restart or after shutdown.
                voice::whisper_server::shutdown();

                // Check if restart was requested
                if check_restart_flag() {
                    clear_restart_flag();
                    info!("Restarting server with updated configuration...");
                    continue;
                } else {
                    break;
                }
            }
        }
        Commands::Setup { config } => {
            let config_path = shellexpand::tilde(&config.to_string_lossy()).to_string();
            config::setup_wizard(&config_path)?;
        }
        Commands::Config { command } => match command {
            ConfigCommands::Show => {
                let config = config::Config::load("~/.osagent/config.toml")?;
                println!("{}", toml::to_string_pretty(&config)?);
            }
            ConfigCommands::Edit => {
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
                let config_path = shellexpand::tilde("~/.osagent/config.toml").to_string();
                std::process::Command::new(editor)
                    .arg(&config_path)
                    .status()?;
            }
        },
        Commands::Service { command } => {
            let os = std::env::consts::OS;
            match command {
                ServiceCommands::Install => {
                    match os {
                        "linux" => install_systemd_service()?,
                        "macos" => install_launchd_service()?,
                        "windows" => install_windows_service()?,
                        _ => {
                            eprintln!("Service installation not supported on this OS: {}", os);
                        }
                    }
                }
                ServiceCommands::Uninstall => match os {
                    "linux" => uninstall_systemd_service()?,
                    "macos" => uninstall_launchd_service()?,
                    "windows" => uninstall_windows_service()?,
                    _ => {
                        eprintln!("Service uninstallation not supported on this OS: {}", os);
                    }
                },
                ServiceCommands::Start => match os {
                    "linux" => run_systemctl("start", "osagent")?,
                    "macos" => run_launchctl("start", "com.osagent")?,
                    "windows" => run_sc(&["start", "osagent"])?,
                    _ => {
                        eprintln!("Service start not supported on this OS: {}", os);
                    }
                },
                ServiceCommands::Stop => match os {
                    "linux" => run_systemctl("stop", "osagent")?,
                    "macos" => run_launchctl("stop", "com.osagent")?,
                    "windows" => run_sc(&["stop", "osagent"])?,
                    _ => {
                        eprintln!("Service stop not supported on this OS: {}", os);
                    }
                },
            }
        }
        Commands::Update { check, channel } => {
            let cfg = config::Config::load("~/.osagent/config.toml")?;
            let channel_str = channel.unwrap_or_else(|| cfg.update.channel.clone());
            let channel = channel_str
                .parse::<update::UpdateChannel>()
                .unwrap_or(update::UpdateChannel::Stable);

            let checker = update::UpdateChecker::new(update::build_version());

            info!("Checking for updates (channel: {})...", channel);
            let result = checker.check(channel).await;

            if check {
                if result.update_available {
                    println!(
                        "Update available: {} -> {}",
                        result.current_version,
                        result.latest_version.as_deref().unwrap_or("?")
                    );
                } else {
                    println!("OSA is up to date (v{})", result.current_version);
                }
            } else {
                println!("Current version: {}", result.current_version);
                if let Some(latest) = &result.latest_version {
                    println!("Latest version: {}", latest);
                }
                println!("Channel: {}", result.channel);
                if result.update_available {
                    println!("\nUpdate available!");
                    if let Some(url) = &result.release_url {
                        println!("Release URL: {}", url);
                    }
                } else {
                    println!("\nNo update available.");
                }
                if let Some(err) = &result.error {
                    println!("Error: {}", err);
                }
            }
        }
    }

    Ok(())
}

/// Registers OSA with the Windows Service Control Manager.
///
/// Uses `sc.exe`, which ships with Windows, rather than sending users off to
/// download NSSM: on the project's primary platform the previous behaviour was
/// to print a link and do nothing.
#[cfg(windows)]
fn install_windows_service() -> anyhow::Result<()> {
    let binary_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to get current exe: {}", e))?;

    // sc.exe takes the whole command line as a single binPath value, and the
    // space between the executable and its argument has to survive quoting.
    let bin_path = format!("\"{}\" start", binary_path.display());

    let status = std::process::Command::new("sc.exe")
        .args([
            "create",
            "osagent",
            "binPath=",
            &bin_path,
            "start=",
            "auto",
            "DisplayName=",
            "OSA AI Assistant",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run sc.exe: {}", e))?;

    if !status.success() {
        anyhow::bail!(
            "sc.exe create failed (exit {:?}). This needs an elevated prompt.",
            status.code()
        );
    }

    let _ = std::process::Command::new("sc.exe")
        .args([
            "description",
            "osagent",
            "Local-first AI agent (https://opensourceagent.net)",
        ])
        .status();

    println!("Service 'osagent' installed and set to start automatically.");
    println!("  Run 'osagent service start' to start it now.");

    Ok(())
}

#[cfg(windows)]
fn uninstall_windows_service() -> anyhow::Result<()> {
    let _ = std::process::Command::new("sc.exe")
        .args(["stop", "osagent"])
        .status();

    let status = std::process::Command::new("sc.exe")
        .args(["delete", "osagent"])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run sc.exe: {}", e))?;

    if !status.success() {
        anyhow::bail!(
            "sc.exe delete failed (exit {:?}). This needs an elevated prompt.",
            status.code()
        );
    }

    println!("Service uninstalled.");
    Ok(())
}

#[cfg(windows)]
fn run_sc(args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("sc.exe")
        .args(args)
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run sc.exe: {}", e))?;

    if !status.success() {
        anyhow::bail!("sc.exe {:?} failed (exit {:?}).", args, status.code());
    }
    Ok(())
}

// The `match os` arms above are compiled on every platform, so these stubs are
// needed off Windows even though those arms are unreachable there.
#[cfg(not(windows))]
fn install_windows_service() -> anyhow::Result<()> {
    anyhow::bail!("Windows services are not available on this platform")
}

#[cfg(not(windows))]
fn uninstall_windows_service() -> anyhow::Result<()> {
    anyhow::bail!("Windows services are not available on this platform")
}

#[cfg(not(windows))]
fn run_sc(_args: &[&str]) -> anyhow::Result<()> {
    anyhow::bail!("Windows services are not available on this platform")
}

fn install_systemd_service() -> anyhow::Result<()> {
    let binary_path =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("Failed to get current exe: {}", e))?;

    let service_content = format!(
        r#"[Unit]
Description=OSA AI Assistant
After=network.target

[Service]
Type=simple
User={}
WorkingDirectory={}
ExecStart={} start
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
"#,
        std::env::var("USER").unwrap_or_else(|_| "root".to_string()),
        std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()),
        binary_path.display()
    );

    std::fs::write("/etc/systemd/system/osagent.service", &service_content)
        .map_err(|e| anyhow::anyhow!("Failed to write service file (need sudo?): {}", e))?;

    println!("✓ Created /etc/systemd/system/osagent.service");

    std::process::Command::new("systemctl")
        .args(["daemon-reload"])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to reload systemd: {}", e))?;

    std::process::Command::new("systemctl")
        .args(["enable", "osagent"])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to enable service: {}", e))?;

    println!("✓ Service installed and enabled.");
    println!("  Run 'sudo systemctl start osagent' to start.");

    Ok(())
}

fn uninstall_systemd_service() -> anyhow::Result<()> {
    println!("Stopping osagent service...");
    let _ = std::process::Command::new("systemctl")
        .args(["stop", "osagent"])
        .status();

    println!("Disabling osagent service...");
    let _ = std::process::Command::new("systemctl")
        .args(["disable", "osagent"])
        .status();

    std::fs::remove_file("/etc/systemd/system/osagent.service")
        .map_err(|e| anyhow::anyhow!("Failed to remove service file (need sudo?): {}", e))?;

    std::process::Command::new("systemctl")
        .args(["daemon-reload"])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to reload systemd: {}", e))?;

    println!("✓ Service uninstalled.");

    Ok(())
}

fn install_launchd_service() -> anyhow::Result<()> {
    let binary_path =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("Failed to get current exe: {}", e))?;

    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;

    let plist_path = format!("{}/Library/LaunchAgents/com.osagent.plist", home);

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.osagent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}/Library/Logs/osagent.log</string>
    <key>StandardErrorPath</key>
    <string>{}/Library/Logs/osagent.log</string>
</dict>
</plist>
"#,
        binary_path.display(),
        home,
        home
    );

    std::fs::create_dir_all(format!("{}/Library/LaunchAgents", home))
        .map_err(|e| anyhow::anyhow!("Failed to create LaunchAgents directory: {}", e))?;

    std::fs::write(&plist_path, &plist_content)
        .map_err(|e| anyhow::anyhow!("Failed to write plist file: {}", e))?;

    println!("✓ Created {}", plist_path);

    std::process::Command::new("launchctl")
        .args(["load", &plist_path])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to load service: {}", e))?;

    println!("✓ Service installed and started.");

    Ok(())
}

fn uninstall_launchd_service() -> anyhow::Result<()> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;

    let plist_path = format!("{}/Library/LaunchAgents/com.osagent.plist", home);

    println!("Unloading osagent service...");
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &plist_path])
        .status();

    if std::path::Path::new(&plist_path).exists() {
        std::fs::remove_file(&plist_path)
            .map_err(|e| anyhow::anyhow!("Failed to remove plist: {}", e))?;
    }

    println!("✓ Service uninstalled.");

    Ok(())
}

fn run_systemctl(action: &str, service: &str) -> anyhow::Result<()> {
    std::process::Command::new("systemctl")
        .args([action, service])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run systemctl {} {}: {}", action, service, e))?;

    println!("✓ Service action completed.");

    Ok(())
}

fn run_launchctl(action: &str, label: &str) -> anyhow::Result<()> {
    std::process::Command::new("launchctl")
        .args([action, label])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run launchctl {} {}: {}", action, label, e))?;

    println!("✓ Service action completed.");

    Ok(())
}
