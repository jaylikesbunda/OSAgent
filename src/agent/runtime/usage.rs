//! Spend limits and the cost guard.
//!
//! Limits live in `[usage]` (session / daily / monthly USD). When one trips,
//! the configured [`crate::config::BudgetAction`] decides between routing the
//! turn through the `small_model` slot (the cost guard downgrade) or refusing
//! it outright. Budget checks read the recorded `usage_events` totals, so
//! they survive restarts and count subagent spend too.

use super::*;
use chrono::Datelike;

pub(crate) enum BudgetDecision {
    Allow,
    Downgrade { limit: String, cost_usd: f64 },
    Block { limit: String, cost_usd: f64 },
}

/// Unix timestamp of the current UTC day start.
fn utc_day_start() -> i64 {
    let now = chrono::Utc::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|midnight| midnight.and_utc().timestamp())
        .unwrap_or_else(|| now.timestamp() - 86_400)
}

/// Unix timestamp of the current UTC month start.
fn utc_month_start() -> i64 {
    let now = chrono::Utc::now();
    let first = now
        .date_naive()
        .with_day(1)
        .and_then(|date| date.and_hms_opt(0, 0, 0));
    first
        .map(|midnight| midnight.and_utc().timestamp())
        .unwrap_or_else(|| now.timestamp() - 31 * 86_400)
}

impl AgentRuntime {
    /// Evaluate the configured spend limits against recorded usage for this
    /// session (and the current day/month windows).
    pub(crate) async fn budget_decision(&self, session_id: &str) -> BudgetDecision {
        let (enabled, session_limit, daily_limit, monthly_limit, action) = {
            let config = self.config.read().await;
            (
                config.usage.enabled,
                config.usage.session_limit_usd.unwrap_or(0.0).max(0.0),
                config.usage.daily_limit_usd.unwrap_or(0.0).max(0.0),
                config.usage.monthly_limit_usd.unwrap_or(0.0).max(0.0),
                config.usage.limit_action,
            )
        };
        if !enabled || (session_limit <= 0.0 && daily_limit <= 0.0 && monthly_limit <= 0.0) {
            return BudgetDecision::Allow;
        }

        let mut tripped: Option<(String, f64)> = None;

        if session_limit > 0.0 {
            if let Ok(totals) = self.storage.usage_totals(Some(session_id), None) {
                if totals.cost_usd >= session_limit {
                    tripped = Some((
                        format!("session limit ${:.2}", session_limit),
                        totals.cost_usd,
                    ));
                }
            }
        }
        if tripped.is_none() && daily_limit > 0.0 {
            if let Ok(totals) = self.storage.usage_totals(None, Some(utc_day_start())) {
                if totals.cost_usd >= daily_limit {
                    tripped = Some((format!("daily limit ${:.2}", daily_limit), totals.cost_usd));
                }
            }
        }
        if tripped.is_none() && monthly_limit > 0.0 {
            if let Ok(totals) = self.storage.usage_totals(None, Some(utc_month_start())) {
                if totals.cost_usd >= monthly_limit {
                    tripped = Some((
                        format!("monthly limit ${:.2}", monthly_limit),
                        totals.cost_usd,
                    ));
                }
            }
        }

        match tripped {
            None => BudgetDecision::Allow,
            Some((limit, cost_usd)) => match action {
                crate::config::BudgetAction::Downgrade => {
                    BudgetDecision::Downgrade { limit, cost_usd }
                }
                crate::config::BudgetAction::Block => BudgetDecision::Block { limit, cost_usd },
            },
        }
    }

    /// Full usage/cost summary (the `/usage` report): lifetime / today /
    /// this-month totals, the current session, per-model and per-session
    /// breakdowns, and the configured limits.
    pub async fn usage_summary(&self, session_id: Option<&str>) -> Result<UsageSummary> {
        let total = self.storage.usage_totals(None, None)?;
        let today = self.storage.usage_totals(None, Some(utc_day_start()))?;
        let this_month = self.storage.usage_totals(None, Some(utc_month_start()))?;
        let session = match session_id {
            Some(session_id) => Some(self.storage.usage_totals(Some(session_id), None)?),
            None => None,
        };
        let by_model = self
            .storage
            .usage_by_model(None, None, 25)?
            .into_iter()
            .map(|(provider, model, totals)| UsageModelRow {
                provider,
                model,
                totals,
            })
            .collect();
        let by_session = self
            .storage
            .usage_by_session(None, 25)?
            .into_iter()
            .map(|(session_id, totals)| UsageSessionRow { session_id, totals })
            .collect();

        let config = self.config.read().await;
        Ok(UsageSummary {
            total,
            today,
            this_month,
            session,
            by_model,
            by_session,
            limits: UsageLimits {
                session_limit_usd: config.usage.session_limit_usd,
                daily_limit_usd: config.usage.daily_limit_usd,
                monthly_limit_usd: config.usage.monthly_limit_usd,
                limit_action: match config.usage.limit_action {
                    crate::config::BudgetAction::Downgrade => "downgrade".to_string(),
                    crate::config::BudgetAction::Block => "block".to_string(),
                },
            },
        })
    }

    /// Human-readable /usage report (CLI output and the chat command).
    pub fn format_usage_report(summary: &UsageSummary, session_id: Option<&str>) -> String {
        let mut out = String::new();
        out.push_str("Usage & costs\n");
        out.push_str(&format!(
            "  total:      ${:.4}  ({} requests, {} tokens)\n",
            summary.total.cost_usd,
            summary.total.requests,
            summary.total.tokens_total()
        ));
        out.push_str(&format!(
            "  today:      ${:.4}  ({} requests)\n",
            summary.today.cost_usd, summary.today.requests
        ));
        out.push_str(&format!(
            "  this month: ${:.4}  ({} requests)\n",
            summary.this_month.cost_usd, summary.this_month.requests
        ));
        if let (Some(session_id), Some(session)) = (session_id, summary.session.as_ref()) {
            out.push_str(&format!(
                "  session {}: ${:.4}  ({} requests, {} tokens)\n",
                session_id,
                session.cost_usd,
                session.requests,
                session.tokens_total()
            ));
        }

        let limits = &summary.limits;
        if limits.session_limit_usd.is_some()
            || limits.daily_limit_usd.is_some()
            || limits.monthly_limit_usd.is_some()
        {
            out.push_str(&format!(
                "  limits: session={:.2} daily={:.2} monthly={:.2} (action: {})\n",
                limits.session_limit_usd.unwrap_or(0.0),
                limits.daily_limit_usd.unwrap_or(0.0),
                limits.monthly_limit_usd.unwrap_or(0.0),
                limits.limit_action
            ));
        } else {
            out.push_str("  limits: none configured\n");
        }

        if !summary.by_model.is_empty() {
            out.push_str("\nBy model:\n");
            for row in &summary.by_model {
                out.push_str(&format!(
                    "  {}/{}: ${:.4} ({} requests, in={} out={} cache_read={})\n",
                    row.provider,
                    row.model,
                    row.totals.cost_usd,
                    row.totals.requests,
                    row.totals.input_tokens,
                    row.totals.output_tokens,
                    row.totals.cached_read_tokens
                ));
            }
        }
        out
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageModelRow {
    pub provider: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: crate::storage::UsageTotals,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageSessionRow {
    pub session_id: String,
    #[serde(flatten)]
    pub totals: crate::storage::UsageTotals,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageLimits {
    pub session_limit_usd: Option<f64>,
    pub daily_limit_usd: Option<f64>,
    pub monthly_limit_usd: Option<f64>,
    pub limit_action: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageSummary {
    pub total: crate::storage::UsageTotals,
    pub today: crate::storage::UsageTotals,
    pub this_month: crate::storage::UsageTotals,
    pub session: Option<crate::storage::UsageTotals>,
    pub by_model: Vec<UsageModelRow>,
    pub by_session: Vec<UsageSessionRow>,
    pub limits: UsageLimits,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_windows_start_in_the_past() {
        let now = chrono::Utc::now().timestamp();
        assert!(utc_day_start() <= now);
        assert!(utc_day_start() > now - 86_400);
        assert!(utc_month_start() <= utc_day_start());
        assert!(utc_month_start() > now - 32 * 86_400);
    }
}
