use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::time::timeout;

use super::{serialize_messages_openai, Provider, ToolCallResponse, ToolDefinition};
use crate::providers::http::{
    build_client, sanitize_error_body, CHAT_REQUEST_TIMEOUT, CHUNK_TIMEOUT, FIRST_CHUNK_TIMEOUT,
};
use crate::types::{CapabilityTier, Message, ModelId, ModelInfo, ProviderId, ToolCall};

/// GitHub Copilot provider using GitHub Models API
/// Free tier: Rate limited per repository/user
pub struct GitHubProvider {
    client: Client,
    token: String,
}

#[derive(Debug, Deserialize)]
struct GitHubMessage {
    #[allow(dead_code)]
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct GitHubChoice {
    message: GitHubMessage,
}

#[derive(Debug, Deserialize)]
struct GitHubResponse {
    choices: Vec<GitHubChoice>,
}

#[derive(Debug, Deserialize)]
struct GitHubStreamChoice {
    delta: GitHubDelta,
}

#[derive(Debug, Deserialize)]
struct GitHubDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubStreamChunk {
    choices: Vec<GitHubStreamChoice>,
}

impl GitHubProvider {
    pub fn new(token: String) -> Self {
        Self {
            // No overall request timeout: shared with chat_stream, which uses
            // per-chunk timeouts. Non-streaming chat() / call_with_tools wrap
            // their requests in tokio::time::timeout instead.
            client: build_client(None).expect("build GitHub HTTP client"),
            token,
        }
    }
}

/// Serialize messages to OpenAI format with tool call / tool result support.
/// Now provided by `super::serialize_messages_openai`.

#[async_trait]
impl Provider for GitHubProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        // GitHub Models available in free tier
        Ok(vec![
            ModelInfo {
                id: ModelId::Gpt4o,
                name: "gpt-4o".to_string(),
                provider: ProviderId::GitHubCopilot,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: true,
            },
            ModelInfo {
                id: ModelId::Custom("gpt-4o-mini".to_string()),
                name: "gpt-4o-mini".to_string(),
                provider: ProviderId::GitHubCopilot,
                capability_tier: CapabilityTier::Advanced,
                context_window: 128_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: true,
            },
            ModelInfo {
                id: ModelId::ClaudeSonnet45,
                name: "claude-3.5-sonnet".to_string(),
                provider: ProviderId::GitHubCopilot,
                capability_tier: CapabilityTier::Frontier,
                context_window: 200_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: true,
            },
        ])
    }

    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String> {
        let model_name = match &model.id {
            ModelId::Gpt4o => "gpt-4o",
            ModelId::ClaudeSonnet45 => "claude-3-5-sonnet-20241022",
            ModelId::Custom(name) => name.as_str(),
            _ => return Err(anyhow!("Invalid model ID for GitHub provider")),
        };

        let github_messages = serialize_messages_openai(messages)?;

        let payload = json!({
            "model": model_name,
            "messages": github_messages,
            "max_tokens": 4096,
        });

        let response = timeout(
            CHAT_REQUEST_TIMEOUT,
            self.client
                .post("https://models.inference.ai.azure.com/chat/completions")
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send(),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "GitHub chat request timed out after {:?}",
                CHAT_REQUEST_TIMEOUT
            )
        })??;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "GitHub API error {}: {}",
                status,
                sanitize_error_body(&body)
            ));
        }

        let github_response: GitHubResponse = response.json().await?;
        Ok(github_response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response from GitHub"))?
            .message
            .content
            .clone())
    }

    async fn chat_stream(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        callback: Box<dyn Fn(String) + Send>,
    ) -> Result<(String, Option<u64>)> {
        let model_name = match &model.id {
            ModelId::Gpt4o => "gpt-4o",
            ModelId::ClaudeSonnet45 => "claude-3-5-sonnet-20241022",
            ModelId::Custom(name) => name.as_str(),
            _ => return Err(anyhow!("Invalid model ID for GitHub provider")),
        };

        let github_messages = serialize_messages_openai(messages)?;

        let payload = json!({
            "model": model_name,
            "messages": github_messages,
            "stream": true,
            "max_tokens": 4096,
        });

        let response = self
            .client
            .post("https://models.inference.ai.azure.com/chat/completions")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("GitHub API error {}: {}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut full_content = String::new();
        let mut buffer = String::new();
        let mut first_chunk = true;
        let mut done = false;

        while !done {
            let wait = if first_chunk {
                FIRST_CHUNK_TIMEOUT
            } else {
                CHUNK_TIMEOUT
            };
            let chunk = match timeout(wait, stream.next()).await {
                Err(_) => return Err(anyhow!("GitHub stream stalled (no chunk for {:?})", wait)),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(e.into()),
                Ok(Some(Ok(c))) => c,
            };
            first_chunk = false;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..];
                if data == "[DONE]" {
                    done = true;
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<GitHubStreamChunk>(data) {
                    for choice in parsed.choices {
                        if let Some(content) = choice.delta.content {
                            callback(content.clone());
                            full_content.push_str(&content);
                        }
                    }
                }
            }
        }

        // GitHub is request-based; return None for tokens (caller uses 1 request)
        Ok((full_content, None))
    }

    async fn call_with_tools(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ToolCallResponse> {
        let model_name = match &model.id {
            ModelId::Gpt4o => "gpt-4o",
            ModelId::ClaudeSonnet45 => "claude-3-5-sonnet-20241022",
            ModelId::Custom(name) => name.as_str(),
            _ => return Err(anyhow!("Invalid model ID for GitHub provider")),
        };

        let github_messages = serialize_messages_openai(messages)?;
        let tool_defs: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();

        let payload = json!({
            "model": model_name,
            "messages": github_messages,
            "tools": tool_defs,
            "tool_choice": "auto",
            "max_tokens": 4096,
        });

        let response = timeout(
            CHAT_REQUEST_TIMEOUT,
            self.client
                .post("https://models.inference.ai.azure.com/chat/completions")
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send(),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "GitHub call_with_tools timed out after {:?}",
                CHAT_REQUEST_TIMEOUT
            )
        })??;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "GitHub API error {}: {}",
                status,
                sanitize_error_body(&body)
            ));
        }

        let raw: serde_json::Value = response.json().await?;
        let choice = raw["choices"][0]["message"].clone();
        let text = choice["content"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        if let Some(tc_arr) = choice["tool_calls"].as_array() {
            let tool_calls: Vec<ToolCall> = tc_arr
                .iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let name = tc["function"]["name"].as_str()?.to_string();
                    let arguments = tc["function"]["arguments"].as_str()?.to_string();
                    Some(ToolCall {
                        id,
                        name,
                        arguments,
                    })
                })
                .collect();
            Ok(ToolCallResponse {
                text,
                tool_calls: Some(tool_calls),
                tokens: None,
            })
        } else {
            Ok(ToolCallResponse {
                text,
                tool_calls: None,
                tokens: None,
            })
        }
    }

    async fn health_check(&self) -> Result<bool> {
        let response = timeout(
            std::time::Duration::from_secs(10),
            self.client
                .get("https://api.github.com/user")
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Accept", "application/vnd.github+json")
                .send(),
        )
        .await
        .map_err(|_| anyhow!("GitHub health check timed out"))??;

        Ok(response.status().is_success())
    }
}
