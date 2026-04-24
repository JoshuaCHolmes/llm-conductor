use anyhow::Result;

use crate::types::{ModelInfo, Task, ProviderId};
use crate::providers::Provider;
use crate::usage_tracking::UsageTracker;
use crate::model_filter::ModelFilter;

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
    
    /// Select best model with filter and usage tracking
    pub fn select_model_filtered(
        &self,
        _task: &Task,
        filter: &ModelFilter,
        usage_tracker: &mut UsageTracker,
    ) -> Option<&ModelInfo> {
        // Filter models
        let filtered: Vec<&ModelInfo> = self.available_models
            .iter()
            .filter(|m| filter.matches(m))
            .collect();
        
        if filtered.is_empty() {
            return None;
        }
        
        // If only one match, return it
        if filtered.len() == 1 {
            return Some(filtered[0]);
        }
        
        // Multiple matches - use usage-aware selection
        let prioritized = usage_tracker.get_prioritized_providers();
        
        // Find first available model from highest priority provider
        for (provider_id, priority) in prioritized {
            if priority > 0.0 {
                if let Some(model) = filtered.iter().find(|m| m.provider == provider_id) {
                    return Some(model);
                }
            }
        }
        
        // Fallback to first filtered model
        filtered.first().copied()
    }
    
    /// Select best model for a task based on usage tracking
    pub fn select_model_with_usage(&self, task: &Task, usage_tracker: &mut UsageTracker) -> Option<&ModelInfo> {
        self.select_model_filtered(task, &ModelFilter::new(), usage_tracker)
    }
    
    /// Select best model for a task
    pub fn select_model(&self, _task: &Task) -> Option<&ModelInfo> {
        self.available_models.first()
    }
    
    /// Find a specific model by name
    pub fn find_model(&self, name: &str) -> Option<&ModelInfo> {
        self.available_models.iter().find(|m| m.name == name)
    }
    
    /// Find provider for a model by checking which provider offers it
    pub fn find_provider_for_model(&self, model: &ModelInfo) -> Option<&Box<dyn Provider>> {
        for provider in &self.providers {
            // Try to get models from provider and see if any match
            // This is sync code but providers use async - we'll need to trust the ProviderId
            // For now, just iterate and trust the first match
            // TODO: Better provider matching
        }
        // Fallback: return first provider (temporary workaround)
        self.providers.first()
    }
}
