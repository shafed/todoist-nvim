// src/models.rs
//
// Shared data types used across fetch, parser, sync, and snapshot modules.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Todoist REST API v1 shapes ──────────────────────────────────────────────

/// Generic cursor-paginated response wrapper used by every list endpoint.
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

// ─── Snapshot (persisted state at last fetch) ────────────────────────────────

/// One task as recorded in the snapshot file.
/// Only fields relevant for diff detection are stored.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotTask {
    pub id: String,
    pub content: String,
    pub project_id: String,
    pub section_id: Option<String>,
    pub parent_id: Option<String>,
}

/// Full snapshot written after every successful fetch.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// ISO-8601 UTC timestamp of when the snapshot was taken.
    pub fetched_at: String,
    /// task_id → SnapshotTask
    pub tasks: HashMap<String, SnapshotTask>,
}

impl Snapshot {
    pub fn new(tasks: HashMap<String, SnapshotTask>) -> Self {
        // Minimal UTC timestamp without chrono dep.
        Snapshot {
            fetched_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
            tasks,
        }
    }
}

// ─── Buffer parsing types ────────────────────────────────────────────────────

/// A task as parsed from the Neovim markdown buffer.
#[derive(Debug, Clone)]
pub struct BufferTask {
    /// Todoist task ID extracted from `<!-- id:XXXX -->`.
    /// `None` means this is a newly added task (must be created).
    pub id: Option<String>,
    /// Task text with the ID comment stripped.
    pub content: String,
    /// `true` if the checkbox is `[x]`.
    pub checked: bool,
    /// 0 = root task, 1 = first-level subtask, etc.
    pub indent_level: usize,
    /// Todoist project ID from the enclosing `# Name <!-- project:ID -->` heading.
    pub project_id: Option<String>,
    /// Todoist section ID from the enclosing `## Name <!-- section:ID -->` heading.
    pub section_id: Option<String>,
    /// Resolved parent Todoist ID (for root tasks this is None).
    /// Set during a second pass after parsing is complete.
    pub parent_id: Option<String>,
    /// 1-based line number in the buffer (useful for error messages).
    pub line_num: usize,
}

// ─── Sync operations ─────────────────────────────────────────────────────────

/// An operation the sync engine wants to apply to Todoist.
#[derive(Debug)]
pub enum SyncOp {
    Create {
        /// Human-readable description (for the summary).
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
    Delete {
        id: String,
        content: String,
    },
}

/// Summary printed to stdout after sync completes.
#[derive(Debug, Default)]
pub struct SyncSummary {
    pub created: usize,
    pub updated: usize,
    pub completed: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncSummary {
    pub fn print(&self) {
        println!("Sync complete:");
        println!("  Created:   {}", self.created);
        println!("  Updated:   {}", self.updated);
        println!("  Completed: {}", self.completed);
        println!("  Deleted:   {}", self.deleted);
        if self.skipped > 0 {
            println!("  Skipped:   {}", self.skipped);
        }
        for w in &self.warnings {
            println!("  WARNING: {}", w);
        }
        for e in &self.errors {
            println!("  ERROR: {}", e);
        }
    }

    pub fn has_changes(&self) -> bool {
        self.created + self.updated + self.completed + self.deleted > 0
    }
}
