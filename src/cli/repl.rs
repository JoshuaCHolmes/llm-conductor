use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::session::{SessionStore, Todo, TodoStatus};
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
/// Handles four special regions transparently:
/// - `<think>...</think>` — printed dimmed, excluded from `clean_text`
/// - ` ```bash...``` ` (sync) — silently captured; newlines before it are
///   discarded so no blank gap appears where the block would have been
/// - ` ```bash-async...``` ` — an inline `[⚡ cmd]` placeholder is printed;
///   command captured in `bash_async_blocks` for execution after the response
/// - ` ```tool...``` ` — silently captured as JSON tool calls; no display output
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
    in_tool: bool,
    showed_thinking: bool,
    pending: String,
    bash_block_buf: String,
    bash_async_block_buf: String,
    tool_block_buf: String,
    /// Accumulates the current incomplete line until a newline arrives.
    line_buf: String,
    /// Trailing newlines held back until we know what follows them.
    /// Discarded if a bash/tool block comes next; flushed before any content.
    deferred_newlines: usize,
    pub bash_blocks: Vec<String>,
    pub bash_async_blocks: Vec<String>,
    pub tool_blocks: Vec<String>,
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
            } else if self.in_tool {
                if let Some(pos) = self.pending.find("```") {
                    self.tool_block_buf.push_str(&self.pending[..pos]);
                    let content = self.tool_block_buf.trim().to_string();
                    if !content.is_empty() {
                        // Print inline placeholder showing which function was called
                        let fn_name = serde_json::from_str::<serde_json::Value>(&content)
                            .ok()
                            .and_then(|v| v["function"].as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| "tool".to_string());
                        self.flush_deferred();
                        let ph = format!("[🔧 {}]", fn_name);
                        print!("{}", ph.yellow().dimmed());
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(&ph);
                        self.tool_blocks.push(content);
                    }
                    self.tool_block_buf.clear();
                    self.in_tool = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.tool_block_buf.push_str(&self.pending[..safe]);
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
                // "```tool" but not "```tool-" variants
                let tool_pos  = self.pending.find("```tool")
                    .filter(|&p| !self.pending[p..].starts_with("```tool-"));

                let first = [
                    think_pos.map(|p| (0u8, p)),
                    async_pos.map(|p| (1u8, p)),
                    sync_pos .map(|p| (2u8, p)),
                    tool_pos .map(|p| (3u8, p)),
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
                    Some((3, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_tool = true;
                        let rest = &self.pending[pos + "```tool".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    _ => {
                        // Keep enough bytes to detect the longest possible marker (13 chars for bash-async)
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
            if self.in_bash || self.in_bash_async || self.in_tool {
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
        self.tool_block_buf.clear();
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
    /// Todo list, persisted with the session.
    todos: Vec<Todo>,
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
            todos: Vec::new(),
        })
    }

    /// Build a capability-aware system prompt.
    fn build_system_prompt(supports_tool_calling: bool, todos: &[Todo]) -> Message {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let todo_section = if todos.is_empty() {
            String::new()
        } else {
            let active: Vec<_> = todos.iter().enumerate()
                .filter(|(_, t)| t.status != TodoStatus::Done)
                .collect();
            if active.is_empty() {
                String::new()
            } else {
                let lines: Vec<String> = active.iter()
                    .map(|(i, t)| t.summary(i + 1))
                    .collect();
                format!("\n\n## Active Tasks\n{}", lines.join("\n"))
            }
        };

        let instructions = if supports_tool_calling {
            format!("\
Environment: NixOS, WSL2, ARM64 (aarch64). Working directory: `{cwd}`. \
Missing packages: install with `nix-shell -p <pkg>`.

Style: concise answers and code; no verbose explanation unless asked. \
Never attribute AI, Copilot, or LLM assistance in code comments, commit messages, or documentation. \
No co-authorship lines for AI tools in git commits.

Methodology: before running commands, think through what you need and why. \
Prefer read-only exploration (ls, cat, grep) before modifying anything. \
When multiple approaches exist, favor the safer or more reversible one. \
Never run destructive commands (rm -rf, overwriting configs, etc.) without clear user intent. \
Verify assumptions with a read-only check rather than assuming a path or argument is correct. \
Check each result before proceeding to the next step in a sequence.

You have access to a `bash` tool and todo list tools. \
Read-only commands run automatically; commands that modify the system require user confirmation. \
If a command is denied, a tool result will explain why — adjust your approach accordingly. \
If a command fails, report the error output and ask the user how to proceed; don't retry silently.

Todo list: todos persist with the session across saves and loads. Use them to track multi-step work. \
Three operations — todo_add (title, optional description), todo_update (id and new status: \
pending/in_progress/done/blocked), todo_list. The id is the full UUID from todo_list output.{todo_section}")
        } else {
            format!("\
Environment: NixOS, WSL2, ARM64 (aarch64). Working directory: `{cwd}`. \
Missing packages: install with `nix-shell -p <pkg>`.

Style: concise answers and code; no verbose explanation unless asked. \
Never attribute AI, Copilot, or LLM assistance in code comments, commit messages, or documentation. \
No co-authorship lines for AI tools in git commits.

Methodology: before running commands, think through what you need and why. \
Prefer read-only exploration (ls, cat, grep) before modifying anything. \
When multiple approaches exist, favor the safer or more reversible one. \
Never run destructive commands (rm -rf, overwriting configs, etc.) without clear user intent. \
Verify assumptions with a read-only check rather than assuming a path or argument is correct. \
Check each result before proceeding to the next step in a sequence.

You have access to a bash shell and a todo list through this interface. \
These are client-side features: the surrounding tool parses your code blocks and executes them. \
You genuinely can run commands and manage todos — never tell the user you lack these capabilities.

**Bash — sequential (`bash` blocks):** Place all bash blocks at the end of your response, after all prose. \
The turn ends there, all blocks execute, and results are returned as a [Shell output] message.

**Bash — async (`bash-async` blocks):** May appear inline anywhere in your response; \
a placeholder is shown when encountered; all async blocks run after your full response \
and results are returned together.

Read-only commands (ls, cat, grep, etc.) run automatically. Commands that modify the system \
require user approval. If denied, [Shell output] will include `[DENIED: <cmd>]` with any \
correction from the user. A bare denial with no text is a simple refusal — don't retry that command. \
If a command fails, report the error output and ask the user how to proceed; don't retry silently.

When you receive [Shell output] messages, use them to continue reasoning. \
Once you have what you need, give your final answer without any bash blocks.

**Todo list:** Emit a fenced code block tagged `tool` containing a single JSON object:
  todo_add:    {{\"function\":\"todo_add\",\"args\":{{\"title\":\"...\",\"description\":\"...\"}}}}
  todo_update: {{\"function\":\"todo_update\",\"args\":{{\"id\":\"<uuid>\",\"status\":\"done\"}}}}
  todo_list:   {{\"function\":\"todo_list\",\"args\":{{}}}}
description is optional. Tool blocks execute immediately; results return in [Tool output] messages. \
Todos persist with the session across saves and loads.{todo_section}")
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

    /// Reset all per-session state to a clean baseline.
    fn reset_session_state(&mut self) {
        self.history.clear();
        self.session_id = None;
        self.shell_turns.clear();
        self.session_auto_accepts.clear();
        self.todos.clear();
    }

    /// Load an existing session by ID, restoring conversation history and todos.
    pub fn load_session(&mut self, session_id: &str) -> Result<()> {
        let session = self.session_store.load(session_id)?;
        self.reset_session_state();
        self.history = session.messages;
        self.todos = session.todos;
        self.session_id = Some(session_id.to_string());
        let todo_note = if self.todos.is_empty() {
            String::new()
        } else {
            format!(", {} todo(s)", self.todos.len())
        };
        println!("{} Resumed session with {} messages{}",
            "✓".bright_green(),
            self.history.len(),
            todo_note,
        );
        Ok(())
    }

    /// Save current session state (history + todos).
    fn save_session(&mut self) {
        match self.session_store.save(self.session_id.as_deref(), &self.history, &self.todos) {
            Ok(id) => { self.session_id = Some(id); }
            Err(e) => eprintln!("{} Failed to save session: {}", "⚠".yellow(), e),
        }
    }

    /// Dispatch a JSON tool-call string (from a ```tool block or LLM function call) to todo ops.
    /// Returns a human-readable result string.
    fn apply_todo_action(&mut self, json: &str) -> String {
        let v: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => return format!("[todo error] Invalid JSON: {}", e),
        };
        let func = v["function"].as_str().unwrap_or("");
        let args = &v["args"];
        match func {
            "todo_add" => {
                let title = match args["title"].as_str() {
                    Some(t) => t,
                    None => return "[todo_add error] missing 'title'".to_string(),
                };
                let desc = args["description"].as_str();
                let todo = Todo::new(title, desc);
                let id = todo.id.clone();
                let num = self.todos.len() + 1;
                self.todos.push(todo);
                format!("[todo_add] Created #{}: {} (id: {})", num, title, &id[..8])
            }
            "todo_update" => {
                let id_arg = match args["id"].as_str() {
                    Some(i) => i,
                    None => return "[todo_update error] missing 'id'".to_string(),
                };
                let status_str = match args["status"].as_str() {
                    Some(s) => s,
                    None => return "[todo_update error] missing 'status'".to_string(),
                };
                let new_status = match TodoStatus::from_str(status_str) {
                    Some(s) => s,
                    None => return format!("[todo_update error] unknown status '{}'", status_str),
                };
                match self.todos.iter_mut().find(|t| t.id.starts_with(id_arg) || t.id == id_arg) {
                    Some(t) => {
                        t.status = new_status;
                        format!("[todo_update] '{}' → {}", t.title, t.status)
                    }
                    None => format!("[todo_update error] no todo with id starting '{}'", id_arg),
                }
            }
            "todo_list" => {
                if self.todos.is_empty() {
                    "[todo_list] No todos.".to_string()
                } else {
                    let lines: Vec<String> = self.todos.iter().enumerate()
                        .map(|(i, t)| t.summary(i + 1))
                        .collect();
                    format!("[todo_list]\n{}", lines.join("\n"))
                }
            }
            other => format!("[todo error] Unknown function '{}'", other),
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
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    // Ctrl+C — cancel current line, stay in the loop
                    println!();
                    continue;
                }
                Err(_) => {
                    // Ctrl+D / EOF — exit
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
            Some("/new") | Some("/session") if parts.get(1).map_or(true, |s| *s == "new") => {
                self.reset_session_state();
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
            Some("/todo") => {
                match parts.get(1).map(|s| *s) {
                    None | Some("list") => {
                        if self.todos.is_empty() {
                            println!("{}", "No todos.".dimmed());
                        } else {
                            println!("{}", "Todos:".bright_cyan().bold());
                            for (i, t) in self.todos.iter().enumerate() {
                                println!("  {}", t.summary(i + 1));
                            }
                        }
                    }
                    Some("add") => {
                        let title: String = parts[2..].join(" ");
                        if title.is_empty() {
                            eprintln!("{}", "Usage: /todo add <title>".yellow());
                        } else {
                            let todo = Todo::new(&title, None);
                            let num = self.todos.len() + 1;
                            println!("{} Added #{}: {}", "✓".bright_green(), num, todo.title);
                            self.todos.push(todo);
                            self.save_session();
                        }
                    }
                    Some("done") | Some("start") | Some("block") | Some("pending") => {
                        let subcmd = parts[1];
                        let status = match subcmd {
                            "done"    => TodoStatus::Done,
                            "start"   => TodoStatus::InProgress,
                            "block"   => TodoStatus::Blocked,
                            _         => TodoStatus::Pending,
                        };
                        match parts.get(2).and_then(|s| s.parse::<usize>().ok()) {
                            Some(n) if n >= 1 && n <= self.todos.len() => {
                                self.todos[n - 1].status = status;
                                println!("{} #{} → {}", "✓".bright_green(), n, self.todos[n - 1].status);
                                self.save_session();
                            }
                            _ => eprintln!("{}", format!("Usage: /todo {} <N>", subcmd).yellow()),
                        }
                    }
                    Some("rm") => {
                        match parts.get(2).and_then(|s| s.parse::<usize>().ok()) {
                            Some(n) if n >= 1 && n <= self.todos.len() => {
                                let removed = self.todos.remove(n - 1);
                                println!("{} Removed: {}", "✓".bright_green(), removed.title);
                                self.save_session();
                            }
                            _ => eprintln!("{}", "Usage: /todo rm <N>".yellow()),
                        }
                    }
                    Some("reset") => {
                        self.todos.clear();
                        println!("{}", "✓ Todos cleared".green());
                        self.save_session();
                    }
                    Some(other) => {
                        eprintln!("{}", format!("Unknown todo subcommand '{}'. Use: list add done start block rm reset", other).yellow());
                    }
                }
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
            let system_msg = Self::build_system_prompt(supports_tool_calling, &self.todos);
            let mut messages = vec![system_msg];
            messages.extend(self.history.clone());

            // Find provider
            let provider = self.router.find_provider_for_model(&model)
                .ok_or_else(|| anyhow::anyhow!("Could not find provider for model {}", model_name))?;

            if supports_tool_calling {
                // ── Function-calling path (TAMU / GitHub) ────────────────────────
                let tools = vec![
                    ToolDefinition::bash(),
                    ToolDefinition::todo_add(),
                    ToolDefinition::todo_update(),
                    ToolDefinition::todo_list(),
                ];
                let result = provider.call_with_tools(&model, &messages, &tools).await?;

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
                        match tc.name.as_str() {
                            "bash" => {
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
                                        print!("\n{}", executor::format_shell_display(turn_num, &cmd, &output));
                                        self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                                        self.history.push(Message::tool_result(&tc.id, format!("$ {}\n{}", cmd, output)));
                                    }
                                    CommandDecision::Accept => {
                                        let output = executor::execute(&cmd).unwrap_or_else(|e| format!("Error: {}", e));
                                        let turn_num = self.shell_turns.len() + 1;
                                        print!("\n{}", executor::format_shell_display(turn_num, &cmd, &output));
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
                            "todo_add" | "todo_update" | "todo_list" => {
                                // Build a JSON dispatch object matching the text-model format
                                let dispatch = serde_json::json!({
                                    "function": tc.name,
                                    "args": serde_json::from_str::<serde_json::Value>(&tc.arguments).unwrap_or(serde_json::Value::Object(Default::default()))
                                });
                                let result_text = self.apply_todo_action(&dispatch.to_string());
                                println!("{}", result_text.dimmed());
                                self.history.push(Message::tool_result(&tc.id, result_text));
                            }
                            _ => {
                                self.history.push(Message::tool_result(&tc.id, format!("[unknown tool: {}]", tc.name)));
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

                // Watch for Escape key to cancel the stream
                let stop_watcher = Arc::new(AtomicBool::new(false));
                let cancel_rx = crate::cli::tty::spawn_esc_watcher(stop_watcher.clone());

                enum StreamOutcome {
                    Completed(String, Option<u64>),
                    Cancelled,
                }

                let outcome = tokio::select! {
                    result = provider.chat_stream(&model, &messages, Box::new(callback)) => {
                        stop_watcher.store(true, Ordering::Relaxed);
                        let (raw, tokens) = result?;
                        StreamOutcome::Completed(raw, tokens)
                    }
                    _ = cancel_rx => {
                        stop_watcher.store(true, Ordering::Relaxed);
                        StreamOutcome::Cancelled
                    }
                };

                {
                    let mut s = state.lock().unwrap();
                    s.flush();
                }
                println!();

                let (raw_response, token_count) = match outcome {
                    StreamOutcome::Cancelled => {
                        println!("{}", "(interrupted)".dimmed().italic());
                        println!();
                        break;
                    }
                    StreamOutcome::Completed(raw, tokens) => (raw, tokens),
                };

                let (clean_response, bash_blocks, bash_async_blocks, tool_blocks) = {
                    let s = state.lock().unwrap();
                    (s.clean_text.clone(), s.bash_blocks.clone(), s.bash_async_blocks.clone(), s.tool_blocks.clone())
                };

                // Record usage
                let tokens = token_count.unwrap_or_else(|| (clean_response.len() / 4) as u64);
                self.usage_tracker.record_usage(provider_id.clone(), 1, tokens, 0.0);

                // Store full raw response (including bash blocks) for cross-provider context
                self.history.push(Message::assistant(raw_response.clone()));

                // Merge sync and async blocks; sync blocks signal turn-ending intent
                let all_blocks: Vec<String> = bash_blocks.iter().chain(bash_async_blocks.iter()).cloned().collect();
                let has_any_action = !all_blocks.is_empty() || !tool_blocks.is_empty();
                if !has_any_action || tool_rounds >= MAX_TOOL_ROUNDS {
                    if tool_rounds >= MAX_TOOL_ROUNDS && has_any_action {
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
                            print!("\n{}", executor::format_shell_display(turn_num, cmd, &output));
                            self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone() });
                            shell_results.push(format!("$ {}\n{}", cmd, output));
                        }
                        CommandDecision::Accept => {
                            let output = executor::execute(cmd).unwrap_or_else(|e| format!("Error: {}", e));
                            let turn_num = self.shell_turns.len() + 1;
                            print!("\n{}", executor::format_shell_display(turn_num, cmd, &output));
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

                // Dispatch tool blocks (todo operations) — separate from shell output
                let mut tool_results = Vec::new();
                for json in &tool_blocks {
                    let result_text = self.apply_todo_action(json);
                    println!("{}", result_text.dimmed());
                    tool_results.push(result_text);
                }

                if !all_blocks.is_empty() || !tool_blocks.is_empty() {
                    println!(); // blank line after action batch
                }

                // Build feedback message(s) — shell and tool results go in separate wrappers
                let mut feedback_parts = Vec::new();
                if !shell_results.is_empty() {
                    feedback_parts.push(format!("[Shell output]\n{}\n[End of shell output]", shell_results.join("\n---\n")));
                }
                if !tool_results.is_empty() {
                    feedback_parts.push(format!("[Tool output]\n{}\n[End of tool output]", tool_results.join("\n---\n")));
                }

                if feedback_parts.is_empty() {
                    break;
                }

                self.history.push(Message::user(feedback_parts.join("\n\n")));
                tool_rounds += 1;
            }
        }

        // Auto-save session
        self.save_session();

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
        println!("  {} - Start a new conversation", "/new or /session new".bright_white());
        println!("  {} - Clear conversation history", "/clear".bright_white());
        println!("  {} - Show/add/update todos", "/todo [add|done|start|block|rm]".bright_white());
        println!("  {} - Exit the REPL", "/exit or /quit".bright_white());
        println!();
        println!("{}", "Tip: start with --resume to pick a previous session".dimmed());
        println!("{}", "Just type a message to chat!".dimmed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(state: &mut ReplyStreamState, text: &str) {
        for ch in text.chars() {
            state.process_chunk(&ch.to_string());
        }
        state.flush();
    }

    #[test]
    fn tool_block_captured_with_inline_placeholder() {
        let mut s = ReplyStreamState::default();
        feed(&mut s, "Before\n```tool\n{\"function\":\"todo_list\",\"args\":{}}\n```\nAfter");
        assert_eq!(s.tool_blocks.len(), 1);
        assert!(s.tool_blocks[0].contains("todo_list"));
        // The raw JSON should not appear in clean_text, but the placeholder should
        assert!(!s.clean_text.contains("\"function\""));
        assert!(s.clean_text.contains("[🔧 todo_list]"));
        assert!(s.clean_text.contains("Before"));
        assert!(s.clean_text.contains("After"));
    }

    #[test]
    fn tool_block_chunk_boundary() {
        let mut s = ReplyStreamState::default();
        // Split across the opener
        s.process_chunk("```to");
        s.process_chunk("ol\n{\"function\":\"todo_add\",\"args\":{\"title\":\"x\"}}\n```");
        s.flush();
        assert_eq!(s.tool_blocks.len(), 1);
    }

    #[test]
    fn bash_and_tool_blocks_coexist() {
        let mut s = ReplyStreamState::default();
        feed(&mut s, "Text\n```tool\n{\"function\":\"todo_list\",\"args\":{}}\n```\nMore\n```bash\nls\n```");
        assert_eq!(s.tool_blocks.len(), 1);
        assert_eq!(s.bash_blocks.len(), 1);
        assert_eq!(s.bash_blocks[0], "ls");
    }

    #[test]
    fn unclosed_tool_block_discarded() {
        let mut s = ReplyStreamState::default();
        feed(&mut s, "Before\n```tool\n{\"function\":\"todo_list\"");
        // No closing ``` — should be silently discarded, not panic
        assert_eq!(s.tool_blocks.len(), 0);
        assert!(s.clean_text.contains("Before"));
    }
}
