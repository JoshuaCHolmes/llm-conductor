//! LLM Conductor - Intelligent LLM orchestration
use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber;

use llm_conductor::cli::Repl;
use llm_conductor::config::{CredentialManager, UserInfoManager};
use llm_conductor::providers::OllamaProvider;
use llm_conductor::router::Router;
use llm_conductor::setup::{FirstRunSetup, InstallStatus, OllamaInstaller};

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
    
    // Add Ollama provider (unless explicitly disabled)
    if std::env::var("OLLAMA_DISABLED").unwrap_or_default() != "true" {
        router.add_provider(Box::new(OllamaProvider::new(None)));
    }
    
    // Load credentials and add cloud providers if configured
    let cred_manager = CredentialManager::new()?;
    
    if let Ok(Some(github_key)) = cred_manager.get_credential("GITHUB_TOKEN") {
        use llm_conductor::providers::GitHubProvider;
        router.add_provider(Box::new(GitHubProvider::new(github_key)));
    }
    
    if let Ok(Some(tamu_key)) = cred_manager.get_credential("TAMU_API_KEY") {
        use llm_conductor::providers::TamuProvider;
        router.add_provider(Box::new(TamuProvider::new(tamu_key)));
    }
    
    if let Ok(Some(nvidia_key)) = cred_manager.get_credential("NVIDIA_NIM_KEY") {
        use llm_conductor::providers::NvidiaProvider;
        router.add_provider(Box::new(NvidiaProvider::new(Some(nvidia_key))));
    }
    
    // Add Outlier provider if credentials are present (cookie and csrf_token)
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
    
    // Refresh available models
    router.refresh_models().await?;
    
    // Create and run REPL
    let mut repl = Repl::new(router);
    repl.run().await?;
    
    Ok(())
}

async fn list_providers() -> Result<()> {
    use colored::*;
    
    println!("{}", "=== Available Providers ===".bright_cyan().bold());
    println!();
    
    // Check Ollama
    print!("{} ", "Ollama".bright_white().bold());
    match OllamaInstaller::check_installation().await {
        InstallStatus::InstalledAndRunning => {
            println!("{}", "✓ Running".bright_green());
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
        ("NVIDIA NIM", "NVIDIA_NIM_KEY"),
        ("GitHub Copilot", "GITHUB_TOKEN"),
        ("TAMU AI", "TAMU_API_KEY"),
        ("Outlier Playground", "OUTLIER_COOKIE"), // Also checks OUTLIER_CSRF
    ];
    
    for (display_name, _key) in all_providers {
        print!("{} ", display_name.bright_white().bold());
        if configured_providers.contains(&display_name.to_string()) {
            println!("{}", "✓ Configured".bright_green());
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
