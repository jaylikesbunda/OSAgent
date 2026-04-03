use crate::config::Config;
use crate::prompt_eval::memory::{MemoryConfig, SuccessEntry, SuccessMemory, TestScore};
use crate::prompt_eval::runner::{EvalConfig, EvaluationRunner};
use crate::prompt_eval::scorer::{AggregateScore, Score, Scorer};
use crate::prompt_eval::test_case::TestCase;
use crate::prompt_eval::variation::{PromptConfig, SearchStrategy};
use chrono::{DateTime, Utc};
use rand::SeedableRng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationResult {
    pub iteration: usize,
    pub config: PromptConfig,
    pub config_hash: String,
    pub aggregate_score: AggregateScore,
    pub timestamp: DateTime<Utc>,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    pub max_iterations: usize,
    pub search_strategy: SearchStrategy,
    pub early_stop_threshold: Option<f32>,
    pub save_interval: usize,
    pub parallel_tests: usize,
    pub seed: Option<u64>,
    pub auto_tune: bool,
    pub max_no_improve: usize,
    pub explore_aggressive: bool,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        OptimizationConfig {
            max_iterations: 100,
            search_strategy: SearchStrategy::Evolutionary {
                population: 50,
                mutation_rate: 0.25,
            },
            early_stop_threshold: Some(0.95),
            save_interval: 5,
            parallel_tests: 1,
            seed: None,
            auto_tune: true,
            max_no_improve: 50,
            explore_aggressive: true,
        }
    }
}

impl OptimizationConfig {
    pub fn from_args(matches: &clap::ArgMatches) -> Self {
        OptimizationConfig {
            max_iterations: matches
                .get_one::<usize>("iterations")
                .copied()
                .unwrap_or(100),
            search_strategy: matches
                .get_one::<String>("strategy")
                .map(|s| SearchStrategy::from_str(s))
                .unwrap_or(SearchStrategy::Evolutionary {
                    population: 50,
                    mutation_rate: 0.25,
                }),
            early_stop_threshold: matches.get_one::<f32>("early-stop").copied(),
            save_interval: 5,
            parallel_tests: matches.get_one::<usize>("parallel").copied().unwrap_or(1),
            seed: matches.get_one::<u64>("seed").copied(),
            auto_tune: matches.get_flag("auto-tune"),
            max_no_improve: matches
                .get_one::<usize>("max-no-improve")
                .copied()
                .unwrap_or(50),
            explore_aggressive: matches.get_flag("explore-aggressive"),
        }
    }
}

pub struct PromptOptimizer {
    runner: EvaluationRunner,
    scorer: Scorer,
    test_cases: Vec<TestCase>,
    results: Arc<RwLock<Vec<OptimizationResult>>>,
    best_result: Arc<RwLock<Option<OptimizationResult>>>,
    output_dir: PathBuf,
    current_run_dir: RwLock<Option<PathBuf>>,
    memory: Arc<RwLock<SuccessMemory>>,
    memory_path: Option<PathBuf>,
}

impl PromptOptimizer {
    pub async fn new(
        osa_config: Config,
        eval_config: EvalConfig,
        test_cases: Vec<TestCase>,
        output_dir: PathBuf,
        memory_config: MemoryConfig,
        memory_path: Option<PathBuf>,
    ) -> Result<Self, crate::prompt_eval::runner::EvalError> {
        let runner = EvaluationRunner::new(osa_config, eval_config).await?;
        let scorer = Scorer::new();

        std::fs::create_dir_all(&output_dir).ok();

        let memory = if memory_config.enabled {
            if let Some(path) = &memory_path {
                if let Some(loaded) = SuccessMemory::load(path) {
                    tracing::info!("Loaded existing memory with {} entries", loaded.len());
                    loaded
                } else {
                    tracing::info!("Creating new memory");
                    SuccessMemory::new(memory_config)
                }
            } else {
                tracing::info!("Creating new memory (no path specified)");
                SuccessMemory::new(memory_config)
            }
        } else {
            SuccessMemory::new(memory_config)
        };

        Ok(PromptOptimizer {
            runner,
            scorer,
            test_cases,
            results: Arc::new(RwLock::new(Vec::new())),
            best_result: Arc::new(RwLock::new(None)),
            output_dir,
            current_run_dir: RwLock::new(None),
            memory: Arc::new(RwLock::new(memory)),
            memory_path,
        })
    }

    pub async fn run(&self, config: OptimizationConfig) -> OptimizationResult {
        let batch_size = 10;

        let run_id = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let run_dir = self.output_dir.join(format!("run_{}", run_id));
        let memory_path = self
            .memory_path
            .clone()
            .unwrap_or_else(|| run_dir.join("memory.json"));
        std::fs::create_dir_all(&run_dir).ok();
        {
            let mut dir_guard = self.current_run_dir.write().await;
            *dir_guard = Some(run_dir.clone());
        }

        let start_time = std::time::Instant::now();
        let mut _best_score_so_far = 0.0;
        let mut no_improve_count = 0;
        let mut consecutive_bests = 0;
        let mut last_save_iteration = 0;
        let mut _last_leaderboard_iteration = 0;

        println!(
            "\nStarting optimization: {} test cases, {} iterations (batch size: {})",
            self.test_cases.len(),
            config.max_iterations,
            batch_size
        );
        println!("Results being saved to: {}\n", run_dir.display());

        let mut last_config: Option<PromptConfig> = None;
        let mut last_score: Option<f32> = None;
        let mut last_test_scores: Option<HashMap<String, TestScore>> = None;

        let mut iteration = 0;
        while iteration < config.max_iterations {
            let remaining = config.max_iterations - iteration;
            let current_batch_size = remaining.min(batch_size);

            let batch_variations = {
                let memory = self.memory.read().await;
                if no_improve_count > 5 && memory.policy.has_sufficient_history() {
                    let failing: Vec<String> = last_test_scores
                        .as_ref()
                        .map(|ts| {
                            ts.iter()
                                .filter(|(_, s)| s.correctness < 0.5)
                                .map(|(name, _)| name.clone())
                                .collect()
                        })
                        .unwrap_or_default();

                    if !failing.is_empty() {
                        print!(
                            "\n\x1b[2K[targeted: {:?}] ",
                            &failing[..3.min(failing.len())]
                        );
                        let seed = config.seed.unwrap_or(42) + iteration as u64;
                        let mut variations = Vec::with_capacity(current_batch_size);
                        for i in 0..current_batch_size {
                            let fallback = PromptConfig::generate_variations_with_memory(
                                1,
                                &config.search_strategy,
                                Some(seed + i as u64),
                                Some(&memory),
                            );
                            let mut child = fallback.into_iter().next().unwrap_or_default();
                            let mut rng = rand::rngs::StdRng::seed_from_u64(seed + i as u64);
                            memory.targeted_mutate(
                                &mut child,
                                last_config.as_ref().unwrap_or(&PromptConfig::default()),
                                &failing,
                                &mut rng,
                            );
                            variations.push(child);
                        }
                        variations
                    } else {
                        PromptConfig::generate_variations_with_memory(
                            current_batch_size,
                            &config.search_strategy,
                            config.seed.map(|s| s + iteration as u64),
                            Some(&memory),
                        )
                    }
                } else {
                    PromptConfig::generate_variations_with_memory(
                        current_batch_size,
                        &config.search_strategy,
                        config.seed.map(|s| s + iteration as u64),
                        Some(&memory),
                    )
                }
            };

            for prompt_config in batch_variations {
                if iteration >= config.max_iterations {
                    break;
                }

                let iter_start = std::time::Instant::now();
                iteration += 1;

                print!("\r[{}/{}] ", iteration, config.max_iterations);

                let results = self
                    .runner
                    .run_tests_parallel(&self.test_cases, &prompt_config, config.parallel_tests)
                    .await;

                let scores: Vec<Score> = results
                    .iter()
                    .zip(self.test_cases.iter())
                    .map(|(result, test)| self.scorer.score(result, test))
                    .collect();

                let aggregate = self.scorer.aggregate_scores(scores.clone());

                let config_hash = prompt_config.hash_key();

                let mut test_scores = HashMap::new();
                for (i, score) in scores.iter().enumerate() {
                    if let Some(test_name) = self.test_cases.get(i).map(|t| t.name.clone()) {
                        test_scores.insert(
                            test_name,
                            TestScore {
                                correctness: score.dimensions.correctness,
                                tool_accuracy: score.dimensions.tool_accuracy,
                                efficiency: score.dimensions.efficiency,
                                safety: score.dimensions.safety,
                                format: score.dimensions.format,
                            },
                        );
                    }
                }

                let opt_result = OptimizationResult {
                    iteration,
                    config: prompt_config.clone(),
                    config_hash: config_hash.clone(),
                    aggregate_score: aggregate.clone(),
                    timestamp: Utc::now(),
                    duration_secs: iter_start.elapsed().as_secs_f64(),
                };

                {
                    let mut results_guard = self.results.write().await;
                    results_guard.push(opt_result.clone());
                }

                {
                    let mut memory = self.memory.write().await;
                    if aggregate.avg_score >= memory.config.threshold {
                        let entry = SuccessEntry::new(
                            prompt_config.clone(),
                            config_hash.clone(),
                            aggregate.avg_score,
                            test_scores.clone(),
                        );
                        memory.add(entry);
                    }

                    if let Some(ref parent_config) = last_config {
                        if let Some(parent_score) = last_score {
                            memory.record_mutation(
                                iteration,
                                parent_config,
                                &prompt_config,
                                parent_score,
                                aggregate.avg_score,
                                last_test_scores.as_ref(),
                                Some(&test_scores),
                            );
                        }
                    }
                }

                last_config = Some(prompt_config.clone());
                last_score = Some(aggregate.avg_score);
                last_test_scores = Some(test_scores);

                let is_best = {
                    let mut best_guard = self.best_result.write().await;
                    match best_guard.as_ref() {
                        None => {
                            *best_guard = Some(opt_result.clone());
                            true
                        }
                        Some(current_best) => {
                            if aggregate.avg_score > current_best.aggregate_score.avg_score {
                                *best_guard = Some(opt_result.clone());
                                true
                            } else {
                                false
                            }
                        }
                    }
                };

                if is_best {
                    _best_score_so_far = aggregate.avg_score;
                    no_improve_count = 0;
                    consecutive_bests += 1;

                    print!("\r\x1b[K");
                    println!(
                        "★ NEW BEST #{:3}: {:.3} (C:{:.2} T:{:.2} E:{:.2} S:{:.2}) [{}s]",
                        consecutive_bests,
                        aggregate.avg_score,
                        aggregate.avg_correctness,
                        aggregate.avg_tool_accuracy,
                        aggregate.avg_efficiency,
                        aggregate.avg_safety,
                        iter_start.elapsed().as_secs()
                    );

                    if let Some(threshold) = config.early_stop_threshold {
                        if aggregate.avg_score >= threshold {
                            println!("\n✓ Reached early stop threshold {:.3}!", threshold);
                            break;
                        }
                    }
                } else {
                    no_improve_count += 1;
                    consecutive_bests = 0;
                }

                if config.auto_tune
                    && no_improve_count > config.max_no_improve / 2
                    && config.explore_aggressive
                    && iteration < config.max_iterations - 10
                {
                    println!(
                        "\n  → Exploring more aggressively after {} no-improves",
                        no_improve_count
                    );
                    no_improve_count = 0;
                }

                if iteration % 5 == 0 || is_best {
                    self.save_leaderboard_file(&run_dir).await;
                    _last_leaderboard_iteration = iteration;
                }

                if iteration % config.save_interval == 0 && iteration != last_save_iteration {
                    self.save_checkpoint(iteration, &run_dir).await;
                    last_save_iteration = iteration;
                }

                if iteration % 20 == 0 {
                    if let Err(e) = self.memory.read().await.save(&memory_path) {
                        tracing::warn!("Failed to save memory: {}", e);
                    }
                }
            }
        }

        if let Err(e) = self.memory.read().await.save(&memory_path) {
            tracing::warn!("Failed to save memory at end: {}", e);
        }

        let total_duration = start_time.elapsed().as_secs_f64();
        self.save_final_results(total_duration, &run_dir).await;

        self.get_best().await.unwrap_or_else(|| OptimizationResult {
            iteration: 0,
            config: PromptConfig::default(),
            config_hash: String::new(),
            aggregate_score: AggregateScore::default(),
            timestamp: Utc::now(),
            duration_secs: 0.0,
        })
    }

    async fn save_leaderboard_file(&self, run_dir: &PathBuf) {
        let results_guard = self.results.read().await;

        let mut sorted: Vec<_> = results_guard.iter().cloned().collect();
        sorted.sort_by(|a, b| {
            b.aggregate_score
                .avg_score
                .partial_cmp(&a.aggregate_score.avg_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let top10: Vec<_> = sorted.iter().take(10).collect();
        let total = results_guard.len();
        let best_score = sorted
            .first()
            .map(|r| r.aggregate_score.avg_score)
            .unwrap_or(0.0);
        let best_iter = sorted.first().map(|r| r.iteration).unwrap_or(0);

        let mut content = format!(
            r#"# Leaderboard - Updated: {}

## Summary
- Total iterations: {}
- Best score: {:.3} (iteration {})
- Last update: iteration {}

## Top 10

| Rank | Score | C | T | E | S | F | Iter | Identity | Tone | Workflow | Notes |
|------|-------|---|---|---|---|---|-------|----------|------|----------|-------|
"#,
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            total,
            best_score,
            best_iter,
            total
        );

        for (i, r) in top10.iter().enumerate() {
            let identity = format!("{:?}", r.config.identity_variant);
            let tone = format!("{:?}", r.config.tone);
            let workflow = format!("{:?}", r.config.workflow_variant);
            let is_best = if Some(r.aggregate_score.avg_score)
                == sorted.first().map(|x| x.aggregate_score.avg_score)
            {
                " ★"
            } else {
                ""
            };

            content.push_str(&format!(
                "| {} | {:.3} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {}{} |\n",
                i + 1,
                r.aggregate_score.avg_score,
                r.aggregate_score.avg_correctness,
                r.aggregate_score.avg_tool_accuracy,
                r.aggregate_score.avg_efficiency,
                r.aggregate_score.avg_safety,
                r.aggregate_score.avg_format,
                r.iteration,
                is_best
            ));
        }

        content.push_str("\n## Top 10 Config Variants\n\n");
        content.push_str("```\n");
        for (i, r) in top10.iter().enumerate() {
            content.push_str(&format!(
                "{:2}. Score:{:.3} Idn:{:?} Pri:{:?} Saf:{:?} Wfl:{:?} Com:{:?} Strat:{:?} Cnxt:{:?} Val:{:?}\n",
                i + 1,
                r.aggregate_score.avg_score,
                r.config.identity_variant,
                r.config.priorities_variant,
                r.config.safety_variant,
                r.config.workflow_variant,
                r.config.communication_variant,
                r.config.decision_strategy,
                r.config.context_behavior,
                r.config.validation_style,
            ));
        }
        content.push_str("```\n");

        content.push_str("\n## Top Prompts (Generated)\n\n");
        for (i, r) in top10.iter().enumerate() {
            let prompt = crate::prompt_eval::variation::build_system_prompt_with_config(&r.config);
            content.push_str(&format!(
                "### #{} (Score: {:.3})\n\n",
                i + 1,
                r.aggregate_score.avg_score
            ));
            content.push_str(&format!("```\n{}\n```\n\n", prompt));
        }

        let leaderboard_path = run_dir.join("LEADERBOARD.md");
        let _ = std::fs::write(&leaderboard_path, content);
    }

    async fn save_checkpoint(&self, iteration: usize, run_dir: &PathBuf) {
        let results_guard = self.results.read().await;
        let best_guard = self.best_result.read().await;

        let checkpoint_dir = run_dir.join(format!("checkpoint_iter_{}", iteration));
        let _ = std::fs::create_dir_all(&checkpoint_dir);

        let checkpoint_path = checkpoint_dir.join("status.json");
        let checkpoint = serde_json::json!({
            "iteration": iteration,
            "timestamp": Utc::now().to_rfc3339(),
            "best_score": best_guard.as_ref().map(|b| b.aggregate_score.avg_score),
            "total_results": results_guard.len(),
            "top_5_scores": results_guard.iter()
                .map(|r| (r.iteration, r.aggregate_score.avg_score))
                .collect::<Vec<_>>()
                .into_iter()
                .take(5)
                .collect::<Vec<_>>(),
        });

        if let Ok(json) = serde_json::to_string_pretty(&checkpoint) {
            let _ = std::fs::write(&checkpoint_path, json);
        }

        if let Some(best) = best_guard.as_ref() {
            let best_config_path = checkpoint_dir.join("best_config.json");
            if let Ok(json) = serde_json::to_string_pretty(&best.config) {
                let _ = std::fs::write(&best_config_path, json);
            }

            let best_score_path = checkpoint_dir.join("best_score.json");
            if let Ok(json) = serde_json::to_string_pretty(&best.aggregate_score) {
                let _ = std::fs::write(&best_score_path, json);
            }

            let summary = self.generate_best_config_summary(best);
            let summary_path = checkpoint_dir.join("BEST_PROMPT.txt");
            let _ = std::fs::write(&summary_path, summary);

            let prompt =
                crate::prompt_eval::variation::build_system_prompt_with_config(&best.config);
            let prompt_path = checkpoint_dir.join("generated_prompt.txt");
            let _ = std::fs::write(&prompt_path, prompt);
        }

        let all_results_path = checkpoint_dir.join("all_results.json");
        if let Ok(json) = serde_json::to_string_pretty(&*results_guard) {
            let _ = std::fs::write(&all_results_path, json);
        }

        let analysis = self.generate_analysis_report(&results_guard, &best_guard);
        let analysis_path = checkpoint_dir.join("analysis.md");
        let _ = std::fs::write(&analysis_path, analysis);

        println!("  Checkpoint saved to: {}", checkpoint_dir.display());
    }

    fn generate_analysis_report(
        &self,
        results: &[OptimizationResult],
        best: &Option<OptimizationResult>,
    ) -> String {
        let mut out = String::new();
        out.push_str("# Optimization Analysis Report\n\n");
        out.push_str(&format!(
            "**Generated:** {}\n\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        out.push_str(&format!("**Total Iterations:** {}\n\n", results.len()));

        if let Some(best) = best {
            out.push_str("## Best Configuration\n\n");
            out.push_str(&format!(
                "**Score:** {:.3} (iteration {})\n\n",
                best.aggregate_score.avg_score, best.iteration
            ));

            out.push_str("### Dimension Scores\n\n");
            out.push_str("| Dimension | Score |\n");
            out.push_str("|-----------|-------|\n");
            out.push_str(&format!(
                "| Correctness | {:.3} |\n",
                best.aggregate_score.avg_correctness
            ));
            out.push_str(&format!(
                "| Tool Accuracy | {:.3} |\n",
                best.aggregate_score.avg_tool_accuracy
            ));
            out.push_str(&format!(
                "| Efficiency | {:.3} |\n",
                best.aggregate_score.avg_efficiency
            ));
            out.push_str(&format!(
                "| Safety | {:.3} |\n",
                best.aggregate_score.avg_safety
            ));
            out.push_str(&format!(
                "| Format | {:.3} |\n\n",
                best.aggregate_score.avg_format
            ));

            out.push_str("### Per-Test Results\n\n");
            out.push_str("| Test | Score | Correct | Tools | Efficiency | Pass |\n");
            out.push_str("|------|-------|---------|-------|-------------|------|\n");
            for score in &best.aggregate_score.individual_scores {
                let pass = if score.passed { "✓" } else { "✗" };
                out.push_str(&format!(
                    "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {} |\n",
                    score.test_name,
                    score.overall,
                    score.dimensions.correctness,
                    score.dimensions.tool_accuracy,
                    score.dimensions.efficiency,
                    pass,
                ));
            }
            out.push('\n');
        }

        out.push_str("## Score Progression\n\n");
        out.push_str("```\n");
        let mut sorted: Vec<_> = results
            .iter()
            .map(|r| r.aggregate_score.avg_score)
            .collect();
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        out.push_str("Top 10 scores:\n");
        for (i, score) in sorted.iter().take(10).enumerate() {
            out.push_str(&format!("  {}. {:.3}\n", i + 1, score));
        }
        out.push_str("```\n\n");

        out.push_str("## Recommendations\n\n");
        if let Some(best) = best {
            if best.aggregate_score.avg_efficiency < 0.7 {
                out.push_str("- **Efficiency is low**: Consider using more direct workflows or fewer validation steps\n");
            }
            if best.aggregate_score.avg_tool_accuracy < 0.8 {
                out.push_str("- **Tool usage could improve**: Review tool selection guidelines\n");
            }
            if best.aggregate_score.avg_correctness < 0.8 {
                out.push_str(
                    "- **Correctness issues**: Consider adding more detailed instructions\n",
                );
            }
        }

        out
    }

    async fn save_final_results(&self, total_duration_secs: f64, run_dir: &PathBuf) {
        let results_guard = self.results.read().await;
        let best_guard = self.best_result.read().await;

        if let Some(best) = best_guard.as_ref() {
            let best_config_path = run_dir.join("best_config.json");
            if let Ok(json) = serde_json::to_string_pretty(&best.config) {
                let _ = std::fs::write(&best_config_path, json);
            }

            let best_score_path = run_dir.join("best_score.json");
            if let Ok(json) = serde_json::to_string_pretty(&best.aggregate_score) {
                let _ = std::fs::write(&best_score_path, json);
            }

            let summary = self.generate_best_config_summary(best);
            let summary_path = run_dir.join("BEST_PROMPT.txt");
            let _ = std::fs::write(&summary_path, summary);

            let prompt =
                crate::prompt_eval::variation::build_system_prompt_with_config(&best.config);
            let prompt_path = run_dir.join("generated_prompt.txt");
            let _ = std::fs::write(&prompt_path, prompt);
        }

        let all_path = run_dir.join("all_iterations.json");
        if let Ok(json) = serde_json::to_string_pretty(&*results_guard) {
            let _ = std::fs::write(&all_path, json);
        }

        let analysis = self.generate_analysis_report(&results_guard, &best_guard);
        let analysis_path = run_dir.join("analysis.md");
        let _ = std::fs::write(&analysis_path, analysis);

        self.save_leaderboard_file(run_dir).await;

        println!("\nFinal results saved to: {}", run_dir.display());
        println!("Total runtime: {:.1}s", total_duration_secs);
    }

    fn generate_best_config_summary(&self, best: &OptimizationResult) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# Best Prompt Configuration (Score: {:.3})\n\n",
            best.aggregate_score.avg_score
        ));
        out.push_str(&format!("Found at iteration: {}\n\n", best.iteration));

        out.push_str("## Configuration\n\n");
        out.push_str(&format!(
            "- Identity Style: {:?}\n",
            best.config.identity_style
        ));
        out.push_str(&format!("- Tone: {:?}\n", best.config.tone));
        out.push_str(&format!("- Verbosity: {:?}\n", best.config.verbosity));
        out.push_str(&format!(
            "- Workflow Style: {:?}\n",
            best.config.workflow_style
        ));
        out.push_str(&format!(
            "- Tool Guidance: {:?}\n",
            best.config.tool_guidance
        ));
        out.push_str(&format!(
            "- Priority Style: {:?}\n",
            best.config.priority_style
        ));

        out.push_str("\n## New Variants\n\n");
        out.push_str(&format!(
            "- Identity Variant: {:?}\n",
            best.config.identity_variant
        ));
        out.push_str(&format!(
            "- Priorities Variant: {:?}\n",
            best.config.priorities_variant
        ));
        out.push_str(&format!(
            "- Safety Variant: {:?}\n",
            best.config.priority_style
        ));
        out.push_str(&format!(
            "- Workflow Variant: {:?}\n",
            best.config.workflow_variant
        ));
        out.push_str(&format!(
            "- Communication Variant: {:?}\n",
            best.config.communication_variant
        ));

        out.push_str("\n## Efficiency Settings\n\n");
        out.push_str(&format!(
            "- Decision Strategy: {:?}\n",
            best.config.decision_strategy
        ));
        out.push_str(&format!(
            "- Context Behavior: {:?}\n",
            best.config.context_behavior
        ));
        out.push_str(&format!(
            "- Validation Style: {:?}\n",
            best.config.validation_style
        ));
        out.push_str(&format!(
            "- Response Brevity: {:?}\n",
            best.config.response_brevity
        ));
        out.push_str(&format!(
            "- Retry Philosophy: {:?}\n",
            best.config.retry_philosophy
        ));
        out.push_str(&format!(
            "- Tool Philosophy: {:?}\n",
            best.config.tool_philosophy
        ));

        out.push_str("\n## Sections\n\n");
        out.push_str(&format!(
            "- Include UI: {}\n",
            best.config.include_ui_section
        ));
        out.push_str(&format!(
            "- Include Codebase Nav: {}\n",
            best.config.include_codebase_nav
        ));
        out.push_str(&format!(
            "- Include Parallel Tools: {}\n",
            best.config.include_parallel_tools
        ));
        out.push_str(&format!(
            "- Include Editing Rules: {}\n",
            best.config.include_editing_rules
        ));
        out.push_str(&format!(
            "- Include Validation: {}\n",
            best.config.include_validation
        ));

        out
    }

    async fn print_final_summary(&self) {
        let results_guard = self.results.read().await;
        let best_guard = self.best_result.read().await;

        println!(
            "\n╔═══════════════════════════════════════════════════════════════════════════════╗"
        );
        println!(
            "║                            FINAL SUMMARY                                     ║"
        );
        println!(
            "╠═══════════════════════════════════════════════════════════════════════════════╣"
        );
        println!(
            "║                                                                               ║"
        );

        let mut sorted: Vec<_> = results_guard.iter().cloned().collect();
        sorted.sort_by(|a, b| {
            b.aggregate_score
                .avg_score
                .partial_cmp(&a.aggregate_score.avg_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let total = sorted.len();

        println!(
            "║  TOP 10 PROMPTS                                                               ║"
        );
        println!(
            "║                                                                               ║"
        );
        println!("║  ┌─────┬────────┬───────────┬───────────┬───────────┬───────────┬───────────┬───────────┬───────────┬─────┐  ║");
        println!("║  │Rank │ Score │  Correct │   Tool   │  Effic.  │  Safety  │  Format  │  Identity │  Tone    │Iter│  ║");
        println!("║  ├─────┼────────┼───────────┼───────────┼───────────┼───────────┼───────────┼───────────┼───────────┼─────┤  ║");

        for (i, result) in sorted.iter().take(10).enumerate() {
            let r = result;
            let identity_short = match r.config.identity_variant {
                crate::prompt_eval::variation::IdentityVariant::Gpt5Efficient => "GPT5E",
                crate::prompt_eval::variation::IdentityVariant::ClaudeStyle => "Claude",
                crate::prompt_eval::variation::IdentityVariant::Minimal => "Mini",
                crate::prompt_eval::variation::IdentityVariant::Standard => "Std",
                crate::prompt_eval::variation::IdentityVariant::Detailed => "Dtl",
                crate::prompt_eval::variation::IdentityVariant::Technical => "Tech",
                crate::prompt_eval::variation::IdentityVariant::Casual => "Cas",
            };
            let tone_short = match r.config.tone {
                crate::prompt_eval::variation::Tone::Direct => "Dir",
                crate::prompt_eval::variation::Tone::Dry => "Dry",
                crate::prompt_eval::variation::Tone::Friendly => "Frn",
                crate::prompt_eval::variation::Tone::Witty => "Wit",
                crate::prompt_eval::variation::Tone::Calm => "Clm",
                crate::prompt_eval::variation::Tone::Assertive => "Asr",
            };
            println!("║  │{:>4} │ {:.3}  │   {:.2}    │   {:.2}    │   {:.2}    │   {:.2}    │   {:.2}    │ {}    │ {}   │{:>4} │  ║",
                i + 1,
                r.aggregate_score.avg_score,
                r.aggregate_score.avg_correctness,
                r.aggregate_score.avg_tool_accuracy,
                r.aggregate_score.avg_efficiency,
                r.aggregate_score.avg_safety,
                r.aggregate_score.avg_format,
                identity_short,
                tone_short,
                r.iteration
            );
        }
        println!("║  └─────┴────────┴───────────┴───────────┴───────────┴───────────┴───────────┴───────────┴───────────┴─────┘  ║");

        if let Some(best) = best_guard.as_ref() {
            println!(
                "║                                                                               ║"
            );
            println!(
                "║  ★ BEST: {:.3} (iteration {})                                              ║",
                best.aggregate_score.avg_score, best.iteration
            );
            println!(
                "║                                                                               ║"
            );
            println!("║  Breakdown: C:{:.2} | T:{:.2} | E:{:.2} | S:{:.2} | F:{:.2}                        ║",
                best.aggregate_score.avg_correctness,
                best.aggregate_score.avg_tool_accuracy,
                best.aggregate_score.avg_efficiency,
                best.aggregate_score.avg_safety,
                best.aggregate_score.avg_format
            );
            println!(
                "║                                                                               ║"
            );
            println!(
                "║  Config Variants:                                                            ║"
            );
            println!(
                "║    Identity: {:?}                                              ║",
                best.config.identity_variant
            );
            println!(
                "║    Priorities: {:?}                                          ║",
                best.config.priorities_variant
            );
            println!(
                "║    Safety: {:?}                                                ║",
                best.config.safety_variant
            );
            println!(
                "║    Workflow: {:?}                                              ║",
                best.config.workflow_variant
            );
            println!(
                "║    Communication: {:?}                                        ║",
                best.config.communication_variant
            );
            println!(
                "║                                                                               ║"
            );
            println!("║  Efficiency Settings:                                                          ║");
            println!(
                "║    Strategy: {:?} | Context: {:?} | Validation: {:?}                   ║",
                best.config.decision_strategy,
                best.config.context_behavior,
                best.config.validation_style
            );
            println!(
                "║    Brevity: {:?} | Retry: {:?} | ToolPhil: {:?}                       ║",
                best.config.response_brevity,
                best.config.retry_philosophy,
                best.config.tool_philosophy
            );
        }

        println!(
            "║                                                                               ║"
        );
        println!(
            "║  Total iterations: {}                                                         ║",
            total
        );
        println!(
            "╚═══════════════════════════════════════════════════════════════════════════════╝"
        );
    }

    pub async fn get_best(&self) -> Option<OptimizationResult> {
        self.best_result.read().await.clone()
    }

    pub async fn get_all_results(&self) -> Vec<OptimizationResult> {
        self.results.read().await.clone()
    }
}
