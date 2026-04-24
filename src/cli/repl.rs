use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::cli::session::SessionStore;
use crate::cli::executor::{self, ShellTurn};
use crate::providers::Provider;
use crate::router::Router;
use crate::types::{Context, CoreContext, Message, Task, ProviderId};
use crate::usage_tracking::UsageTracker;
use crate::model_filter::ModelFilter;

/// State machine for streaming responses that may contain <think>...</think> blocks.
/// Think blocks are printed dimmed; clean_text accumulates the response without them.
#[derive(Default)]
struct ThinkStreamState {
    in_think: bool,
    pending: String,
    clean_text: String,
    showed_thinking: bool,
}

impl ThinkStreamState {
    fn process_chunk(&mut self, chunk: &str) {
        self.pending.push_str(chunk);
        loop {
            if self.in_think {
                if let Some(pos) = self.pending.find("</think>") {
                    let before = &self.pending[..pos];
                    if !before.is_empty() {
                        print!("{}", before.dimmed());
                        std::io::stdout().flush().unwrap();
                    }
                    self.in_think = false;
                    self.pending = self.pending[pos + "</think>".len()..].to_string();
                } else {
                    // Keep last 8 chars buffered in case </think> spans chunks.
                    // safe_len is a byte offset; walk back to a valid char boundary.
                    let safe_len = self.pending.len().saturating_sub(8);
                    let safe_len = (0..=safe_len).rev()
                        .find(|&i| self.pending.is_char_boundary(i))
                        .unwrap_or(0);
                    if safe_len > 0 {
                        let to_print = self.pending[..safe_len].to_string();
                        print!("{}", to_print.dimmed());
                        std::io::stdout().flush().unwrap();
                        self.pending = self.pending[safe_len..].to_string();
                    }
                    break;
                }
            } else {
                if let Some(pos) = self.pending.find("<think>") {
                    let before = &self.pending[..pos];
                    if !before.is_empty() {
                        print!("{}", before);
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(before);
                    }
                    if !self.showed_thinking {
                        println!("{}", "💭 Thinking...".dimmed().italic());
                        self.showed_thinking = true;
                    }
                    self.in_think = true;
                    self.pending = self.pending[pos + "<think>".len()..].to_string();
                } else {
                    // Keep last 7 chars buffered in case <think> spans chunks.
                    // safe_len is a byte offset; walk back to a valid char boundary.
                    let safe_len = self.pending.len().saturating_sub(7);
                    let safe_len = (0..=safe_len).rev()
                        .find(|&i| self.pending.is_char_boundary(i))
                        .unwrap_or(0);
                    if safe_len > 0 {
                        let to_print = self.pending[..safe_len].to_string();
                        print!("{}", to_print);
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(&to_print);
                        self.pending = self.pending[safe_len..].to_string();
                    }
                    break;
                }
            }
        }
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            if self.in_think {
                print!("{}", self.pending.dimmed());
            } else {
                print!("{}", self.pending);
                self.clean_text.push_str(&self.pending);
            }
            std::io::stdout().flush().unwrap();
            self.pending.clear();
        }
    }
}

pub struct Repl {
    router: Router,
    context: Context,
    history: Vec<Message>,
    usage_tracker: UsageTracker,
    model_filter: ModelFilter,
    session_store: SessionStore,
    /// Current session ID (set after first message)
    session_id: Option<String>,
    /// Current page for /sessions pagination
    sessions_page: usize,
    /// Shell turns this session: index 0 = turn #1
    shell_turns: Vec<ShellTurn>,
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
        let session_store = SessionStore::new(&config_dir)?;
        
        Ok(Self {
            router,
            context: Context::new(core),
            history: Vec::new(),
            usage_tracker,
            model_filter: ModelFilter::new(),
            session_store,
            session_id: None,
            sessions_page: 0,
            shell_turns: Vec::new(),
        })
    }

    /// Load an existing session by ID, restoring conversation history.
    pub fn load_session(&mut self, session_id: &str) -> Result<()> {
        let session = self.session_store.load(session_id)?;
        self.history = session.messages;
        self.session_id = Some(session_id.to_string());
        println!("{} Resumed session with {} messages",
            "✓".bright_green(),
            self.history.len()
        );
        Ok(())
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
                self.session_id = None;
                self.shell_turns.clear();
                println!("{}", "✓ Started new conversation".green());
                Ok(true)
            }
            Some("/sessions") => {
                let arg = parts.get(1).map(|s| *s);
                match arg {
                    Some(">") => {
                        let total = self.session_store.list()?.len();
                        let max_page = total.div_ceil(10).saturating_sub(1);
                        if self.sessions_page < max_page {
                            self.sessions_page += 1;
                        }
                    }
                    Some("<") => {
                        self.sessions_page = self.sessions_page.saturating_sub(1);
                    }
                    _ => {
                        self.sessions_page = 0;
                    }
                }
                self.session_store.print_page(self.sessions_page)?;
                println!("{}", "Use /load N to resume a session.".dimmed());
                Ok(true)
            }
            Some("/load") => {
                match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(n) => {
                        let meta = self.session_store.get_by_number(n)?;
                        self.load_session(&meta.id)?;
                    }
                    None => {
                        eprintln!("{}", "Usage: /load N  (use /sessions to see numbers)".yellow());
                    }
                }
                Ok(true)
            }
            Some("/show") => {
                match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(n) if n >= 1 && n <= self.shell_turns.len() => {
                        let turn = &self.shell_turns[n - 1];
                        println!("{} {} {}", "●".bright_cyan(), turn.cmd.bright_white(), format!("(shell #{})", n).dimmed());
                        for line in turn.output.lines() {
                            println!("  {} {}", "│".dimmed(), line);
                        }
                        println!("  {}", "└".dimmed());
                    }
                    _ => {
                        if self.shell_turns.is_empty() {
                            eprintln!("{}", "No shell turns in this session.".yellow());
                        } else {
                            eprintln!("{}", format!("Usage: /show N  (1–{})", self.shell_turns.len()).yellow());
                        }
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

        const MAX_TOOL_ROUNDS: usize = 5;
        let mut tool_rounds = 0;

        loop {
            // Create task from last user message
            let last_user = self.history.iter().rev()
                .find(|m| matches!(m.role, crate::types::Role::User))
                .map(|m| m.content.clone())
                .unwrap_or_default();

            let task = Task::new("User query", &last_user);

            // Select model (clone to release borrow)
            let model = self.router.select_model_filtered(&task, &self.model_filter, &mut self.usage_tracker)
                .ok_or_else(|| anyhow::anyhow!("No suitable model available with current filters"))?
                .clone();

            let model_name = model.name.clone();
            let provider_id = model.provider.clone();
            let provider_display = model.provider.to_string();

            // Show "Using X from Y" with current usage (only on first round)
            if tool_rounds == 0 {
                let usage_suffix = self.format_usage_suffix(&provider_id);
                println!("{} {} {} {}{}",
                    "Using".dimmed(),
                    model_name.bright_white(),
                    "from".dimmed(),
                    provider_display.bright_cyan(),
                    usage_suffix
                );
                println!();
            }

            // Get messages for model
            let mut messages = self.context.to_messages();
            messages.extend(self.history.clone());

            // Find provider
            let provider = self.router.find_provider_for_model(&model)
                .ok_or_else(|| anyhow::anyhow!("Could not find provider for model {}", model_name))?;

            // Stream response
            print!("{} ", "❯".bright_green().bold());

            let think_state = Arc::new(Mutex::new(ThinkStreamState::default()));
            let think_state_cb = think_state.clone();
            let callback = move |chunk: String| {
                think_state_cb.lock().unwrap().process_chunk(&chunk);
            };

            let (_raw_response, token_count) = provider.chat_stream(&model, &messages, Box::new(callback)).await?;
            think_state.lock().unwrap().flush();
            let clean_response = think_state.lock().unwrap().clean_text.clone();
            println!("\n");

            // Record usage
            let tokens = token_count.unwrap_or_else(|| (clean_response.len() / 4) as u64);
            self.usage_tracker.record_usage(provider_id.clone(), 1, tokens, 0.0);

            // Add assistant response to history
            self.history.push(Message::assistant(clean_response.clone()));

            // Extract bash blocks and execute (up to MAX_TOOL_ROUNDS)
            let bash_blocks = executor::extract_bash_blocks(&clean_response);
            if bash_blocks.is_empty() || tool_rounds >= MAX_TOOL_ROUNDS {
                break;
            }

            // Process each bash block
            let mut shell_results = Vec::new();
            for cmd in &bash_blocks {
                let kind = executor::classify(cmd);
                let approved = match kind {
                    executor::CommandKind::ReadOnly => true,
                    executor::CommandKind::NeedsConfirm => {
                        print!("{} {} {} ",
                            "⚡".yellow(),
                            "Run:".bright_white(),
                            cmd.bright_yellow()
                        );
                        print!("{}", " [y/N] ".dimmed());
                        std::io::stdout().flush()?;
                        let mut ans = String::new();
                        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut ans)?;
                        ans.trim().eq_ignore_ascii_case("y")
                    }
                };

                if approved {
                    let output = executor::execute(cmd).unwrap_or_else(|e| format!("Error: {}", e));
                    let turn_num = self.shell_turns.len() + 1;
                    print!("{}", executor::format_shell_display(turn_num, cmd, &output));
                    self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                    shell_results.push(format!("$ {}\n{}", cmd, output));
                } else {
                    println!("{}", "  (skipped)".dimmed());
                }
            }

            if shell_results.is_empty() {
                // All blocks were skipped — don't loop
                break;
            }

            // Feed shell results back to model as a user message, then loop
            let feedback = format!("[Shell output]\n{}\n[End of shell output]", shell_results.join("\n---\n"));
            self.history.push(Message::user(feedback));
            tool_rounds += 1;
        }

        // Auto-save session
        match self.session_store.save(self.session_id.as_deref(), &self.history) {
            Ok(id) => { self.session_id = Some(id); }
            Err(e) => eprintln!("{} Failed to auto-save session: {}", "⚠".yellow(), e),
        }

        Ok(())
    }

    fn format_usage_suffix(&mut self, provider_id: &crate::types::ProviderId) -> String {
        use crate::usage_tracking::LimitType;
        if let Some(usage) = self.usage_tracker.get_usage(provider_id) {
            match &usage.limit_type {
                LimitType::Unlimited => String::new(),
                LimitType::RequestBased { max_requests, current_requests, .. } => {
                    let remaining = max_requests.saturating_sub(*current_requests);
                    format!(" {} {} requests remaining", "·".dimmed(), remaining.to_string().bright_yellow())
                }
                LimitType::TokenBased { max_tokens, current_tokens, .. } => {
                    let remaining_pct = ((*max_tokens).saturating_sub(*current_tokens) as f64 / *max_tokens as f64) * 100.0;
                    format!(" {} {}% tokens remaining", "·".dimmed(), format!("{:.1}", remaining_pct).bright_yellow())
                }
                LimitType::CostBased { max_cost, current_cost, .. } => {
                    let remaining = max_cost - current_cost;
                    format!(" {} ${:.2} remaining", "·".dimmed(), remaining.to_string().bright_yellow())
                }
            }
        } else {
            String::new()
        }
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
    
    fn list_models(&mut self) {
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

        // Show usage summary for all tracked providers
        use crate::usage_tracking::LimitType;
        let mut printed_header = false;
        for provider_id in [
            crate::types::ProviderId::Tamu,
            crate::types::ProviderId::GitHubCopilot,
            crate::types::ProviderId::Outlier,
        ] {
            if let Some(usage) = self.usage_tracker.get_usage(&provider_id) {
                let line = match &usage.limit_type {
                    LimitType::Unlimited => continue,
                    LimitType::RequestBased { max_requests, current_requests, .. } => {
                        let remaining = max_requests.saturating_sub(*current_requests);
                        format!("{}: {} requests remaining", provider_id.to_string().bright_cyan(), remaining.to_string().bright_yellow())
                    }
                    LimitType::TokenBased { max_tokens, current_tokens, .. } => {
                        let remaining_pct = ((*max_tokens).saturating_sub(*current_tokens) as f64 / *max_tokens as f64) * 100.0;
                        format!("{}: {}% tokens remaining", provider_id.to_string().bright_cyan(), format!("{:.1}", remaining_pct).bright_yellow())
                    }
                    LimitType::CostBased { max_cost, current_cost, .. } => {
                        let remaining = max_cost - current_cost;
                        format!("{}: ${:.2} remaining", provider_id.to_string().bright_cyan(), remaining)
                    }
                };
                if !printed_header {
                    println!();
                    println!("{}", "Usage:".dimmed());
                    printed_header = true;
                }
                println!("  {}", line);
            }
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
        println!("  {} - Show full shell output for turn N", "/show N".bright_white());
        println!("  {} - Resume a saved session by number", "/load N".bright_white());
        println!("  {} - List saved sessions (> / < to page)", "/sessions".bright_white());
        println!("  {} - Start a new conversation", "/new".bright_white());
        println!("  {} - Clear conversation history", "/clear".bright_white());
        println!("  {} - Exit the REPL", "/exit or /quit".bright_white());
        println!();
        println!("{}", "Tip: start with --resume to pick a previous session".dimmed());
        println!("{}", "Just type a message to chat!".dimmed());
    }
}
