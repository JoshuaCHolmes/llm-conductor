use async_trait::async_trait;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::types::{CapabilityTier, Message, ModelId, ModelInfo, ProviderId, Role};
use super::Provider;

/// NVIDIA NIM provider for free AI inference
/// Free tier with rate limits (varies by model)
pub struct NvidiaProvider {
    client: Client,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NvidiaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct NvidiaChoice {
    message: NvidiaMessage,
}

#[derive(Debug, Deserialize)]
struct NvidiaResponse {
    choices: Vec<NvidiaChoice>,
}

#[derive(Debug, Deserialize)]
struct NvidiaStreamChoice {
    delta: NvidiaDelta,
}

#[derive(Debug, Deserialize)]
struct NvidiaDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NvidiaStreamChunk {
    choices: Vec<NvidiaStreamChoice>,
}

impl NvidiaProvider {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    fn get_api_model_name(&self, model_id: &ModelId) -> String {
        match model_id {
            ModelId::Glm5Plus => "z-ai/glm5".to_string(),
            ModelId::Custom(name) => name.clone(),
            _ => "nvidia/llama-3.1-nemotron-70b-instruct".to_string(),
        }
    }
}

#[async_trait]
impl Provider for NvidiaProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        // NVIDIA NIM available models (free tier highlights)
        Ok(vec![
            ModelInfo {
                id: ModelId::Glm5Plus,
                name: "glm-5-89b".to_string(),
                provider: ProviderId::NvidiaNim,
                capability_tier: CapabilityTier::Advanced,
                context_window: 128_000,
                supports_vision: true,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Custom("nvidia/llama-3.1-nemotron-70b-instruct".to_string()),
                name: "llama-3.1-nemotron-70b".to_string(),
                provider: ProviderId::NvidiaNim,
                capability_tier: CapabilityTier::Advanced,
                context_window: 32_768,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Custom("meta/llama-3.1-405b-instruct".to_string()),
                name: "llama-3.1-405b-instruct".to_string(),
                provider: ProviderId::NvidiaNim,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Custom("mistralai/mistral-large-2-instruct".to_string()),
                name: "mistral-large-2".to_string(),
                provider: ProviderId::NvidiaNim,
                capability_tier: CapabilityTier::Advanced,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
            ModelInfo {
                id: ModelId::Custom("qwen/qwen2.5-coder-32b-instruct".to_string()),
                name: "qwen-2.5-coder-32b".to_string(),
                provider: ProviderId::NvidiaNim,
                capability_tier: CapabilityTier::Advanced,
                context_window: 32_768,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
            },
        ])
    }

    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String> {
        let model_name = self.get_api_model_name(&model.id);

        let nvidia_messages: Vec<serde_json::Value> = messages
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
            "messages": nvidia_messages,
            "max_tokens": 4096,
            "temperature": 0.7,
            "top_p": 0.9,
        });

        let mut request = self
            .client
            .post("https://integrate.api.nvidia.com/v1/chat/completions")
            .header("Content-Type", "application/json");

        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let response = request.json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("NVIDIA API error {}: {}", status, body));
        }

        let nvidia_response: NvidiaResponse = response.json().await?;
        Ok(nvidia_response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response from NVIDIA"))?
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
        let model_name = self.get_api_model_name(&model.id);

        let nvidia_messages: Vec<serde_json::Value> = messages
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
            "messages": nvidia_messages,
            "stream": true,
            "max_tokens": 4096,
            "temperature": 0.7,
            "top_p": 0.9,
        });

        let mut request = self
            .client
            .post("https://integrate.api.nvidia.com/v1/chat/completions")
            .header("Content-Type", "application/json");

        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let response = request.json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("NVIDIA API error {}: {}", status, body));
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

                if let Ok(parsed) = serde_json::from_str::<NvidiaStreamChunk>(data) {
                    for choice in parsed.choices {
                        if let Some(content) = choice.delta.content {
                            callback(content.clone());
                            full_content.push_str(&content);
                        }
                    }
                }
            }
        }

        Ok((full_content, None))
    }

    async fn health_check(&self) -> Result<bool> {
        // Check if we can list models
        let mut request = self
            .client
            .get("https://integrate.api.nvidia.com/v1/models");

        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let response = request.send().await?;
        Ok(response.status().is_success())
    }
}
