use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::Message;

const SESSIONS_PER_PAGE: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<Message>,
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
    pub fn save(&self, session_id: Option<&str>, messages: &[Message]) -> Result<String> {
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
        };

        fs::write(&path, serde_json::to_string_pretty(&session)?)?;

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
        fs::write(self.index_path(), serde_json::to_string_pretty(&index)?)?;
        Ok(())
    }

    /// Look up a session by 1-based display number (sorted by most recent).
    pub fn get_by_number(&self, n: usize) -> Result<SessionMeta> {
        let sessions = self.list()?;
        sessions.into_iter().nth(n.saturating_sub(1))
            .ok_or_else(|| anyhow::anyhow!("No session with number {}", n))
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
