use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MessageError {
    #[error("expected Role::Tool but message has a different role")]
    NotToolRole,
    #[error("Role::Tool message is missing tool_call_id or it is empty")]
    EmptyToolCallId,
}

/// A tool call made by an assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    /// Tool name (currently only "bash")
    pub name: String,
    /// JSON-encoded arguments string (e.g. `{"command":"ls"}`)
    pub arguments: String,
}

/// A message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    /// Message text. Empty string for assistant messages that only contain tool_calls.
    pub content: String,
    /// Tool calls requested by the assistant. Only set when role == Assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Ties a Role::Tool message back to the assistant tool call that triggered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Provenance: who/what produced this message.
    /// E.g. "user", "outlier/claude-opus-4.6", "github/gpt-4o", "conductor/feedback", "conductor/rubberduck-result"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Role of a message sender
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    /// Result of a tool execution — only used by function-calling providers
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            source: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            source: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            source: None,
        }
    }

    /// Create an assistant message that requested tool calls (content may be empty)
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            source: None,
        }
    }

    /// Create a tool result message (response to a specific tool call).
    ///
    /// `call_id` MUST be the non-empty `id` returned by the provider in the matching
    /// assistant `tool_calls` entry. OpenAI-compatible APIs reject tool messages with
    /// an empty `tool_call_id`, so we substitute a synthetic id rather than ever
    /// silently producing an invalid message — the synthetic id is logged so
    /// upstream bugs are visible.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        let id = call_id.into();
        let id = if id.is_empty() {
            let synth = format!("synth-{}", uuid::Uuid::new_v4());
            tracing::error!(
                "Message::tool_result called with empty tool_call_id; substituting synthetic id {}",
                synth
            );
            synth
        } else {
            id
        };
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(id),
            source: None,
        }
    }

    /// Return the tool_call_id if this is a valid tool-result message.
    /// Returns `Err` if `role != Tool` or `tool_call_id` is missing/empty.
    pub fn require_tool_call_id(&self) -> Result<&str, MessageError> {
        if !matches!(self.role, Role::Tool) {
            return Err(MessageError::NotToolRole);
        }
        match self.tool_call_id.as_deref() {
            Some(id) if !id.is_empty() => Ok(id),
            _ => Err(MessageError::EmptyToolCallId),
        }
    }

    /// Attach a source label to this message (builder pattern)
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn token_count(&self) -> usize {
        // Rough estimate: 4 chars per token
        self.content.len() / 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_with_valid_id_round_trips() {
        let m = Message::tool_result("call_abc", "ok");
        assert_eq!(m.require_tool_call_id().unwrap(), "call_abc");
    }

    #[test]
    fn tool_result_synthesises_id_when_empty() {
        let m = Message::tool_result("", "ok");
        let id = m.require_tool_call_id().unwrap();
        assert!(id.starts_with("synth-"), "got {}", id);
    }

    #[test]
    fn require_tool_call_id_rejects_non_tool_roles() {
        assert_eq!(
            Message::user("hi").require_tool_call_id().unwrap_err(),
            MessageError::NotToolRole
        );
    }
}
