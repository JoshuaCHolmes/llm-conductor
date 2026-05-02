use async_trait::async_trait;
use anyhow::Result;
use serde_json::json;

use crate::types::{Message, ModelInfo, Role, ToolCall};

pub mod ollama;
pub mod github;
pub mod tamu;
pub mod nvidia;
pub mod outlier;
pub mod http;

pub use ollama::OllamaProvider;
pub use github::GitHubProvider;
pub use tamu::TamuProvider;
pub use nvidia::NvidiaProvider;
pub use outlier::OutlierProvider;

/// Serialize a slice of `Message`s into the OpenAI `chat/completions` shape.
///
/// Returns `Err` if any tool-result message fails validation (missing or empty
/// `tool_call_id`) — sending such a payload causes 400 errors at every
/// OpenAI-compatible API and silent context loss at non-strict ones.
pub fn serialize_messages_openai(messages: &[Message]) -> Result<Vec<serde_json::Value>> {
    messages.iter().map(|m| {
        let is_conductor = m.source.as_deref().map(|s| s.starts_with("conductor/")).unwrap_or(false);
        let value = match m.role {
            Role::Tool => {
                let id = m.require_tool_call_id()
                    .map_err(|e| anyhow::anyhow!("invalid tool message: {}", e))?;
                json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": m.content,
                })
            }
            Role::Assistant if m.tool_calls.is_some() => {
                let tcs = m.tool_calls.as_ref().unwrap();
                let tc: Vec<serde_json::Value> = tcs.iter().map(|tc| json!({
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.name, "arguments": tc.arguments }
                })).collect();
                let content_val = if m.content.is_empty() {
                    serde_json::Value::Null
                } else {
                    json!(m.content)
                };
                json!({ "role": "assistant", "content": content_val, "tool_calls": tc })
            }
            Role::User if is_conductor => json!({
                "role": "user",
                "content": format!("[conductor]: {}", m.content),
            }),
            _ => json!({ "role": m.role.as_str(), "content": m.content }),
        };
        Ok(value)
    }).collect()
}

/// A function/tool definition to pass to OpenAI-compatible APIs
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's parameters
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// The single bash tool definition shared across all tool-calling providers
    pub fn bash() -> Self {
        Self {
            name: "bash".to_string(),
            description: "Run a shell command and return its output. Use this to inspect files, run programs, or perform system tasks relevant to the user's request.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to run"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    pub fn todo_add() -> Self {
        Self {
            name: "todo_add".to_string(),
            description: "Add a new task to the todo list with pending status.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short task title" },
                    "description": { "type": "string", "description": "Optional longer description" }
                },
                "required": ["title"]
            }),
        }
    }

    pub fn todo_update() -> Self {
        Self {
            name: "todo_update".to_string(),
            description: "Update the status of an existing todo by its UUID. Valid statuses: pending, in_progress, done, blocked.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Full UUID of the todo to update" },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "done", "blocked"] }
                },
                "required": ["id", "status"]
            }),
        }
    }

    pub fn todo_list() -> Self {
        Self {
            name: "todo_list".to_string(),
            description: "Return the current todo list as text.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    pub fn rubberduck() -> Self {
        Self {
            name: "rubberduck".to_string(),
            description: "Spawn an adversarial critic review of your current plan, approach, or a specific decision. \
Use this before multi-step work, destructive commands, or when you are uncertain. \
The reviewer is deliberately critical and will surface risks and gaps you may have missed.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to think about — your plan, a specific decision, or a risk to evaluate"
                    }
                },
                "required": ["query"]
            }),
        }
    }
}

/// Response from `call_with_tools` — either text or a list of tool calls
#[derive(Debug)]
pub struct ToolCallResponse {
    /// Text content (may be non-empty even when tool_calls is present)
    pub text: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tokens: Option<u64>,
}

/// Trait that all providers must implement
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get information about available models
    async fn available_models(&self) -> Result<Vec<ModelInfo>>;
    
    /// Send messages and get response
    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String>;
    
    /// Send messages and stream response.
    /// Returns (full_response_text, Option<total_tokens_used>).
    async fn chat_stream(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        callback: Box<dyn Fn(String) + Send>,
    ) -> Result<(String, Option<u64>)>;
    
    /// Send messages with tool definitions (non-streaming). Used for function calling.
    /// Returns either a text response or a set of tool calls.
    /// Default implementation returns an error — only OpenAI-compat providers override this.
    async fn call_with_tools(
        &self,
        _model: &ModelInfo,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<ToolCallResponse> {
        Err(anyhow::anyhow!("Tool calling not supported by this provider"))
    }

    /// Check if provider is available/healthy
    async fn health_check(&self) -> Result<bool>;

    /// Reset any server-side session state (e.g. Outlier conversation ID).
    /// Default is a no-op; only providers with server-side sessions need to override.
    async fn reset_session(&self) {}
}
