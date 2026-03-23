use crate::error::{OSAgentError, Result};
use crate::skills::SkillLoader;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct SkillTool {
    loader: Arc<SkillLoader>,
}

impl SkillTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load a specialized skill that provides domain-specific instructions and workflows"
    }

    fn when_to_use(&self) -> &str {
        "Use to load domain-specific skills for specialized tasks like GitHub operations, code review, testing, etc."
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if no specific domain skill is needed for the task"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Load GitHub skill".to_string(),
                input: json!({
                    "name": "github"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Load code review skill".to_string(),
                input: json!({
                    "name": "code-review"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill from available_skills"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let skill_name = args["name"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'name' parameter".to_string()))?;

        let skill = self.loader.get(skill_name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Skill not found: {}", skill_name))
        })?;

        let mut output = format!(
            "<skill_content name=\"{}\">\n{}\n</skill_content>",
            skill.name, skill.content
        );

        if !skill.scripts.is_empty() {
            output.push_str("\n\n<skill_scripts>\n");
            for (name, path) in &skill.scripts {
                output.push_str(&format!("- {}: {}\n", name, path.to_string_lossy()));
            }
            output.push_str("</skill_scripts>");
        }

        if !skill.references.is_empty() {
            output.push_str("\n\n<skill_references>\n");
            for (name, path) in &skill.references {
                output.push_str(&format!("- {}: {}\n", name, path.to_string_lossy()));
            }
            output.push_str("</skill_references>");
        }

        Ok(output)
    }
}

pub struct SkillListTool {
    loader: Arc<SkillLoader>,
}

impl SkillListTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn description(&self) -> &str {
        "List all available skills that can be loaded"
    }

    fn when_to_use(&self) -> &str {
        "Use to discover what skills are available before loading one"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if you already know which skill you need"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let skills = self.loader.list();

        if skills.is_empty() {
            return Ok("No skills available.".to_string());
        }

        let mut output = "Available skills:\n".to_string();
        for skill in skills {
            output.push_str(&format!("- **{}**: {}", skill.name, skill.description));

            if let Some(ref meta) = skill.metadata {
                if let Some(ref emoji) = meta.emoji {
                    output.push_str(&format!(" [{}]", emoji));
                }
            }
            output.push('\n');
        }

        Ok(output)
    }
}
