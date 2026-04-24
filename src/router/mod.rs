use anyhow::Result;

use crate::types::{ModelInfo, Task, ProviderId};
use crate::providers::Provider;
use crate::usage_tracking::UsageTracker;

/// Routes tasks to appropriate models
pub struct Router {
    providers: Vec<Box<dyn Provider>>,
    available_models: Vec<ModelInfo>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            available_models: Vec::new(),
        }
    }
    
    pub fn add_provider(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }
    
    pub fn providers(&self) -> &[Box<dyn Provider>] {
        &self.providers
    }
    
    pub async fn refresh_models(&mut self) -> Result<()> {
        self.available_models.clear();
        
        for provider in &self.providers {
            if let Ok(models) = provider.available_models().await {
                self.available_models.extend(models);
            }
        }
        
        Ok(())
    }
    
    pub fn available_models(&self) -> &[ModelInfo] {
        &self.available_models
    }
    
    /// Select best model for a task based on usage tracking
    pub fn select_model_with_usage(&self, _task: &Task, usage_tracker: &mut UsageTracker) -> Option<&ModelInfo> {
        // Get prioritized providers
        let prioritized = usage_tracker.get_prioritized_providers();
        
        // Find first available model from highest priority provider
        for (provider_id, priority) in prioritized {
            if priority > 0.0 {
                // Find a model from this provider
                if let Some(model) = self.available_models.iter().find(|m| m.provider == provider_id) {
                    return Some(model);
                }
            }
        }
        
        // Fallback to first available model
        self.available_models.first()
    }
    
    /// Select best model for a task
    pub fn select_model(&self, _task: &Task) -> Option<&ModelInfo> {
        // For now, just return the first available model
        // TODO: Implement complexity-based selection
        self.available_models.first()
    }
    
    /// Find a specific model by name
    pub fn find_model(&self, name: &str) -> Option<&ModelInfo> {
        self.available_models.iter().find(|m| m.name == name)
    }
    
    /// Find provider for a model
    pub fn find_provider(&self, model: &ModelInfo) -> Option<&Box<dyn Provider>> {
        self.providers.iter().find(|p| {
            // Check if provider matches - we need to check available models
            // For now, just return first provider (we'll improve this)
            true
        })
    }
}
