/// Model-facing goal tools over `GoalStore`. Ported from DSH's
/// `tool-goal`: `get_goal` / `create_goal` / `update_goal` with the
/// same action vocabulary (edit/pause/resume/complete/blocked) and
/// stable error codes.
use crate::agent::goal::{GoalStore, DEFAULT_MAX_ROUNDS};
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct GetGoalTool {
    goals: Arc<GoalStore>,
}

impl GetGoalTool {
    pub fn new(goals: Arc<GoalStore>) -> Self {
        Self { goals }
    }
}

#[async_trait]
impl Tool for GetGoalTool {
    fn name(&self) -> &str {
        "get_goal"
    }

    fn description(&self) -> &str {
        "Read the current goal for this session (objective, phase, rounds started/max)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let session_id = args["session_id"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'session_id' parameter".to_string())
        })?;
        match self.goals.get(session_id)? {
            Some(goal) => Ok(serde_json::to_string_pretty(&json!({
                "goal": {
                    "id": goal.id,
                    "revision": goal.revision,
                    "objective": goal.objective,
                    "phase": goal.phase.as_str(),
                    "blocked_reason": goal.blocked_reason,
                    "rounds_started": goal.rounds_started,
                    "max_rounds": goal.max_rounds,
                }
            }))
            .unwrap_or_default()),
            None => Ok(serde_json::to_string_pretty(&json!({
                "goal": null,
                "hint": "Use create_goal to start one, or report to the user that no goal is set."
            }))
            .unwrap_or_default()),
        }
    }
}

pub struct CreateGoalTool {
    goals: Arc<GoalStore>,
}

impl CreateGoalTool {
    pub fn new(goals: Arc<GoalStore>) -> Self {
        Self { goals }
    }
}

#[async_trait]
impl Tool for CreateGoalTool {
    fn name(&self) -> &str {
        "create_goal"
    }

    fn description(&self) -> &str {
        "Create the single current goal for this session. Once created, the goal can auto-continue across rounds until complete, blocked, or paused. Fails if a goal already exists."
    }

    fn when_to_use(&self) -> &str {
        "Use when the user gives a multi-step objective worth pursuing over several turns, and no goal exists yet."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "description": "Session ID (injected automatically)"},
                "objective": {"type": "string", "description": "What the user wants, stated as a durable objective"},
                "max_rounds": {"type": "integer", "description": "Maximum goal rounds before the goal pauses", "default": DEFAULT_MAX_ROUNDS}
            },
            "required": ["objective"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let session_id = args["session_id"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'session_id' parameter".to_string())
        })?;
        let objective = args["objective"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'objective' parameter".to_string()))?;
        let max_rounds = args["max_rounds"].as_i64().unwrap_or(DEFAULT_MAX_ROUNDS);
        let goal = self.goals.create(session_id, objective, max_rounds)?;
        Ok(format!(
            "Goal created (id {}, revision {}): \"{}\" — up to {} rounds. Subsequent turns continue working toward it automatically.",
            goal.id, goal.revision, goal.objective, goal.max_rounds
        ))
    }
}

pub struct UpdateGoalTool {
    goals: Arc<GoalStore>,
}

impl UpdateGoalTool {
    pub fn new(goals: Arc<GoalStore>) -> Self {
        Self { goals }
    }
}

#[async_trait]
impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        "Update the session's goal: edit its objective, or change its phase (pause/resume/complete/blocked). Requires the revision from get_goal; blocked is rejected before enough rounds have run."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "description": "Session ID (injected automatically)"},
                "revision": {"type": "integer", "description": "Revision from get_goal"},
                "action": {"type": "string", "enum": ["edit", "pause", "resume", "complete", "blocked"]},
                "objective": {"type": "string", "description": "New objective (action=edit)"},
                "blocked_reason": {"type": "string", "description": "Why the goal is blocked (action=blocked)"}
            },
            "required": ["revision", "action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let session_id = args["session_id"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'session_id' parameter".to_string())
        })?;
        let revision = args["revision"].as_i64().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'revision' parameter".to_string())
        })?;
        let action = args["action"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'action' parameter".to_string())
        })?;
        let goal = self.goals.apply_action(
            session_id,
            revision,
            action,
            args["objective"].as_str(),
            args["blocked_reason"].as_str(),
        )?;
        Ok(format!(
            "Goal updated (revision {}): phase={} objective=\"{}\" rounds={}/{}",
            goal.revision,
            goal.phase.as_str(),
            goal.objective,
            goal.rounds_started,
            goal.max_rounds
        ))
    }
}
