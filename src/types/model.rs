use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a model
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelId {
    /// Claude Opus 4.5
    ClaudeOpus45,
    /// Claude Sonnet 4.5
    ClaudeSonnet45,
    /// GPT-4o
    Gpt4o,
    /// GLM-5 Plus (89B) via NVIDIA NIM
    Glm5Plus,
    /// Ollama local models
    Ollama(String),
    /// Custom model
    Custom(String),
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelId::ClaudeOpus45 => write!(f, "claude-opus-4.5"),
            ModelId::ClaudeSonnet45 => write!(f, "claude-sonnet-4.5"),
            ModelId::Gpt4o => write!(f, "gpt-4o"),
            ModelId::Glm5Plus => write!(f, "glm-5-plus"),
            ModelId::Ollama(name) => write!(f, "ollama:{}", name),
            ModelId::Custom(name) => write!(f, "custom:{}", name),
        }
    }
}

/// Model capability tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CapabilityTier {
    /// Small local models (Phi-3, Qwen 2.5 3B)
    Basic = 1,
    /// Mid-tier models (GLM-5, Sonnet)
    Advanced = 2,
    /// Frontier models (Opus, GPT-4o)
    Frontier = 3,
}

/// Information about a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub name: String,
    pub provider: ProviderId,
    pub capability_tier: CapabilityTier,
    pub context_window: usize,
    pub supports_vision: bool,
    pub supports_streaming: bool,
    pub cost_per_token: f64,
}

/// Provider identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    Ollama,
    NvidiaNim,
    GitHubCopilot,
    Tamu,
    Custom(String),
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderId::Ollama => write!(f, "ollama"),
            ProviderId::NvidiaNim => write!(f, "nvidia-nim"),
            ProviderId::GitHubCopilot => write!(f, "github-copilot"),
            ProviderId::Tamu => write!(f, "tamu"),
            ProviderId::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// Complexity level of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ComplexityLevel {
    Trivial = 1,
    Simple = 2,
    Moderate = 3,
    Complex = 4,
    Expert = 5,
}

impl ComplexityLevel {
    /// Get minimum capability tier needed for this complexity
    pub fn min_capability_tier(&self) -> CapabilityTier {
        match self {
            ComplexityLevel::Trivial | ComplexityLevel::Simple => CapabilityTier::Basic,
            ComplexityLevel::Moderate => CapabilityTier::Advanced,
            ComplexityLevel::Complex | ComplexityLevel::Expert => CapabilityTier::Frontier,
        }
    }
}
