//! Session goal control (see crate::agent::goal::GoalStore)
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn get_session_goal(&self, session_id: &str) -> Result<Option<crate::storage::Goal>> {
        self.goal_store.get(session_id)
    }

    pub fn is_session_goal_armed(&self, session_id: &str) -> bool {
        self.goal_store.is_armed(session_id)
    }

    pub async fn clear_session_goal(&self, session_id: &str) -> Result<bool> {
        self.remove_pending_goal_rounds(session_id)?;
        self.goal_store.clear(session_id)
    }

    pub(super) fn remove_pending_goal_rounds(&self, session_id: &str) -> Result<()> {
        for item in self.storage.list_queued_messages(session_id)? {
            if item.status == crate::storage::QueuedMessageStatus::Pending
                && item.client_message_id.starts_with("goal-round-")
            {
                self.storage
                    .delete_session_queued_message(session_id, &item.id)?;
            }
        }
        Ok(())
    }

    pub async fn control_session_goal(
        self: Arc<Self>,
        session_id: &str,
        action: &str,
        objective: Option<&str>,
        max_rounds: Option<i64>,
    ) -> Result<crate::storage::Goal> {
        let session = self
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        if session.agent_type != "primary" {
            return Err(OSAgentError::Session(
                "Goal commands require a primary session".to_string(),
            ));
        }
        if action != "pause" && self.is_session_busy(session_id) {
            return Err(OSAgentError::Session(
                "Stop the current turn before creating or resuming a goal".to_string(),
            ));
        }
        let goal = match action {
            "create" => {
                let objective = objective
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| {
                        OSAgentError::Config("A goal requires an objective".to_string())
                    })?;
                if self
                    .goal_store
                    .get(session_id)?
                    .is_some_and(|goal| goal.phase == crate::storage::GoalPhase::Complete)
                {
                    self.clear_session_goal(session_id).await?;
                }
                self.goal_store.create(
                    session_id,
                    objective,
                    max_rounds.unwrap_or(crate::agent::goal::DEFAULT_MAX_ROUNDS),
                )?
            }
            "resume" => {
                self.remove_pending_goal_rounds(session_id)?;
                self.goal_store.resume_by_user(session_id, max_rounds)?
            }
            "pause" => {
                let current = self
                    .goal_store
                    .get(session_id)?
                    .ok_or_else(|| OSAgentError::Session("No goal to pause".to_string()))?;
                let goal = self.goal_store.apply_action(
                    session_id,
                    current.revision,
                    "pause",
                    None,
                    None,
                )?;
                self.remove_pending_goal_rounds(session_id)?;
                goal
            }
            _ => {
                return Err(OSAgentError::Config(
                    "Expected create, pause, or resume".to_string(),
                ))
            }
        };
        if action != "pause" {
            self.resume_session_queue(session_id);
            self.maybe_queue_goal_round(&session).await?;
            self.clone()
                .spawn_next_queued_message_run(session_id.to_string(), "web".to_string())?;
        }
        Ok(self.goal_store.get(session_id)?.unwrap_or(goal))
    }
}
