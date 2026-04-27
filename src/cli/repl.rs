use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::cli::session::SessionStore;
use crate::cli::executor::{self, ShellTurn};
use crate::providers::{ToolDefinition};
use crate::router::Router;
use crate::types::{Message, Task};
use crate::usage_tracking::UsageTracker;
use crate::model_filter::ModelFilter;

/// Find the byte offset of a sync ` ```bash ` block (not ` ```bash-async `).
fn find_sync_bash(s: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(rel) = s[start..].find("```bash") {
        let abs = start + rel;
        if !s[abs..].starts_with("```bash-async") {
            return Some(abs);
        }
        start = abs + 7;
    }
    None
}

/// Strip `**`, `*`, `` ` ``, `_` marker characters (used for heading text).
fn strip_md_markers(s: &str) -> String {
    s.chars().filter(|&c| c != '*' && c != '`' && c != '_').collect()
}

/// Render inline markdown spans in a string to ANSI terminal sequences.
/// Handles `**bold**`, `` `code` ``, and `*italic*` (asterisks stripped).
fn render_inline(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        // **bold**
        if s[i..].starts_with("**") {
            if let Some(end) = s.get(i + 2..).and_then(|t| t.find("**")) {
                out.push_str(&s[i + 2..i + 2 + end].bold().to_string());
                i += 4 + end;
                continue;
            }
        }
        // `code` (not triple-backtick)
        if s[i..].starts_with('`') && !s[i..].starts_with("```") {
            if let Some(end) = s.get(i + 1..).and_then(|t| t.find('`')) {
                out.push_str(&s[i + 1..i + 1 + end].cyan().to_string());
                i += 2 + end;
                continue;
            }
        }
        // *italic* — strip asterisks, keep text
        if s[i..].starts_with('*') && !s[i..].starts_with("**") {
            if let Some(end) = s.get(i + 1..).and_then(|t| t.find('*')) {
                let close = i + 1 + end;
                if !s[close..].starts_with("**") {
                    out.push_str(&s[i + 1..close]);
                    i = close + 1;
                    continue;
                }
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Render a complete buffered line with markdown formatting to a display string.
fn render_markdown_line(line: &str) -> String {
    // Headings: strip markers, apply bold+bright_white
    if let Some(rest) = line.strip_prefix("### ") {
        return strip_md_markers(rest).bold().bright_white().to_string();
    }
    if let Some(rest) = line.strip_prefix("## ") {
        return strip_md_markers(rest).bold().bright_white().to_string();
    }
    if let Some(rest) = line.strip_prefix("# ") {
        return strip_md_markers(rest).bold().bright_white().to_string();
    }
    // Horizontal rule
    let t = line.trim();
    if t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '=')) {
        return "──────────────────────────────".dimmed().to_string();
    }
    // List items (with optional indentation)
    let leading = line.len() - line.trim_start().len();
    let indent = &line[..leading];
    let rest = &line[leading..];
    if let Some(item) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return format!("{}• {}", indent, render_inline(item));
    }
    // Numbered list: "1. " etc.
    let digit_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if digit_end > 0 && rest[digit_end..].starts_with(". ") {
        let num = &rest[..digit_end];
        let item = &rest[digit_end + 2..];
        return format!("{}{}. {}", indent, num, render_inline(item));
    }
    // Normal prose — inline formatting only
    render_inline(line)
}

/// State machine for streaming model responses.
///
/// Handles three special regions transparently:
/// - `<think>...</think>` — printed dimmed, excluded from `clean_text`
/// - ` ```bash...``` ` (sync) — silently captured; newlines before it are
///   discarded so no blank gap appears where the block would have been
/// - ` ```bash-async...``` ` — an inline `[⚡ cmd]` placeholder is printed;
///   command captured in `bash_async_blocks` for execution after the response
///
/// Normal prose is rendered through `render_markdown_line` on a line-by-line
/// basis so markdown formatting (bold, code, headings, lists) is properly
/// displayed instead of showing raw markdown markers.
///
/// Trailing newlines at the end of the response are discarded to avoid a blank
/// line between the model's prose and the shell-output display.
#[derive(Default)]
struct ReplyStreamState {
    in_think: bool,
    in_bash: bool,
    in_bash_async: bool,
    showed_thinking: bool,
    pending: String,
    bash_block_buf: String,
    bash_async_block_buf: String,
    /// Accumulates the current incomplete line until a newline arrives.
    line_buf: String,
    /// Trailing newlines held back until we know what follows them.
    /// Discarded if a bash block comes next; flushed before any content.
    deferred_newlines: usize,
    pub bash_blocks: Vec<String>,
    pub bash_async_blocks: Vec<String>,
    pub clean_text: String,
}

impl ReplyStreamState {
    fn char_safe_len(s: &str, raw_len: usize) -> usize {
        (0..=raw_len).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
    }

    /// Print any deferred blank lines before new content.
    fn flush_deferred(&mut self) {
        for _ in 0..self.deferred_newlines {
            print!("\n");
            self.clean_text.push('\n');
        }
        self.deferred_newlines = 0;
        std::io::stdout().flush().unwrap();
    }

    /// Render and print the current `line_buf` content (does not touch deferred_newlines).
    fn flush_line_buf(&mut self) {
        if !self.line_buf.is_empty() {
            let rendered = render_markdown_line(&self.line_buf);
            print!("{}", rendered);
            self.clean_text.push_str(&self.line_buf);
            self.line_buf.clear();
            std::io::stdout().flush().unwrap();
        }
    }

    /// Buffer prose text into lines; flush complete lines through the markdown renderer.
    /// Trailing newlines are deferred rather than printed immediately.
    fn print_normal(&mut self, text: &str) {
        let mut remaining = text;
        loop {
            match remaining.find('\n') {
                Some(nl_pos) => {
                    let segment = &remaining[..nl_pos];
                    if !segment.is_empty() || !self.line_buf.is_empty() {
                        self.line_buf.push_str(segment);
                        self.flush_deferred();
                        self.flush_line_buf();
                    }
                    self.deferred_newlines += 1;
                    remaining = &remaining[nl_pos + 1..];
                }
                None => {
                    if !remaining.is_empty() {
                        self.flush_deferred();
                        self.line_buf.push_str(remaining);
                    }
                    break;
                }
            }
        }
    }

    /// Flush text that precedes a bash block: trim trailing whitespace, print via
    /// normal renderer, then discard any deferred newlines so no blank gap appears.
    fn flush_before_bash(&mut self, before: &str) {
        let trimmed = before.trim_end_matches(|c: char| c == '\n' || c == ' ');
        if !trimmed.is_empty() {
            self.print_normal(trimmed);
        }
        self.flush_line_buf(); // flush partial line if any
        self.deferred_newlines = 0;
    }

    pub fn process_chunk(&mut self, chunk: &str) {
        self.pending.push_str(chunk);
        loop {
            if self.in_bash {
                if let Some(pos) = self.pending.find("```") {
                    self.bash_block_buf.push_str(&self.pending[..pos]);
                    let cmd = self.bash_block_buf.trim().to_string();
                    if !cmd.is_empty() {
                        self.bash_blocks.push(cmd);
                    }
                    self.bash_block_buf.clear();
                    self.in_bash = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.bash_block_buf.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                    }
                    break;
                }
            } else if self.in_bash_async {
                if let Some(pos) = self.pending.find("```") {
                    self.bash_async_block_buf.push_str(&self.pending[..pos]);
                    let cmd = self.bash_async_block_buf.trim().to_string();
                    if !cmd.is_empty() {
                        let preview: String = cmd.lines().next().unwrap_or("").chars().take(40).collect();
                        self.flush_deferred();
                        let ph = format!("[⚡ {}]", preview);
                        print!("{}", ph.yellow().dimmed());
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(&ph);
                        self.bash_async_blocks.push(cmd);
                    }
                    self.bash_async_block_buf.clear();
                    self.in_bash_async = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.bash_async_block_buf.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                    }
                    break;
                }
            } else if self.in_think {
                if let Some(pos) = self.pending.find("</think>") {
                    let before = &self.pending[..pos];
                    if !before.is_empty() {
                        print!("{}", before.dimmed());
                        std::io::stdout().flush().unwrap();
                    }
                    self.in_think = false;
                    self.pending = self.pending[pos + "</think>".len()..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(8));
                    if safe > 0 {
                        print!("{}", self.pending[..safe].dimmed());
                        std::io::stdout().flush().unwrap();
                        self.pending = self.pending[safe..].to_string();
                    }
                    break;
                }
            } else {
                let think_pos = self.pending.find("<think>");
                let async_pos = self.pending.find("```bash-async");
                let sync_pos  = find_sync_bash(&self.pending);

                let first = [
                    think_pos.map(|p| (0u8, p)),
                    async_pos.map(|p| (1u8, p)),
                    sync_pos .map(|p| (2u8, p)),
                ].iter().filter_map(|x| *x).min_by_key(|&(_, p)| p);

                match first {
                    Some((0, pos)) => {
                        let before = self.pending[..pos].to_string();
                        if !before.is_empty() {
                            self.print_normal(&before);
                        }
                        self.flush_line_buf();
                        self.flush_deferred();
                        if !self.showed_thinking {
                            println!("{}", "💭 Thinking...".dimmed().italic());
                            self.showed_thinking = true;
                        }
                        self.in_think = true;
                        self.pending = self.pending[pos + "<think>".len()..].to_string();
                    }
                    Some((1, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_bash_async = true;
                        let rest = &self.pending[pos + "```bash-async".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    Some((2, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_bash = true;
                        let rest = &self.pending[pos + "```bash".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    _ => {
                        let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(13));
                        if safe > 0 {
                            let to_print = self.pending[..safe].to_string();
                            self.print_normal(&to_print);
                            self.pending = self.pending[safe..].to_string();
                        }
                        break;
                    }
                }
            }
        }
    }

    pub fn flush(&mut self) {
        // Flush any remaining line content first
        if !self.line_buf.is_empty() {
            let rendered = render_markdown_line(&self.line_buf);
            print!("{}", rendered);
            self.clean_text.push_str(&self.line_buf);
            self.line_buf.clear();
            std::io::stdout().flush().unwrap();
        }
        // Discard trailing deferred newlines (end-of-response whitespace)
        self.deferred_newlines = 0;
        if !self.pending.is_empty() {
            if self.in_bash || self.in_bash_async {
                // Truncated/unclosed block — discard silently
            } else if self.in_think {
                print!("{}", self.pending.dimmed());
                std::io::stdout().flush().unwrap();
            } else {
                let trimmed = self.pending.trim_end_matches('\n');
                if !trimmed.is_empty() {
                    let rendered = render_markdown_line(trimmed);
                    print!("{}", rendered);
                    self.clean_text.push_str(trimmed);
                    std::io::stdout().flush().unwrap();
                }
            }
            self.pending.clear();
        }
        self.bash_block_buf.clear();
        self.bash_async_block_buf.clear();
    }
}

/// Decision returned when prompting the user about a destructive command.
#[derive(Debug)]
enum CommandDecision {
    Accept,
    /// Accept and remember this exact command for the rest of the session.
    AcceptForSession,
    /// Deny — the string is the user's correction text (may be empty).
    Deny(String),
}

pub struct Repl {
    router: Router,
    history: Vec<Message>,
    usage_tracker: UsageTracker,
    model_filter: ModelFilter,
    session_store: SessionStore,
    session_id: Option<String>,
    sessions_page: usize,
    shell_turns: Vec<ShellTurn>,
    /// Commands that have been accepted for the full session (exact match).
    session_auto_accepts: HashSet<String>,
}

impl Repl {
    pub fn new(router: Router, config_dir: PathBuf) -> Result<Self> {
        let usage_tracker = UsageTracker::new(&config_dir)?;
        let session_store = SessionStore::new(&config_dir)?;
        
        Ok(Self {
            router,
            history: Vec::new(),
            usage_tracker,
            model_filter: ModelFilter::new(),
            session_store,
            session_id: None,
            sessions_page: 0,
            shell_turns: Vec::new(),
            session_auto_accepts: HashSet::new(),
        })
    }

    /// Build a capability-aware system prompt.
    fn build_system_prompt(supports_tool_calling: bool) -> Message {
        let base = "You are a helpful AI assistant.";
        let instructions = if supports_tool_calling {
            format!("{}\n\n\
You have access to a `bash` tool. Use it freely to run shell commands. \
Read-only commands (ls, cat, grep, etc.) run automatically. Commands that \
modify the system require the user's confirmation — if a command is denied, \
you will receive a tool result explaining why, so adjust your approach accordingly.",
                base)
        } else {
            format!("{}\n\n\
You have access to a bash shell through this interface. This is a client-side \
feature: the surrounding tool parses your code blocks and executes them — you \
genuinely can run commands. Never tell the user you lack shell access.\n\n\
Two block modes are available:\n\n\
**Sequential (```bash):** All bash blocks must be at the VERY END of your response, \
after all prose. Do not write anything after them. The turn ends there, all blocks \
are executed, and results are returned so you can continue.\n\n\
**Inline (```bash-async):** May appear anywhere in your response. A placeholder is \
shown inline; all async commands run after your full response and results are \
returned together.\n\n\
Read-only commands (ls, cat, grep, etc.) run automatically. Commands that modify \
the system require the user's approval. If a command is denied, the `[Shell output]` \
message will say `[DENIED: <cmd>]` along with the user's correction — adjust your \
approach based on that feedback. A bare refusal with no text means the user simply \
does not want that command run.\n\n\
When you receive `[Shell output]` messages, use them to continue. Once you have \
what you need, give your final answer with no bash blocks.",
                base)
        };
        Message::system(instructions)
    }

    /// Prompt the user about a command that requires confirmation.
    /// Returns immediately (Accept) if the command was session-accepted previously.
    fn prompt_command_decision(cmd: &str, auto_accepts: &HashSet<String>) -> Result<CommandDecision> {
        if auto_accepts.contains(cmd) {
            return Ok(CommandDecision::Accept);
        }
        println!("{} {} {}", "⚡".yellow(), "Run:".bright_white(), cmd.bright_yellow());
        print!("{}", "  [y] accept · [Y] session accept · [text] correct: ".dimmed());
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut ans)?;
        let ans = ans.trim().to_string();
        Ok(match ans.as_str() {
            "y" => CommandDecision::Accept,
            "Y" => CommandDecision::AcceptForSession,
            _ => CommandDecision::Deny(ans),
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

        const MAX_TOOL_ROUNDS: usize = 30; // safety limit; user confirmation is the primary gate
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
            let supports_tool_calling = model.supports_tool_calling;

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

            // Build messages: dynamic system prompt + history
            let system_msg = Self::build_system_prompt(supports_tool_calling);
            let mut messages = vec![system_msg];
            messages.extend(self.history.clone());

            // Find provider
            let provider = self.router.find_provider_for_model(&model)
                .ok_or_else(|| anyhow::anyhow!("Could not find provider for model {}", model_name))?;

            if supports_tool_calling {
                // ── Function-calling path (TAMU / GitHub) ────────────────────────
                let result = provider.call_with_tools(&model, &messages, &[ToolDefinition::bash()]).await?;

                // Display any text content the model returned alongside the tool call
                if let Some(ref text) = result.text {
                    if !text.is_empty() {
                        print!("{} ", "❯".bright_green().bold());
                        println!("{}", text);
                        println!();
                    }
                }

                let tokens = result.tokens.unwrap_or_else(|| {
                    result.text.as_deref().map(|t| (t.len() / 4) as u64).unwrap_or(1)
                });
                self.usage_tracker.record_usage(provider_id.clone(), 1, tokens, 0.0);

                if let Some(tool_calls) = result.tool_calls {
                    if tool_rounds >= MAX_TOOL_ROUNDS {
                        eprintln!("{} Tool round limit reached", "⚠".yellow());
                        break;
                    }

                    // Add assistant tool-call message to history
                    self.history.push(Message::assistant_tool_calls(
                        result.text.clone().unwrap_or_default(),
                        tool_calls.clone(),
                    ));

                    for tc in &tool_calls {
                        if tc.name != "bash" {
                            continue;
                        }
                        let cmd = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                            .ok()
                            .and_then(|v| v["command"].as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| tc.arguments.clone());

                        let kind = executor::classify(&cmd);
                        let decision = if kind == executor::CommandKind::ReadOnly {
                            CommandDecision::Accept
                        } else {
                            Self::prompt_command_decision(&cmd, &self.session_auto_accepts)?
                        };

                        match decision {
                            CommandDecision::AcceptForSession => {
                                self.session_auto_accepts.insert(cmd.clone());
                                let output = executor::execute(&cmd).unwrap_or_else(|e| format!("Error: {}", e));
                                let turn_num = self.shell_turns.len() + 1;
                                print!("{}", executor::format_shell_display(turn_num, &cmd, &output));
                                self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                                self.history.push(Message::tool_result(&tc.id, format!("$ {}\n{}", cmd, output)));
                            }
                            CommandDecision::Accept => {
                                let output = executor::execute(&cmd).unwrap_or_else(|e| format!("Error: {}", e));
                                let turn_num = self.shell_turns.len() + 1;
                                print!("{}", executor::format_shell_display(turn_num, &cmd, &output));
                                self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                                self.history.push(Message::tool_result(&tc.id, format!("$ {}\n{}", cmd, output)));
                            }
                            CommandDecision::Deny(reason) => {
                                let denial = if reason.is_empty() {
                                    format!("[Command denied: {}] (no reason given)", cmd)
                                } else {
                                    format!("[Command denied: {}]\nUser correction: {}", cmd, reason)
                                };
                                println!("{}", "  (denied)".dimmed());
                                self.history.push(Message::tool_result(&tc.id, denial));
                            }
                        }
                    }
                    println!(); // blank line after shell output batch
                    tool_rounds += 1;
                    continue;
                } else {
                    // Text response, no tool calls — already displayed above if non-empty
                    let text = result.text.unwrap_or_default();
                    self.history.push(Message::assistant(&text));
                    break;
                }
            } else {
                // ── Streaming code-block path (Outlier / Ollama / TAMU) ──────────────
                print!("{} ", "❯".bright_green().bold());

                let state = Arc::new(Mutex::new(ReplyStreamState::default()));
                let state_cb = state.clone();
                let callback = move |chunk: String| {
                    state_cb.lock().unwrap().process_chunk(&chunk);
                };

                let (raw_response, token_count) = provider.chat_stream(&model, &messages, Box::new(callback)).await?;
                {
                    let mut s = state.lock().unwrap();
                    s.flush();
                }
                println!();

                let (clean_response, bash_blocks, bash_async_blocks) = {
                    let s = state.lock().unwrap();
                    (s.clean_text.clone(), s.bash_blocks.clone(), s.bash_async_blocks.clone())
                };

                // Record usage
                let tokens = token_count.unwrap_or_else(|| (clean_response.len() / 4) as u64);
                self.usage_tracker.record_usage(provider_id.clone(), 1, tokens, 0.0);

                // Store full raw response (including bash blocks) for cross-provider context
                self.history.push(Message::assistant(raw_response.clone()));

                // Merge sync and async blocks; sync blocks signal turn-ending intent
                let all_blocks: Vec<String> = bash_blocks.iter().chain(bash_async_blocks.iter()).cloned().collect();
                if all_blocks.is_empty() || tool_rounds >= MAX_TOOL_ROUNDS {
                    if tool_rounds >= MAX_TOOL_ROUNDS && !all_blocks.is_empty() {
                        eprintln!("{} Shell round limit reached", "⚠".yellow());
                    }
                    break;
                }

                // Execute each bash block and collect results
                let mut shell_results = Vec::new();
                for cmd in &all_blocks {
                    let kind = executor::classify(cmd);
                    let decision = if kind == executor::CommandKind::ReadOnly {
                        CommandDecision::Accept
                    } else {
                        Self::prompt_command_decision(cmd, &self.session_auto_accepts)?
                    };

                    match decision {
                        CommandDecision::AcceptForSession => {
                            self.session_auto_accepts.insert(cmd.clone());
                            let output = executor::execute(cmd).unwrap_or_else(|e| format!("Error: {}", e));
                            let turn_num = self.shell_turns.len() + 1;
                            print!("{}", executor::format_shell_display(turn_num, cmd, &output));
                            self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                            shell_results.push(format!("$ {}\n{}", cmd, output));
                        }
                        CommandDecision::Accept => {
                            let output = executor::execute(cmd).unwrap_or_else(|e| format!("Error: {}", e));
                            let turn_num = self.shell_turns.len() + 1;
                            print!("{}", executor::format_shell_display(turn_num, cmd, &output));
                            self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                            shell_results.push(format!("$ {}\n{}", cmd, output));
                        }
                        CommandDecision::Deny(reason) => {
                            let entry = if reason.is_empty() {
                                format!("[DENIED: {}] (no reason given)", cmd)
                            } else {
                                format!("[DENIED: {}]\nUser correction: {}", cmd, reason)
                            };
                            println!("{}", "  (denied)".dimmed());
                            shell_results.push(entry);
                        }
                    }
                }
                println!(); // blank line after shell output batch

                if shell_results.is_empty() {
                    break;
                }

                let feedback = format!("[Shell output]\n{}\n[End of shell output]", shell_results.join("\n---\n"));
                self.history.push(Message::user(feedback));
                tool_rounds += 1;
            }
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
