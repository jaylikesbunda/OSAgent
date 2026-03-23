use crate::agent::events::{generate_question_id, EventBus, QuestionChannel};
use crate::error::{OSAgentError, Result};
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionResponse {
    pub question_index: usize,
    pub answers: Vec<String>,
}

pub struct QuestionTool {
    event_bus: Arc<EventBus>,
}

impl QuestionTool {
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }
}

#[async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        "Use this tool when you need to ask the user questions during execution"
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to gather user preferences, clarify ambiguous instructions, or get decisions on implementation choices"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for simple yes/no questions where reasonable defaults exist"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Ask about framework choice".to_string(),
                input: json!({
                    "questions": [{
                        "question": "Which frontend framework would you like to use?",
                        "header": "Framework",
                        "options": [
                            {"label": "React (Recommended)", "description": "Component-based UI library"},
                            {"label": "Vue", "description": "Progressive framework"},
                            {"label": "Svelte", "description": "Compiled framework"}
                        ]
                    }]
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Ask multiple questions".to_string(),
                input: json!({
                    "questions": [{
                        "question": "Should I add type checking?",
                        "header": "Types",
                        "options": [
                            {"label": "TypeScript (Recommended)", "description": "Static type checking"},
                            {"label": "JavaScript", "description": "No type checking"}
                        ]
                    },
                    {"question": "Which testing framework?",
                        "header": "Testing",
                        "options": [
                            {"label": "Jest (Recommended)", "description": "Full testing solution"},
                            {"label": "Vitest", "description": "Vite-native testing"},
                            {"label": "None", "description": "Skip tests"}
                        ]
                    }]
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "Complete question"
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label (max 30 chars)"
                            },
                            "options": {
                                "type": "array",
                                "description": "Available choices",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Display text (1-5 words, concise)"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Explanation of choice"
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiple": {
                                "type": "boolean",
                                "description": "Allow selecting multiple choices"
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let questions_input = args["questions"]
            .as_array()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'questions' array".to_string()))?;

        let mut questions: Vec<Question> = Vec::new();
        for q in questions_input {
            let question = q["question"].as_str().ok_or_else(|| {
                OSAgentError::ToolExecution("Missing 'question' field".to_string())
            })?;

            let header = q["header"]
                .as_str()
                .ok_or_else(|| OSAgentError::ToolExecution("Missing 'header' field".to_string()))?;

            let options_input = q["options"].as_array().ok_or_else(|| {
                OSAgentError::ToolExecution("Missing 'options' field".to_string())
            })?;

            let mut options: Vec<QuestionOption> = Vec::new();
            for opt in options_input {
                let label = opt["label"].as_str().ok_or_else(|| {
                    OSAgentError::ToolExecution("Missing 'label' in option".to_string())
                })?;

                let description = opt["description"].as_str().unwrap_or("");

                options.push(QuestionOption {
                    label: label.to_string(),
                    description: description.to_string(),
                });
            }

            let multiple = q["multiple"].as_bool().unwrap_or(false);

            questions.push(Question {
                question: question.to_string(),
                header: header.to_string(),
                options,
                multiple,
            });
        }

        let question_id = generate_question_id();
        let (response_tx, response_rx) = oneshot::channel::<Vec<Vec<String>>>();

        let session_id = args["session_id"].as_str().unwrap_or("").to_string();

        let channel = QuestionChannel {
            question_id: question_id.clone(),
            questions: questions.clone(),
            response_tx,
        };

        self.event_bus.register_question(session_id, channel).await;

        let answers = response_rx.await.map_err(|_| {
            OSAgentError::ToolExecution("Question response channel closed".to_string())
        })?;

        let mut output = String::new();
        for (i, (q, answer)) in questions.iter().zip(answers.iter()).enumerate() {
            if i > 0 {
                output.push('\n');
            }
            output.push_str(&format!("Q: {}\n", q.question));
            output.push_str(&format!("A: {}", answer.join(", ")));
        }

        Ok(output)
    }
}
