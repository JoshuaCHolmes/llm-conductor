use anyhow::Result;

use crate::types::{ModelInfo, Task};
use crate::providers::Provider;

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
    
    /// Select best model for a task
    pub fn select_model(&self, task: &Task) -> Option<&ModelInfo> {
        // For now, just return the first available model
        // TODO: Implement complexity-based selection
        self.available_models.first()
    }
}
