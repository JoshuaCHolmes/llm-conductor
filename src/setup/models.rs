use anyhow::{anyhow, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

/// Manages model downloads and availability
pub struct ModelManager {
    cache_dir: PathBuf,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        // Use ~/.cache/llm-conductor/models
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow!("Could not find cache directory"))?
            .join("llm-conductor")
            .join("models");
        
        std::fs::create_dir_all(&cache_dir)?;
        
        Ok(Self { cache_dir })
    }
    
    /// Ensure required models are available
    pub async fn ensure_models(&self) -> Result<()> {
        println!("{}", "Checking required models...".bright_white());
        
        // Check what models Ollama has
        let ollama_models = self.list_ollama_models().await?;
        
        // Required models for basic operation
        let required = vec![
            ("qwen2.5:3b", "Fast local model for routing and basic tasks"),
        ];
        
        for (model, description) in required {
            if ollama_models.iter().any(|m| m.name.starts_with(model)) {
                println!("  {} {}", "✓".bright_green(), model);
            } else {
                println!("  {} {} - {}", "↓".bright_yellow(), model, description.dimmed());
                self.pull_model(model).await?;
            }
        }
        
        println!();
        Ok(())
    }
    
    /// List models available in Ollama
    pub async fn list_ollama_models(&self) -> Result<Vec<OllamaModel>> {
        let client = reqwest::Client::new();
        
        let response = client
            .get("http://localhost:11434/api/tags")
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch models from Ollama"));
        }
        
        let json: OllamaTagsResponse = response.json().await?;
        Ok(json.models)
    }
    
    /// Pull a model from Ollama
    pub async fn pull_model(&self, model_name: &str) -> Result<()> {
        println!("  Downloading {}...", model_name.bright_white());
        
        // Create progress bar
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("    {spinner:.cyan} {msg}")
                .unwrap()
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        
        // Start pull command
        let mut child = Command::new("ollama")
            .args(&["pull", model_name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("Failed to start ollama pull: {}", e))?;
        
        // Monitor progress
        pb.set_message("Downloading...");
        
        // Wait for completion
        let status = child.wait()
            .map_err(|e| anyhow!("Failed to wait for ollama pull: {}", e))?;
        
        pb.finish_and_clear();
        
        if status.success() {
            println!("  {} {}", "✓".bright_green(), model_name);
            Ok(())
        } else {
            Err(anyhow!("Failed to pull model: {}", model_name))
        }
    }
    
    /// Check if a specific model is available
    pub async fn is_model_available(&self, model_name: &str) -> Result<bool> {
        let models = self.list_ollama_models().await?;
        Ok(models.iter().any(|m| m.name.starts_with(model_name)))
    }
    
    /// Get model info
    pub async fn get_model_info(&self, model_name: &str) -> Result<OllamaModel> {
        let models = self.list_ollama_models().await?;
        
        models
            .into_iter()
            .find(|m| m.name == model_name || m.name.starts_with(&format!("{}:", model_name)))
            .ok_or_else(|| anyhow!("Model not found: {}", model_name))
    }
    
    /// List recommended models for download
    pub fn recommended_models() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("qwen2.5:3b", "1.9GB", "Fast general-purpose model"),
            ("phi3:3.8b", "2.3GB", "Microsoft's efficient model"),
            ("llama3.2:3b", "2.0GB", "Meta's latest small model"),
            ("qwen2.5:7b", "4.7GB", "More capable, slower"),
        ]
    }
}

// Ollama API types

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub digest: String,
}
