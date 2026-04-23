use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for individual provider control
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSettings {
    /// Whether this provider is enabled
    pub enabled: bool,
    /// Priority for model selection (higher = preferred)
    pub priority: u8,
    /// Custom settings per provider
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 50,
            custom: HashMap::new(),
        }
    }
}

/// Main provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Ollama local model provider
    pub ollama: ProviderSettings,
    
    /// GitHub Copilot (50 req/month free)
    pub github: ProviderSettings,
    
    /// TAMU AI (daily limits)
    pub tamu: ProviderSettings,
    
    /// NVIDIA NIM (rate limits)
    pub nvidia: ProviderSettings,
    
    /// Outlier Playground (unlimited via contract)
    pub outlier: ProviderSettings,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            ollama: ProviderSettings::default(),
            github: ProviderSettings::default(),
            tamu: ProviderSettings::default(),
            nvidia: ProviderSettings::default(),
            outlier: ProviderSettings::default(),
        }
    }
}

/// Manager for provider configuration
pub struct ProviderConfigManager {
    config_path: PathBuf,
}

impl ProviderConfigManager {
    pub fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
            .join("llm-conductor");
        
        std::fs::create_dir_all(&config_dir)?;
        
        Ok(Self {
            config_path: config_dir.join("providers.toml"),
        })
    }
    
    /// Load provider configuration
    pub fn load(&self) -> Result<ProvidersConfig> {
        if !self.config_path.exists() {
            // Create default config file
            let default = ProvidersConfig::default();
            self.save(&default)?;
            return Ok(default);
        }
        
        let contents = std::fs::read_to_string(&self.config_path)?;
        let config: ProvidersConfig = toml::from_str(&contents)?;
        Ok(config)
    }
    
    /// Save provider configuration
    pub fn save(&self, config: &ProvidersConfig) -> Result<()> {
        let contents = toml::to_string_pretty(config)?;
        std::fs::write(&self.config_path, contents)?;
        Ok(())
    }
    
    /// Check if a provider is enabled
    pub fn is_enabled(&self, provider: &str) -> bool {
        let config = match self.load() {
            Ok(c) => c,
            Err(_) => return true, // Default to enabled if config fails
        };
        
        match provider.to_lowercase().as_str() {
            "ollama" => config.ollama.enabled,
            "github" | "copilot" => config.github.enabled,
            "tamu" => config.tamu.enabled,
            "nvidia" | "nim" => config.nvidia.enabled,
            "outlier" => config.outlier.enabled,
            _ => true,
        }
    }
    
    /// Set provider enabled/disabled
    pub fn set_enabled(&self, provider: &str, enabled: bool) -> Result<()> {
        let mut config = self.load()?;
        
        match provider.to_lowercase().as_str() {
            "ollama" => config.ollama.enabled = enabled,
            "github" | "copilot" => config.github.enabled = enabled,
            "tamu" => config.tamu.enabled = enabled,
            "nvidia" | "nim" => config.nvidia.enabled = enabled,
            "outlier" => config.outlier.enabled = enabled,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
        
        self.save(&config)?;
        
        println!("✓ Provider '{}' {}", provider, if enabled { "enabled" } else { "disabled" });
        Ok(())
    }
    
    /// Set provider priority
    pub fn set_priority(&self, provider: &str, priority: u8) -> Result<()> {
        let mut config = self.load()?;
        
        match provider.to_lowercase().as_str() {
            "ollama" => config.ollama.priority = priority,
            "github" | "copilot" => config.github.priority = priority,
            "tamu" => config.tamu.priority = priority,
            "nvidia" | "nim" => config.nvidia.priority = priority,
            "outlier" => config.outlier.priority = priority,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
        
        self.save(&config)?;
        
        println!("✓ Provider '{}' priority set to {}", provider, priority);
        Ok(())
    }
    
    /// Get custom setting for a provider
    pub fn get_custom<T: for<'de> Deserialize<'de>>(
        &self,
        provider: &str,
        key: &str,
    ) -> Option<T> {
        let config = self.load().ok()?;
        
        let settings = match provider.to_lowercase().as_str() {
            "ollama" => &config.ollama,
            "github" | "copilot" => &config.github,
            "tamu" => &config.tamu,
            "nvidia" | "nim" => &config.nvidia,
            "outlier" => &config.outlier,
            _ => return None,
        };
        
        settings.custom.get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
    
    /// Set custom setting for a provider
    pub fn set_custom(
        &self,
        provider: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let mut config = self.load()?;
        
        let settings = match provider.to_lowercase().as_str() {
            "ollama" => &mut config.ollama,
            "github" | "copilot" => &mut config.github,
            "tamu" => &mut config.tamu,
            "nvidia" | "nim" => &mut config.nvidia,
            "outlier" => &mut config.outlier,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        };
        
        settings.custom.insert(key.to_string(), value);
        self.save(&config)?;
        
        println!("✓ Set {} for provider '{}'", key, provider);
        Ok(())
    }
    
    /// Get all enabled providers sorted by priority
    pub fn get_enabled_providers(&self) -> Vec<(String, u8)> {
        let config = match self.load() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        
        let mut providers = vec![];
        
        if config.ollama.enabled {
            providers.push(("ollama".to_string(), config.ollama.priority));
        }
        if config.github.enabled {
            providers.push(("github".to_string(), config.github.priority));
        }
        if config.tamu.enabled {
            providers.push(("tamu".to_string(), config.tamu.priority));
        }
        if config.nvidia.enabled {
            providers.push(("nvidia".to_string(), config.nvidia.priority));
        }
        if config.outlier.enabled {
            providers.push(("outlier".to_string(), config.outlier.priority));
        }
        
        // Sort by priority (descending)
        providers.sort_by(|a, b| b.1.cmp(&a.1));
        
        providers
    }
}
