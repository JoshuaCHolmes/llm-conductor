//! LLM Conductor - Intelligent LLM orchestration
use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber;

use llm_conductor::cli::Repl;
use llm_conductor::config::{CredentialManager, ProviderConfigManager, UserInfoManager};
use llm_conductor::providers::OllamaProvider;
use llm_conductor::router::Router;
use llm_conductor::setup::{FirstRunSetup, InstallStatus, OllamaInstaller};
use llm_conductor::usage_tracking::{UsageTracker, ProviderUsage, ResetPeriod};
use llm_conductor::types::ProviderId;

#[derive(Parser)]
#[command(name = "llm-conductor", version, about = "Intelligent LLM orchestration")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive chat (default)
    Chat,
    
    /// Run first-time setup
    Setup,
    
    /// Show system status
    Status,
    
    /// List available providers
    Providers,
    
    /// Configuration management
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    
    /// Add API key
    AddKey {
        /// Provider name (nvidia, github, tamu, outlier_cookie, outlier_csrf)
        provider: String,
        /// API key value
        key: String,
    },
    
    /// Setup API keys interactively
    SetupKeys,
    
    /// Auto-extract Outlier credentials from browser
    AddOutlier {
        /// Browser to extract from (vivaldi, chrome, edge)
        #[arg(long, default_value = "vivaldi")]
        browser: String,
        /// Browser profile name
        #[arg(long, default_value = "Default")]
        profile: String,
    },
    
    /// Enable or disable a provider
    SetProvider {
        /// Provider name (ollama, github, tamu, nvidia, outlier)
        provider: String,
        /// Enable the provider
        #[arg(long)]
        enable: Option<bool>,
        /// Disable the provider
        #[arg(long, conflicts_with = "enable")]
        disable: bool,
        /// Set priority (0-100, higher = preferred)
        #[arg(long)]
        priority: Option<u8>,
    },
    
    /// Configure user information
    User,
    
    /// Add additional context
    AddContext {
        /// Context to add
        context: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(false)
        .init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Some(Commands::Setup) => {
            // Force setup
            let mut setup = FirstRunSetup::new()?;
            setup.run().await?;
            setup.mark_complete()?;
        }
        
        Some(Commands::Status) => {
            FirstRunSetup::status().await?;
        }
        
        Some(Commands::Providers) => {
            list_providers().await?;
        }
        
        Some(Commands::Config(config_cmd)) => {
            handle_config_command(config_cmd).await?;
        }
        
        Some(Commands::Chat) | None => {
            // Default: Start chat
            run_chat().await?;
        }
    }
    
    Ok(())
}

async fn run_chat() -> Result<()> {
    // Check if first run
    if !FirstRunSetup::is_setup_complete() {
        let mut setup = FirstRunSetup::new()?;
        setup.run().await?;
        setup.mark_complete()?;
    }
    
    // Ensure Ollama is running
    match OllamaInstaller::check_installation().await {
        InstallStatus::InstalledNotRunning => {
            use colored::*;
            println!("{}", "Starting Ollama server...".yellow());
            OllamaInstaller::start_server().await?;
        }
        InstallStatus::NotInstalled => {
            use colored::*;
            eprintln!("{}", "Ollama not found!".bright_red());
            eprintln!("Please run: {}", "llm-conductor setup".bright_white());
            return Err(anyhow::anyhow!("Ollama not installed"));
        }
        _ => {}
    }
    
    // Create router
    let mut router = Router::new();
    
    // Load provider configuration
    let provider_config_manager = ProviderConfigManager::new()?;
    
    // Add Ollama provider if enabled
    if provider_config_manager.is_enabled("ollama") {
        router.add_provider(Box::new(OllamaProvider::new(None)));
    }
    
    // Load credentials and add cloud providers if configured AND enabled
    let cred_manager = CredentialManager::new()?;
    
    if let Ok(Some(github_key)) = cred_manager.get_credential("GITHUB_TOKEN") {
        if provider_config_manager.is_enabled("github") {
            use llm_conductor::providers::GitHubProvider;
            router.add_provider(Box::new(GitHubProvider::new(github_key)));
        }
    }
    
    if let Ok(Some(tamu_key)) = cred_manager.get_credential("TAMU_API_KEY") {
        if provider_config_manager.is_enabled("tamu") {
            use llm_conductor::providers::TamuProvider;
            router.add_provider(Box::new(TamuProvider::new(tamu_key)));
        }
    }
    
    if let Ok(Some(nvidia_key)) = cred_manager.get_credential("NVIDIA_NIM_KEY") {
        if provider_config_manager.is_enabled("nvidia") {
            use llm_conductor::providers::NvidiaProvider;
            router.add_provider(Box::new(NvidiaProvider::new(Some(nvidia_key))));
        }
    }
    
    // Add Outlier provider if credentials are present AND enabled
    if provider_config_manager.is_enabled("outlier") {
        if let (Ok(Some(cookie)), Ok(Some(csrf_token))) = (
            cred_manager.get_credential("OUTLIER_COOKIE"),
            cred_manager.get_credential("OUTLIER_CSRF"),
        ) {
            use llm_conductor::providers::OutlierProvider;
            match OutlierProvider::new(cookie, csrf_token) {
                Ok(provider) => router.add_provider(Box::new(provider)),
                Err(e) => eprintln!("⚠ Failed to initialize Outlier provider: {}", e),
            }
        }
    }
    
    // Refresh available models
    router.refresh_models().await?;
    
    // Initialize usage tracker (will set defaults automatically)
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
        .join("llm-conductor");
    let _usage_tracker = UsageTracker::new(&config_dir)?;
    
    // Create and run REPL
    let mut repl = Repl::new(router, config_dir)?;
    repl.run().await?;
    
    Ok(())
}

async fn list_providers() -> Result<()> {
    use colored::*;
    
    // Get config directory
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
        .join("llm-conductor");
    
    // Initialize usage tracker
    let mut usage_tracker = UsageTracker::new(&config_dir)?;
    
    println!("{}", "=== Available Providers ===".bright_cyan().bold());
    println!();
    
    // Check Ollama
    print!("{} ", "Ollama".bright_white().bold());
    match OllamaInstaller::check_installation().await {
        InstallStatus::InstalledAndRunning => {
            print!("{}", "✓ Running".bright_green());
            if let Some(usage) = usage_tracker.get_usage(&ProviderId::Ollama) {
                let remaining = usage.remaining_capacity();
                println!(" ({})", format!("Unlimited").dimmed());
            } else {
                println!(" ({})", "No usage data".dimmed());
            }
        }
        InstallStatus::InstalledNotRunning => {
            println!("{}", "! Installed but not running".bright_yellow());
        }
        InstallStatus::NotInstalled => {
            println!("{}", "✗ Not installed".bright_red());
        }
    }
    
    // Check configured API keys
    let cred_manager = CredentialManager::new()?;
    let configured_providers = cred_manager.list_configured()?;
    
    // Define all possible providers with their display names
    let all_providers = vec![
        ("NVIDIA NIM", "NVIDIA_NIM_KEY", ProviderId::NvidiaNim),
        ("GitHub Copilot", "GITHUB_TOKEN", ProviderId::GitHubCopilot),
        ("TAMU AI", "TAMU_API_KEY", ProviderId::Tamu),
        ("Outlier Playground", "OUTLIER_COOKIE", ProviderId::Outlier),
    ];
    
    for (display_name, _key, provider_id) in all_providers {
        print!("{} ", display_name.bright_white().bold());
        if configured_providers.contains(&display_name.to_string()) {
            print!("{}", "✓ Configured".bright_green());
            
            // Show usage info if available
            if let Some(usage) = usage_tracker.get_usage(&provider_id) {
                use llm_conductor::usage_tracking::LimitType;
                match &usage.limit_type {
                    LimitType::Unlimited => {
                        println!(" ({})", "Unlimited".dimmed());
                    }
                    LimitType::RequestBased { max_requests, current_requests, reset_period, next_reset } => {
                        let remaining = max_requests - current_requests;
                        let formatted_remaining = if remaining == 0 {
                            remaining.to_string().bright_red()
                        } else if (remaining as f64 / *max_requests as f64) < 0.2 {
                            remaining.to_string().bright_yellow()
                        } else {
                            remaining.to_string().bright_green()
                        };
                        println!(" ({}/{} requests, resets {:?})",
                            formatted_remaining,
                            max_requests,
                            reset_period
                        );
                    }
                    LimitType::TokenBased { max_tokens, current_tokens, reset_period, .. } => {
                        let remaining = max_tokens - current_tokens;
                        let pct = (remaining as f64 / *max_tokens as f64) * 100.0;
                        let formatted_pct = if pct < 10.0 {
                            format!("{:.1}%", pct).bright_red()
                        } else if pct < 30.0 {
                            format!("{:.1}%", pct).bright_yellow()
                        } else {
                            format!("{:.1}%", pct).bright_green()
                        };
                        println!(" ({} tokens remaining, resets {:?})",
                            formatted_pct,
                            reset_period
                        );
                    }
                    LimitType::CostBased { max_cost, current_cost, reset_period, .. } => {
                        let remaining = max_cost - current_cost;
                        let pct = (remaining / max_cost) * 100.0;
                        let formatted_remaining = if pct < 10.0 {
                            format!("${:.2}", remaining).bright_red()
                        } else if pct < 30.0 {
                            format!("${:.2}", remaining).bright_yellow()
                        } else {
                            format!("${:.2}", remaining).bright_green()
                        };
                        println!(" ({}/{:.2}, resets {:?})",
                            formatted_remaining,
                            max_cost,
                            reset_period
                        );
                    }
                }
            } else {
                println!(" ({})", "No usage data".dimmed());
            }
        } else {
            println!("{}", "○ Not configured".dimmed());
        }
    }
    
    println!();
    
    Ok(())
}

async fn handle_config_command(cmd: ConfigCommands) -> Result<()> {
    use colored::*;
    
    match cmd {
        ConfigCommands::Show => {
            println!("{}", "=== Configuration ===".bright_cyan().bold());
            println!();
            
            // User info
            let user_manager = UserInfoManager::new()?;
            if let Some(info) = user_manager.load_user_info()? {
                println!("{}", "User Information:".bright_white());
                println!("  Name: {}", info.name);
                if let Some(inst) = info.institution {
                    println!("  Institution: {}", inst);
                }
                if let Some(role) = info.role {
                    println!("  Role: {}", role);
                }
                if !info.additional_context.is_empty() {
                    println!("  Additional context:");
                    for ctx in &info.additional_context {
                        println!("    - {}", ctx);
                    }
                }
                println!();
            }
            
            // Credentials
            let cred_manager = CredentialManager::new()?;
            let providers = cred_manager.list_configured()?;
            
            println!("{}", "API Keys:".bright_white());
            if providers.is_empty() {
                println!("  {}", "None configured".dimmed());
            } else {
                for provider in providers {
                    println!("  ✓ {}", provider);
                }
            }
            println!();
        }
        
        ConfigCommands::AddKey { provider, key } => {
            let cred_manager = CredentialManager::new()?;
            cred_manager.add_credential(&provider, &key)?;
        }
        
        ConfigCommands::SetupKeys => {
            let cred_manager = CredentialManager::new()?;
            cred_manager.interactive_setup().await?;
        }
        
        ConfigCommands::AddOutlier { browser, profile } => {
            use colored::*;
            println!("{}", "⚠ Automated Outlier cookie extraction not yet implemented".yellow());
            println!();
            println!("Please extract cookies manually:");
            println!("1. Open {} in {}", "https://playground.outlier.ai".cyan(), browser);
            println!("2. Press F12 → Application → Cookies");
            println!("3. Find: _jwt, _session, _csrf");
            println!();
            println!("Then run:");
            println!("  llm-conductor config add-key outlier_cookie '_jwt=...; _session=...; _csrf=...'");
            println!("  llm-conductor config add-key outlier_csrf 'YOUR_CSRF_VALUE'");
            println!();
            println!("See: docs/OUTLIER_SETUP.md");
        }
        
        ConfigCommands::SetProvider { provider, enable, disable, priority } => {
            use colored::*;
            let provider_manager = ProviderConfigManager::new()?;
            
            // Determine enabled state
            let enabled = if disable {
                false
            } else if let Some(e) = enable {
                e
            } else {
                true // Default to enable if neither flag specified
            };
            
            provider_manager.set_enabled(&provider, enabled)?;
            
            if let Some(prio) = priority {
                provider_manager.set_priority(&provider, prio)?;
            }
            
            println!();
            println!("{}", "Provider configuration updated!".bright_green());
            println!();
            println!("Current enabled providers:");
            for (name, prio) in provider_manager.get_enabled_providers() {
                println!("  {} (priority: {})", name.bright_white(), prio);
            }
        }
        
        ConfigCommands::User => {
            let user_manager = UserInfoManager::new()?;
            user_manager.interactive_setup()?;
        }
        
        ConfigCommands::AddContext { context } => {
            let user_manager = UserInfoManager::new()?;
            user_manager.add_context(context)?;
        }
    }
    
    Ok(())
}
