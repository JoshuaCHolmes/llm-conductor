use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};

use crate::providers::Provider;
use crate::types::{CapabilityTier, Message, ModelId, ModelInfo, ProviderId};

const BASE_URL: &str = "https://playground.outlier.ai";

/// Maximum time to wait for the first SSE chunk after connection established.
const FIRST_CHUNK_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum silence between subsequent SSE chunks mid-stream.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
struct TurnRequest {
    #[serde(rename = "isMysteryModel")]
    is_mystery_model: bool,
    model: String,
    #[serde(rename = "modelId")]
    model_id: String,
    prompt: PromptData,
    #[serde(rename = "turnMode")]
    turn_mode: String,
    #[serde(rename = "turnType")]
    turn_type: String,
}

#[derive(Debug, Serialize)]
struct PromptData {
    model: String,
    #[serde(rename = "turnType")]
    turn_type: String,
    #[serde(rename = "modelId")]
    model_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    delta: Option<Delta>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
}

pub struct OutlierProvider {
    client: Client,
    cookie: String,
    csrf_token: String,
}

impl OutlierProvider {
    pub fn new(cookie: String, csrf_token: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self { client, cookie, csrf_token })
    }

    fn get_api_model_name<'a>(&self, model_id: &'a ModelId) -> &'a str {
        match model_id {
            ModelId::ClaudeOpus45 => "claude-opus-4-6",
            ModelId::ClaudeSonnet45 => "claude-sonnet-4-6",
            ModelId::Gpt4o => "gpt-5.2-chat-latest",
            ModelId::Custom(name) => name.as_str(),
            _ => "claude-opus-4-6",
        }
    }

    fn get_outlier_model_id(&self, model_name: &str) -> &str {
        match model_name {
            "claude-opus-4-6" => "6984f819cfe8911f7189318e",
            "claude-sonnet-4-6" => "69843e1d7cce9fb8fe25b99e",
            "claude-haiku-4-5-20251001" => "67130cfdb7e918a2cb03e5ae",
            "gpt-5.2-chat-latest" => "67812345abcd1234ef567890",
            _ => "6984f819cfe8911f7189318e",
        }
    }

    fn build_turn_request(&self, text: &str, model_name: &str, model_id_str: &str) -> TurnRequest {
        TurnRequest {
            is_mystery_model: false,
            model: model_name.to_string(),
            model_id: model_id_str.to_string(),
            prompt: PromptData {
                model: model_name.to_string(),
                turn_type: "Text".to_string(),
                model_id: model_id_str.to_string(),
                text: text.to_string(),
            },
            turn_mode: "Normal".to_string(),
            turn_type: "Text".to_string(),
        }
    }

    /// Always creates a fresh conversation with the given text as the initial message.
    async fn create_conversation(&self, text: &str, model_name: &str, model_id: &str) -> Result<String> {
        let url = format!("{}/internal/experts/assistant/conversations", BASE_URL);
        let payload = serde_json::json!({
            "prompt": { "text": text, "images": [] },
            "model": model_name,
            "modelId": model_id,
            "challengeId": "",
            "initialTurnMode": "Normal",
            "initialTurnType": "Text",
            "isMysteryModel": false
        });

        let response = self.client.post(&url)
            .header("origin", BASE_URL)
            .header("referer", format!("{}/chat", BASE_URL))
            .header("content-type", "application/json")
            .header("x-csrf-token", &self.csrf_token)
            .header("cookie", &self.cookie)
            .json(&payload)
            .send().await?;

        if response.status().is_success() {
            let body = response.text().await?;
            let data: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| anyhow::anyhow!("Outlier create_conversation: invalid JSON: {e}\nBody: {body}"))?;
            if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                return Ok(id.to_string());
            }
            return Err(anyhow::anyhow!("Conversation created but no ID in response: {body}"));
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("Failed to create Outlier conversation: {status}\nBody: {body}"))
    }

    /// Delete a conversation by ID. Best-effort — errors are logged and swallowed.
    async fn delete_conversation(&self, conv_id: &str) {
        let url = format!("{}/internal/experts/assistant/conversations/{}", BASE_URL, conv_id);
        match self.client.delete(&url)
            .header("origin", BASE_URL)
            .header("referer", format!("{}/conversation", BASE_URL))
            .header("x-csrf-token", &self.csrf_token)
            .header("cookie", &self.cookie)
            .send().await
        {
            Ok(r) => tracing::debug!("Deleted Outlier conversation {}: {}", conv_id, r.status()),
            Err(e) => tracing::debug!("Failed to delete Outlier conversation {}: {}", conv_id, e),
        }
    }

    /// POST the turn-streaming endpoint and return the response (caller checks status).
    async fn send_turn(&self, conv_id: &str, text: &str, model_name: &str, model_id_str: &str) -> Result<reqwest::Response> {
        let url = format!(
            "{}/internal/experts/assistant/conversations/{}/turn-streaming",
            BASE_URL, conv_id
        );
        let payload = self.build_turn_request(text, model_name, model_id_str);
        let response = self.client.post(&url)
            .header("origin", BASE_URL)
            .header("referer", format!("{}/conversation", BASE_URL))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("x-csrf-token", &self.csrf_token)
            .header("cookie", &self.cookie)
            .json(&payload)
            .send().await?;
        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Outlier API error: {}", response.status()));
        }
        Ok(response)
    }

    /// Drain an SSE response into a String (no streaming callback). Used by non-streaming `chat()`.
    async fn drain_sse(response: reqwest::Response) -> Result<String> {
        let mut stream = response.bytes_stream();
        let mut result = String::new();
        let mut sse_buf = String::new();
        let mut first_chunk = true;

        loop {
            let wait = if first_chunk { FIRST_CHUNK_TIMEOUT } else { CHUNK_TIMEOUT };
            match timeout(wait, stream.next()).await {
                Err(_elapsed) => return Err(anyhow::anyhow!("Outlier stream timed out")),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(e.into()),
                Ok(Some(Ok(bytes))) => {
                    first_chunk = false;
                    sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                    let mut done = false;
                    while let Some(nl) = sse_buf.find('\n') {
                        let line = sse_buf[..nl].trim_end_matches('\r').to_string();
                        sse_buf = sse_buf[nl + 1..].to_string();
                        if line.starts_with("data: ") {
                            let data = &line[6..];
                            if data == "[DONE]" { done = true; break; }
                            if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                                if let Some(content) = chunk.choices.first()
                                    .and_then(|c| c.delta.as_ref())
                                    .and_then(|d| d.content.as_ref())
                                {
                                    result.push_str(content);
                                }
                            }
                        }
                    }
                    if done { break; }
                }
            }
        }
        Ok(result)
    }

    fn messages_to_text(&self, messages: &[Message]) -> String {
        use crate::types::Role;
        messages.iter().map(|msg| {
            let is_conductor = msg.source.as_deref().map(|s| s.starts_with("conductor/")).unwrap_or(false);
            match msg.role {
                Role::System => format!("System: {}", msg.content),
                Role::User if is_conductor => format!("[conductor]: {}", msg.content),
                Role::User => format!("User: {}", msg.content),
                Role::Assistant => {
                    if let Some(ref tcs) = msg.tool_calls {
                        let calls: Vec<String> = tcs.iter().map(|tc| {
                            let cmd = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                                .ok()
                                .and_then(|v| v["command"].as_str().map(|s| s.to_string()))
                                .unwrap_or_else(|| tc.arguments.clone());
                            format!("[Called bash: {}]", cmd)
                        }).collect();
                        let prefix = if msg.content.is_empty() { String::new() } else { format!("{}\n", msg.content) };
                        format!("Assistant: {}{}", prefix, calls.join("\n"))
                    } else {
                        format!("Assistant: {}", msg.content)
                    }
                }
                Role::Tool => format!("[conductor]: [Shell output]\n{}\n[End of shell output]", msg.content),
            }
        }).collect::<Vec<_>>().join("\n\n")
    }
}

#[async_trait]
impl Provider for OutlierProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: ModelId::ClaudeOpus45,
                name: "claude-opus-4.6".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Frontier,
                context_window: 200_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::ClaudeSonnet45,
                name: "claude-sonnet-4.6".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Advanced,
                context_window: 200_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Gpt4o,
                name: "gpt-5.2".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Custom("gpt-5.1-chat-latest".to_string()),
                name: "gpt-5.1".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Custom("o3".to_string()),
                name: "o3".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Frontier,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Custom("gemini-3.1-pro-preview".to_string()),
                name: "gemini-3.1-pro".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Frontier,
                context_window: 1_000_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Custom("grok-3".to_string()),
                name: "grok-3".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Advanced,
                context_window: 128_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
            ModelInfo {
                id: ModelId::Custom("deepseek-v3p2".to_string()),
                name: "deepseek-v3.2".to_string(),
                provider: ProviderId::Outlier,
                capability_tier: CapabilityTier::Advanced,
                context_window: 64_000,
                supports_vision: false,
                supports_streaming: true,
                cost_per_token: 0.0,
                supports_tool_calling: false,
            },
        ])
    }

    async fn chat(&self, model: &ModelInfo, messages: &[Message]) -> Result<String> {
        let model_name = self.get_api_model_name(&model.id);
        let model_id_str = self.get_outlier_model_id(model_name);
        let text = self.messages_to_text(messages);

        for attempt in 0..2u8 {
            if attempt > 0 {
                eprintln!("⚠  Outlier stalled, retrying with fresh conversation…");
            }
            let conv_id = self.create_conversation(&text, model_name, model_id_str).await?;

            let result: Result<String> = match self.send_turn(&conv_id, &text, model_name, model_id_str).await {
                Err(e) => Err(e),
                Ok(resp) => Self::drain_sse(resp).await,
            };
            self.delete_conversation(&conv_id).await;

            match result {
                Ok(content) => return Ok(content),
                Err(ref e) if e.to_string().contains("timed out") && attempt == 0 => continue,
                Err(e) => return Err(e),
            }
        }
        Err(anyhow::anyhow!("Outlier: request timed out after retry"))
    }

    async fn chat_stream(
        &self,
        model: &ModelInfo,
        messages: &[Message],
        callback: Box<dyn Fn(String) + Send>,
    ) -> Result<(String, Option<u64>)> {
        let model_name = self.get_api_model_name(&model.id);
        let model_id_str = self.get_outlier_model_id(model_name);
        let text = self.messages_to_text(messages);

        // NOTE: `callback` is Box<dyn Fn + Send> but NOT Sync.
        // To avoid holding &callback across .await (which requires Sync), we expand the SSE loop
        // inline here. The owned `callback` is Send and can be held across .await directly.
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..2u8 {
            if attempt > 0 {
                eprintln!("⚠  Outlier stalled, retrying with fresh conversation…");
            }
            let conv_id = self.create_conversation(&text, model_name, model_id_str).await?;

            let response = match self.send_turn(&conv_id, &text, model_name, model_id_str).await {
                Err(e) => {
                    self.delete_conversation(&conv_id).await;
                    last_err = Some(e);
                    continue;
                }
                Ok(r) => r,
            };

            let mut stream = response.bytes_stream();
            let mut full_content = String::new();
            let mut sse_buf = String::new();
            let mut first_chunk = true;
            let mut stream_err: Option<anyhow::Error> = None;

            'sse: loop {
                let wait = if first_chunk { FIRST_CHUNK_TIMEOUT } else { CHUNK_TIMEOUT };
                match timeout(wait, stream.next()).await {
                    Err(_elapsed) => {
                        stream_err = Some(anyhow::anyhow!("Outlier stream timed out"));
                        break 'sse;
                    }
                    Ok(None) => break 'sse,
                    Ok(Some(Err(e))) => {
                        stream_err = Some(e.into());
                        break 'sse;
                    }
                    Ok(Some(Ok(bytes))) => {
                        first_chunk = false;
                        sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                        let mut done = false;
                        while let Some(nl) = sse_buf.find('\n') {
                            let line = sse_buf[..nl].trim_end_matches('\r').to_string();
                            sse_buf = sse_buf[nl + 1..].to_string();
                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" { done = true; break; }
                                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                                    if let Some(content) = chunk.choices.first()
                                        .and_then(|c| c.delta.as_ref())
                                        .and_then(|d| d.content.as_ref())
                                    {
                                        callback(content.clone());
                                        full_content.push_str(content);
                                    }
                                }
                            }
                        }
                        if done { break 'sse; }
                    }
                }
            }

            self.delete_conversation(&conv_id).await;

            if let Some(e) = stream_err {
                if e.to_string().contains("timed out") && attempt == 0 {
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
            return Ok((full_content, None));
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Outlier: request timed out after retry")))
    }

    async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/internal/experts/assistant/conversations/", BASE_URL);
        let response = self.client.get(&url)
            .header("origin", BASE_URL)
            .header("x-csrf-token", &self.csrf_token)
            .header("cookie", &self.cookie)
            .send().await?;
        Ok(response.status().is_success())
    }
}
