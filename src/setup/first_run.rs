use anyhow::{anyhow, Result};
use colored::*;
use dialoguer::Confirm;

use super::{OllamaInstaller, InstallStatus, ModelManager};
use crate::config::{CredentialManager, UserInfoManager};

/// Handles first-time setup
pub struct FirstRunSetup {
    credentials: CredentialManager,
    user_info: UserInfoManager,
    models: ModelManager,
}

impl FirstRunSetup {
    pub fn new() -> Result<Self> {
        Ok(Self {
            credentials: CredentialManager::new()?,
            user_info: UserInfoManager::new()?,
            models: ModelManager::new()?,
        })
    }
    
    /// Run complete first-time setup
    pub async fn run(&mut self) -> Result<()> {
        println!("\n{}", "╔═══════════════════════════════════════╗".bright_cyan());
        println!("{}", "║  Welcome to llm-conductor! 🎭        ║".bright_cyan());
        println!("{}", "╚═══════════════════════════════════════╝".bright_cyan());
        println!("\nLet's get you set up. This will take a few minutes.\n");
        
        // Step 1: User Information
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        println!("{} {}", "Step 1:".bright_white().bold(), "User Information".bright_white());
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        
        if Confirm::new()
            .with_prompt("Would you like to configure user information? (recommended)")
            .default(true)
            .interact()?
        {
            self.user_info.interactive_setup()?;
        } else {
            println!("{}", "Skipped. You can configure later with: llm-conductor config user".yellow());
        }
        
        println!();
        
        // Step 2: Ollama Installation
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        println!("{} {}", "Step 2:".bright_white().bold(), "Ollama Setup".bright_white());
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        
        match OllamaInstaller::check_installation().await {
            InstallStatus::InstalledAndRunning => {
                println!("{} Ollama is installed and running", "✓".bright_green());
            }
            InstallStatus::InstalledNotRunning => {
                println!("{} Ollama installed but not running", "!".bright_yellow());
                println!("Starting Ollama server...");
                OllamaInstaller::start_server().await?;
            }
            InstallStatus::NotInstalled => {
                println!("{} Ollama not found", "✗".bright_red());
                
                if Confirm::new()
                    .with_prompt("Install Ollama now? (required for local models)")
                    .default(true)
                    .interact()?
                {
                    OllamaInstaller::install().await?;
                    OllamaInstaller::start_server().await?;
                } else {
                    println!("{}", "⚠ Ollama is required for local models.".yellow());
                    println!("Install manually from: {}", "https://ollama.com".bright_cyan());
                    return Err(anyhow!("Ollama installation declined"));
                }
            }
        }
        
        println!();
        
        // Step 3: Model Downloads
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        println!("{} {}", "Step 3:".bright_white().bold(), "Local Models".bright_white());
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        
        self.models.ensure_models().await?;
        
        // Step 4: API Credentials (Optional)
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        println!("{} {}", "Step 4:".bright_white().bold(), "API Credentials (Optional)".bright_white());
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
        
        println!("You can add API keys for cloud providers to access more powerful models.");
        println!("  • {}: GLM-5 Plus (89B parameters)", "NVIDIA NIM".bright_white());
        println!("  • {}: GPT-4o, Claude Sonnet", "GitHub Copilot".bright_white());
        println!("  • {}: Claude Opus 4.5", "TAMU AI".bright_white());
        println!();
        
        if Confirm::new()
            .with_prompt("Configure API keys now?")
            .default(false)
            .interact()?
        {
            self.credentials.interactive_setup().await?;
        } else {
            println!("{}", "Skipped. You can add keys later with: llm-conductor config add-key".yellow());
        }
        
        println!();
        
        // Done!
        println!("{}", "╔═══════════════════════════════════════╗".bright_green());
        println!("{}", "║  ✓ Setup Complete! 🎉                ║".bright_green());
        println!("{}", "╚═══════════════════════════════════════╝".bright_green());
        println!();
        println!("You're ready to use llm-conductor!");
        println!();
        println!("Try these commands:");
        println!("  {} - Start chatting", "llm-conductor".bright_white());
        println!("  {} - List available providers", "llm-conductor providers".bright_white());
        println!("  {} - View configuration", "llm-conductor config show".bright_white());
        println!();
        
        Ok(())
    }
    
    /// Check if setup has been completed
    pub fn is_setup_complete() -> bool {
        let config_dir = match dirs::config_dir() {
            Some(dir) => dir.join("llm-conductor"),
            None => return false,
        };
        
        config_dir.join(".setup_complete").exists()
    }
    
    /// Mark setup as complete
    pub fn mark_complete(&self) -> Result<()> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?
            .join("llm-conductor");
        
        std::fs::create_dir_all(&config_dir)?;
        
        let marker = config_dir.join(".setup_complete");
        std::fs::write(marker, "")?;
        
        Ok(())
    }
    
    /// Quick status check
    pub async fn status() -> Result<()> {
        println!("{}", "=== llm-conductor Status ===".bright_cyan().bold());
        println!();
        
        // Ollama
        print!("Ollama:          ");
        match OllamaInstaller::check_installation().await {
            InstallStatus::InstalledAndRunning => {
                println!("{} Running", "✓".bright_green());
                if let Ok(version) = OllamaInstaller::get_version() {
                    println!("                 {}", version.dimmed());
                }
            }
            InstallStatus::InstalledNotRunning => {
                println!("{} Installed but not running", "!".bright_yellow());
            }
            InstallStatus::NotInstalled => {
                println!("{} Not installed", "✗".bright_red());
            }
        }
        
        // Models
        let model_manager = ModelManager::new()?;
        if let Ok(models) = model_manager.list_ollama_models().await {
            println!();
            println!("Local Models:    {} available", models.len());
            for model in models.iter().take(5) {
                println!("                 • {}", model.name.bright_white());
            }
            if models.len() > 5 {
                println!("                 ... and {} more", models.len() - 5);
            }
        }
        
        // Credentials
        let cred_manager = CredentialManager::new()?;
        if let Ok(providers) = cred_manager.list_configured() {
            println!();
            println!("API Keys:        {}", if providers.is_empty() {
                "None configured".to_string()
            } else {
                format!("{} configured", providers.len())
            });
            for provider in providers {
                println!("                 • {}", provider.bright_white());
            }
        }
        
        // User Info
        let user_manager = UserInfoManager::new()?;
        if let Ok(Some(info)) = user_manager.load_user_info() {
            println!();
            println!("User:            {}", info.name.bright_white());
            if let Some(institution) = info.institution {
                println!("                 {}", institution.dimmed());
            }
        }
        
        println!();
        
        Ok(())
    }
}
