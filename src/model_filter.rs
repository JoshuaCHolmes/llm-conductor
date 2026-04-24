use crate::types::{ModelInfo, ProviderId, CapabilityTier};

/// Filters for model selection
#[derive(Debug, Clone, Default)]
pub struct ModelFilter {
    pub model_name_pattern: Option<String>,
    pub providers: Vec<ProviderId>,
    pub tiers: Vec<CapabilityTier>,
    pub require_vision: Option<bool>,
    pub require_streaming: Option<bool>,
}

impl ModelFilter {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Parse filter arguments from command
    pub fn from_args(args: &[&str]) -> Self {
        let mut filter = Self::new();
        
        for arg in args {
            let arg_lower = arg.to_lowercase();
            
            // Check for provider names
            if let Some(provider) = Self::parse_provider(&arg_lower) {
                filter.providers.push(provider);
                continue;
            }
            
            // Check for tier names
            if let Some(tier) = Self::parse_tier(&arg_lower) {
                filter.tiers.push(tier);
                continue;
            }
            
            // Check for feature flags
            match arg_lower.as_str() {
                "vision" => filter.require_vision = Some(true),
                "streaming" => filter.require_streaming = Some(true),
                _ => {
                    // Treat as model name pattern
                    filter.model_name_pattern = Some(arg_lower);
                }
            }
        }
        
        filter
    }
    
    fn parse_provider(s: &str) -> Option<ProviderId> {
        match s {
            "outlier" => Some(ProviderId::Outlier),
            "github" | "github-copilot" => Some(ProviderId::GitHubCopilot),
            "tamu" => Some(ProviderId::Tamu),
            "nvidia" | "nvidia-nim" => Some(ProviderId::NvidiaNim),
            "ollama" => Some(ProviderId::Ollama),
            _ => None,
        }
    }
    
    fn parse_tier(s: &str) -> Option<CapabilityTier> {
        match s {
            "frontier" => Some(CapabilityTier::Frontier),
            "advanced" => Some(CapabilityTier::Advanced),
            "basic" => Some(CapabilityTier::Basic),
            _ => None,
        }
    }
    
    /// Check if a model matches this filter
    pub fn matches(&self, model: &ModelInfo) -> bool {
        // Check model name pattern
        if let Some(pattern) = &self.model_name_pattern {
            if !model.name.contains(pattern) {
                return false;
            }
        }
        
        // Check providers
        if !self.providers.is_empty() && !self.providers.contains(&model.provider) {
            return false;
        }
        
        // Check tiers
        if !self.tiers.is_empty() && !self.tiers.contains(&model.capability_tier) {
            return false;
        }
        
        // Check vision
        if let Some(requires_vision) = self.require_vision {
            if model.supports_vision != requires_vision {
                return false;
            }
        }
        
        // Check streaming
        if let Some(requires_streaming) = self.require_streaming {
            if model.supports_streaming != requires_streaming {
                return false;
            }
        }
        
        true
    }
    
    /// Check if this filter is empty (no restrictions)
    pub fn is_empty(&self) -> bool {
        self.model_name_pattern.is_none()
            && self.providers.is_empty()
            && self.tiers.is_empty()
            && self.require_vision.is_none()
            && self.require_streaming.is_none()
    }
    
    /// Get a human-readable description of this filter
    pub fn description(&self) -> String {
        let mut parts = Vec::new();
        
        if let Some(pattern) = &self.model_name_pattern {
            parts.push(format!("model: {}", pattern));
        }
        
        if !self.providers.is_empty() {
            let providers: Vec<_> = self.providers.iter().map(|p| p.to_string()).collect();
            parts.push(format!("providers: {}", providers.join(", ")));
        }
        
        if !self.tiers.is_empty() {
            let tiers: Vec<_> = self.tiers.iter().map(|t| format!("{:?}", t)).collect();
            parts.push(format!("tiers: {}", tiers.join(", ")));
        }
        
        if self.require_vision == Some(true) {
            parts.push("with vision".to_string());
        }
        
        if self.require_streaming == Some(true) {
            parts.push("with streaming".to_string());
        }
        
        if parts.is_empty() {
            "none (automatic)".to_string()
        } else {
            parts.join(", ")
        }
    }
}
