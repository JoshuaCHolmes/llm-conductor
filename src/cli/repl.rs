use anyhow::Result;
use colored::*;
use rustyline::DefaultEditor;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::session::{SessionStore, Todo, TodoStatus};
use crate::cli::executor::{self, Shell, ShellTurn};
use crate::providers::{ToolDefinition};
use crate::router::Router;
use crate::types::{Message, Task};
use crate::types::message::Role;
use crate::usage_tracking::UsageTracker;
use crate::model_filter::ModelFilter;

/// Find the byte offset of a plain ` ```bash ` block — not bash-long or bash-sub.
fn find_sync_bash(s: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(rel) = s[start..].find("```bash") {
        let abs = start + rel;
        let tail = &s[abs..];
        if !tail.starts_with("```bash-long") && !tail.starts_with("```bash-sub") {
            return Some(abs);
        }
        start = abs + 7;
    }
    None
}

/// Ordered action extracted from a streaming model response.
#[derive(Debug, Clone)]
pub enum Action {
    /// Sequential command in the persistent shell; timeout auto-detected.
    Bash(String),
    /// Sequential command, extended (300 s) timeout for builds/installs.
    BashLong(String),
    /// Stateless command run in a fresh subshell, independent of session.
    BashSub(String),
    /// JSON tool invocation (todo operations).
    Tool(String),
    /// Adversarial think request — spawns a critic model call.
    Think(String),
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

/// Strip raw action fence blocks from a stored assistant message for display.
/// Removes ```bash, ```bash-long, ```bash-sub, ```tool, ```rubberduck blocks entirely.
fn strip_action_fences(text: &str) -> String {
    let fences = ["```bash-long", "```bash-sub", "```bash", "```tool", "```rubberduck"];
    let mut out = String::with_capacity(text.len());
    let mut chars = text;
    'outer: while !chars.is_empty() {
        for fence in &fences {
            if chars.starts_with(fence) {
                // Skip to closing ```
                if let Some(end) = chars[fence.len()..].find("\n```") {
                    chars = &chars[fence.len() + end + 4..]; // skip past closing ```
                    // eat leading newline if present
                    chars = chars.strip_prefix('\n').unwrap_or(chars);
                } else {
                    // No closing fence — drop the rest
                    chars = "";
                }
                continue 'outer;
            }
        }
        // Emit up to next newline (or end)
        if let Some(nl) = chars.find('\n') {
            out.push_str(&chars[..nl + 1]);
            chars = &chars[nl + 1..];
        } else {
            out.push_str(chars);
            break;
        }
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
    // Blockquote — render dimmed with a bar prefix
    if let Some(rest) = line.strip_prefix("> ") {
        return format!("▏ {}", render_inline(rest).dimmed());
    }
    if line == ">" {
        return "▏".dimmed().to_string();
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
/// Handles six special regions transparently:
/// - `<think>...</think>` — printed dimmed, excluded from `clean_text`
/// - ` ```bash...``` ` (sync) — silently captured; blank gap suppressed
/// - ` ```bash-long...``` ` — same as bash but uses extended timeout on run
/// - ` ```bash-sub...``` ` — runs in stateless subshell (not persistent)
/// - ` ```tool...``` ` — silently captured as JSON tool calls
/// - ` ```rubberduck...``` ` — silently captured as adversarial review requests
///
/// Normal prose is rendered through `render_markdown_line` on a line-by-line
/// basis. Trailing newlines are discarded to avoid blank lines after output.
#[derive(Default)]
struct ReplyStreamState {
    in_think: bool,
    in_bash: bool,
    in_bash_long: bool,
    in_bash_sub: bool,
    in_tool: bool,
    in_rubberduck: bool,
    showed_thinking: bool,
    pending: String,
    bash_block_buf: String,
    bash_long_block_buf: String,
    bash_sub_block_buf: String,
    tool_block_buf: String,
    rubberduck_buf: String,
    /// Accumulates the current incomplete line until a newline arrives.
    line_buf: String,
    /// Trailing newlines held back until we know what follows them.
    /// Discarded if a bash/tool block comes next; flushed before any content.
    deferred_newlines: usize,
    /// Suppresses leading whitespace/newlines before the first real content.
    has_started: bool,
    /// Ordered list of actions to execute after the response completes.
    pub actions: Vec<Action>,
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

        // Suppress leading whitespace/newlines before the first real content
        if !self.has_started {
            let trimmed = self.pending.trim_start_matches(|c: char| c == '\n' || c == '\r' || c == ' ');
            if !trimmed.is_empty() {
                self.has_started = true;
                self.pending = trimmed.to_string();
            } else {
                self.pending.clear();
                return;
            }
        }

        loop {
            if self.in_bash {
                if let Some(pos) = self.pending.find("```") {
                    self.bash_block_buf.push_str(&self.pending[..pos]);
                    let cmd = self.bash_block_buf.trim().to_string();
                    if !cmd.is_empty() {
                        self.actions.push(Action::Bash(cmd));
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
            } else if self.in_bash_long {
                if let Some(pos) = self.pending.find("```") {
                    self.bash_long_block_buf.push_str(&self.pending[..pos]);
                    let cmd = self.bash_long_block_buf.trim().to_string();
                    if !cmd.is_empty() {
                        self.actions.push(Action::BashLong(cmd));
                    }
                    self.bash_long_block_buf.clear();
                    self.in_bash_long = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.bash_long_block_buf.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                    }
                    break;
                }
            } else if self.in_bash_sub {
                if let Some(pos) = self.pending.find("```") {
                    self.bash_sub_block_buf.push_str(&self.pending[..pos]);
                    let cmd = self.bash_sub_block_buf.trim().to_string();
                    if !cmd.is_empty() {
                        let preview: String = cmd.lines().next().unwrap_or("").chars().take(40).collect();
                        self.flush_deferred();
                        let ph = format!("[⚡ {}]", preview);
                        print!("{}", ph.yellow().dimmed());
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(&ph);
                        self.actions.push(Action::BashSub(cmd));
                    }
                    self.bash_sub_block_buf.clear();
                    self.in_bash_sub = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.bash_sub_block_buf.push_str(&self.pending[..safe]);
                        self.pending = self.pending[safe..].to_string();
                    }
                    break;
                }
            } else if self.in_tool {
                if let Some(pos) = self.pending.find("```") {
                    self.tool_block_buf.push_str(&self.pending[..pos]);
                    let content = self.tool_block_buf.trim().to_string();
                    if !content.is_empty() {
                        let fn_name = serde_json::from_str::<serde_json::Value>(&content)
                            .ok()
                            .and_then(|v| v["function"].as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| "tool".to_string());
                        self.flush_deferred();
                        let ph = format!("[🔧 {}]", fn_name);
                        print!("{}", ph.yellow().dimmed());
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(&ph);
                        self.actions.push(Action::Tool(content));
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
            } else if self.in_rubberduck {
                if let Some(pos) = self.pending.find("```") {
                    self.rubberduck_buf.push_str(&self.pending[..pos]);
                    let query = self.rubberduck_buf.trim().to_string();
                    if !query.is_empty() {
                        self.flush_deferred();
                        let ph = "[🦆 rubberduck...]";
                        print!("{}", ph.cyan().dimmed());
                        std::io::stdout().flush().unwrap();
                        self.clean_text.push_str(ph);
                        self.actions.push(Action::Think(query));
                    }
                    self.rubberduck_buf.clear();
                    self.in_rubberduck = false;
                    self.pending = self.pending[pos + 3..].to_string();
                } else {
                    let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(3));
                    if safe > 0 {
                        self.rubberduck_buf.push_str(&self.pending[..safe]);
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
                let think_pos    = self.pending.find("<think>");
                let sub_pos      = self.pending.find("```bash-sub");
                let long_pos     = self.pending.find("```bash-long");
                let sync_pos     = find_sync_bash(&self.pending);
                let tool_pos     = self.pending.find("```tool")
                    .filter(|&p| !self.pending[p..].starts_with("```tool-"));
                let rubberduck_pos = self.pending.find("```rubberduck")
                    .filter(|&p| !self.pending[p..].starts_with("```rubberduck-"));

                let first = [
                    think_pos       .map(|p| (0u8, p)),
                    sub_pos         .map(|p| (1u8, p)),
                    long_pos        .map(|p| (2u8, p)),
                    sync_pos        .map(|p| (3u8, p)),
                    tool_pos        .map(|p| (4u8, p)),
                    rubberduck_pos .map(|p| (5u8, p)),
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
                        self.in_bash_sub = true;
                        let rest = &self.pending[pos + "```bash-sub".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    Some((2, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_bash_long = true;
                        let rest = &self.pending[pos + "```bash-long".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    Some((3, pos)) => {
                        let after_start = pos + "```bash".len();
                        let remaining = self.pending[after_start..].to_string();
                        // Guard: need enough lookahead to rule out -long/-sub suffix.
                        // If remaining is empty or starts with '-' with < 5 chars, wait.
                        if remaining.is_empty() || (remaining.starts_with('-') && remaining.len() < "-sub".len()) {
                            let safe = Self::char_safe_len(&self.pending, self.pending.len().saturating_sub(13));
                            if safe > 0 {
                                let to_print = self.pending[..safe].to_string();
                                self.print_normal(&to_print);
                                self.pending = self.pending[safe..].to_string();
                            }
                            break;
                        }
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_bash = true;
                        self.pending = if remaining.starts_with('\n') { remaining[1..].to_string() } else { remaining };
                    }
                    Some((4, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_tool = true;
                        let rest = &self.pending[pos + "```tool".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    Some((5, pos)) => {
                        let before = self.pending[..pos].to_string();
                        self.flush_before_bash(&before);
                        self.in_rubberduck = true;
                        let rest = &self.pending[pos + "```rubberduck".len()..];
                        self.pending = rest.strip_prefix('\n').unwrap_or(rest).to_string();
                    }
                    _ => {
                        // Keep enough bytes to detect the longest marker ("```rubberduck" = 13)
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
        if !self.line_buf.is_empty() {
            let rendered = render_markdown_line(&self.line_buf);
            print!("{}", rendered);
            self.clean_text.push_str(&self.line_buf);
            self.line_buf.clear();
            std::io::stdout().flush().unwrap();
        }
        self.deferred_newlines = 0;
        if !self.pending.is_empty() {
            let in_any_block = self.in_bash || self.in_bash_long || self.in_bash_sub
                || self.in_tool || self.in_rubberduck;
            if in_any_block {
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
        self.bash_long_block_buf.clear();
        self.bash_sub_block_buf.clear();
        self.tool_block_buf.clear();
        self.rubberduck_buf.clear();
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

/// Estimated token count for a history slice (rough: 4 chars per token).
fn estimated_tokens(history: &[Message]) -> usize {
    history.iter().map(|m| m.content.len() / 4 + 10).sum()
}

/// Find a clean compaction split point: the first Role::User at or after
/// `history.len() - desired_keep`. This ensures we never split a
/// tool-call / tool-result group.
fn find_compact_boundary(history: &[Message], desired_keep: usize) -> usize {
    if history.len() <= desired_keep {
        return 0;
    }
    let split = history.len() - desired_keep;
    for i in split..history.len() {
        if matches!(history[i].role, crate::types::Role::User) {
            return i;
        }
    }
    split
}

/// UTF-8-safe head+tail truncation for compaction serialization.
fn summarize_content(s: &str) -> String {
    const MAX: usize = 1500;
    const TAIL: usize = 300;
    let n = s.chars().count();
    if n <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX - TAIL - 3).collect();
    let tail: String = s.chars().skip(n - TAIL).collect();
    format!("{}…[…]…{}", head, tail)
}

/// Serialize a history slice to plain text for summarization.
/// Handles tool calls and tool results explicitly.
fn serialize_for_compaction(messages: &[Message]) -> String {
    use crate::types::Role;
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => continue,
            Role::User => {
                let content = summarize_content(&msg.content);
                if !content.is_empty() {
                    parts.push(format!("[User]: {}", content));
                }
            }
            Role::Assistant => {
                let mut label = "[Assistant]".to_string();
                if let Some(ref calls) = msg.tool_calls {
                    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
                    if !names.is_empty() {
                        label = format!("[Assistant — called: {}]", names.join(", "));
                    }
                }
                let content = summarize_content(&msg.content);
                if !content.is_empty() {
                    parts.push(format!("{}: {}", label, content));
                } else {
                    parts.push(label);
                }
            }
            Role::Tool => {
                let content = summarize_content(&msg.content);
                parts.push(format!("[Tool result]: {}", content));
            }
        }
    }
    parts.join("\n\n")
}

/// Display a single message for session resume history, styled like live output.
fn print_message_replay(msg: &Message) {
    match msg.role {
        Role::User => {
            if msg.source.as_deref().map(|s| s.starts_with("conductor/")).unwrap_or(false) {
                return;
            }
            print!("{} ", "❯".bright_blue().bold());
            println!("{}", msg.content.trim());
            println!();
        }
        Role::Assistant => {
            print!("{} ", "❯".bright_green().bold());
            let content = strip_action_fences(&msg.content);
            let mut lines = content.trim().lines();
            if let Some(first_line) = lines.next() {
                println!("{}", render_markdown_line(first_line));
            }
            for line in lines {
                println!("{}", render_markdown_line(line));
            }
            println!();
        }
        _ => {}
    }
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
    shell: Shell,
    /// Commands that have been accepted for the full session (exact match).
    session_auto_accepts: HashSet<String>,
    /// Todo list, persisted with the session.
    todos: Vec<Todo>,
    /// Summary of compacted conversation history, injected into system prompt.
    compacted_summary: Option<String>,
    /// Prevents recursive think invocations.
    is_thinking: bool,
}

impl Repl {
    pub async fn new(router: Router, config_dir: PathBuf) -> Result<Self> {
        let usage_tracker = UsageTracker::new(&config_dir)?;
        let session_store = SessionStore::new(&config_dir)?;
        let shell = Shell::new().await?;
        
        Ok(Self {
            router,
            history: Vec::new(),
            usage_tracker,
            model_filter: ModelFilter::new(),
            session_store,
            session_id: None,
            sessions_page: 0,
            shell_turns: Vec::new(),
            shell,
            session_auto_accepts: HashSet::new(),
            todos: Vec::new(),
            compacted_summary: None,
            is_thinking: false,
        })
    }

    /// Build a capability-aware system prompt.
    fn build_system_prompt(supports_tool_calling: bool, todos: &[Todo], compacted_summary: Option<&str>, cwd: &str) -> Message {
        let summary_section = match compacted_summary {
            Some(s) if !s.is_empty() => format!("\n\n## Earlier Conversation Summary\n{}", s),
            _ => String::new(),
        };

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
Check each result before proceeding to the next step in a sequence. \
For multi-step plans, destructive actions, or when uncertain: use the `rubberduck` tool to get an \
adversarial critique of your plan before acting. This is the same rubber-duck process used in \
advanced coding assistants — catching blind spots early prevents wasted effort.

You have access to a `bash` tool, a `rubberduck` tool, and todo list tools. \
Read-only commands run automatically; commands that modify the system require user confirmation. \
If a command is denied, a tool result will explain why — adjust your approach accordingly. \
If a command fails, report the error output and ask the user how to proceed; don't retry silently.

**Conversation turns:** Messages from the user come as plain user messages. \
Automated feedback from the conductor (shell results, rubberduck critiques, tool output) \
is prefixed with `[conductor]:` — system-generated, not typed by the user.

Rubberduck tool: pass a description of your plan or decision as `query`. You will receive an adversarial \
critique pointing out risks and gaps. Use this before complex or risky actions.

Todo list: todos persist with the session across saves and loads. Use them to track multi-step work. \
Three operations — todo_add (title, optional description), todo_update (id and new status: \
pending/in_progress/done/blocked), todo_list. The id is the full UUID from todo_list output.{summary_section}{todo_section}")
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
Check each result before proceeding to the next step in a sequence. \
For multi-step plans, destructive actions, or when uncertain: use a `rubberduck` block to get an \
adversarial critique before acting — catching blind spots early prevents wasted effort.

You have access to a bash shell, a rubberduck block, and a todo list through this interface. \
These are client-side features: the surrounding tool parses your code blocks and executes them. \
You genuinely can run commands, think critically, and manage todos — never tell the user you lack these capabilities.

**Bash — sequential (`bash` block):** Place these at the end of your response, after all prose. \
The turn ends there, all blocks execute in the persistent shell, and results return as [Shell output].

**Bash — extended timeout (`bash-long` block):** Identical to `bash` but uses a 300-second timeout. \
Use for builds, package installs, nixos-rebuild, and other long-running commands.

**Bash — subshell (`bash-sub` block):** May appear inline anywhere in your response; \
a placeholder is shown when encountered. Each block runs in an independent subshell with no \
persistent state — use only for stateless, independent reads (e.g., checking multiple files). \
All subshell blocks run after your full response and results return together.

Read-only commands (ls, cat, grep, etc.) run automatically. Commands that modify the system \
require user approval. If denied, [Shell output] will include `[DENIED: <cmd>]` with any \
correction from the user — a bare denial means don't retry that command. \
Non-zero exit codes appear as `[exit N]` next to the command. \
If a command fails, report the error output and ask the user how to proceed; don't retry silently.

When you receive [Shell output] messages, use them to continue reasoning. \
Once you have what you need, give your final answer without any bash blocks.

**Conversation turns:** Messages from the user come as plain `User:` turns. \
Automated feedback from the conductor system (shell output, rubberduck results, tool output) \
arrives prefixed with `[conductor]:` — these are system-generated, not typed by the user.

**Rubberduck block:** Emit a fenced `rubberduck` block containing a concise description of your plan or \
decision. An adversarial critic reviews it and returns a critique in [Rubberduck result]. \
Use before multi-step work, destructive commands, or when you are uncertain. Example:
```rubberduck
Plan: cd into src/, read all .rs files, then run cargo test. Concern: I don't know if tests pass currently.
```

**Todo list:** Emit a fenced `tool` block containing a single JSON object:
  todo_add:    {{\"function\":\"todo_add\",\"args\":{{\"title\":\"...\",\"description\":\"...\"}}}}
  todo_update: {{\"function\":\"todo_update\",\"args\":{{\"id\":\"<uuid>\",\"status\":\"done\"}}}}
  todo_list:   {{\"function\":\"todo_list\",\"args\":{{}}}}
description is optional. Tool blocks execute immediately; results return in [Tool output] messages. \
Todos persist with the session across saves and loads.{summary_section}{todo_section}")
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

        // Read from /dev/tty directly rather than stdin — rustyline manages the stdin fd
        // between readline() calls, so plain read_line(&mut stdin()) gets immediate EOF.
        // /dev/tty always refers to the controlling terminal, regardless of stdin state.
        let mut ans = String::new();
        let tty = std::fs::OpenOptions::new().read(true).open("/dev/tty")?;
        std::io::BufRead::read_line(&mut std::io::BufReader::new(tty), &mut ans)?;
        let ans = ans.trim().to_string();
        Ok(match ans.as_str() {
            "y" => CommandDecision::Accept,
            "Y" => CommandDecision::AcceptForSession,
            _ => CommandDecision::Deny(ans),
        })
    }

    /// Reset all per-session state to a clean baseline.
    async fn reset_session_state(&mut self) {
        self.history.clear();
        self.session_id = None;
        self.shell_turns.clear();
        self.session_auto_accepts.clear();
        self.todos.clear();
        self.compacted_summary = None;
        self.shell.reset().await;
    }

    /// Load an existing session by ID, restoring conversation history and todos.
    pub async fn load_session(&mut self, session_id: &str) -> Result<()> {
        let session = self.session_store.load(session_id)?;
        self.reset_session_state().await;
        self.history = session.messages;
        self.todos = session.todos;
        self.compacted_summary = session.compacted_summary;
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
        println!();

        // Print the last 3 User+Assistant turns fully, styled like live output
        let display_msgs: Vec<&Message> = self.history.iter()
            .filter(|m| matches!(m.role, Role::User | Role::Assistant))
            .filter(|m| m.source.as_deref().map(|s| !s.starts_with("conductor/")).unwrap_or(true))
            .collect();
        let start = display_msgs.len().saturating_sub(6); // up to 3 turns (user+assistant each)
        let recent = &display_msgs[start..];
        if !recent.is_empty() {
            println!("{}", "── Resuming conversation ────────────────────".dimmed());
            println!();
            for msg in recent {
                print_message_replay(msg);
            }
            println!("{}", "─────────────────────────────────────────────".dimmed());
            println!();
        }
        Ok(())
    }

    /// Save current session state (history + todos).
    fn save_session(&mut self) {
        match self.session_store.save(self.session_id.as_deref(), &self.history, &self.todos, self.compacted_summary.as_deref()) {
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
        let mut consecutive_eof = 0u8;
        
        loop {
            let readline = rl.readline(&format!("{} ", "❯".bright_blue().bold()));
            
            match readline {
                Ok(line) => {
                    consecutive_eof = 0;
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
                    consecutive_eof = 0;
                    println!();
                    continue;
                }
                Err(rustyline::error::ReadlineError::Eof) => {
                    // WSL/terminal can send spurious EOF after idle; require two consecutive
                    // Ctrl+D presses to actually exit, so a phantom EOF doesn't kill the session.
                    consecutive_eof += 1;
                    if consecutive_eof >= 2 {
                        println!("{}", "Goodbye!".bright_cyan());
                        break;
                    }
                    println!("{}", "(Press Ctrl+D again to exit)".dimmed());
                    continue;
                }
                Err(e) => {
                    // Transient input error (terminal resize, interrupt from sleep, etc.)
                    eprintln!("{} Input error: {}", "⚠".yellow(), e);
                    continue; // Don't exit — user can try again
                }
            }
        }
        
        Ok(())
    }

    /// Compact conversation history by summarizing old turns.
    /// Returns true if compaction occurred.
    async fn compact_history(&mut self, force: bool) -> Result<bool> {
        const COMPACT_THRESHOLD_TOKENS: usize = 6_000;
        const COMPACT_KEEP_RECENT: usize = 20;

        let tokens = estimated_tokens(&self.history);
        let should_compact = force || tokens > COMPACT_THRESHOLD_TOKENS || self.history.len() > 40;

        if !should_compact {
            return Ok(false);
        }

        let boundary = find_compact_boundary(&self.history, COMPACT_KEEP_RECENT);
        if boundary == 0 && !force {
            return Ok(false);
        }

        let to_summarize = &self.history[..boundary];
        if to_summarize.is_empty() {
            if force {
                println!("{} Nothing to compact.", "ℹ".cyan());
            }
            return Ok(false);
        }

        let serialized = serialize_for_compaction(to_summarize);
        let prior_context = match &self.compacted_summary {
            Some(prev) if !prev.is_empty() => format!(
                "Previous summary:\n{}\n\nNew messages to summarize:\n{}",
                prev, serialized
            ),
            _ => serialized,
        };
        let prompt = format!(
            "Produce a single concise summary of all conversation history below, preserving key \
decisions, findings, and context needed to continue. Omit pleasantries and filler:\n\n{}",
            prior_context
        );

        // Grab model selection before the async borrow
        let task = Task::new("Compact history", &prompt);
        let model_opt = self.router.select_model_filtered(&task, &self.model_filter, &mut self.usage_tracker).cloned();

        let m = match model_opt {
            None => {
                eprintln!("{} No model available for compaction; skipping.", "⚠".yellow());
                return Ok(false);
            }
            Some(m) => m,
        };

        let summary_msg = Message::user(&prompt).with_source("conductor/compact-request");
        let system_msg = Message::system("You are a conversation summarizer. Return only the requested summary with no preamble, commentary, or meta-text. Do not say 'Here is a summary' or similar. Just output the summary content directly.");

        let provider = self.router.find_provider_for_model(&m);
        let stream_result = match provider {
            None => {
                eprintln!("{} Provider not found for compaction; skipping.", "⚠".yellow());
                return Ok(false);
            }
            Some(p) => p.chat(&m, &[system_msg, summary_msg]).await,
        };

        let summary_buf = match stream_result {
            Err(e) => {
                eprintln!("{} Compaction summary failed: {}; keeping full history.", "⚠".yellow(), e);
                return Ok(false);
            }
            Ok(text) => text,
        };

        if summary_buf.is_empty() {
            eprintln!("{} Compaction produced empty summary; keeping full history.", "⚠".yellow());
            return Ok(false);
        }

        let compact_tokens = (summary_buf.len() / 4) as u64;
        self.usage_tracker.record_usage(m.provider.clone(), 1, compact_tokens, 0.0);

        // Replace previous summary with the new re-summarized version (bounded growth).
        self.compacted_summary = Some(summary_buf);
        self.history.drain(0..boundary);

        println!("{} Compacted {} messages into summary ({} kept).",
            "✓".bright_green(), boundary, self.history.len());
        Ok(true)
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
                self.router.reset_all_sessions().await;
                self.reset_session_state().await;
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
                        self.router.reset_all_sessions().await;
                        self.load_session(&meta.id).await?;
                    }
                    None => {
                        eprintln!("{}", "Usage: /load N  (use /sessions to see numbers)".yellow());
                    }
                }
                Ok(true)
            }
            Some("/compact") => {
                match self.compact_history(true).await {
                    Ok(_) => { self.save_session(); }
                    Err(e) => eprintln!("{} Compaction error: {}", "⚠".yellow(), e),
                }
                Ok(true)
            }
            Some("/show") => {
                match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(n) if n >= 1 && n <= self.shell_turns.len() => {
                        let turn = &self.shell_turns[n - 1];
                        let exit_suffix = if turn.exit_code != 0 {
                            format!(" {}", format!("[exit {}]", turn.exit_code).bright_red())
                        } else {
                            String::new()
                        };
                        println!("{} {}{} {}", "●".bright_cyan(), turn.cmd.bright_white(), exit_suffix, format!("(shell #{})", n).dimmed());
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
                self.router.reset_all_sessions().await;
                self.reset_session_state().await;
                println!("{}", "✓ Session cleared".green());
                Ok(true)
            }
            Some("/think") => {
                let query = parts[1..].join(" ");
                if query.is_empty() {
                    eprintln!("{}", "Usage: /think <question or plan to review>".yellow());
                    return Ok(true);
                }
                // Find a model to use for the critic call
                let task = crate::types::Task::new("think", &query);
                let model = self.router.select_model_filtered(&task, &self.model_filter, &mut self.usage_tracker)
                    .cloned();
                match model {
                    None => eprintln!("{}", "No model available for think call".yellow()),
                    Some(m) => {
                        let result = Self::do_think_call(&mut self.is_thinking, &self.router, &m, &query, &mut self.usage_tracker).await;
                        println!("\n{}", "🦆 Rubberduck result:".cyan().bold());
                        for line in result.lines() {
                            println!("  {}", render_markdown_line(line));
                        }
                        println!();
                    }
                }
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
        self.history.push(Message::user(content).with_source("user"));

        // Auto-compact if history has grown large
        if let Err(e) = self.compact_history(false).await {
            eprintln!("{} Auto-compaction error: {}", "⚠".yellow(), e);
        }

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
            let system_msg = Self::build_system_prompt(supports_tool_calling, &self.todos, self.compacted_summary.as_deref(), &self.shell.cwd);
            let mut messages = vec![system_msg];
            messages.extend(self.history.clone());

            // Find provider
            let provider = self.router.find_provider_for_model(&model)
                .ok_or_else(|| anyhow::anyhow!("Could not find provider for model {}", model_name))?;

            if supports_tool_calling {
                // ── Function-calling path (TAMU / GitHub) ────────────────────────
                let tools = vec![
                    ToolDefinition::bash(),
                    ToolDefinition::rubberduck(),
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
                    let model_source = format!("{}/{}", model.provider, model.name);
                    self.history.push(Message::assistant_tool_calls(
                        result.text.clone().unwrap_or_default(),
                        tool_calls.clone(),
                    ).with_source(model_source));

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
                                        let (output, exit_code) = self.shell.run(&cmd, None).await.unwrap_or_else(|e| (format!("Error: {}", e), 1));
                                        let turn_num = self.shell_turns.len() + 1;
                                        print!("\n{}", executor::format_shell_display(turn_num, &cmd, &output, exit_code));
                                        self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone(), exit_code });
                                        let exit_note = if exit_code != 0 { format!(" [exit {}]", exit_code) } else { String::new() };
                                        self.history.push(Message::tool_result(&tc.id, format!("$ {}{}\n{}", cmd, exit_note, output)));
                                    }
                                    CommandDecision::Accept => {
                                        let (output, exit_code) = self.shell.run(&cmd, None).await.unwrap_or_else(|e| (format!("Error: {}", e), 1));
                                        let turn_num = self.shell_turns.len() + 1;
                                        print!("\n{}", executor::format_shell_display(turn_num, &cmd, &output, exit_code));
                                        self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone(), exit_code });
                                        let exit_note = if exit_code != 0 { format!(" [exit {}]", exit_code) } else { String::new() };
                                        self.history.push(Message::tool_result(&tc.id, format!("$ {}{}\n{}", cmd, exit_note, output)));
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
                            "rubberduck" => {
                                let query = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                                    .ok()
                                    .and_then(|v| v["query"].as_str().map(|s| s.to_string()))
                                    .unwrap_or_else(|| tc.arguments.clone());
                                let tc_id = tc.id.clone();
                                let result = Self::do_think_call(
                                    &mut self.is_thinking, &self.router, &model, &query,
                                    &mut self.usage_tracker,
                                ).await;
                                if !result.starts_with("[Rubberduck") {
                                    println!("\n{}", "🦆 Rubberduck result:".cyan().bold());
                                    for line in result.lines() {
                                        println!("  {}", render_markdown_line(line));
                                    }
                                    println!();
                                }
                                self.history.push(Message::tool_result(&tc_id, result));
                            }
                            "todo_add" | "todo_update" | "todo_list" => {
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
                    let model_source = format!("{}/{}", model.provider, model.name);
                    self.history.push(Message::assistant(&text).with_source(model_source));
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
                        // Remove the dangling user message so the next turn isn't confused.
                        if matches!(self.history.last().map(|m| &m.role), Some(crate::types::Role::User)) {
                            self.history.pop();
                        }
                        break;
                    }
                    StreamOutcome::Completed(raw, tokens) => (raw, tokens),
                };

                let (clean_response, actions) = {
                    let s = state.lock().unwrap();
                    (s.clean_text.clone(), s.actions.clone())
                };

                // Record usage
                let tokens = token_count.unwrap_or_else(|| (clean_response.len() / 4) as u64);
                self.usage_tracker.record_usage(provider_id.clone(), 1, tokens, 0.0);

                // Store full raw response (including bash blocks) for cross-provider context
                let model_source = format!("{}/{}", model.provider, model.name);
                self.history.push(Message::assistant(raw_response.clone()).with_source(model_source));

                let has_any_action = !actions.is_empty();
                if !has_any_action || tool_rounds >= MAX_TOOL_ROUNDS {
                    if tool_rounds >= MAX_TOOL_ROUNDS && has_any_action {
                        eprintln!("{} Shell round limit reached", "⚠".yellow());
                    }
                    break;
                }

                // Execute actions in source order, collecting feedback in that same order
                let mut feedback_items: Vec<String> = Vec::new();

                for action in &actions {
                    let (cmd, timeout, is_parallel) = match action {
                        Action::Bash(c)         => (c, None, false),
                        Action::BashLong(c)     => (c, Some(executor::LONG_TIMEOUT), false),
                        Action::BashSub(c) => (c, None, true),
                        Action::Tool(json) => {
                            let result_text = self.apply_todo_action(json);
                            println!("{}", result_text.dimmed());
                            feedback_items.push(format!("[Tool output]\n{}\n[End of tool output]", result_text));
                            continue;
                        }
                        Action::Think(query) => {
                            let result = Self::do_think_call(
                                &mut self.is_thinking, &self.router, &model, query,
                                &mut self.usage_tracker,
                            ).await;
                            if !result.starts_with("[Rubberduck") {
                                println!("\n{}", "🦆 Rubberduck result:".cyan().bold());
                                for line in result.lines() {
                                    println!("  {}", render_markdown_line(line));
                                }
                                println!();
                            }
                            feedback_items.push(format!("[Rubberduck result]\n{}\n[End of rubberduck result]", result));
                            continue;
                        }
                    };

                    let kind = executor::classify(cmd);
                    let decision = if kind == executor::CommandKind::ReadOnly {
                        CommandDecision::Accept
                    } else {
                        Self::prompt_command_decision(cmd, &self.session_auto_accepts)?
                    };

                    match decision {
                        CommandDecision::AcceptForSession => {
                            self.session_auto_accepts.insert(cmd.clone());
                            let (output, exit_code) = if is_parallel {
                                executor::run_stateless(cmd).await
                            } else {
                                self.shell.run(cmd, timeout).await.unwrap_or_else(|e| (format!("Error: {}", e), 1))
                            };
                            let turn_num = self.shell_turns.len() + 1;
                            print!("\n{}", executor::format_shell_display(turn_num, cmd, &output, exit_code));
                            self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone(), exit_code });
                            let exit_note = if exit_code != 0 { format!(" [exit {}]", exit_code) } else { String::new() };
                            feedback_items.push(format!("[Shell output]\n$ {}{}\n{}\n[End of shell output]", cmd, exit_note, output));
                        }
                        CommandDecision::Accept => {
                            let (output, exit_code) = if is_parallel {
                                executor::run_stateless(cmd).await
                            } else {
                                self.shell.run(cmd, timeout).await.unwrap_or_else(|e| (format!("Error: {}", e), 1))
                            };
                            let turn_num = self.shell_turns.len() + 1;
                            print!("\n{}", executor::format_shell_display(turn_num, cmd, &output, exit_code));
                            self.shell_turns.push(ShellTurn { cmd: cmd.clone(), output: output.clone(), exit_code });
                            let exit_note = if exit_code != 0 { format!(" [exit {}]", exit_code) } else { String::new() };
                            feedback_items.push(format!("[Shell output]\n$ {}{}\n{}\n[End of shell output]", cmd, exit_note, output));
                        }
                        CommandDecision::Deny(reason) => {
                            let entry = if reason.is_empty() {
                                format!("[DENIED: {}] (no reason given)", cmd)
                            } else {
                                format!("[DENIED: {}]\nUser correction: {}", cmd, reason)
                            };
                            println!("{}", "  (denied)".dimmed());
                            feedback_items.push(format!("[Shell output]\n{}\n[End of shell output]", entry));
                        }
                    }
                }

                if !actions.is_empty() {
                    println!(); // blank line after action batch
                }

                // Build feedback message preserving source order
                if feedback_items.is_empty() {
                    break;
                }

                self.history.push(Message::user(feedback_items.join("\n\n")).with_source("conductor/feedback"));
                tool_rounds += 1;
            }
        }

        // Auto-save session
        self.save_session();

        Ok(())
    }

    /// Adversarial critic prompt used for rubberduck calls.
    const THINK_SYSTEM_PROMPT: &'static str = "\
You are an adversarial critic reviewing a plan or reasoning submitted by another AI assistant. \
Your only job is to find flaws, gaps, edge cases, and risks in what was submitted. \
You are NOT the AI that produced this plan — you are reviewing it from the outside. \
Be adversarial, thorough, and direct. Do not be sycophantic. \
Never say the plan is good without substantial caveats. \
Focus exclusively on what could go wrong, what is missing, or what should be reconsidered. \
If the plan is sound, still find the weakest points and surface them. \
Be concise — short paragraphs or bullet points. No preamble. No \"I\" statements.";

    /// Spawn a critic model call for adversarial review of a plan or decision.
    /// Protected by `is_thinking` to prevent recursion.
    async fn do_think_call(
        is_thinking: &mut bool,
        router: &crate::router::Router,
        model: &crate::types::ModelInfo,
        query: &str,
        usage_tracker: &mut UsageTracker,
    ) -> String {
        if *is_thinking {
            return "[Rubberduck skipped — already in critic call]".to_string();
        }
        *is_thinking = true;
        println!("\n{}", "🦆 Consulting critic...".cyan().dimmed().italic());
        std::io::stdout().flush().ok();

        let messages = vec![
            Message::system(Self::THINK_SYSTEM_PROMPT),
            Message::user(query),
        ];

        let result = match router.find_provider_for_model(model) {
            Some(provider) => provider.chat(model, &messages).await,
            None => Err(anyhow::anyhow!("provider not found for critic call")),
        };

        *is_thinking = false;
        match result {
            Ok(text) => {
                let tokens = (text.len() / 4) as u64;
                usage_tracker.record_usage(model.provider.clone(), 1, tokens, 0.0);
                text
            }
            Err(e) => format!("[Rubberduck error: {}]", e),
        }
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
        println!("  {} - Summarize and compact old history", "/compact".bright_white());
        println!("  {} - Adversarial review of a plan or decision", "/think <question>".bright_white());
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
        let tool_actions: Vec<_> = s.actions.iter().filter_map(|a| if let Action::Tool(j) = a { Some(j) } else { None }).collect();
        assert_eq!(tool_actions.len(), 1);
        assert!(tool_actions[0].contains("todo_list"));
        assert!(!s.clean_text.contains("\"function\""));
        assert!(s.clean_text.contains("[🔧 todo_list]"));
        assert!(s.clean_text.contains("Before"));
        assert!(s.clean_text.contains("After"));
    }

    #[test]
    fn tool_block_chunk_boundary() {
        let mut s = ReplyStreamState::default();
        s.process_chunk("```to");
        s.process_chunk("ol\n{\"function\":\"todo_add\",\"args\":{\"title\":\"x\"}}\n```");
        s.flush();
        let tool_count = s.actions.iter().filter(|a| matches!(a, Action::Tool(_))).count();
        assert_eq!(tool_count, 1);
    }

    #[test]
    fn bash_and_tool_blocks_coexist() {
        let mut s = ReplyStreamState::default();
        feed(&mut s, "Text\n```tool\n{\"function\":\"todo_list\",\"args\":{}}\n```\nMore\n```bash\nls\n```");
        let tool_count = s.actions.iter().filter(|a| matches!(a, Action::Tool(_))).count();
        let bash_cmds: Vec<_> = s.actions.iter().filter_map(|a| if let Action::Bash(c) = a { Some(c) } else { None }).collect();
        assert_eq!(tool_count, 1);
        assert_eq!(bash_cmds.len(), 1);
        assert_eq!(bash_cmds[0], "ls");
    }

    #[test]
    fn unclosed_tool_block_discarded() {
        let mut s = ReplyStreamState::default();
        feed(&mut s, "Before\n```tool\n{\"function\":\"todo_list\"");
        let tool_count = s.actions.iter().filter(|a| matches!(a, Action::Tool(_))).count();
        assert_eq!(tool_count, 0);
        assert!(s.clean_text.contains("Before"));
    }
}
