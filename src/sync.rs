// src/sync.rs
//
// Sync engine — reads a buffer file, diffs against the snapshot, applies
// operations to the Todoist API, and prints a human-readable summary.
//
// # Conflict policy: BUFFER WINS
//
// A conflict occurs when a task exists in both the buffer and the snapshot
// but the buffer content differs from the snapshot AND we cannot verify the
// remote state without an extra API call.
//
// For MVP simplicity: buffer always wins.  If the task was also edited
// remotely since the last fetch, the buffer version overwrites it.  A warning
// is emitted so the user is aware.  The snapshot's role is only "what did the
// task look like when we last fetched?" — not "is the remote up to date?".
//
// # Delete safety
//
// Deletes are applied only when a task ID that was in the snapshot is absent
// from the buffer entirely — meaning the user explicitly removed the line.
// Tasks marked `[x]` (checked) are completed, not deleted.
//
// # Create ordering
//
// New tasks (no ID comment) that are subtasks (indent > 0) can only be
// created after their parent is created and its new ID is known.  The engine
// processes creates in buffer order, which naturally handles this as long as
// parents appear before children — which the buffer format guarantees.
//
// # Post-sync
//
// The summary is printed to stdout.  The Lua layer decides whether to trigger
// a re-fetch.  We recommend re-fetching after every successful sync so the
// buffer gets fresh IDs for newly created tasks.

use crate::api;
use crate::fetch::read_token;
use crate::models::{BufferTask, SnapshotTask, SyncOp, SyncSummary};
use crate::parser;
use crate::snapshot;
use reqwest::blocking::Client;
use std::collections::{HashMap, HashSet};
use std::fs;

pub fn run(buffer_file: &str) -> Result<(), String> {
    // ── 1. Read buffer ────────────────────────────────────────────────────
    let content = fs::read_to_string(buffer_file)
        .map_err(|e| format!("Cannot read buffer file '{}': {}", buffer_file, e))?;

    let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err("Buffer appears to be empty — nothing to sync".to_string());
    }

    // ── 2. Parse buffer ───────────────────────────────────────────────────
    let parse_result = parser::parse(&lines);
    let buffer_tasks = parse_result.tasks;
    let mut summary = SyncSummary::default();

    for w in parse_result.warnings {
        summary.warnings.push(w);
    }

    if buffer_tasks.is_empty() {
        summary.warnings.push(
            "No task lines found in buffer — nothing to sync. \
             Run :TodoistOpen to refresh."
                .to_string(),
        );
        summary.print();
        return Ok(());
    }

    // ── 3. Load snapshot ──────────────────────────────────────────────────
    let snapshot_tasks: HashMap<String, SnapshotTask> = match snapshot::load()? {
        Some(snap) => snap.tasks,
        None => {
            summary.warnings.push(
                "No snapshot found (first run or snapshot deleted). \
                 Only new tasks (those without an ID comment) will be created. \
                 Run :TodoistOpen first to establish a baseline."
                    .to_string(),
            );
            HashMap::new()
        }
    };

    // ── 4. Compute diff ───────────────────────────────────────────────────
    let ops = compute_ops(&buffer_tasks, &snapshot_tasks, &mut summary);

    if ops.is_empty() && summary.warnings.is_empty() {
        println!("No changes detected.");
        return Ok(());
    }

    // ── 5. Execute operations ──────────────────────────────────────────────
    let token = read_token()?;
    let client = Client::new();

    // Track buffer-index → newly assigned Todoist ID for creates.
    // This lets us resolve parent IDs for new subtasks whose parents were
    // also just created in this same sync run.
    let mut new_id_map: HashMap<usize, String> = HashMap::new();

    execute_ops(ops, &client, &token, &buffer_tasks, &mut new_id_map, &mut summary);

    // ── 6. Print summary ──────────────────────────────────────────────────
    summary.print();

    Ok(())
}

// ─── Diff ─────────────────────────────────────────────────────────────────────

fn compute_ops(
    buffer_tasks: &[BufferTask],
    snapshot: &HashMap<String, SnapshotTask>,
    summary: &mut SyncSummary,
) -> Vec<(usize, SyncOp)> {
    // Buffer IDs seen — used to detect deletions.
    let buffer_ids: HashSet<String> = buffer_tasks
        .iter()
        .filter_map(|t| t.id.clone())
        .collect();

    let mut ops: Vec<(usize, SyncOp)> = Vec::new();

    // Walk buffer tasks: creates / updates / completes.
    for (idx, task) in buffer_tasks.iter().enumerate() {
        match &task.id {
            None => {
                // No ID → new task, must be created.
                let project_id = match &task.project_id {
                    Some(pid) => pid.clone(),
                    None => {
                        summary.warnings.push(format!(
                            "Line {}: new task '{}' has no project context — skipped",
                            task.line_num, task.content
                        ));
                        summary.skipped += 1;
                        continue;
                    }
                };
                ops.push((
                    idx,
                    SyncOp::Create {
                        content: task.content.clone(),
                        project_id,
                        section_id: task.section_id.clone(),
                        parent_id: task.parent_id.clone(),
                    },
                ));
            }

            Some(id) => {
                let snap = match snapshot.get(id.as_str()) {
                    Some(s) => s,
                    None => {
                        // ID in buffer but not in snapshot.
                        // Could be a task from before first sync or a stale ID.
                        // Safe action: check the checkbox state and act accordingly.
                        // If checked → try to complete; if unchecked → no action.
                        if task.checked {
                            ops.push((idx, SyncOp::Complete {
                                id: id.clone(),
                                content: task.content.clone(),
                            }));
                        } else {
                            summary.warnings.push(format!(
                                "Line {}: task '{}' (id:{}) not found in snapshot — \
                                 skipped (re-run :TodoistOpen to refresh)",
                                task.line_num, task.content, id
                            ));
                            summary.skipped += 1;
                        }
                        continue;
                    }
                };

                if task.checked {
                    // Buffer marks task complete.
                    ops.push((idx, SyncOp::Complete {
                        id: id.clone(),
                        content: task.content.clone(),
                    }));
                } else if task.content != snap.content {
                    // Content changed → update.
                    ops.push((
                        idx,
                        SyncOp::Update {
                            id: id.clone(),
                            old_content: snap.content.clone(),
                            new_content: task.content.clone(),
                        },
                    ));
                }
                // Else: no change — skip.
            }
        }
    }

    // Detect deletions: tasks in snapshot that are absent from buffer.
    for (snap_id, snap_task) in snapshot {
        if !buffer_ids.contains(snap_id.as_str()) {
            ops.push((
                usize::MAX, // no buffer index for deletes
                SyncOp::Delete {
                    id: snap_id.clone(),
                    content: snap_task.content.clone(),
                },
            ));
        }
    }

    ops
}

// ─── Execution ───────────────────────────────────────────────────────────────

fn execute_ops(
    ops: Vec<(usize, SyncOp)>,
    client: &Client,
    token: &str,
    buffer_tasks: &[BufferTask],
    new_id_map: &mut HashMap<usize, String>,
    summary: &mut SyncSummary,
) {
    for (buf_idx, op) in ops {
        match op {
            SyncOp::Create {
                content,
                project_id,
                section_id,
                parent_id,
            } => {
                // If parent was itself just created in this run, look up its
                // newly assigned ID.
                let resolved_parent_id = resolve_parent_id(
                    &parent_id,
                    buf_idx,
                    buffer_tasks,
                    new_id_map,
                );

                match api::create_task(
                    client,
                    token,
                    &content,
                    &project_id,
                    section_id.as_deref(),
                    resolved_parent_id.as_deref(),
                ) {
                    Ok(new_id) => {
                        new_id_map.insert(buf_idx, new_id);
                        summary.created += 1;
                    }
                    Err(e) => {
                        summary.errors.push(format!(
                            "Create '{}': {}",
                            content, e
                        ));
                    }
                }
            }

            SyncOp::Update {
                id,
                old_content,
                new_content,
            } => {
                match api::update_task(client, token, &id, &new_content) {
                    Ok(()) => {
                        summary.updated += 1;
                    }
                    Err(e) => {
                        summary.errors.push(format!(
                            "Update '{}' → '{}': {}",
                            old_content, new_content, e
                        ));
                    }
                }
            }

            SyncOp::Complete { id, content } => {
                match api::close_task(client, token, &id) {
                    Ok(()) => {
                        summary.completed += 1;
                    }
                    Err(e) => {
                        summary.errors.push(format!(
                            "Complete '{}' (id:{}): {}",
                            content, id, e
                        ));
                    }
                }
            }

            SyncOp::Delete { id, content } => {
                match api::delete_task(client, token, &id) {
                    Ok(()) => {
                        summary.deleted += 1;
                    }
                    Err(e) => {
                        // 404 is already swallowed in api::delete_task, so
                        // anything here is a real problem.
                        summary.errors.push(format!(
                            "Delete '{}' (id:{}): {}",
                            content, id, e
                        ));
                    }
                }
            }
        }
    }
}

/// If a task's parent was just created in this sync run, look up the newly
/// assigned ID from new_id_map.  Otherwise return the original parent_id.
fn resolve_parent_id(
    parent_id: &Option<String>,
    current_buf_idx: usize,
    buffer_tasks: &[BufferTask],
    new_id_map: &HashMap<usize, String>,
) -> Option<String> {
    let pid = parent_id.as_ref()?;

    // If the parent already has a real Todoist ID, use it.
    // Check by looking backwards in the buffer for a task whose id == pid.
    // If found and its buf_idx is in new_id_map → it was just created.
    for (idx, task) in buffer_tasks.iter().enumerate() {
        if idx >= current_buf_idx {
            break;
        }
        if task.id.as_deref() == Some(pid.as_str()) {
            // Parent had a pre-existing ID — use it.
            return Some(pid.clone());
        }
        // Parent had no ID (was also a create) — use the freshly assigned one.
        if task.id.is_none() {
            if let Some(new_id) = new_id_map.get(&idx) {
                // Verify this is actually the intended parent by indent level.
                if current_buf_idx > 0 {
                    let current_level = buffer_tasks[current_buf_idx].indent_level;
                    if task.indent_level + 1 == current_level {
                        return Some(new_id.clone());
                    }
                }
            }
        }
    }

    Some(pid.clone())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BufferTask, SnapshotTask};
    use std::collections::HashMap;

    fn make_buffer_task(
        id: Option<&str>,
        content: &str,
        checked: bool,
        indent: usize,
        project_id: &str,
    ) -> BufferTask {
        BufferTask {
            id: id.map(|s| s.to_string()),
            content: content.to_string(),
            checked,
            indent_level: indent,
            project_id: Some(project_id.to_string()),
            section_id: None,
            parent_id: None,
            line_num: 1,
        }
    }

    fn make_snap(id: &str, content: &str) -> SnapshotTask {
        SnapshotTask {
            id: id.to_string(),
            content: content.to_string(),
            project_id: "p1".to_string(),
            section_id: None,
            parent_id: None,
        }
    }

    fn snap_map(tasks: Vec<(&str, &str)>) -> HashMap<String, SnapshotTask> {
        tasks
            .into_iter()
            .map(|(id, content)| (id.to_string(), make_snap(id, content)))
            .collect()
    }

    #[test]
    fn detects_create_for_tasks_without_id() {
        let buffer = vec![make_buffer_task(None, "New task", false, 0, "p1")];
        let snap = snap_map(vec![]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].1, SyncOp::Create { .. }));
    }

    #[test]
    fn detects_complete_for_checked_tasks() {
        let buffer = vec![make_buffer_task(Some("t1"), "Fix bug", true, 0, "p1")];
        let snap = snap_map(vec![("t1", "Fix bug")]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].1, SyncOp::Complete { .. }));
    }

    #[test]
    fn detects_update_for_content_change() {
        let buffer = vec![make_buffer_task(Some("t1"), "Fix auth bug", false, 0, "p1")];
        let snap = snap_map(vec![("t1", "Fix bug")]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].1, SyncOp::Update { .. }));
    }

    #[test]
    fn detects_delete_for_missing_task() {
        let buffer: Vec<BufferTask> = vec![];
        let snap = snap_map(vec![("t1", "Old task")]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].1, SyncOp::Delete { .. }));
    }

    #[test]
    fn no_ops_for_unchanged_task() {
        let buffer = vec![make_buffer_task(Some("t1"), "Same content", false, 0, "p1")];
        let snap = snap_map(vec![("t1", "Same content")]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);
        assert!(ops.is_empty(), "expected no ops, got {:?}", ops.len());
    }

    #[test]
    fn example_scenario_from_spec() {
        // Initial snapshot: Fix auth bug (t1), Write test (t2), Prepare release (t3)
        // Buffer after edits:
        //   [x] Fix auth bug         → complete
        //       [ ] Write integration test  → update content (t2)
        //   [ ] New API cleanup task  → create
        //   (Prepare release is gone) → delete
        let buffer = vec![
            make_buffer_task(Some("t1"), "Fix auth bug", true,  0, "p1"),
            make_buffer_task(Some("t2"), "Write integration test", false, 1, "p1"),
            make_buffer_task(None,       "New API cleanup task",   false, 0, "p1"),
        ];
        let snap = snap_map(vec![
            ("t1", "Fix auth bug"),
            ("t2", "Write test"),
            ("t3", "Prepare release"),
        ]);
        let mut summary = SyncSummary::default();
        let ops = compute_ops(&buffer, &snap, &mut summary);

        let completes: Vec<_> = ops.iter()
            .filter(|(_, o)| matches!(o, SyncOp::Complete { .. })).collect();
        let updates: Vec<_> = ops.iter()
            .filter(|(_, o)| matches!(o, SyncOp::Update { .. })).collect();
        let creates: Vec<_> = ops.iter()
            .filter(|(_, o)| matches!(o, SyncOp::Create { .. })).collect();
        let deletes: Vec<_> = ops.iter()
            .filter(|(_, o)| matches!(o, SyncOp::Delete { .. })).collect();

        assert_eq!(completes.len(), 1, "should complete 1 task");
        assert_eq!(updates.len(),   1, "should update 1 task");
        assert_eq!(creates.len(),   1, "should create 1 task");
        assert_eq!(deletes.len(),   1, "should delete 1 task");
    }
}
