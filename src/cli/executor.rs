use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::{timeout, Duration};
use std::process::Stdio;

const MAX_PREVIEW_LINES: usize = 5;
const MAX_OUTPUT_BYTES: usize = 8_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const LONG_TIMEOUT: Duration = Duration::from_secs(300);

/// Programs known to take a long time — upgraded to LONG_TIMEOUT automatically.
const LONG_RUNNING_PROGRAMS: &[&str] = &[
    "cargo", "rustc", "npm", "yarn", "pnpm", "bun",
    "pip", "pip3", "pip3.11", "poetry", "uv",
    "make", "cmake", "ninja", "gradle", "mvn", "ant", "bazel", "buck", "meson",
    "docker", "podman",
    "nix", "nix-build", "nix-shell", "nixos-rebuild",
    "go", "tsc", "webpack", "vite", "esbuild", "rollup",
    "pytest", "jest", "cargo-test",
];

/// Sentinel strings used to delimit shell output from tracking lines.
/// Double-underscored to reduce collision risk with real output.
const SENTINEL_CWD: &str = "__LLM_CWD__:";
const SENTINEL_EXIT: &str = "__LLM_EXIT__:";

#[derive(Debug, Clone, PartialEq)]
pub enum CommandKind {
    ReadOnly,
    NeedsConfirm,
}

#[derive(Debug, Clone)]
pub struct ShellTurn {
    pub cmd: String,
    pub output: String,
    pub exit_code: i32,
}

struct ShellProcess {
    _child: Child,
    stdin: tokio::io::BufWriter<ChildStdin>,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
}

/// Persistent bash shell. Working directory and environment variables survive
/// between `run()` calls within the same session.
pub struct Shell {
    process: Option<ShellProcess>,
    pub cwd: String,
}

impl Shell {
    pub async fn new() -> Result<Self> {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| String::from("/"));
        let mut shell = Shell { process: None, cwd };
        shell.start().await?;
        Ok(shell)
    }

    async fn start(&mut self) -> Result<()> {
        let mut child = tokio::process::Command::new("bash")
            .args(["--norc", "--noprofile"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let mut proc = ShellProcess {
            _child: child,
            stdin: tokio::io::BufWriter::new(stdin),
            lines: BufReader::new(stdout).lines(),
        };

        // Redirect stderr to stdout for the whole session, then cd to working dir.
        let safe_cwd = self.cwd.replace('\'', "'\\''");
        let init = format!("exec 2>&1\ncd '{}' 2>/dev/null || true\n", safe_cwd);
        proc.stdin.write_all(init.as_bytes()).await?;
        proc.stdin.flush().await?;

        self.process = Some(proc);
        Ok(())
    }

    /// Reset shell to a fresh process (used on /new, /load, /clear).
    pub async fn reset(&mut self) {
        if let Some(mut proc) = self.process.take() {
            let _ = proc._child.kill().await;
        }
        let _ = self.start().await;
    }

    /// Run a command. Returns `(output, exit_code)`.
    /// Working directory and environment persist across calls.
    /// Pass `None` for timeout to use auto-detection via `classify_timeout`.
    pub async fn run(&mut self, cmd: &str, timeout_dur: Option<Duration>) -> Result<(String, i32)> {
        if self.process.is_none() {
            self.start().await?;
        }

        let t = timeout_dur.unwrap_or_else(|| classify_timeout(cmd));

        // Write command followed by sentinels that capture exit code and cwd.
        {
            let proc = self.process.as_mut().unwrap();
            let script = format!(
                "{}\n__llm_ec=$?\nprintf '{}%s\\n' \"$(pwd)\"\nprintf '{}%d\\n' \"$__llm_ec\"\n",
                cmd, SENTINEL_CWD, SENTINEL_EXIT
            );
            proc.stdin.write_all(script.as_bytes()).await?;
            proc.stdin.flush().await?;
        }

        // Take the process out to avoid overlapping borrows during async read.
        let mut proc = self.process.take().unwrap();
        let cwd_snapshot = self.cwd.clone();

        let read_result = timeout(t, collect_output(&mut proc, &cwd_snapshot)).await;

        match read_result {
            Ok(Ok((output, code, new_cwd))) => {
                self.cwd = new_cwd;
                self.process = Some(proc);
                Ok((output, code))
            }
            Ok(Err(_e)) => {
                let _ = proc._child.kill().await;
                let _ = self.start().await;
                Err(anyhow::anyhow!("Shell process terminated unexpectedly"))
            }
            Err(_) => {
                let _ = proc._child.kill().await;
                let _ = self.start().await;
                Ok((
                    format!("[command timed out after {}s — use bash-long for extended tasks]", t.as_secs()),
                    124,
                ))
            }
        }
    }
}

/// Determine the appropriate execution timeout for a command block.
/// Scans all non-comment lines; any line whose first meaningful token is a
/// known long-running program upgrades the whole block to LONG_TIMEOUT.
pub fn classify_timeout(cmd: &str) -> Duration {
    for line in cmd.trim().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Skip common wrappers and env-var assignments, find the real program.
        let mut tokens = line.split_whitespace().peekable();
        let program = loop {
            match tokens.next() {
                None => break None,
                Some(tok) => match tok {
                    "env" | "time" | "sudo" | "doas" | "nice" | "nohup" | "taskset" => continue,
                    t if t.contains('=') && !t.starts_with('-') => continue,
                    t => break Some(t.rsplit('/').next().unwrap_or(t)),
                },
            }
        };
        if let Some(prog) = program {
            if LONG_RUNNING_PROGRAMS.contains(&prog) {
                return LONG_TIMEOUT;
            }
        }
    }
    DEFAULT_TIMEOUT
}

/// Run a command in a fresh subshell with no persistent state (used for
/// bash-sub blocks). Returns `(output, exit_code)`.
pub async fn run_stateless(cmd: &str) -> (String, i32) {
    let result = timeout(
        DEFAULT_TIMEOUT,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(out)) => {
            let mut output = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.is_empty() {
                output.push_str(&stderr);
            }
            if output.len() > MAX_OUTPUT_BYTES {
                let mut boundary = MAX_OUTPUT_BYTES;
                while !output.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                output.truncate(boundary);
                output.push_str("\n[output truncated]");
            }
            (output, out.status.code().unwrap_or(1))
        }
        Ok(Err(e)) => (format!("Error: {}", e), 1),
        Err(_) => (
            format!("[command timed out after {}s]", DEFAULT_TIMEOUT.as_secs()),
            124,
        ),
    }
}

/// Read lines from the shell until sentinels appear; returns (output, exit_code, new_cwd).
async fn collect_output(
    proc: &mut ShellProcess,
    current_cwd: &str,
) -> Result<(String, i32, String)> {
    let mut lines_out: Vec<String> = Vec::new();
    let mut exit_code = 0i32;
    let mut new_cwd = current_cwd.to_string();

    loop {
        match proc.lines.next_line().await? {
            None => return Err(anyhow::anyhow!("shell EOF")),
            Some(line) => {
                if let Some(cwd) = line.strip_prefix(SENTINEL_CWD) {
                    new_cwd = cwd.to_string();
                } else if let Some(code_str) = line.strip_prefix(SENTINEL_EXIT) {
                    exit_code = code_str.trim().parse().unwrap_or(0);
                    break;
                } else {
                    lines_out.push(line);
                }
            }
        }
    }

    let mut output = lines_out.join("\n");
    // Truncate on a char boundary — byte-based truncate can panic on multibyte chars.
    if output.len() > MAX_OUTPUT_BYTES {
        let mut boundary = MAX_OUTPUT_BYTES;
        while !output.is_char_boundary(boundary) {
            boundary -= 1;
        }
        output.truncate(boundary);
        output.push_str("\n[output truncated]");
    }

    Ok((output, exit_code, new_cwd))
}

// ── Command classification ────────────────────────────────────────────────────

/// Safe, bounded read-only programs that run without confirmation.
/// Interactive/hanging commands (top, htop, less, ping …) and env/printenv
/// (which could expose secrets) are intentionally excluded.
const SAFE_PROGRAMS: &[&str] = &[
    "ls", "cat", "head", "tail", "grep", "rg", "find", "echo",
    "pwd", "which", "whereis", "wc", "stat", "file", "type",
    "date", "uname", "id", "whoami",
    "df", "du", "free", "uptime", "ps", "lsof", "ss", "ip",
    "ifconfig", "hostname",
    "md5sum", "sha256sum", "sha1sum", "xxd", "strings",
    "diff", "sort", "uniq", "tr", "cut",
];

/// Programs that always require confirmation regardless of arguments.
const DENY_PROGRAMS: &[&str] = &[
    "rm", "rmdir", "mv", "cp", "sudo", "su", "chmod", "chown",
    "dd", "mkfs", "fdisk", "parted", "shred", "truncate",
    "curl", "wget", "ssh", "scp", "rsync", "git", "npm", "pip",
    "cargo", "make", "sh", "bash", "zsh", "fish", "python",
    "python3", "node", "ruby", "perl", "tee", "xargs", "kill",
    "pkill", "killall", "systemctl", "service",
    "env", "printenv", // may expose credentials in shell environment
];

/// Check for shell metacharacters that imply side-effects even for safe programs.
/// Pipes and sequential operators are handled separately in classify_line so that
/// fully read-only pipelines (e.g. `find . | head -20 | grep foo`) don't require confirmation.
fn has_dangerous_metacharacters(raw: &str) -> bool {
    let danger_patterns = [">", ">>", "`", "$(", "${", "\\"];
    danger_patterns.iter().any(|p| raw.contains(p))
}

/// Classify a single (already-split) command line, handling `|`, `;`, `&&`, `||`.
fn classify_line(raw: &str) -> CommandKind {
    if has_dangerous_metacharacters(raw) {
        return CommandKind::NeedsConfirm;
    }
    // Split on pipe/sequencing operators and classify each segment.
    // If any segment is NeedsConfirm, the whole line is NeedsConfirm.
    for segment in raw.split(|c| c == '|' || c == ';') {
        // Strip leading `&` to handle `&&` and `||` (they leave empty/`&` segments after split)
        let segment = segment.trim_start_matches('&').trim();
        if segment.is_empty() {
            continue;
        }
        let program = segment.split_whitespace().next().unwrap_or("");
        if program.is_empty() {
            continue;
        }
        if DENY_PROGRAMS.contains(&program) {
            return CommandKind::NeedsConfirm;
        }
        if !SAFE_PROGRAMS.contains(&program) {
            return CommandKind::NeedsConfirm;
        }
    }
    CommandKind::ReadOnly
}

/// Classify a raw command block. Multi-line blocks are checked line by line;
/// any line that needs confirmation causes the whole block to require it.
pub fn classify(raw: &str) -> CommandKind {
    for line in raw.trim().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if classify_line(line) == CommandKind::NeedsConfirm {
            return CommandKind::NeedsConfirm;
        }
    }
    CommandKind::ReadOnly
}

// ── Bash block extraction ─────────────────────────────────────────────────────

/// Extract all ```bash … ``` blocks from a model response.
pub fn extract_bash_blocks(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("```bash") {
        let after_fence = &remaining[start + 7..];
        let after_nl = after_fence.strip_prefix('\n').unwrap_or(after_fence);
        if let Some(end) = after_nl.find("```") {
            let cmd = after_nl[..end].trim().to_string();
            if !cmd.is_empty() {
                commands.push(cmd);
            }
            remaining = &after_nl[end + 3..];
        } else {
            break;
        }
    }

    commands
}

// ── Display ───────────────────────────────────────────────────────────────────

/// Format a shell turn for display in the REPL.
pub fn format_shell_display(turn_num: usize, cmd: &str, output: &str, exit_code: i32) -> String {
    use colored::Colorize;

    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let preview_count = total.min(MAX_PREVIEW_LINES);

    let mut out = String::new();

    let exit_suffix = if exit_code != 0 {
        format!(" {}", format!("[exit {}]", exit_code).bright_red())
    } else {
        String::new()
    };

    out.push_str(&format!(
        "{} {}{} {}\n",
        "●".bright_cyan(),
        cmd.bright_white(),
        exit_suffix,
        format!("(shell #{})", turn_num).dimmed(),
    ));

    for line in &lines[..preview_count] {
        out.push_str(&format!("  {} {}\n", "│".dimmed(), line));
    }

    let hidden = total.saturating_sub(MAX_PREVIEW_LINES);
    if hidden > 0 {
        out.push_str(&format!(
            "  {} {} {}\n",
            "└".dimmed(),
            format!("{} more lines…", hidden).dimmed(),
            format!("(/show {} for full output)", turn_num).dimmed(),
        ));
    } else if total == 0 {
        out.push_str(&format!("  {} {}\n", "└".dimmed(), "(no output)".dimmed()));
    } else {
        out.push_str(&format!("  {}\n", "└".dimmed()));
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_safe_commands() {
        assert_eq!(classify("ls -la"), CommandKind::ReadOnly);
        assert_eq!(classify("cat /etc/hosts"), CommandKind::ReadOnly);
        assert_eq!(classify("grep foo bar.txt"), CommandKind::ReadOnly);
    }

    #[test]
    fn classify_dangerous_commands() {
        assert_eq!(classify("rm -rf /"), CommandKind::NeedsConfirm);
        assert_eq!(classify("sudo apt install foo"), CommandKind::NeedsConfirm);
        assert_eq!(classify("git push"), CommandKind::NeedsConfirm);
    }

    #[test]
    fn classify_metacharacters_require_confirm() {
        // Redirections and subshell expansion always require confirmation
        assert_eq!(classify("ls > out.txt"), CommandKind::NeedsConfirm);
        assert_eq!(classify("echo $(pwd)"), CommandKind::NeedsConfirm);
        // Pipes between safe programs are now read-only
        assert_eq!(classify("cat foo | grep bar"), CommandKind::ReadOnly);
        assert_eq!(classify("find . -type f | head -80"), CommandKind::ReadOnly);
        // Pipes involving non-safe programs still require confirmation
        assert_eq!(classify("cat foo | xargs rm"), CommandKind::NeedsConfirm);
    }

    #[test]
    fn classify_multiline_safe() {
        assert_eq!(classify("ls -la\ncat file.txt"), CommandKind::ReadOnly);
    }

    #[test]
    fn classify_multiline_mixed_requires_confirm() {
        // Second line is dangerous — whole block needs confirm
        assert_eq!(classify("echo hello\nrm -rf target"), CommandKind::NeedsConfirm);
        assert_eq!(classify("ls\ngit push"), CommandKind::NeedsConfirm);
    }

    #[test]
    fn classify_multiline_comments_ignored() {
        // Comment lines should not affect classification
        assert_eq!(classify("# check files\nls -la"), CommandKind::ReadOnly);
    }

    #[test]
    fn classify_env_needs_confirm() {
        assert_eq!(classify("env"), CommandKind::NeedsConfirm);
        assert_eq!(classify("printenv"), CommandKind::NeedsConfirm);
    }

    #[test]
    fn extract_single_block() {
        let text = "Here is a command:\n```bash\nls -la\n```\nDone.";
        let blocks = extract_bash_blocks(text);
        assert_eq!(blocks, vec!["ls -la"]);
    }

    #[test]
    fn extract_multiple_blocks() {
        let text = "```bash\npwd\n```\nand\n```bash\nls\n```";
        let blocks = extract_bash_blocks(text);
        assert_eq!(blocks, vec!["pwd", "ls"]);
    }

    #[test]
    fn extract_no_blocks() {
        let text = "No shell commands here.";
        assert!(extract_bash_blocks(text).is_empty());
    }

    #[test]
    fn utf8_truncation_safe() {
        // 3-byte UTF-8 char: '€' = 0xE2 0x82 0xAC
        let s = "€".repeat(3_000); // 9_000 bytes
        let mut output = s;
        if output.len() > MAX_OUTPUT_BYTES {
            let mut boundary = MAX_OUTPUT_BYTES;
            while !output.is_char_boundary(boundary) {
                boundary -= 1;
            }
            output.truncate(boundary);
            output.push_str("\n[output truncated]");
        }
        assert!(output.ends_with("[output truncated]"));
        // Must be valid UTF-8 (no panic)
        let _: &str = &output;
    }
}
