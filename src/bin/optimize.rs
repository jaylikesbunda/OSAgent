use clap::Parser;
use osagent::config::Config;
use osagent::prompt_eval::{
    EvalConfig, MemoryConfig, OptimizationConfig, PromptOptimizer, SearchStrategy, TestCase,
    TestCaseLoader,
};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "osagent-optimize")]
#[command(about = "Optimize OSAgent internal prompts via automated testing")]
#[command(version)]
struct Cli {
    /// Path to test cases file or directory (JSON or TOML)
    #[arg(short, long, default_value = "test_cases/basic.json")]
    tests: PathBuf,

    /// Path to OSAgent config file
    #[arg(short, long, default_value = "~/.osagent/config.toml")]
    config: PathBuf,

    /// Workspace directory for tool execution (relative to launch dir)
    #[arg(long, default_value = "prompt_eval_workspace")]
    workspace: PathBuf,

    /// Number of optimization iterations
    #[arg(short = 'i', long, default_value = "500")]
    iterations: usize,

    /// Search strategy: grid, random, evolutionary, exhaustive
    #[arg(long, default_value = "evolutionary")]
    strategy: String,

    /// Output directory for results
    #[arg(short, long, default_value = "./prompt_eval_results")]
    output: PathBuf,

    /// Temperature for LLM calls (0.0 = deterministic)
    #[arg(long, default_value = "0.0")]
    temperature: f32,

    /// Actually execute tools (vs simulate)
    #[arg(long)]
    execute_tools: bool,

    /// Parallel test execution (careful with API rate limits)
    #[arg(long, default_value = "1")]
    parallel: usize,

    /// Early stop when score exceeds threshold (0.0-1.0)
    #[arg(long)]
    early_stop: Option<f32>,

    /// Random seed for reproducibility
    #[arg(long)]
    seed: Option<u64>,

    /// Maximum tokens per response
    #[arg(long, default_value = "2048")]
    max_tokens: usize,

    /// Timeout per test in seconds
    #[arg(long, default_value = "60")]
    timeout: u64,

    /// Auto-tune: adapt exploration based on results
    #[arg(long, default_value = "true")]
    auto_tune: bool,

    /// Max iterations with no improvement before exploring more
    #[arg(long, default_value = "30")]
    max_no_improve: usize,

    /// Aggressive exploration mode
    #[arg(long, default_value = "true")]
    explore_aggressive: bool,

    /// Population size for evolutionary strategy
    #[arg(long, default_value = "50")]
    population: usize,

    /// Mutation rate for evolutionary (0.0-1.0)
    #[arg(long, default_value = "0.25")]
    mutation_rate: f32,

    /// Enable memory-guided mutation (loads from memory.json if exists)
    #[arg(long, default_value = "true")]
    memory_enabled: bool,

    /// Minimum score threshold to store configs in memory (0.0-1.0)
    #[arg(long, default_value = "0.6")]
    memory_threshold: f32,

    /// Maximum number of entries to store in memory
    #[arg(long, default_value = "100")]
    memory_max_entries: usize,

    /// Probability of using guided mutation vs random (0.0-1.0)
    #[arg(long, default_value = "0.7")]
    guided_mutation_rate: f32,

    /// Path to load/save memory file (default: output_dir/memory.json)
    #[arg(long)]
    memory_path: Option<PathBuf>,

    /// Number of top configs to sample mutations from
    #[arg(long, default_value = "5")]
    policy_top_k: usize,

    /// Short-term window size for credit calculation
    #[arg(long, default_value = "20")]
    short_window: usize,

    /// Long-term window size for credit calculation
    #[arg(long, default_value = "100")]
    long_window: usize,

    /// Confidence learning rate (higher = trust early samples more)
    #[arg(long, default_value = "5.0")]
    confidence_k: f32,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_max_level(if cli.verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .with_target(false)
        .compact()
        .init();

    let config_path = shellexpand::tilde(&cli.config.to_string_lossy()).to_string();
    let osa_config = Config::load(&config_path)?;

    let test_cases = load_test_cases(&cli.tests)?;

    if test_cases.is_empty() {
        eprintln!("Error: No test cases found at {}", cli.tests.display());
        std::process::exit(1);
    }

    // Ensure workspace directory exists and is safe
    let workspace_path = if cli.workspace.is_absolute() {
        cli.workspace.clone()
    } else {
        std::env::current_dir()?.join(&cli.workspace)
    };

    if !workspace_path.exists() {
        std::fs::create_dir_all(&workspace_path)?;
        println!("Created workspace: {}", workspace_path.display());
    }

    // Safety check: don't allow workspace to be root or home directory
    let workspace_str = workspace_path.to_string_lossy();
    let dangerous_paths = ["/", "/home", "/Users", "C:\\", "C:\\Users"];
    if dangerous_paths
        .iter()
        .any(|p| workspace_str.starts_with(p) && workspace_path.components().count() <= 3)
    {
        eprintln!(
            "Error: Workspace path {} is too dangerous. Use a subdirectory.",
            workspace_path.display()
        );
        std::process::exit(1);
    }

    let search_strategy = match cli.strategy.to_lowercase().as_str() {
        "grid" => SearchStrategy::GridSearch,
        "random" => SearchStrategy::RandomSample,
        "exhaustive" => SearchStrategy::Exhaustive,
        _ => SearchStrategy::Evolutionary {
            population: cli.population,
            mutation_rate: cli.mutation_rate,
        },
    };

    let eval_config = EvalConfig {
        temperature: cli.temperature,
        max_tokens: cli.max_tokens,
        execute_tools: cli.execute_tools,
        workspace_path,
        timeout_secs: cli.timeout,
    };

    let opt_config = OptimizationConfig {
        max_iterations: cli.iterations,
        search_strategy,
        early_stop_threshold: cli.early_stop,
        save_interval: 5,
        parallel_tests: cli.parallel,
        seed: cli.seed,
        auto_tune: cli.auto_tune,
        max_no_improve: cli.max_no_improve,
        explore_aggressive: cli.explore_aggressive,
    };

    let memory_config = MemoryConfig {
        enabled: cli.memory_enabled,
        threshold: cli.memory_threshold,
        max_entries: cli.memory_max_entries,
        guided_mutation_rate: cli.guided_mutation_rate,
        random_mutation_min: 1,
        random_mutation_max: 2,
        policy_top_k: cli.policy_top_k,
        short_window: cli.short_window,
        long_window: cli.long_window,
        confidence_k: cli.confidence_k,
    };

    let memory_path = cli.memory_path.clone().or_else(|| {
        let path = cli.output.join("memory.json");
        Some(path)
    });

    let optimizer = PromptOptimizer::new(
        osa_config,
        eval_config,
        test_cases,
        cli.output.clone(),
        memory_config,
        memory_path,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to initialize optimizer: {}", e))?;

    let _best = optimizer.run(opt_config).await;

    println!(
        "\nBest prompt config saved to: {}//BEST_PROMPT.txt",
        cli.output.display()
    );
    println!("Or: {}/latest/BEST_PROMPT.txt", cli.output.display());

    Ok(())
}

fn load_test_cases(path: &Path) -> Result<Vec<TestCase>, anyhow::Error> {
    if path.is_dir() {
        TestCaseLoader::load_directory(path)
            .map_err(|e| anyhow::anyhow!("Failed to load test cases: {}", e))
    } else if path.is_file() {
        TestCase::load_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load test cases: {}", e))
    } else {
        Err(anyhow::anyhow!(
            "Test path does not exist: {}",
            path.display()
        ))
    }
}
