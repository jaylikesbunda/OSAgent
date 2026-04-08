use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use clap::Parser;
use osagent::config::Config;
use regex::Regex;
use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const EXE_SUFFIX: &str = ".exe";
#[cfg(not(windows))]
const EXE_SUFFIX: &str = "";

#[derive(Parser, Debug, Clone)]
#[command(name = "osagent-bench")]
#[command(about = "Reproducible runtime benchmarks for OSA")]
struct Args {
    #[arg(long, default_value = "debug,release")]
    profiles: String,

    #[arg(long, default_value_t = 10)]
    iterations: usize,

    #[arg(long, default_value_t = 30_000)]
    startup_timeout_ms: u64,

    #[arg(long, default_value_t = 2_000)]
    idle_wait_ms: u64,

    #[arg(long, default_value = "benchmark_results")]
    output_dir: PathBuf,

    #[arg(long)]
    json_out: Option<PathBuf>,

    #[arg(long)]
    md_out: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    skip_build: bool,
}

#[derive(Debug, Clone, Copy)]
enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    fn parse_list(raw: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();

        for token in raw.split(',').map(|s| s.trim().to_lowercase()) {
            if token.is_empty() {
                continue;
            }
            let profile = match token.as_str() {
                "debug" | "dev" => Self::Debug,
                "release" | "rel" => Self::Release,
                _ => bail!("Unknown profile '{token}'. Use debug or release."),
            };
            if seen.insert(profile.as_str().to_string()) {
                out.push(profile);
            }
        }

        if out.is_empty() {
            bail!("No valid profiles selected");
        }
        Ok(out)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    fn target_dir(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    fn binary_path(self) -> PathBuf {
        PathBuf::from("target")
            .join(self.target_dir())
            .join(format!("osagent{EXE_SUFFIX}"))
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    generated_at: String,
    settings: BenchmarkSettings,
    profiles: Vec<ProfileReport>,
}

#[derive(Debug, Serialize)]
struct BenchmarkSettings {
    profiles: Vec<String>,
    iterations: usize,
    startup_timeout_ms: u64,
    idle_wait_ms: u64,
    workloads: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProfileReport {
    profile: String,
    binary_path: String,
    build_time_ms: Option<f64>,
    iterations: Vec<IterationReport>,
    aggregate: AggregateReport,
}

#[derive(Debug, Serialize)]
struct IterationReport {
    iteration: usize,
    run_dir: String,
    startup_ready_ms: Option<f64>,
    rss_ready_mb: Option<f64>,
    idle_rss_mb: Option<f64>,
    workloads: Vec<WorkloadReport>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkloadReport {
    name: String,
    duration_ms: Option<f64>,
    ok: bool,
    detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct AggregateReport {
    successful_iterations: usize,
    startup_ready_ms: Option<Stats>,
    rss_ready_mb: Option<Stats>,
    idle_rss_mb: Option<Stats>,
    workloads: BTreeMap<String, Stats>,
}

#[derive(Debug, Serialize, Clone)]
struct Stats {
    avg: f64,
    min: f64,
    max: f64,
    p50: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let profiles = BuildProfile::parse_list(&args.profiles)?;

    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("Failed to create {}", args.output_dir.display()))?;
    fs::create_dir_all(args.output_dir.join("runs")).with_context(|| {
        format!(
            "Failed to create {}",
            args.output_dir.join("runs").display()
        )
    })?;

    let mut build_times = BTreeMap::<String, f64>::new();
    if !args.skip_build {
        for profile in &profiles {
            println!("Building {} profile...", profile.as_str());
            let elapsed = build_profile(*profile)?;
            build_times.insert(profile.as_str().to_string(), elapsed);
            println!("  build complete: {:.1} ms", elapsed);
        }
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to build HTTP client")?;

    let mut profile_reports = Vec::new();
    for profile in profiles {
        let binary_path = profile.binary_path();
        if !binary_path.exists() {
            bail!(
                "Binary not found at {}. Build failed or wrong profile.",
                binary_path.display()
            );
        }

        println!(
            "Running {} benchmark ({} iterations)...",
            profile.as_str(),
            args.iterations
        );

        let mut iterations = Vec::new();
        for i in 1..=args.iterations {
            let report = run_iteration(
                &client,
                &binary_path,
                profile,
                i,
                &args.output_dir,
                Duration::from_millis(args.startup_timeout_ms),
                Duration::from_millis(args.idle_wait_ms),
            )
            .await;

            if let Some(err) = &report.error {
                println!("  iter {:02}: error - {}", i, err);
            } else {
                println!(
                    "  iter {:02}: startup {:.1} ms, ready RSS {:.1} MB, idle RSS {:.1} MB",
                    i,
                    report.startup_ready_ms.unwrap_or(-1.0),
                    report.rss_ready_mb.unwrap_or(-1.0),
                    report.idle_rss_mb.unwrap_or(-1.0),
                );
            }
            iterations.push(report);
        }

        let aggregate = aggregate_iterations(&iterations);
        profile_reports.push(ProfileReport {
            profile: profile.as_str().to_string(),
            binary_path: binary_path.display().to_string(),
            build_time_ms: build_times.get(profile.as_str()).copied(),
            iterations,
            aggregate,
        });
    }

    let report = BenchmarkReport {
        generated_at: Utc::now().to_rfc3339(),
        settings: BenchmarkSettings {
            profiles: BuildProfile::parse_list(&args.profiles)?
                .into_iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            iterations: args.iterations,
            startup_timeout_ms: args.startup_timeout_ms,
            idle_wait_ms: args.idle_wait_ms,
            workloads: vec![
                "health_ping_20x".to_string(),
                "frontend_assets".to_string(),
                "frontend_boot_api".to_string(),
                "session_crud".to_string(),
            ],
        },
        profiles: profile_reports,
    };

    let stamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let json_path = args
        .json_out
        .unwrap_or_else(|| args.output_dir.join(format!("runtime_bench_{stamp}.json")));
    let md_path = args
        .md_out
        .unwrap_or_else(|| args.output_dir.join(format!("runtime_bench_{stamp}.md")));

    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    if let Some(parent) = md_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&json_path, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("Failed writing {}", json_path.display()))?;
    fs::write(&md_path, render_markdown(&report))
        .with_context(|| format!("Failed writing {}", md_path.display()))?;

    println!("\nWrote JSON: {}", json_path.display());
    println!("Wrote Markdown: {}", md_path.display());

    Ok(())
}

fn build_profile(profile: BuildProfile) -> Result<f64> {
    let start = Instant::now();
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--bin").arg("osagent");
    if matches!(profile, BuildProfile::Release) {
        cmd.arg("--release");
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to run cargo build for {}", profile.as_str()))?;
    if !status.success() {
        bail!("cargo build failed for {}", profile.as_str());
    }
    Ok(elapsed_ms(start))
}

async fn run_iteration(
    client: &Client,
    binary_path: &Path,
    profile: BuildProfile,
    iteration: usize,
    output_dir: &Path,
    startup_timeout: Duration,
    idle_wait: Duration,
) -> IterationReport {
    let run_dir =
        output_dir
            .join("runs")
            .join(format!("{}_iter_{:02}", profile.as_str(), iteration));
    let _ = fs::remove_dir_all(&run_dir);

    let mut report = IterationReport {
        iteration,
        run_dir: run_dir.display().to_string(),
        startup_ready_ms: None,
        rss_ready_mb: None,
        idle_rss_mb: None,
        workloads: Vec::new(),
        error: None,
    };

    let result = async {
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("Failed to create {}", run_dir.display()))?;
        let workspace_dir = run_dir.join("workspace");
        let data_dir = run_dir.join("data");
        fs::create_dir_all(&workspace_dir)
            .with_context(|| format!("Failed to create {}", workspace_dir.display()))?;
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create {}", data_dir.display()))?;

        let port = reserve_port()?;
        let base_url = format!("http://127.0.0.1:{port}");

        let config_path = run_dir.join("config.toml");
        write_benchmark_config(&config_path, port, &workspace_dir, &data_dir)?;

        let stdout_log = File::create(run_dir.join("stdout.log"))
            .context("Failed to create stdout.log for benchmark run")?;
        let stderr_log = File::create(run_dir.join("stderr.log"))
            .context("Failed to create stderr.log for benchmark run")?;

        let mut child = spawn_server(binary_path, &config_path, stdout_log, stderr_log)?;

        let run_result = async {
            let startup_ready_ms =
                wait_until_ready(client, &mut child, &base_url, startup_timeout).await?;
            report.startup_ready_ms = Some(startup_ready_ms);
            report.rss_ready_mb = process_rss_mb(child.id());

            let token = login(client, &base_url).await?;

            report.workloads.push(
                run_workload("health_ping_20x", || async {
                    workload_health_ping(client, &base_url).await
                })
                .await,
            );

            report.workloads.push(
                run_workload("frontend_assets", || async {
                    workload_frontend_assets(client, &base_url).await
                })
                .await,
            );

            report.workloads.push(
                run_workload("frontend_boot_api", || async {
                    workload_frontend_boot_api(client, &base_url, &token).await
                })
                .await,
            );

            report.workloads.push(
                run_workload("session_crud", || async {
                    workload_session_crud(client, &base_url, &token).await
                })
                .await,
            );

            tokio::time::sleep(idle_wait).await;
            report.idle_rss_mb = process_rss_mb(child.id());

            Result::<()>::Ok(())
        }
        .await;

        terminate_child(&mut child).context("Failed while terminating benchmark child process")?;
        run_result
    }
    .await;

    if let Err(err) = result {
        report.error = Some(err.to_string());
        let _ = append_error_marker(&run_dir, &err.to_string());
    }

    report
}

fn append_error_marker(run_dir: &Path, message: &str) -> Result<()> {
    let mut f = File::create(run_dir.join("error.txt"))
        .with_context(|| format!("Failed to create error marker in {}", run_dir.display()))?;
    f.write_all(message.as_bytes())
        .context("Failed to write benchmark error marker")?;
    Ok(())
}

fn spawn_server(
    binary_path: &Path,
    config_path: &Path,
    stdout_log: File,
    stderr_log: File,
) -> Result<Child> {
    let mut cmd = Command::new(binary_path);
    cmd.arg("start")
        .arg("--config")
        .arg(config_path)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));

    cmd.spawn().with_context(|| {
        format!(
            "Failed to spawn server '{}' with config '{}'",
            binary_path.display(),
            config_path.display()
        )
    })
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    child
        .kill()
        .context("Failed to kill benchmark child process")?;
    for _ in 0..40 {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    bail!("Timed out waiting for child process to exit")
}

async fn wait_until_ready(
    client: &Client,
    child: &mut Child,
    base_url: &str,
    timeout: Duration,
) -> Result<f64> {
    let start = Instant::now();
    let url = format!("{base_url}/api/auth/status");

    loop {
        if let Some(status) = child
            .try_wait()
            .context("Failed checking child process status")?
        {
            bail!("Server process exited early with status {status}");
        }

        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(elapsed_ms(start));
            }
        }

        if start.elapsed() > timeout {
            bail!("Timed out waiting for readiness at {url}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn login(client: &Client, base_url: &str) -> Result<String> {
    let url = format!("{base_url}/api/auth/login");
    let resp = client
        .post(&url)
        .json(&json!({"password": ""}))
        .send()
        .await
        .context("Failed to call /api/auth/login")?;

    let resp = expect_success(resp, "login").await?;
    let payload: Value = resp
        .json()
        .await
        .context("Failed to parse /api/auth/login response")?;

    payload
        .get("token")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Missing token in /api/auth/login response"))
}

async fn run_workload<F, Fut>(name: &str, workload: F) -> WorkloadReport
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Option<String>>>,
{
    let start = Instant::now();
    match workload().await {
        Ok(detail) => WorkloadReport {
            name: name.to_string(),
            duration_ms: Some(elapsed_ms(start)),
            ok: true,
            detail,
        },
        Err(err) => WorkloadReport {
            name: name.to_string(),
            duration_ms: None,
            ok: false,
            detail: Some(err.to_string()),
        },
    }
}

async fn workload_health_ping(client: &Client, base_url: &str) -> Result<Option<String>> {
    let url = format!("{base_url}/api/auth/status");
    for _ in 0..20 {
        let resp = client
            .get(&url)
            .send()
            .await
            .context("health ping failed")?;
        let _ = expect_success(resp, "health ping").await?;
    }
    Ok(Some("20 successful /api/auth/status calls".to_string()))
}

async fn workload_frontend_assets(client: &Client, base_url: &str) -> Result<Option<String>> {
    let index_resp = client
        .get(format!("{base_url}/"))
        .send()
        .await
        .context("Failed to fetch /")?;
    let index_resp = expect_success(index_resp, "frontend index").await?;
    let html = index_resp
        .text()
        .await
        .context("Failed reading frontend index HTML")?;

    let assets = extract_local_asset_paths(&html)?;
    let mut total_bytes: usize = 0;

    for asset in &assets {
        let url = format!("{base_url}{asset}");
        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch asset {asset}"))?;
        let resp = expect_success(resp, "frontend asset request").await?;
        let body = resp
            .bytes()
            .await
            .with_context(|| format!("Failed to read asset body {asset}"))?;
        total_bytes += body.len();
    }

    Ok(Some(format!(
        "{} assets, {} bytes",
        assets.len(),
        total_bytes
    )))
}

async fn workload_frontend_boot_api(
    client: &Client,
    base_url: &str,
    token: &str,
) -> Result<Option<String>> {
    for endpoint in [
        "/api/sessions",
        "/api/workspaces",
        "/api/personas",
        "/api/model",
        "/api/providers/catalog",
        "/api/config",
    ] {
        let resp = client
            .get(format!("{base_url}{endpoint}"))
            .bearer_auth(token)
            .send()
            .await
            .with_context(|| format!("Failed to call {endpoint}"))?;
        let _ = expect_success(resp, endpoint).await?;
    }

    Ok(Some("Fetched initial frontend API fanout".to_string()))
}

async fn workload_session_crud(
    client: &Client,
    base_url: &str,
    token: &str,
) -> Result<Option<String>> {
    let create_resp = client
        .post(format!("{base_url}/api/sessions"))
        .bearer_auth(token)
        .json(&json!({"workspace_id": "default"}))
        .send()
        .await
        .context("Failed to create session")?;
    let create_resp = expect_success(create_resp, "create session").await?;
    let created: Value = create_resp
        .json()
        .await
        .context("Failed parsing create session response")?;
    let session_id = created
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Create session response missing id"))?
        .to_string();

    let list_resp = client
        .get(format!("{base_url}/api/sessions"))
        .bearer_auth(token)
        .send()
        .await
        .context("Failed to list sessions")?;
    let _ = expect_success(list_resp, "list sessions").await?;

    let get_resp = client
        .get(format!("{base_url}/api/sessions/{session_id}"))
        .bearer_auth(token)
        .send()
        .await
        .context("Failed to get created session")?;
    let _ = expect_success(get_resp, "get session").await?;

    let delete_resp = client
        .delete(format!("{base_url}/api/sessions/{session_id}"))
        .bearer_auth(token)
        .send()
        .await
        .context("Failed to delete created session")?;
    let _ = expect_success(delete_resp, "delete session").await?;

    Ok(Some("create/list/get/delete successful".to_string()))
}

async fn expect_success(resp: reqwest::Response, context: &str) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }

    let status = resp.status();
    let body = resp
        .text()
        .await
        .unwrap_or_else(|_| "<unreadable body>".to_string());
    bail!("{context} failed with {status}: {body}")
}

fn extract_local_asset_paths(html: &str) -> Result<Vec<String>> {
    let re = Regex::new(r#"(?i)<(?:link|script|img)[^>]+(?:href|src)=[\"']([^\"']+)[\"']"#)
        .context("Failed to compile asset extraction regex")?;

    let mut assets = BTreeSet::new();
    for cap in re.captures_iter(html) {
        let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if raw.is_empty()
            || raw.starts_with("http://")
            || raw.starts_with("https://")
            || raw.starts_with("//")
            || raw.starts_with("data:")
        {
            continue;
        }

        let cleaned = raw.split('#').next().unwrap_or(raw).trim();
        if cleaned.is_empty() {
            continue;
        }

        let normalized = if cleaned.starts_with('/') {
            cleaned.to_string()
        } else {
            format!("/{}", cleaned.trim_start_matches("./"))
        };

        assets.insert(normalized);
    }

    Ok(assets.into_iter().collect())
}

fn write_benchmark_config(
    config_path: &Path,
    port: u16,
    workspace: &Path,
    data_dir: &Path,
) -> Result<()> {
    let mut cfg = Config::default_config();
    cfg.server.bind = "127.0.0.1".to_string();
    cfg.server.port = port;
    cfg.server.password.clear();
    cfg.server.password_enabled = false;
    cfg.search.enabled = false;
    cfg.search.index_on_startup = false;
    cfg.update.check_on_startup = false;
    cfg.agent.workspace = workspace.to_string_lossy().to_string();
    cfg.storage.database = data_dir.join("osagent.db").to_string_lossy().to_string();
    cfg.ensure_workspace_defaults();
    cfg.save(config_path).with_context(|| {
        format!(
            "Failed saving benchmark config to {}",
            config_path.display()
        )
    })
}

fn reserve_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to bind ephemeral port")?;
    let port = listener
        .local_addr()
        .context("Failed to inspect ephemeral port")?
        .port();
    drop(listener);
    Ok(port)
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn aggregate_iterations(iterations: &[IterationReport]) -> AggregateReport {
    let mut startup = Vec::new();
    let mut rss_ready = Vec::new();
    let mut idle_rss = Vec::new();
    let mut workload_map: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    let successful_iterations = iterations.iter().filter(|it| it.error.is_none()).count();
    for it in iterations {
        if let Some(v) = it.startup_ready_ms {
            startup.push(v);
        }
        if let Some(v) = it.rss_ready_mb {
            rss_ready.push(v);
        }
        if let Some(v) = it.idle_rss_mb {
            idle_rss.push(v);
        }
        for w in &it.workloads {
            if w.ok {
                if let Some(ms) = w.duration_ms {
                    workload_map.entry(w.name.clone()).or_default().push(ms);
                }
            }
        }
    }

    let workloads = workload_map
        .into_iter()
        .filter_map(|(name, values)| stats_from(&values).map(|s| (name, s)))
        .collect();

    AggregateReport {
        successful_iterations,
        startup_ready_ms: stats_from(&startup),
        rss_ready_mb: stats_from(&rss_ready),
        idle_rss_mb: stats_from(&idle_rss),
        workloads,
    }
}

fn stats_from(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let avg = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let min = *sorted.first()?;
    let max = *sorted.last()?;
    let p50 = sorted[sorted.len() / 2];

    Some(Stats { avg, min, max, p50 })
}

fn render_markdown(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str("# OSA Runtime Benchmark Report\n\n");
    out.push_str(&format!("Generated at: `{}`\n\n", report.generated_at));
    out.push_str("## Settings\n\n");
    out.push_str(&format!(
        "- Profiles: `{}`\n- Iterations: `{}`\n- Startup timeout: `{}` ms\n- Idle wait: `{}` ms\n- Workloads: `{}`\n\n",
        report.settings.profiles.join(","),
        report.settings.iterations,
        report.settings.startup_timeout_ms,
        report.settings.idle_wait_ms,
        report.settings.workloads.join(", ")
    ));

    out.push_str("## Startup and Memory\n\n");
    out.push_str("| Profile | Startup avg (ms) | Startup p50 (ms) | Ready RSS avg (MB) | Idle RSS avg (MB) | Successful iters |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|\n");
    for p in &report.profiles {
        let startup_avg = p
            .aggregate
            .startup_ready_ms
            .as_ref()
            .map(|s| format!("{:.2}", s.avg))
            .unwrap_or_else(|| "n/a".to_string());
        let startup_p50 = p
            .aggregate
            .startup_ready_ms
            .as_ref()
            .map(|s| format!("{:.2}", s.p50))
            .unwrap_or_else(|| "n/a".to_string());
        let rss_ready_avg = p
            .aggregate
            .rss_ready_mb
            .as_ref()
            .map(|s| format!("{:.2}", s.avg))
            .unwrap_or_else(|| "n/a".to_string());
        let idle_rss_avg = p
            .aggregate
            .idle_rss_mb
            .as_ref()
            .map(|s| format!("{:.2}", s.avg))
            .unwrap_or_else(|| "n/a".to_string());

        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            p.profile,
            startup_avg,
            startup_p50,
            rss_ready_avg,
            idle_rss_avg,
            p.aggregate.successful_iterations
        ));
    }

    out.push_str("\n## Workload Latency\n\n");
    out.push_str("| Profile | Workload | Avg (ms) | P50 (ms) | Min (ms) | Max (ms) |\n");
    out.push_str("|---|---|---:|---:|---:|---:|\n");
    for p in &report.profiles {
        for (name, stats) in &p.aggregate.workloads {
            out.push_str(&format!(
                "| {} | {} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
                p.profile, name, stats.avg, stats.p50, stats.min, stats.max
            ));
        }
    }

    out.push_str("\n## Notes\n\n");
    out.push_str("- Startup time measures process spawn to first successful `/api/auth/status`.\n");
    out.push_str("- `ready RSS` is sampled immediately after readiness; `idle RSS` is sampled after the configured idle wait.\n");
    out.push_str("- Workloads are provider-free: health pings, frontend assets, frontend boot APIs, and session CRUD.\n");

    out
}

fn process_rss_mb(pid: u32) -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let status_path = format!("/proc/{pid}/status");
        let content = fs::read_to_string(status_path).ok()?;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())?;
                return Some(kb / 1024.0);
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let kb = text.parse::<f64>().ok()?;
        Some(kb / 1024.0)
    }

    #[cfg(windows)]
    {
        let cmd = format!("(Get-Process -Id {pid}).WorkingSet64");
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &cmd])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let bytes_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let bytes = bytes_text.parse::<f64>().ok()?;
        Some(bytes / (1024.0 * 1024.0))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pid;
        None
    }
}
