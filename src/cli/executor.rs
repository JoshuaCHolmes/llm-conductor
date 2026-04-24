use anyhow::Result;

const MAX_PREVIEW_LINES: usize = 5;
const MAX_OUTPUT_BYTES: usize = 8_000;

#[derive(Debug, Clone, PartialEq)]
pub enum CommandKind {
    ReadOnly,
    NeedsConfirm,
}

pub struct ShellTurn {
    pub cmd: String,
    pub output: String,
}

/// Returns whether `raw` contains shell metacharacters that require confirm.
fn has_metacharacters(raw: &str) -> bool {
    // Any of these patterns require confirmation regardless of program
    let danger_patterns = [">", ">>", "|", ";", "&&", "||", "`", "$(", "${", "\\"];
    danger_patterns.iter().any(|p| raw.contains(p))
}

/// Classify a raw command string. Read-only commands that use only safe programs
/// and have no metacharacters run automatically; everything else requires confirmation.
pub fn classify(raw: &str) -> CommandKind {
    let raw = raw.trim();

    // Anything with shell metacharacters, redirections, or piping requires confirm
    if has_metacharacters(raw) {
        return CommandKind::NeedsConfirm;
    }

    let program = raw.split_whitespace().next().unwrap_or("");

    // Explicitly destructive or elevated programs always require confirm
    let deny = ["rm", "rmdir", "mv", "cp", "sudo", "su", "chmod", "chown",
                "dd", "mkfs", "fdisk", "parted", "shred", "truncate",
                "curl", "wget", "ssh", "scp", "rsync", "git", "npm", "pip",
                "cargo", "make", "sh", "bash", "zsh", "fish", "python",
                "python3", "node", "ruby", "perl", "tee", "xargs", "kill",
                "pkill", "killall", "systemctl", "service"];
    if deny.contains(&program) {
        return CommandKind::NeedsConfirm;
    }

    // Safe read-only programs
    let safe = ["ls", "cat", "head", "tail", "grep", "rg", "find", "echo",
                "pwd", "which", "whereis", "wc", "stat", "file", "type",
                "env", "printenv", "date", "uname", "id", "whoami",
                "df", "du", "free", "uptime", "ps", "top", "htop",
                "lsof", "ss", "ip", "ifconfig", "hostname", "nslookup",
                "dig", "ping", "traceroute", "less", "more", "diff",
                "md5sum", "sha256sum", "xxd", "strings"];
    if safe.contains(&program) {
        CommandKind::ReadOnly
    } else {
        CommandKind::NeedsConfirm
    }
}

/// Run `cmd` via `sh -c`, capture combined stdout+stderr, truncate at MAX_OUTPUT_BYTES.
pub fn execute(cmd: &str) -> Result<String> {
    use std::process::Command;

    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()?;

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if combined.len() > MAX_OUTPUT_BYTES {
        combined.truncate(MAX_OUTPUT_BYTES);
        combined.push_str("\n[output truncated]");
    }

    Ok(combined)
}

/// Extract all ```bash ... ``` blocks from a model response.
pub fn extract_bash_blocks(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("```bash") {
        let after_fence = &remaining[start + 7..]; // skip "```bash"
        // Skip optional newline after fence
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

/// Format a shell turn for display in the REPL.
/// Returns (header_line, preview_lines, hidden_count).
pub fn format_shell_display(turn_num: usize, cmd: &str, output: &str) -> String {
    use colored::Colorize;

    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let preview_count = total.min(MAX_PREVIEW_LINES);

    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "{} {} {}\n",
        "●".bright_cyan(),
        cmd.bright_white(),
        format!("(shell #{})", turn_num).dimmed(),
    ));

    // Preview lines
    for line in &lines[..preview_count] {
        out.push_str(&format!("  {} {}\n", "│".dimmed(), line));
    }

    // Footer
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
        assert_eq!(classify("ls > out.txt"), CommandKind::NeedsConfirm);
        assert_eq!(classify("cat foo | grep bar"), CommandKind::NeedsConfirm);
        assert_eq!(classify("echo $(pwd)"), CommandKind::NeedsConfirm);
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
}
