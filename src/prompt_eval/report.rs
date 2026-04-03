use crate::prompt_eval::optimizer::OptimizationResult;
use crate::prompt_eval::scorer::AggregateScore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    pub total_iterations: usize,
    pub total_duration_secs: f64,
    pub best_result: Option<OptimizationResult>,
    pub test_cases_count: usize,
}

pub struct ReportGenerator;

impl ReportGenerator {
    pub fn generate_markdown(report: &RunReport) -> String {
        let mut md = String::new();

        md.push_str("# Prompt Optimization Report\n\n");

        md.push_str(&format!("**Run ID:** {}\n", report.run_id));
        md.push_str(&format!(
            "**Timestamp:** {}\n",
            report.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        md.push_str(&format!("**Iterations:** {}\n", report.total_iterations));
        md.push_str(&format!("**Test Cases:** {}\n", report.test_cases_count));
        md.push_str(&format!(
            "**Duration:** {:.1}s\n\n",
            report.total_duration_secs
        ));

        if let Some(best) = &report.best_result {
            md.push_str("## Best Result\n\n");
            md.push_str(&format!(
                "**Score:** {:.3}\n\n",
                best.aggregate_score.avg_score
            ));

            md.push_str("### Score Breakdown\n\n");
            md.push_str("| Metric | Value |\n");
            md.push_str("|--------|-------|\n");
            md.push_str(&format!(
                "| Overall | {:.3} |\n",
                best.aggregate_score.avg_score
            ));
            md.push_str(&format!(
                "| Correctness | {:.3} |\n",
                best.aggregate_score.avg_correctness
            ));
            md.push_str(&format!(
                "| Tool Accuracy | {:.3} |\n",
                best.aggregate_score.avg_tool_accuracy
            ));
            md.push_str(&format!(
                "| Efficiency | {:.3} |\n",
                best.aggregate_score.avg_efficiency
            ));
            md.push_str(&format!(
                "| Safety | {:.3} |\n",
                best.aggregate_score.avg_safety
            ));
            md.push_str(&format!(
                "| Tests Passed | {}/{} |\n\n",
                best.aggregate_score.passed_tests, best.aggregate_score.total_tests
            ));

            md.push_str("### Best Configuration\n\n");
            md.push_str("```json\n");
            if let Ok(json) = serde_json::to_string_pretty(&best.config) {
                md.push_str(&json);
            }
            md.push_str("\n```\n\n");
        } else {
            md.push_str("## No Results\n\nNo successful iterations completed.\n\n");
        }

        md
    }

    pub fn generate_json(report: &RunReport) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(report)
    }

    pub fn generate_csv(results: &[OptimizationResult]) -> String {
        let mut csv = String::from(
            "iteration,score,correctness,tool_accuracy,efficiency,safety,duration_secs\n",
        );

        for result in results {
            csv.push_str(&format!(
                "{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2}\n",
                result.iteration,
                result.aggregate_score.avg_score,
                result.aggregate_score.avg_correctness,
                result.aggregate_score.avg_tool_accuracy,
                result.aggregate_score.avg_efficiency,
                result.aggregate_score.avg_safety,
                result.duration_secs,
            ));
        }

        csv
    }

    pub fn save_report(report: &RunReport, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;

        let md_path = dir.join("report.md");
        std::fs::write(&md_path, Self::generate_markdown(report))?;

        let json_path = dir.join("report.json");
        std::fs::write(&json_path, Self::generate_json(report).unwrap_or_default())?;

        Ok(())
    }
}
