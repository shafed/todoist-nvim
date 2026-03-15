// src/models.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Todoist API v1 shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub results: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(alias = "childOrder", default)]
    pub child_order: i64,
    #[allow(dead_code)]
    #[serde(alias = "inboxProject", default)]
    pub inbox_project: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Section {
    pub id: String,
    #[serde(alias = "projectId")]
    pub project_id: String,
    pub name: String,
    #[serde(alias = "sectionOrder", default)]
    pub section_order: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Task {
    pub id: String,
    pub content: String,
    #[serde(alias = "projectId")]
    pub project_id: String,
    #[serde(alias = "sectionId")]
    pub section_id: Option<String>,
    #[serde(alias = "parentId")]
    pub parent_id: Option<String>,
    #[serde(alias = "childOrder", default)]
    pub child_order: i64,
}

impl Task {
    pub fn section_key(&self) -> Option<&str> {
        self.section_id.as_deref().filter(|s| !s.is_empty())
    }
    pub fn parent_key(&self) -> Option<&str> {
        self.parent_id.as_deref().filter(|s| !s.is_empty())
    }
}

/// A completed task returned by the completed-tasks endpoints.
#[derive(Debug, Deserialize, Clone)]
pub struct CompletedTask {
    pub id: String,
    pub content: String,
    #[serde(alias = "projectId")]
    pub project_id: String,
    #[serde(alias = "completedAt")]
    pub completed_at: Option<String>,
}

// ─── Snapshot ────────────────────────────────────────────────────────────────

/// Snapshot of a single task at last-fetch time.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotTask {
    pub id: String,
    pub content: String,
    pub project_id: String,
    pub section_id: Option<String>,
    pub parent_id: Option<String>,
    /// Whether the task was checked (completed) in the buffer at snapshot time.
    /// Used to detect [x] → [ ] transitions (reopen).
    #[serde(default)]
    pub checked: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub fetched_at: String,
    pub tasks: HashMap<String, SnapshotTask>,
}

impl Snapshot {
    pub fn new(tasks: HashMap<String, SnapshotTask>) -> Self {
        Snapshot {
            fetched_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
            tasks,
        }
    }
}

// ─── Buffer parsing ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BufferTask {
    pub id: Option<String>,
    pub content: String,
    pub checked: bool,
    pub indent_level: usize,
    pub project_id: Option<String>,
    pub section_id: Option<String>,
    pub parent_id: Option<String>,
    pub line_num: usize,
}

// ─── Sync operations ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SyncOp {
    Create {
        content: String,
        project_id: String,
        section_id: Option<String>,
        parent_id: Option<String>,
    },
    Update {
        id: String,
        old_content: String,
        new_content: String,
    },
    Complete {
        id: String,
        content: String,
    },
    Reopen {
        id: String,
        content: String,
    },
    Delete {
        id: String,
        content: String,
    },
}

#[derive(Debug, Default)]
pub struct SyncSummary {
    pub created: usize,
    pub updated: usize,
    pub completed: usize,
    pub reopened: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncSummary {
    pub fn print(&self) {
        if !self.has_changes() && self.warnings.is_empty() && self.errors.is_empty() {
            println!("No changes detected.");
            return;
        }
        println!("Sync complete:");
        if self.created   > 0 { println!("  Created:   {}", self.created); }
        if self.updated   > 0 { println!("  Updated:   {}", self.updated); }
        if self.completed > 0 { println!("  Completed: {}", self.completed); }
        if self.reopened  > 0 { println!("  Reopened:  {}", self.reopened); }
        if self.deleted   > 0 { println!("  Deleted:   {}", self.deleted); }
        if self.skipped   > 0 { println!("  Skipped:   {}", self.skipped); }
        for w in &self.warnings { println!("  WARNING: {}", w); }
        for e in &self.errors   { println!("  ERROR: {}", e); }
    }

    pub fn has_changes(&self) -> bool {
        self.created + self.updated + self.completed + self.reopened + self.deleted > 0
    }
}
