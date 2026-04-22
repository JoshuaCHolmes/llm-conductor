use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::Message;

/// Context provided to a model
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    /// Core context (always included)
    pub core: CoreContext,
    /// Project-level context
    pub project: Option<ProjectContext>,
    /// Session-level context
    pub session: Option<SessionContext>,
    /// Task-specific context
    pub task: Option<TaskContext>,
}

impl Context {
    pub fn new(core: CoreContext) -> Self {
        Self {
            core,
            project: None,
            session: None,
            task: None,
        }
    }

    /// Estimate total token count
    pub fn token_count(&self) -> usize {
        let mut total = self.core.token_count();
        
        if let Some(ref project) = self.project {
            total += project.token_count();
        }
        if let Some(ref session) = self.session {
            total += session.token_count();
        }
        if let Some(ref task) = self.task {
            total += task.token_count();
        }
        
        total
    }

    /// Convert to messages for model
    pub fn to_messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        
        // System message with core context
        messages.push(Message::system(self.core.to_string()));
        
        // Add project context if present
        if let Some(ref project) = self.project {
            messages.push(Message::system(project.to_string()));
        }
        
        // Add session history
        if let Some(ref session) = self.session {
            messages.extend(session.history.clone());
        }
        
        messages
    }
}

/// Core context (always included, ~1K tokens)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoreContext {
    pub system_instructions: String,
    pub user_info: Option<UserInfo>,
    pub constraints: Vec<String>,
}

impl CoreContext {
    fn token_count(&self) -> usize {
        self.system_instructions.len() / 4 + 
        self.constraints.iter().map(|s| s.len() / 4).sum::<usize>()
    }

    fn to_string(&self) -> String {
        let mut parts = vec![self.system_instructions.clone()];
        
        if !self.constraints.is_empty() {
            parts.push(format!("Constraints:\n{}", self.constraints.join("\n")));
        }
        
        parts.join("\n\n")
    }
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub name: String,
    pub institution: Option<String>,
    pub preferences: HashMap<String, String>,
}

/// Project-level context (~5K tokens)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    pub name: String,
    pub description: String,
    pub architecture: String,
    pub key_files: Vec<String>,
    pub conventions: Vec<String>,
}

impl ProjectContext {
    fn token_count(&self) -> usize {
        (self.description.len() + self.architecture.len()) / 4 +
        self.key_files.len() * 20 +
        self.conventions.len() * 10
    }

    fn to_string(&self) -> String {
        format!(
            "Project: {}\n{}\n\nArchitecture:\n{}\n\nKey files: {}",
            self.name,
            self.description,
            self.architecture,
            self.key_files.join(", ")
        )
    }
}

/// Session-level context (~20K tokens)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionContext {
    pub history: Vec<Message>,
    pub decisions: Vec<String>,
}

impl SessionContext {
    fn token_count(&self) -> usize {
        self.history.iter().map(|m| m.token_count()).sum()
    }
}

/// Task-specific context (~50K tokens)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub relevant_code: Vec<CodeSnippet>,
    pub related_docs: Vec<String>,
}

impl TaskContext {
    fn token_count(&self) -> usize {
        self.relevant_code.iter().map(|c| c.content.len() / 4).sum::<usize>() +
        self.related_docs.iter().map(|d| d.len() / 4).sum::<usize>()
    }
}

/// Code snippet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSnippet {
    pub file_path: String,
    pub content: String,
    pub language: Option<String>,
}
