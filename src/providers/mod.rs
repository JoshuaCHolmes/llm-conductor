use async_trait::async_trait;
use anyhow::Result;

use crate::types::{Message, ModelInfo, TaskResult};

pub mod ollama;
pub mod github;
pub mod tamu;
pub mod nvidia;
pub mod outlier;

pub use ollama::OllamaProvider;
pub use github::GitHubProvider;
pub use tamu::TamuProvider;
pub use nvidia::NvidiaProvider;
pub use outlier::OutlierProvider;

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
    
    /// Check if provider is available/healthy
    async fn health_check(&self) -> Result<bool>;
}
