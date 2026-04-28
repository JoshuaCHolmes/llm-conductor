use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::Message;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
}

impl TodoStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "pending" => Some(Self::Pending),
            "in_progress" | "inprogress" | "active" => Some(Self::InProgress),
            "done" | "complete" | "completed" | "finished" => Some(Self::Done),
            "blocked" | "block" => Some(Self::Blocked),
            _ => None,
        }
    }
}

impl fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Pending    => "pending",
            Self::InProgress => "in_progress",
            Self::Done       => "done",
            Self::Blocked    => "blocked",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: TodoStatus,
    pub created_at: DateTime<Utc>,
}

impl Todo {
    pub fn new(title: &str, description: Option<&str>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            description: description.map(|s| s.to_string()),
            status: TodoStatus::Pending,
            created_at: Utc::now(),
        }
    }

    /// One-line display: "● [status] title (id_prefix)"
    pub fn summary(&self, num: usize) -> String {
        let icon = match self.status {
            TodoStatus::Pending    => "○",
            TodoStatus::InProgress => "◐",
            TodoStatus::Done       => "✓",
            TodoStatus::Blocked    => "✗",
        };
        format!("{:2}. {} [{}] {}", num, icon, self.status, self.title)
    }
}

const SESSIONS_PER_PAGE: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    /// Summary of history that was compacted away. Injected into system prompt on resume.
    #[serde(default)]
    pub compacted_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub filename: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Preview: first user message, truncated
    pub preview: String,
}

pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    pub fn new(config_dir: &Path) -> Result<Self> {
        let sessions_dir = config_dir.join("sessions");
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }

    /// Save or update a session. Returns the session ID (created on first save).
    pub fn save(&self, session_id: Option<&str>, messages: &[Message], todos: &[Todo], compacted_summary: Option<&str>) -> Result<String> {
        let id = session_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let filename = format!("{}.json", id);
        let path = self.sessions_dir.join(&filename);

        let preview = messages
            .iter()
            .find(|m| matches!(m.role, crate::types::Role::User))
            .map(|m| {
                let s = m.content.chars().take(80).collect::<String>();
                if m.content.len() > 80 { format!("{}…", s) } else { s }
            })
            .unwrap_or_default();

        // Preserve original created_at if the file already exists
        let created_at = if path.exists() {
            let existing: SessionFile = serde_json::from_str(&fs::read_to_string(&path)?)?;
            existing.created_at
        } else {
            Utc::now()
        };

        let session = SessionFile {
            id: id.clone(),
            created_at,
            updated_at: Utc::now(),
            messages: messages.to_vec(),
            todos: todos.to_vec(),
            compacted_summary: compacted_summary.map(|s| s.to_string()),
        };

        // Atomic write: serialize to a temp file then rename
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, serde_json::to_string_pretty(&session)?)?;
        fs::rename(&tmp_path, &path)?;

        // Update index
        self.update_index(&SessionMeta {
            id: id.clone(),
            filename,
            created_at,
            updated_at: session.updated_at,
            preview,
        })?;

        Ok(id)
    }

    pub fn load(&self, session_id: &str) -> Result<SessionFile> {
        let path = self.sessions_dir.join(format!("{}.json", session_id));
        let contents = fs::read_to_string(&path)
            .map_err(|_| anyhow::anyhow!("Session '{}' not found", session_id))?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// List sessions sorted by most recent first.
    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        let index = self.load_index()?;
        let mut metas = index;
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    /// Print a page of sessions. Returns total number of sessions.
    pub fn print_page(&self, page: usize) -> Result<usize> {
        use colored::Colorize;
        let sessions = self.list()?;
        let total = sessions.len();

        if total == 0 {
            println!("{}", "No saved sessions.".dimmed());
            return Ok(0);
        }

        let start = page * SESSIONS_PER_PAGE;
        let end = (start + SESSIONS_PER_PAGE).min(total);
        let page_sessions = &sessions[start..end];

        println!("{}", "Saved sessions:".bright_cyan().bold());
        for (i, meta) in page_sessions.iter().enumerate() {
            let num = start + i + 1;
            let age = format_age(meta.updated_at);
            println!("  {} {}  {}  {}",
                format!("[{}]", num).bright_yellow().bold(),
                age.dimmed(),
                meta.preview.bright_white(),
                meta.id.dimmed(),
            );
        }

        let pages = total.div_ceil(SESSIONS_PER_PAGE);
        if pages > 1 {
            println!();
            println!("{}", format!("Page {}/{} — use > / < to navigate", page + 1, pages).dimmed());
        }

        Ok(total)
    }

    fn index_path(&self) -> PathBuf {
        self.sessions_dir.join("index.json")
    }

    fn load_index(&self) -> Result<Vec<SessionMeta>> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    fn update_index(&self, meta: &SessionMeta) -> Result<()> {
        let mut index = self.load_index()?;
        // Replace existing entry or push new one
        if let Some(pos) = index.iter().position(|m| m.id == meta.id) {
            index[pos] = meta.clone();
        } else {
            index.push(meta.clone());
        }
        let idx_path = self.index_path();
        let tmp_idx = idx_path.with_extension("json.tmp");
        fs::write(&tmp_idx, serde_json::to_string_pretty(&index)?)?;
        fs::rename(&tmp_idx, &idx_path)?;
        Ok(())
    }

    /// Look up a session by 1-based display number (sorted by most recent).
    pub fn get_by_number(&self, n: usize) -> Result<SessionMeta> {
        let sessions = self.list()?;
        sessions.into_iter().nth(n.saturating_sub(1))
            .ok_or_else(|| anyhow::anyhow!("No session with number {}", n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Role};
    use tempfile::TempDir;

    fn make_store(dir: &TempDir) -> SessionStore {
        SessionStore::new(dir.path()).unwrap()
    }

    fn msg(role: Role, content: &str) -> Message {
        Message { role, content: content.to_string(), tool_calls: None, tool_call_id: None }
    }

    #[test]
    fn resume_writes_back_to_same_slot() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);

        // First save — no id yet, should create a new slot
        let msgs1 = vec![msg(Role::User, "hello"), msg(Role::Assistant, "hi")];
        let id = store.save(None, &msgs1, &[], None).unwrap();

        // Verify single entry in index
        assert_eq!(store.list().unwrap().len(), 1);

        // Resume: save more messages back to same id
        let msgs2 = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi"),
            msg(Role::User, "second turn"),
            msg(Role::Assistant, "still here"),
        ];
        let id2 = store.save(Some(&id), &msgs2, &[], None).unwrap();

        // Must reuse the same id and not create a new slot
        assert_eq!(id, id2, "resumed save should reuse the same session id");
        assert_eq!(store.list().unwrap().len(), 1, "should still be exactly 1 session");

        // Loaded session should have the updated messages
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.messages.len(), 4);
    }

    #[test]
    fn todos_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);

        let todos = vec![Todo::new("Task A", None), Todo::new("Task B", Some("do the thing"))];
        let id = store.save(None, &[], &todos, None).unwrap();

        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.todos.len(), 2);
        assert_eq!(loaded.todos[0].title, "Task A");
        assert_eq!(loaded.todos[1].description.as_deref(), Some("do the thing"));
    }

    #[test]
    fn old_session_without_todos_loads_cleanly() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);

        // Write a session file that has no `todos` field
        let id = uuid::Uuid::new_v4().to_string();
        let path = dir.path().join("sessions").join(format!("{}.json", id));
        let raw = serde_json::json!({
            "id": id,
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "messages": []
        });
        std::fs::write(&path, raw.to_string()).unwrap();

        let loaded = store.load(&id).unwrap();
        assert!(loaded.todos.is_empty(), "old session should deserialize with empty todos");
    }

    #[test]
    fn two_separate_new_sessions_create_two_slots() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);

        let msgs = vec![msg(Role::User, "turn 1"), msg(Role::Assistant, "a")];
        store.save(None, &msgs, &[], None).unwrap();

        let msgs2 = vec![msg(Role::User, "turn 2"), msg(Role::Assistant, "b")];
        store.save(None, &msgs2, &[], None).unwrap();

        assert_eq!(store.list().unwrap().len(), 2, "two fresh sessions should produce two slots");
    }
}

fn format_age(dt: DateTime<Utc>) -> String {
    let secs = (Utc::now() - dt).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
