//! Session persona binding (see crate::agent::persona)
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub(super) fn active_persona_from_session(session: &Session) -> Option<ActivePersona> {
        session
            .metadata
            .get("active_persona")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn list_personas(&self) -> Vec<persona::PersonaOption> {
        persona::persona_options()
    }

    pub async fn get_session_persona(&self, session_id: &str) -> Result<Option<ActivePersona>> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        Ok(Self::active_persona_from_session(&session))
    }

    pub async fn set_session_persona(
        &self,
        session_id: &str,
        persona_id: String,
        roleplay_character: Option<String>,
    ) -> Result<ActivePersona> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        let active = persona::resolve_active_persona(&persona_id, roleplay_character)
            .map_err(OSAgentError::ToolExecution)?;
        Self::write_active_persona_to_session(&mut session, Some(active.clone()))?;
        self.session_manager.update_session(&session).await?;

        Ok(active)
    }

    pub async fn reset_session_persona(&self, session_id: &str) -> Result<()> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        Self::write_active_persona_to_session(&mut session, None)?;
        self.session_manager.update_session(&session).await?;
        Ok(())
    }
}
