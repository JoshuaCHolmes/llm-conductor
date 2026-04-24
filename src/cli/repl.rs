use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::path::PathBuf;

use crate::providers::Provider;
use crate::router::Router;
use crate::types::{Context, CoreContext, Message, Task, ProviderId};
use crate::usage_tracking::UsageTracker;

pub struct Repl {
    router: Router,
    context: Context,
    history: Vec<Message>,
    usage_tracker: UsageTracker,
    forced_model: Option<String>, // User can force a specific model
}

impl Repl {
    pub fn new(router: Router, config_dir: PathBuf) -> Result<Self> {
        // Default core context
        let core = CoreContext {
            system_instructions: "You are a helpful AI assistant.".to_string(),
            user_info: None,
            constraints: Vec::new(),
        };
        
        let usage_tracker = UsageTracker::new(&config_dir)?;
        
        Ok(Self {
            router,
            context: Context::new(core),
            history: Vec::new(),
            usage_tracker,
            forced_model: None,
        })
    }
    
    pub async fn run(&mut self) -> Result<()> {
        println!("{}", "llm-conductor v0.1.0".bright_cyan().bold());
        println!("{}", "Type your message or /help for commands".dimmed());
        println!();
        
        // Initialize models
        self.router.refresh_models().await?;
        
        let models = self.router.available_models();
        if models.is_empty() {
            eprintln!("{}", "No models available!".bright_red().bold());
            eprintln!("{}", "Make sure Ollama is running: ollama serve".yellow());
            return Ok(());
        }
        
        println!("{} {} models available", "✓".bright_green(), models.len());
        for model in models {
            println!("  • {} ({})", model.name.bright_white(), model.provider.to_string().dimmed());
        }
        println!();
        
        // REPL loop
        let mut rl = DefaultEditor::new()?;
        
        loop {
            let readline = rl.readline(&format!("{} ", "❯".bright_blue().bold()));
            
            match readline {
                Ok(line) => {
                    let line = line.trim();
                    
                    if line.is_empty() {
                        continue;
                    }
                    
                    // Handle commands
                    if line.starts_with('/') {
                        match self.handle_command(line).await {
                            Ok(should_continue) => {
                                if !should_continue {
                                    break;
                                }
                            }
                            Err(e) => {
                                eprintln!("{} {}", "Error:".bright_red(), e);
                            }
                        }
                        continue;
                    }
                    
                    // Handle user message
                    if let Err(e) = self.handle_message(line).await {
                        eprintln!("{} {}", "Error:".bright_red(), e);
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    async fn handle_command(&mut self, command: &str) -> Result<bool> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        
        match parts.get(0).map(|s| *s) {
            Some("/help") => {
                self.print_help();
                Ok(true)
            }
            Some("/model") => {
                if parts.len() == 1 {
                    // List available models
                    self.list_models();
                } else if parts.len() == 2 && parts[1] == "reset" {
                    self.forced_model = None;
                    println!("{}", "Model selection reset to automatic".green());
                } else {
                    // Set forced model
                    let model_name = parts[1..].join(" ");
                    let models = self.router.available_models();
                    if models.iter().any(|m| m.name == model_name) {
                        self.forced_model = Some(model_name.clone());
                        println!("{} {}", "Forced model set to:".green(), model_name.bright_white());
                    } else {
                        eprintln!("{} Model not found: {}", "Error:".bright_red(), model_name);
                        println!("Available models:");
                        self.list_models();
                    }
                }
                Ok(true)
            }
            Some("/providers") => {
                self.list_providers().await?;
                Ok(true)
            }
            Some("/clear") => {
                self.history.clear();
                println!("{}", "History cleared".green());
                Ok(true)
            }
            Some("/exit") | Some("/quit") => {
                println!("{}", "Goodbye!".bright_cyan());
                Ok(false)
            }
            _ => {
                eprintln!("{}", "Unknown command. Type /help for available commands.".yellow());
                Ok(true)
            }
        }
    }
    
    async fn handle_message(&mut self, content: &str) -> Result<()> {
        // Add user message to history
        self.history.push(Message::user(content));
        
        // Create task
        let task = Task::new(
            "User query",
            content,
        );
        
        // Select model (forced or automatic)
        let model = if let Some(forced_name) = &self.forced_model {
            self.router.find_model(forced_name)
                .ok_or_else(|| anyhow::anyhow!("Forced model '{}' not found", forced_name))?
        } else {
            self.router.select_model_with_usage(&task, &mut self.usage_tracker)
                .ok_or_else(|| anyhow::anyhow!("No suitable model available"))?
        };
        
        // Clone model info for later use
        let model_name = model.name.clone();
        let provider_id = model.provider.clone();
        let provider_display = model.provider.to_string();
        
        println!("{} {} {} {}", 
            "Using".dimmed(),
            model_name.bright_white(),
            "from".dimmed(),
            provider_display.bright_cyan()
        );
        println!();
        
        // Get messages for model
        let mut messages = self.context.to_messages();
        messages.extend(self.history.clone());
        
        // Find provider for this model
        // We need to match the provider - for now use first provider
        // TODO: Properly match provider to model
        let provider = &self.router.providers()[0];
        
        // Stream response
        print!("{} ", "❯".bright_green().bold());
        
        let mut response = String::new();
        let callback = |chunk: String| {
            print!("{}", chunk);
            use std::io::{self, Write};
            io::stdout().flush().unwrap();
        };
        
        response = provider.chat_stream(model, &messages, Box::new(callback)).await?;
        
        println!("\n");
        
        // Record usage (1 request, estimate tokens from response length)
        let estimated_tokens = (response.len() / 4) as u64; // Rough estimate: 4 chars per token
        self.usage_tracker.record_usage(provider_id.clone(), 1, estimated_tokens, 0.0);
        
        // Show usage info
        if let Some(usage) = self.usage_tracker.get_usage(&provider_id) {
            use crate::usage_tracking::LimitType;
            match &usage.limit_type {
                LimitType::Unlimited => {
                    // Don't show anything for unlimited
                }
                LimitType::RequestBased { max_requests, current_requests, .. } => {
                    let remaining = max_requests - current_requests;
                    println!("{} {} requests remaining", "└─".dimmed(), remaining.to_string().bright_yellow());
                }
                LimitType::TokenBased { max_tokens, current_tokens, .. } => {
                    let remaining_pct = ((max_tokens - current_tokens) as f64 / *max_tokens as f64) * 100.0;
                    println!("{} {:.1}% tokens remaining", "└─".dimmed(), remaining_pct.to_string().bright_yellow());
                }
                LimitType::CostBased { max_cost, current_cost, .. } => {
                    let remaining = max_cost - current_cost;
                    println!("{} ${:.2} remaining", "└─".dimmed(), remaining.to_string().bright_yellow());
                }
            }
        }
        println!();
        
        // Add assistant response to history
        self.history.push(Message::assistant(response));
        
        Ok(())
    }
    
    async fn list_providers(&self) -> Result<()> {
        println!("{}", "Available Providers:".bright_cyan().bold());
        
        for model in self.router.available_models() {
            println!("  {} {}", 
                "•".bright_blue(),
                model.name.bright_white()
            );
            println!("    Provider: {}", model.provider.to_string().dimmed());
            println!("    Tier: {:?}", model.capability_tier);
            println!("    Context: {} tokens", model.context_window);
        }
        
        Ok(())
    }
    
    fn list_models(&self) {
        let models = self.router.available_models();
        for model in models {
            let marker = if self.forced_model.as_ref() == Some(&model.name) {
                "→".bright_green()
            } else {
                "•".bright_blue()
            };
            println!("  {} {} ({})", marker, model.name.bright_white(), model.provider.to_string().dimmed());
        }
    }
    
    fn print_help(&self) {
        println!("{}", "Available Commands:".bright_cyan().bold());
        println!("  {} - Show this help message", "/help".bright_white());
        println!("  {} - List available models", "/model".bright_white());
        println!("  {} - Force use of a specific model", "/model <name>".bright_white());
        println!("  {} - Reset to automatic model selection", "/model reset".bright_white());
        println!("  {} - List available providers", "/providers".bright_white());
        println!("  {} - Clear conversation history", "/clear".bright_white());
        println!("  {} - Exit the REPL", "/exit or /quit".bright_white());
        println!();
        println!("{}", "Just type a message to chat!".dimmed());
    }
}
