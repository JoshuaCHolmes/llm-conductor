use async_trait::async_trait;
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::timeout;

use crate::providers::http::{build_client, CHAT_REQUEST_TIMEOUT, CHUNK_TIMEOUT, FIRST_CHUNK_TIMEOUT};
use crate::types::{CapabilityTier, Message, ModelId, ModelInfo, ProviderId, Role};
use super::Provider;

/// Ollama provider for local models
pub struct OllamaProvider {
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            // No overall request timeout: this client is shared with chat_stream.
            // Local Ollama can take >2 min on cold model loads.
            client: build_client(None).expect("build Ollama HTTP client"),
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.base_url);
        
        let response = self.client
            .get(&url)
            .send()
            .await?
            .json::<OllamaTagsResponse>()
            .await?;
        
        let models = response.models.into_iter()
            .map(|m| ModelInfo {
                id: ModelId::Ollama(m.name.clone()),
                name: m.name,
                provider: ProviderId::Ollama,
                capability_tier: CapabilityTier::Basic,  // Local models are Basic tier
                context_window: 8192,  // Default, would need to query model details
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,  // Free!
                supports_tool_calling: false,
            })
            .collect();
        
        Ok(models)
    }
    
    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        
        let ollama_messages: Vec<OllamaMessage> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::System => "system".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();
        
        let model_name = match &model.id {
            ModelId::Ollama(name) => name.clone(),
            _ => return Err(anyhow!("Invalid model ID for Ollama provider")),
        };
        
        let request = json!({
            "model": model_name,
            "messages": ollama_messages,
            "stream": false,
        });
        
        let response = timeout(CHAT_REQUEST_TIMEOUT, async {
            self.client
                .post(&url)
                .json(&request)
                .send()
                .await?
                .json::<OllamaChatResponse>()
                .await
        })
            .await
            .map_err(|_| anyhow!("Ollama chat timed out after {:?}", CHAT_REQUEST_TIMEOUT))??;

        Ok(response.message.content)
    }
    
    async fn chat_stream(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        callback: Box<dyn Fn(String) + Send>,
    ) -> Result<(String, Option<u64>)> {
        use futures::StreamExt;
        
        let url = format!("{}/api/chat", self.base_url);
        
        let ollama_messages: Vec<OllamaMessage> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::System => "system".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();
        
        let model_name = match &model.id {
            ModelId::Ollama(name) => name.clone(),
            _ => return Err(anyhow!("Invalid model ID for Ollama provider")),
        };
        
        let request = json!({
            "model": model_name,
            "messages": ollama_messages,
            "stream": true,
        });
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;
        
        let mut stream = response.bytes_stream();
        let mut full_response = String::new();
        let mut buffer = String::new();
        let mut done = false;
        let mut first_chunk = true;

        while !done {
            let wait = if first_chunk { FIRST_CHUNK_TIMEOUT } else { CHUNK_TIMEOUT };
            let chunk = match timeout(wait, stream.next()).await {
                Err(_) => return Err(anyhow!(
                    "Ollama stream stalled (no chunk for {:?})", wait
                )),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(e.into()),
                Ok(Some(Ok(c))) => c,
            };
            first_chunk = false;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Drain complete newline-terminated JSON objects, retaining any
            // partial trailing fragment for the next iteration.
            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].trim().to_string();
                buffer.drain(..=idx);

                if line.is_empty() {
                    continue;
                }

                if let Ok(chunk_response) = serde_json::from_str::<OllamaChatResponse>(&line) {
                    let content = chunk_response.message.content;
                    full_response.push_str(&content);
                    callback(content);

                    if chunk_response.done {
                        done = true;
                        break;
                    }
                }
            }
        }

        // Process any final fragment that lacks a trailing newline.
        if !done {
            let tail = buffer.trim();
            if !tail.is_empty() {
                if let Ok(chunk_response) = serde_json::from_str::<OllamaChatResponse>(tail) {
                    let content = chunk_response.message.content;
                    full_response.push_str(&content);
                    callback(content);
                }
            }
        }

        Ok((full_response, None))
    }
    
    async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/tags", self.base_url);

        match timeout(std::time::Duration::from_secs(5), self.client.get(&url).send()).await {
            Ok(Ok(response)) => Ok(response.status().is_success()),
            _ => Ok(false),
        }
    }
}

// Ollama API types

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaResponseMessage,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    content: String,
}
