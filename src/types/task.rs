use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

use super::{ComplexityLevel, ModelId};

/// Unique identifier for a task
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// A task to be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub description: String,
    pub prompt: String,
    pub complexity: Option<ComplexityLevel>,
    pub dependencies: Vec<TaskId>,
    pub estimated_tokens: Option<usize>,
    pub timeout: Option<std::time::Duration>,
    pub metadata: TaskMetadata,
}

impl Task {
    pub fn new(description: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: TaskId::new(),
            description: description.into(),
            prompt: prompt.into(),
            complexity: None,
            dependencies: Vec::new(),
            estimated_tokens: None,
            timeout: None,
            metadata: TaskMetadata::default(),
        }
    }
}

/// Metadata about a task
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskMetadata {
    #[serde(skip)]
    pub created_at: Option<Instant>,
    #[serde(skip)]
    pub started_at: Option<Instant>,
    #[serde(skip)]
    pub completed_at: Option<Instant>,
    pub assigned_to: Option<ModelId>,
    pub attempts: usize,
    pub tags: Vec<String>,
}

/// Result of executing a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: TaskId,
    pub success: bool,
    pub output: String,
    pub model: ModelId,
    pub tokens_used: Option<usize>,
    #[serde(skip)]
    pub duration: std::time::Duration,
    pub warnings: Vec<String>,
}

/// Status of a task in execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Assigned,
    Running,
    Completed,
    Failed,
    Cancelled,
}
