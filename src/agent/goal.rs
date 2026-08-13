/// Durable per-session goal state with revision fencing, plus an
/// in-memory activation latch. Ported from DeepSeek Harness's
/// `goal` / `goal-round-driver` / `tool-goal` family.
///
/// Key DSH properties preserved:
/// - The durable row is the single current objective; every mutation is
///   compare-and-set fenced on `revision`.
/// - Phases are a closed set: active / paused / blocked / complete,
///   where "blocked" subsumes all stop reasons with a policy code.
/// - `blocked` is mechanically rejected below `blocked_after_rounds`
///   consecutive started rounds (`GOAL_BLOCK_THRESHOLD`).
/// - Activation (armed/disarmed) is never persisted: a process restart
///   or a resumed session keeps the objective but never auto-continues
///   work. Only `create_goal` and `update_goal(resume)` arm.
use crate::error::{OSAgentError, Result};
use crate::storage::models::{Goal, GoalCasOutcome, GoalPhase};
use crate::storage::SqliteStorage;
use dashmap::DashMap;
use std::sync::Arc;

pub const DEFAULT_MAX_ROUNDS: i64 = 5;
pub const DEFAULT_BLOCKED_AFTER_ROUNDS: i64 = 3;

/// Stable error codes surfaced to the model, mirroring DSH's closed
/// failure-code vocabulary.
pub const GOAL_NOT_FOUND: &str = "GOAL_NOT_FOUND";
pub const GOAL_ALREADY_EXISTS: &str = "GOAL_ALREADY_EXISTS";
pub const GOAL_CONFLICT: &str = "GOAL_CONFLICT";
pub const GOAL_INVALID_PHASE: &str = "GOAL_INVALID_PHASE";
pub const GOAL_BLOCK_THRESHOLD: &str = "GOAL_BLOCK_THRESHOLD";
pub const GOAL_ROUNDS_EXHAUSTED: &str = "GOAL_ROUNDS_EXHAUSTED";

fn goal_error(code: &str, message: impl Into<String>) -> OSAgentError {
    OSAgentError::ToolExecution(format!("{}: {}", code, message.into()))
}

pub struct GoalStore {
    storage: Arc<SqliteStorage>,
    /// Armed sessions: objective may auto-continue into further rounds.
    /// In-memory only — a restart disarms every session.
    armed: DashMap<String, ()>,
    blocked_after_rounds: i64,
}

impl GoalStore {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            storage,
            armed: DashMap::new(),
            blocked_after_rounds: DEFAULT_BLOCKED_AFTER_ROUNDS,
        }
    }

    pub fn with_blocked_after_rounds(mut self, rounds: i64) -> Self {
        self.blocked_after_rounds = rounds.max(1);
        self
    }

    pub fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        self.storage.load_goal(session_id)
    }

    pub fn is_armed(&self, session_id: &str) -> bool {
        self.armed.contains_key(session_id)
    }

    pub fn arm(&self, session_id: &str) {
        self.armed.insert(session_id.to_string(), ());
    }

    pub fn disarm(&self, session_id: &str) {
        self.armed.remove(session_id);
    }

    /// Create the current objective. Fails when one already exists; an
    /// unfinished goal is never silently replaced. Arms the session.
    pub fn create(&self, session_id: &str, objective: &str, max_rounds: i64) -> Result<Goal> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(goal_error(GOAL_NOT_FOUND, "Objective cannot be empty"));
        }
        let max_rounds = max_rounds.clamp(1, 100);
        match self.storage.create_goal_row(session_id, objective, max_rounds)? {
            Some(goal) => {
                self.arm(session_id);
                Ok(goal)
            }
            None => Err(goal_error(
                GOAL_ALREADY_EXISTS,
                "A goal already exists for this session; update it instead",
            )),
        }
    }

    /// Validate a phase transition. Complete and blocked are terminal
    /// from the model's perspective (they disarm); paused can resume.
    fn validate_transition(from: GoalPhase, to: GoalPhase) -> bool {
        match (from, to) {
            (GoalPhase::Complete, _) => false,
            (GoalPhase::Blocked, GoalPhase::Active) | (GoalPhase::Blocked, GoalPhase::Paused) => false,
            (_, _) => true,
        }
    }

    /// Apply a mutation through the revision fence. Returns the updated
    /// goal; conflicts and absences surface as stable error codes.
    pub fn update(
        &self,
        session_id: &str,
        revision: i64,
        mutate: impl FnOnce(&mut Goal) -> Result<()>,
    ) -> Result<Goal> {
        let mut mutation_error: Option<String> = None;
        let outcome = self.storage.update_goal_cas(session_id, revision, |goal| {
            match mutate(goal) {
                Ok(()) => Ok(()),
                Err(error) => {
                    mutation_error = Some(error.to_string());
                    Err(error.to_string())
                }
            }
        })?;
        if let Some(error) = mutation_error {
            return Err(OSAgentError::ToolExecution(error));
        }
        match outcome {
            GoalCasOutcome::Stored { goal } => Ok(goal),
            GoalCasOutcome::Conflict { current } => Err(goal_error(
                GOAL_CONFLICT,
                format!(
                    "Goal changed since revision {} (current revision {}); re-read and retry",
                    revision, current.revision
                ),
            )),
            GoalCasOutcome::Missing => Err(goal_error(GOAL_NOT_FOUND, "No goal for this session")),
        }
    }

    /// Model-facing update entry point. Actions: edit / pause / resume /
    /// complete / blocked. `blocked` is rejected below the consecutive
    /// rounds threshold and requires a reason; terminal phases disarm
    /// the session, resume/complete of an unfinished goal arms it.
    pub fn apply_action(
        &self,
        session_id: &str,
        revision: i64,
        action: &str,
        objective: Option<&str>,
        blocked_reason: Option<&str>,
    ) -> Result<Goal> {
        let current = self
            .get(session_id)?
            .ok_or_else(|| goal_error(GOAL_NOT_FOUND, "No goal for this session"))?;
        if current.revision != revision {
            return Err(goal_error(
                GOAL_CONFLICT,
                format!(
                    "Goal changed since revision {} (current revision {}); re-read and retry",
                    revision, current.revision
                ),
            ));
        }

        match action.to_ascii_lowercase().as_str() {
            "edit" => {
                let new_objective = objective
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| goal_error(GOAL_NOT_FOUND, "edit requires a non-empty objective"))?;
                let goal = self.update(session_id, revision, |goal| {
                    goal.objective = new_objective.to_string();
                    Ok(())
                })?;
                Ok(goal)
            }
            "pause" => {
                if !Self::validate_transition(current.phase, GoalPhase::Paused) {
                    return Err(goal_error(
                        GOAL_INVALID_PHASE,
                        format!("Cannot pause a goal in phase {}", current.phase.as_str()),
                    ));
                }
                self.disarm(session_id);
                self.update(session_id, revision, |goal| {
                    goal.phase = GoalPhase::Paused;
                    goal.blocked_reason = None;
                    goal.policy_code = None;
                    Ok(())
                })
            }
            "resume" => {
                if !Self::validate_transition(current.phase, GoalPhase::Active) {
                    return Err(goal_error(
                        GOAL_INVALID_PHASE,
                        format!("Cannot resume a goal in phase {}", current.phase.as_str()),
                    ));
                }
                self.arm(session_id);
                self.update(session_id, revision, |goal| {
                    goal.phase = GoalPhase::Active;
                    goal.blocked_reason = None;
                    goal.policy_code = None;
                    Ok(())
                })
            }
            "complete" => {
                self.disarm(session_id);
                self.update(session_id, revision, |goal| {
                    goal.phase = GoalPhase::Complete;
                    goal.blocked_reason = None;
                    goal.policy_code = None;
                    Ok(())
                })
            }
            "blocked" => {
                if !Self::validate_transition(current.phase, GoalPhase::Blocked) {
                    return Err(goal_error(
                        GOAL_INVALID_PHASE,
                        format!("Cannot block a goal in phase {}", current.phase.as_str()),
                    ));
                }
                if current.rounds_started < self.blocked_after_rounds {
                    return Err(goal_error(
                        GOAL_BLOCK_THRESHOLD,
                        format!(
                            "Goal can only be marked blocked after {} rounds ({} started); \
                             keep working or mark it complete instead",
                            self.blocked_after_rounds, current.rounds_started
                        ),
                    ));
                }
                let reason = blocked_reason
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| {
                        goal_error(GOAL_NOT_FOUND, "blocked requires a blocked_reason")
                    })?;
                self.disarm(session_id);
                self.update(session_id, revision, |goal| {
                    goal.phase = GoalPhase::Blocked;
                    goal.blocked_reason = Some(reason.to_string());
                    goal.policy_code = Some("goal_blocked".to_string());
                    Ok(())
                })
            }
            other => Err(goal_error(
                GOAL_INVALID_PHASE,
                format!("Unknown action '{}' (expected edit/pause/resume/complete/blocked)", other),
            )),
        }
    }

    pub fn clear(&self, session_id: &str) -> Result<bool> {
        self.disarm(session_id);
        self.storage.clear_goal(session_id)
    }

    /// Round driver reservation: CAS-increment `rounds_started` and
    /// return the reserved round number. `None` means no round is due
    /// (not armed, wrong phase, or rounds exhausted).
    pub fn reserve_round(&self, session_id: &str) -> Result<Option<(i64, Goal)>> {
        let Some(current) = self.get(session_id)? else {
            return Ok(None);
        };
        if !self.is_armed(session_id) {
            return Ok(None);
        }
        if current.phase != GoalPhase::Active {
            return Ok(None);
        }
        if current.rounds_started >= current.max_rounds {
            self.disarm(session_id);
            return Ok(None);
        }

        let revision = current.revision;
        let mut reserved: Option<(i64, Goal)> = None;
        match self.storage.update_goal_cas(session_id, revision, |goal| {
            goal.rounds_started += 1;
            reserved = Some((goal.rounds_started, goal.clone()));
            Ok(())
        })? {
            GoalCasOutcome::Stored { goal: _ } => Ok(reserved),
            GoalCasOutcome::Conflict { current: _ } | GoalCasOutcome::Missing => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> GoalStore {
        let storage = Arc::new(SqliteStorage::new_in_memory().expect("in-memory storage"));
        GoalStore::new(storage)
    }

    #[test]
    fn create_arms_and_fences() {
        let store = store();
        let goal = store.create("s1", "Fix the build", 5).unwrap();
        assert_eq!(goal.revision, 1);
        assert_eq!(goal.phase, GoalPhase::Active);
        assert!(store.is_armed("s1"));

        let err = store.create("s1", "Another goal", 5).unwrap_err();
        assert!(err.to_string().contains(GOAL_ALREADY_EXISTS));
    }

    #[test]
    fn stale_revision_conflicts() {
        let store = store();
        let goal = store.create("s1", "Objective", 5).unwrap();

        let err = store
            .apply_action("s1", goal.revision + 5, "pause", None, None)
            .unwrap_err();
        assert!(err.to_string().contains(GOAL_CONFLICT));
    }

    #[test]
    fn blocked_rejected_below_threshold() {
        let store = store();
        let goal = store.create("s1", "Objective", 5).unwrap();

        let err = store
            .apply_action("s1", goal.revision, "blocked", None, Some("too hard"))
            .unwrap_err();
        assert!(err.to_string().contains(GOAL_BLOCK_THRESHOLD));

        // After enough rounds the same action succeeds.
        for _ in 0..3 {
            store.reserve_round("s1").unwrap();
        }
        let current = store.get("s1").unwrap().unwrap();
        let updated = store
            .apply_action("s1", current.revision, "blocked", None, Some("too hard"))
            .unwrap();
        assert_eq!(updated.phase, GoalPhase::Blocked);
        assert!(!store.is_armed("s1"));
    }

    #[test]
    fn resume_rearms_and_pause_disarms() {
        let store = store();
        let goal = store.create("s1", "Objective", 5).unwrap();

        let paused = store
            .apply_action("s1", goal.revision, "pause", None, None)
            .unwrap();
        assert_eq!(paused.phase, GoalPhase::Paused);
        assert!(!store.is_armed("s1"));

        let resumed = store
            .apply_action("s1", paused.revision, "resume", None, None)
            .unwrap();
        assert_eq!(resumed.phase, GoalPhase::Active);
        assert!(store.is_armed("s1"));
    }

    #[test]
    fn rounds_are_reserved_only_while_armed_and_active() {
        let store = store();
        store.create("s1", "Objective", 2).unwrap();

        let first = store.reserve_round("s1").unwrap();
        assert_eq!(first.as_ref().map(|(round, _)| *round), Some(1));
        let second = store.reserve_round("s1").unwrap();
        assert_eq!(second.as_ref().map(|(round, _)| *round), Some(2));

        // Rounds exhausted: nothing more to reserve, and the session
        // disarms itself.
        assert!(store.reserve_round("s1").unwrap().is_none());
        assert!(!store.is_armed("s1"));
    }

    #[test]
    fn clearing_removes_and_disarms() {
        let store = store();
        store.create("s1", "Objective", 5).unwrap();
        assert!(store.clear("s1").unwrap());
        assert!(store.get("s1").unwrap().is_none());
        assert!(!store.is_armed("s1"));
    }

    #[test]
    fn completed_goal_cannot_transition_further() {
        let store = store();
        let goal = store.create("s1", "Objective", 5).unwrap();
        let done = store
            .apply_action("s1", goal.revision, "complete", None, None)
            .unwrap();
        assert_eq!(done.phase, GoalPhase::Complete);

        let err = store
            .apply_action("s1", done.revision, "resume", None, None)
            .unwrap_err();
        assert!(err.to_string().contains(GOAL_INVALID_PHASE));
    }
}
