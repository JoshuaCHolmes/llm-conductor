use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;

use crate::providers::Provider;
use crate::router::Router;
use crate::types::{Context, CoreContext, Message, Task};

pub struct Repl {
    router: Router,
    context: Context,
    history: Vec<Message>,
}

impl Repl {
    pub fn new(router: Router) -> Self {
        // Default core context
        let core = CoreContext {
            system_instructions: "You are a helpful AI assistant.".to_string(),
            user_info: None,
            constraints: Vec::new(),
        };
        
        Self {
            router,
            context: Context::new(core),
            history: Vec::new(),
        }
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
        
        // Select model
        let model = self.router.select_model(&task)
            .ok_or_else(|| anyhow::anyhow!("No suitable model available"))?;
        
        println!("{} {} {}", 
            "Using".dimmed(),
            model.name.bright_white(),
            "...".dimmed()
        );
        println!();
        
        // Get messages for model
        let mut messages = self.context.to_messages();
        messages.extend(self.history.clone());
        
        // Get provider for this model
        let provider = &self.router.providers[0];  // TODO: Match provider to model
        
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
    
    fn print_help(&self) {
        println!("{}", "Available Commands:".bright_cyan().bold());
        println!("  {} - Show this help message", "/help".bright_white());
        println!("  {} - List available providers", "/providers".bright_white());
        println!("  {} - Clear conversation history", "/clear".bright_white());
        println!("  {} - Exit the REPL", "/exit or /quit".bright_white());
        println!();
        println!("{}", "Just type a message to chat!".dimmed());
    }
}
