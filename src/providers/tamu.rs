use async_trait::async_trait;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::types::{CapabilityTier, Message, ModelId, ModelInfo, ProviderId, Role};
use super::Provider;

/// TAMU AI provider using Texas A&M's OpenWebUI-based API
/// Daily quotas with reset at 6-7 PM Central
pub struct TamuProvider {
    client: Client,
    api_key: String,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct TamuMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct TamuChoice {
    message: TamuMessage,
}

#[derive(Debug, Deserialize)]
struct TamuResponse {
    choices: Vec<TamuChoice>,
}

#[derive(Debug, Deserialize)]
struct TamuStreamChoice {
    delta: TamuDelta,
}

#[derive(Debug, Deserialize)]
struct TamuDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TamuStreamChunk {
    choices: Vec<TamuStreamChoice>,
}

impl TamuProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: "https://chat-api.tamu.ai".to_string(),
        }
    }

    fn get_api_model_name(&self, model_id: &ModelId) -> String {
        let name = match model_id {
            ModelId::ClaudeOpus45 => "Claude Opus 4.5",
            ModelId::ClaudeSonnet45 => "Claude Sonnet 4.5",
            ModelId::Gpt4o => "gpt-4o",
            ModelId::Custom(name) => name.as_str(),
            _ => "gpt-4o",
        };
        format!("protected.{}", name)
    }
}

#[async_trait]
impl Provider for TamuProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        // TAMU AI available models
        Ok(vec![
            ModelInfo {
                id: ModelId::ClaudeOpus45,
                name: "claude-opus-4.5".to_string(),
                provider: ProviderId::Tamu,
                capability_tier: CapabilityTier::Frontier,
                context_window: 200_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::ClaudeSonnet45,
                name: "claude-sonnet-4.5".to_string(),
                provider: ProviderId::Tamu,
                capability_tier: CapabilityTier::Frontier,
                context_window: 200_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Gpt4o,
                name: "gpt-4o".to_string(),
                provider: ProviderId::Tamu,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Custom("gemini-2.5-pro".to_string()),
                name: "gemini-2.5-pro".to_string(),
                provider: ProviderId::Tamu,
                capability_tier: CapabilityTier::Frontier,
                context_window: 2_000_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
        ])
    }

    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String> {
        let model_name = self.get_api_model_name(&model.id);

        let tamu_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        let payload = json!({
            "model": model_name,
            "messages": tamu_messages,
            "max_tokens": 4096,
        });

        let response = self
            .client
            .post(format!("{}/api/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("TAMU API error {}: {}", status, body));
        }

        let tamu_response: TamuResponse = response.json().await?;
        Ok(tamu_response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response from TAMU"))?
            .message
            .content
            .clone())
    }

    async fn chat_stream(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        callback: Box<dyn Fn(String) + Send>,
    ) -> Result<String> {
        let model_name = self.get_api_model_name(&model.id);

        let tamu_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content
                })
            })
            .collect();

        let payload = json!({
            "model": model_name,
            "messages": tamu_messages,
            "stream": true,
            "max_tokens": 4096,
        });

        let response = self
            .client
            .post(format!("{}/api/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("TAMU API error {}: {}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut full_content = String::new();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }

                let data = &line[6..];
                if data == "[DONE]" {
                    break;
                }

                if let Ok(parsed) = serde_json::from_str::<TamuStreamChunk>(data) {
                    for choice in parsed.choices {
                        if let Some(content) = choice.delta.content {
                            callback(content.clone());
                            full_content.push_str(&content);
                        }
                    }
                }
            }
        }

        Ok(full_content)
    }

    async fn health_check(&self) -> Result<bool> {
        // Test with a minimal request
        let test_payload = json!({
            "model": "protected.gpt-4o",
            "messages": [{"role": "user", "content": "test"}],
            "max_tokens": 1
        });

        let response = self
            .client
            .post(format!("{}/api/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&test_payload)
            .send()
            .await?;

        // Success or rate limit means the API key works
        Ok(response.status().is_success() || response.status().as_u16() == 429)
    }
}
