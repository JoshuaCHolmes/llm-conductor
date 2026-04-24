use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::path::PathBuf;

use crate::providers::Provider;
use crate::router::Router;
use crate::types::{Context, CoreContext, Message, Task, ProviderId};
use crate::usage_tracking::UsageTracker;
use crate::model_filter::ModelFilter;

pub struct Repl {
    router: Router,
    context: Context,
    history: Vec<Message>,
    usage_tracker: UsageTracker,
    model_filter: ModelFilter, // User-specified filter
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
            model_filter: ModelFilter::new(),
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
                    // List available models with current filter
                    self.list_models();
                } else if parts.len() == 2 && parts[1] == "reset" {
                    self.model_filter = ModelFilter::new();
                    println!("{}", "Model filter reset to automatic selection".green());
                } else {
                    // Parse filter arguments
                    let args = &parts[1..];
                    self.model_filter = ModelFilter::from_args(args);
                    
                    // Show what models match the filter
                    let filtered: Vec<_> = self.router.available_models()
                        .iter()
                        .filter(|m| self.model_filter.matches(m))
                        .collect();
                    
                    if filtered.is_empty() {
                        eprintln!("{} No models match filter: {}", 
                            "Error:".bright_red(), 
                            self.model_filter.description());
                        println!("\nAvailable models:");
                        self.list_models();
                    } else {
                        println!("{} Applied filter: {}", 
                            "✓".bright_green(), 
                            self.model_filter.description());
                        println!("\nMatching models:");
                        for model in filtered {
                            println!("  • {} ({}, {:?}, {}k ctx)", 
                                model.name.bright_white(),
                                model.provider.to_string().dimmed(),
                                model.capability_tier,
                                model.context_window / 1000
                            );
                        }
                    }
                }
                Ok(true)
            }
            Some("/new") => {
                self.history.clear();
                println!("{}", "✓ Started new conversation (history cleared)".green());
                println!("{}", "Note: Outlier reuses last conversation - refresh Outlier Playground to see new one".dimmed());
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
        
        // Select model using filter
        let model = self.router.select_model_filtered(&task, &self.model_filter, &mut self.usage_tracker)
            .ok_or_else(|| anyhow::anyhow!("No suitable model available with current filters"))?;
        
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
        let provider = self.router.find_provider_for_model(model)
            .ok_or_else(|| anyhow::anyhow!("Could not find provider for model {}", model_name))?;
        
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
        println!("Active filter: {}", self.model_filter.description());
        println!();
        
        let models = self.router.available_models();
        let mut shown = 0;
        
        for model in models {
            // Show all models if no filter, or only matching models
            if !self.model_filter.is_empty() && !self.model_filter.matches(model) {
                continue;
            }
            
            println!("  • {} ({}, {:?}, {}k ctx)", 
                model.name.bright_white(),
                model.provider.to_string().dimmed(),
                model.capability_tier,
                model.context_window / 1000
            );
            shown += 1;
        }
        
        if shown == 0 {
            println!("  {}", "No models match current filter".yellow());
        }
    }
    
    fn print_help(&self) {
        println!("{}", "Available Commands:".bright_cyan().bold());
        println!("  {} - Show this help message", "/help".bright_white());
        println!("  {} - List available models", "/model".bright_white());
        println!("  {} - Filter by model/provider/tier", "/model <filters...>".bright_white());
        println!("    Examples:");
        println!("      /model claude-opus-4.6        - Use specific model");
        println!("      /model tamu                   - Use TAMU models only");
        println!("      /model frontier               - Use frontier-tier models");
        println!("      /model claude-opus tamu       - Use Opus from TAMU");
        println!("      /model outlier frontier       - Use Outlier frontier models");
        println!("  {} - Reset to automatic model selection", "/model reset".bright_white());
        println!("  {} - List available providers", "/providers".bright_white());
        println!("  {} - Start a new conversation", "/new".bright_white());
        println!("  {} - Clear conversation history", "/clear".bright_white());
        println!("  {} - Exit the REPL", "/exit or /quit".bright_white());
        println!();
        println!("{}", "Just type a message to chat!".dimmed());
    }
}
