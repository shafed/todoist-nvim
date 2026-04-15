// src/sync.rs — diff + execute, now with Reopen support.
//
// Conflict policy: BUFFER WINS.
// Reopen detection: if snapshot.checked == false AND buffer shows [x] → Complete.
//                   if snapshot.checked == true  AND buffer shows [ ] → Reopen.
//                   (Snapshot stores checked=false for active tasks.)

use crate::api;
use crate::fetch::read_token;
use crate::models::{BufferTask, SnapshotTask, SyncOp, SyncSummary};
use crate::parser;
use crate::snapshot;
use reqwest::blocking::Client;
use std::collections::{HashMap, HashSet};
use std::fs;

pub fn run(buffer_file: &str) -> Result<(), String> {
    let content = fs::read_to_string(buffer_file)
        .map_err(|e| format!("Cannot read buffer file '{}': {}", buffer_file, e))?;

    let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err("Buffer is empty — nothing to sync".to_string());
    }

    let parse_result = parser::parse(&lines);
    let buffer_tasks = parse_result.tasks;
    let mut summary  = SyncSummary::default();

    for w in parse_result.warnings { summary.warnings.push(w); }

    if buffer_tasks.is_empty() {
        summary.warnings.push(
            "No task lines found. Run :TodoistOpen to refresh.".to_string()
        );
        summary.print();
        return Ok(());
    }

    let snapshot_tasks: HashMap<String, SnapshotTask> = match snapshot::load()? {
        Some(snap) => snap.tasks,
        None => {
            summary.warnings.push(
                "No snapshot found — run :TodoistOpen first to establish a baseline.".to_string()
            );
            HashMap::new()
        }
    };

    let original_snapshot = snapshot_tasks.clone();

    let mut ops = compute_ops(&buffer_tasks, &snapshot_tasks, &mut summary);

    // Detect reorder: compare buffer position order vs snapshot child_order.
    let reorder_op = compute_reorder(&buffer_tasks, &snapshot_tasks);
    if let Some(op) = reorder_op {
        ops.push((usize::MAX, op));
    }

    if ops.is_empty() && summary.warnings.is_empty() {
        println!("No changes detected.");
        return Ok(());
    }

    let token  = read_token()?;
    let client = Client::new();
    let mut new_id_map: HashMap<usize, String> = HashMap::new();

    execute_ops(ops, &client, &token, &buffer_tasks, &mut new_id_map, &mut summary);

    if summary.errors.is_empty() {
        let new_snap_tasks = rebuild_snapshot(&original_snapshot, &buffer_tasks, &new_id_map);
        let snap = crate::models::Snapshot::new(new_snap_tasks);
        if let Err(e) = snapshot::save(&snap) {
            summary.warnings.push(format!("Could not save post-sync snapshot: {}", e));
        }
    } else {
        summary.warnings.push(
            "Snapshot not updated due to sync errors; next sync may show phantom changes.".to_string()
        );
    }

    summary.print();
    Ok(())
}

// ─── Reopen (restore from active buffer) ─────────────────────────────────────

/// Called when the user wants to restore a completed task by ID.
pub fn run_reopen(task_id: &str) -> Result<(), String> {
    let token  = read_token()?;
    let client = Client::new();
    api::reopen_task(&client, &token, task_id)
        .map_err(|e| format!("Reopen failed: {}", e))?;
    println!("Task {} reopened.", task_id);
    Ok(())
}

// ─── Diff ─────────────────────────────────────────────────────────────────────

fn compute_ops(
    buffer_tasks: &[BufferTask],
    snapshot: &HashMap<String, SnapshotTask>,
    summary: &mut SyncSummary,
) -> Vec<(usize, SyncOp)> {
    let buffer_ids: HashSet<String> = buffer_tasks
        .iter().filter_map(|t| t.id.clone()).collect();

    let mut ops: Vec<(usize, SyncOp)> = Vec::new();

    for (idx, task) in buffer_tasks.iter().enumerate() {
        match &task.id {
            None => {
                let project_id = match &task.project_id {
                    Some(pid) => pid.clone(),
                    None => {
                        summary.warnings.push(format!(
                            "Line {}: '{}' has no project — skipped", task.line_num, task.content
                        ));
                        summary.skipped += 1;
                        continue;
                    }
                };
                ops.push((idx, SyncOp::Create {
                    content: task.content.clone(),
                    project_id,
                    section_id: task.section_id.clone(),
                    parent_id: task.parent_id.clone(),
                    due: task.due.clone(),
                }));
            }

            Some(id) => {
                let snap = match snapshot.get(id.as_str()) {
                    Some(s) => s,
                    None => {
                        if task.checked {
                            ops.push((idx, SyncOp::Complete { id: id.clone(), content: task.content.clone() }));
                        } else {
                            summary.warnings.push(format!(
                                "Line {}: '{}' (id:{}) not in snapshot — skipped",
                                task.line_num, task.content, id
                            ));
                            summary.skipped += 1;
                        }
                        continue;
                    }
                };

                match (snap.checked, task.checked) {
                    (false, true)  => ops.push((idx, SyncOp::Complete { id: id.clone(), content: task.content.clone() })),
                    (true,  false) => ops.push((idx, SyncOp::Reopen   { id: id.clone(), content: task.content.clone() })),
                    _ => {
                        if task.content != snap.content || task.due != snap.due {
                            ops.push((idx, SyncOp::Update {
                                id: id.clone(),
                                old_content: snap.content.clone(),
                                new_content: task.content.clone(),
                                old_due: snap.due.clone(),
                                new_due: task.due.clone(),
                            }));
                        }
                        let proj_changed = task.project_id.as_deref() != Some(snap.project_id.as_str());
                        let sec_changed  = task.section_id != snap.section_id;
                        let par_changed  = task.parent_id != snap.parent_id;
                        if proj_changed || sec_changed || par_changed {
                            if let Some(pid) = &task.project_id {
                                ops.push((idx, SyncOp::Move {
                                    id: id.clone(),
                                    content: task.content.clone(),
                                    project_id: pid.clone(),
                                    section_id: task.section_id.clone(),
                                    parent_id: task.parent_id.clone(),
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // Deletions: in snapshot but absent from buffer.
    for (snap_id, snap_task) in snapshot {
        if !buffer_ids.contains(snap_id.as_str()) {
            ops.push((usize::MAX, SyncOp::Delete {
                id: snap_id.clone(),
                content: snap_task.content.clone(),
            }));
        }
    }

    ops
}

// ─── Reorder detection ──────────────────────────────────────────────────────

/// Compare buffer ordering against snapshot child_order for each sibling group.
/// A sibling group is tasks sharing the same (project_id, section_id, parent_id).
/// If buffer order differs from snapshot order, emit a single Reorder op.
fn compute_reorder(
    buffer_tasks: &[BufferTask],
    snapshot: &HashMap<String, SnapshotTask>,
) -> Option<SyncOp> {
    // Group existing (has ID) buffer tasks by sibling key, preserving buffer order.
    let mut sibling_groups: HashMap<(Option<String>, Option<String>, Option<String>), Vec<&BufferTask>> =
        HashMap::new();

    for task in buffer_tasks {
        if let Some(ref id) = task.id {
            if snapshot.contains_key(id.as_str()) {
                let key = (
                    task.project_id.clone(),
                    task.section_id.clone(),
                    task.parent_id.clone(),
                );
                sibling_groups.entry(key).or_default().push(task);
            }
        }
    }

    let mut reorder_items: Vec<(String, i64)> = Vec::new();

    for (_key, tasks) in &sibling_groups {
        if tasks.len() < 2 {
            continue;
        }

        // Get the snapshot ordering for these tasks.
        let mut snap_ordered: Vec<(&str, i64)> = tasks
            .iter()
            .filter_map(|t| {
                let id = t.id.as_ref()?;
                let snap = snapshot.get(id.as_str())?;
                Some((id.as_str(), snap.child_order))
            })
            .collect();
        snap_ordered.sort_by_key(|&(_, order)| order);
        let snap_id_order: Vec<&str> = snap_ordered.iter().map(|&(id, _)| id).collect();

        // Buffer order (already in buffer sequence).
        let buf_id_order: Vec<&str> = tasks
            .iter()
            .filter_map(|t| t.id.as_deref())
            .collect();

        if snap_id_order != buf_id_order {
            // Assign new child_order values based on buffer position.
            for (i, task) in tasks.iter().enumerate() {
                if let Some(ref id) = task.id {
                    reorder_items.push((id.clone(), i as i64 + 1));
                }
            }
        }
    }

    if reorder_items.is_empty() {
        None
    } else {
        Some(SyncOp::Reorder { items: reorder_items })
    }
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
            SyncOp::Create { content, project_id, section_id, parent_id, due } => {
                let resolved = resolve_parent_id(&parent_id, buf_idx, buffer_tasks, new_id_map, summary);
                match api::create_task(client, token, &content, &project_id,
                                       section_id.as_deref(), resolved.as_deref(),
                                       due.as_deref()) {
                    Ok(new_id) => { new_id_map.insert(buf_idx, new_id); summary.created += 1; }
                    Err(e)     => summary.errors.push(format!("Create '{}': {}", content, e)),
                }
            }
            SyncOp::Update { id, old_content, new_content, old_due: _, new_due } => {
                match api::update_task(client, token, &id, &new_content, new_due.as_deref()) {
                    Ok(())  => summary.updated += 1,
                    Err(e)  => summary.errors.push(format!("Update '{}' → '{}': {}", old_content, new_content, e)),
                }
            }
            SyncOp::Complete { id, content } => {
                match api::close_task(client, token, &id) {
                    Ok(())  => summary.completed += 1,
                    Err(e)  => summary.errors.push(format!("Complete '{}': {}", content, e)),
                }
            }
            SyncOp::Reopen { id, content } => {
                match api::reopen_task(client, token, &id) {
                    Ok(())  => summary.reopened += 1,
                    Err(e)  => summary.errors.push(format!("Reopen '{}': {}", content, e)),
                }
            }
            SyncOp::Delete { id, content } => {
                match api::delete_task(client, token, &id) {
                    Ok(())  => summary.deleted += 1,
                    Err(e)  => summary.errors.push(format!("Delete '{}': {}", content, e)),
                }
            }
            SyncOp::Reorder { items } => {
                match api::reorder_tasks(client, token, &items) {
                    Ok(())  => summary.reordered += items.len(),
                    Err(e)  => summary.errors.push(format!("Reorder: {}", e)),
                }
            }
            SyncOp::Move { id, content, project_id, section_id, parent_id } => {
                let resolved_parent = resolve_parent_id(&parent_id, buf_idx, buffer_tasks, new_id_map, summary);
                match api::move_task(
                    client, token, &id,
                    Some(project_id.as_str()),
                    section_id.as_deref(),
                    resolved_parent.as_deref(),
                ) {
                    Ok(())  => summary.moved += 1,
                    Err(e)  => summary.errors.push(format!("Move '{}': {}", content, e)),
                }
            }
        }
    }
}

fn resolve_parent_id(
    parent_id: &Option<String>,
    current_idx: usize,
    buffer_tasks: &[BufferTask],
    new_id_map: &HashMap<usize, String>,
    summary: &mut SyncSummary,
) -> Option<String> {
    let pid = parent_id.as_ref()?;
    for (idx, task) in buffer_tasks.iter().enumerate() {
        if idx >= current_idx { break; }
        if task.id.as_deref() == Some(pid.as_str()) { return Some(pid.clone()); }
        if task.id.is_none() {
            if let Some(new_id) = new_id_map.get(&idx) {
                let current_level = buffer_tasks[current_idx].indent_level;
                if task.indent_level + 1 == current_level {
                    return Some(new_id.clone());
                }
            }
        }
    }
    summary.warnings.push(format!(
        "Parent task {} not found — child will be placed at root.",
        pid
    ));
    None
}

// ─── Snapshot rebuild ────────────────────────────────────────────────────────

fn rebuild_snapshot(
    original: &HashMap<String, SnapshotTask>,
    buffer_tasks: &[BufferTask],
    new_id_map: &HashMap<usize, String>,
) -> HashMap<String, SnapshotTask> {
    let buffer_ids: HashSet<String> = buffer_tasks
        .iter()
        .filter_map(|t| t.id.clone())
        .collect();

    // Start from a clone; we will mutate it to reflect post-sync state.
    let mut result = original.clone();

    // Remove tasks that were in the snapshot but absent from the buffer (user deleted them).
    result.retain(|id, _| buffer_ids.contains(id.as_str()));

    for (buf_idx, task) in buffer_tasks.iter().enumerate() {
        match &task.id {
            Some(id) => {
                if task.checked {
                    result.remove(id.as_str());
                } else if let Some(entry) = result.get_mut(id.as_str()) {
                    entry.content    = task.content.clone();
                    entry.due        = task.due.clone();
                    entry.project_id = task.project_id.clone().unwrap_or_else(|| entry.project_id.clone());
                    entry.section_id = task.section_id.clone();
                    entry.parent_id  = task.parent_id.clone();
                    entry.checked    = false;
                }
            }
            None => {
                if let Some(server_id) = new_id_map.get(&buf_idx) {
                    let new_task = SnapshotTask {
                        id:         server_id.clone(),
                        content:    task.content.clone(),
                        project_id: task.project_id.clone().unwrap_or_default(),
                        section_id: task.section_id.clone(),
                        parent_id:  task.parent_id.clone(),
                        due:        task.due.clone(),
                        checked:    false,
                        child_order: 0,
                    };
                    result.insert(server_id.clone(), new_task);
                }
                // If no entry in new_id_map the create failed; skip.
            }
        }
    }

    result
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BufferTask, SnapshotTask};
    use std::collections::HashMap;

    fn bt(id: Option<&str>, content: &str, checked: bool) -> BufferTask {
        BufferTask {
            id: id.map(|s| s.to_string()), content: content.to_string(),
            checked, indent_level: 0,
            project_id: Some("p1".to_string()), section_id: None,
            parent_id: None, line_num: 1, due: None,
        }
    }
    fn snap(id: &str, content: &str, checked: bool) -> (String, SnapshotTask) {
        (id.to_string(), SnapshotTask {
            id: id.to_string(), content: content.to_string(),
            project_id: "p1".to_string(), section_id: None,
            parent_id: None, checked, child_order: 0, due: None,
        })
    }
    fn snap_map(v: Vec<(&str, &str, bool)>) -> HashMap<String, SnapshotTask> {
        v.into_iter().map(|(id, c, ch)| snap(id, c, ch)).collect()
    }

    #[test]
    fn detects_complete() {
        let ops = compute_ops(
            &[bt(Some("t1"), "Task", true)],
            &snap_map(vec![("t1", "Task", false)]),
            &mut SyncSummary::default(),
        );
        assert!(matches!(ops[0].1, SyncOp::Complete { .. }));
    }

    #[test]
    fn detects_reopen() {
        let ops = compute_ops(
            &[bt(Some("t1"), "Task", false)],
            &snap_map(vec![("t1", "Task", true)]),
            &mut SyncSummary::default(),
        );
        assert!(matches!(ops[0].1, SyncOp::Reopen { .. }));
    }

    #[test]
    fn detects_create() {
        let ops = compute_ops(
            &[bt(None, "New", false)],
            &snap_map(vec![]),
            &mut SyncSummary::default(),
        );
        assert!(matches!(ops[0].1, SyncOp::Create { .. }));
    }

    #[test]
    fn detects_delete() {
        let ops = compute_ops(
            &[],
            &snap_map(vec![("t1", "Old", false)]),
            &mut SyncSummary::default(),
        );
        assert!(matches!(ops[0].1, SyncOp::Delete { .. }));
    }

    #[test]
    fn detects_update() {
        let ops = compute_ops(
            &[bt(Some("t1"), "New content", false)],
            &snap_map(vec![("t1", "Old content", false)]),
            &mut SyncSummary::default(),
        );
        assert!(matches!(ops[0].1, SyncOp::Update { .. }));
    }

    #[test]
    fn no_op_when_unchanged() {
        let ops = compute_ops(
            &[bt(Some("t1"), "Same", false)],
            &snap_map(vec![("t1", "Same", false)]),
            &mut SyncSummary::default(),
        );
        assert!(ops.is_empty());
    }

    fn snap_ordered(id: &str, content: &str, child_order: i64) -> (String, SnapshotTask) {
        (id.to_string(), SnapshotTask {
            id: id.to_string(), content: content.to_string(),
            project_id: "p1".to_string(), section_id: None,
            parent_id: None, checked: false, child_order, due: None,
        })
    }

    #[test]
    fn detects_reorder() {
        // Snapshot order: t1(order=1), t2(order=2)
        // Buffer order: t2, t1 — swapped
        let snap: HashMap<String, SnapshotTask> = vec![
            snap_ordered("t1", "Task 1", 1),
            snap_ordered("t2", "Task 2", 2),
        ].into_iter().collect();

        let buf = vec![
            bt(Some("t2"), "Task 2", false),
            bt(Some("t1"), "Task 1", false),
        ];

        let result = compute_reorder(&buf, &snap);
        assert!(result.is_some());
        if let Some(SyncOp::Reorder { items }) = result {
            assert_eq!(items.len(), 2);
        }
    }

    fn bt_with_due(id: Option<&str>, content: &str, due: Option<&str>) -> BufferTask {
        BufferTask {
            id: id.map(|s| s.to_string()), content: content.to_string(),
            checked: false, indent_level: 0,
            project_id: Some("p1".to_string()), section_id: None,
            parent_id: None, line_num: 1,
            due: due.map(|s| s.to_string()),
        }
    }
    fn snap_with_due(id: &str, content: &str, due: Option<&str>) -> (String, SnapshotTask) {
        (id.to_string(), SnapshotTask {
            id: id.to_string(), content: content.to_string(),
            project_id: "p1".to_string(), section_id: None,
            parent_id: None, checked: false, child_order: 0,
            due: due.map(|s| s.to_string()),
        })
    }

    #[test]
    fn detects_date_added() {
        // Snapshot has no date; buffer now has one → should emit Update
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_with_due("t1", "Task", None)].into_iter().collect();
        let buf = vec![bt_with_due(Some("t1"), "Task", Some("2026-04-20"))];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0].1, SyncOp::Update { new_due, .. } if new_due.as_deref() == Some("2026-04-20")));
    }

    #[test]
    fn detects_date_changed() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_with_due("t1", "Task", Some("2026-04-20"))].into_iter().collect();
        let buf = vec![bt_with_due(Some("t1"), "Task", Some("2026-05-01"))];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0].1, SyncOp::Update { new_due, old_due, .. }
            if new_due.as_deref() == Some("2026-05-01") && old_due.as_deref() == Some("2026-04-20")));
    }

    #[test]
    fn detects_date_removed() {
        // Snapshot has date; buffer has cleared it → Update with new_due=None
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_with_due("t1", "Task", Some("2026-04-20"))].into_iter().collect();
        let buf = vec![bt_with_due(Some("t1"), "Task", None)];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0].1, SyncOp::Update { new_due, .. } if new_due.is_none()));
    }

    #[test]
    fn no_op_when_date_unchanged() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_with_due("t1", "Task", Some("2026-04-20"))].into_iter().collect();
        let buf = vec![bt_with_due(Some("t1"), "Task", Some("2026-04-20"))];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert!(ops.is_empty());
    }

    #[test]
    fn no_reorder_when_same_order() {
        let snap: HashMap<String, SnapshotTask> = vec![
            snap_ordered("t1", "Task 1", 1),
            snap_ordered("t2", "Task 2", 2),
        ].into_iter().collect();

        let buf = vec![
            bt(Some("t1"), "Task 1", false),
            bt(Some("t2"), "Task 2", false),
        ];

        let result = compute_reorder(&buf, &snap);
        assert!(result.is_none());
    }

    #[test]
    fn rebuild_snapshot_applies_create() {
        let original = snap_map(vec![]);
        let buf = vec![bt(None, "Brand new", false)];
        let mut id_map = HashMap::new();
        id_map.insert(0usize, "server-99".to_string());
        let result = rebuild_snapshot(&original, &buf, &id_map);
        assert!(result.contains_key("server-99"));
        assert_eq!(result["server-99"].content, "Brand new");
    }

    #[test]
    fn rebuild_snapshot_applies_update() {
        let original = snap_map(vec![("t1", "Old content", false)]);
        let buf = vec![bt(Some("t1"), "New content", false)];
        let result = rebuild_snapshot(&original, &buf, &HashMap::new());
        assert_eq!(result["t1"].content, "New content");
    }

    #[test]
    fn rebuild_snapshot_removes_completed() {
        let original = snap_map(vec![("t1", "Task", false)]);
        let buf = vec![bt(Some("t1"), "Task", true)];
        let result = rebuild_snapshot(&original, &buf, &HashMap::new());
        assert!(!result.contains_key("t1"));
    }

    #[test]
    fn rebuild_snapshot_removes_deleted() {
        let original = snap_map(vec![("t1", "Task A", false), ("t2", "Task B", false)]);
        // Buffer only has t1; t2 was deleted by the user.
        let buf = vec![bt(Some("t1"), "Task A", false)];
        let result = rebuild_snapshot(&original, &buf, &HashMap::new());
        assert!(result.contains_key("t1"));
        assert!(!result.contains_key("t2"));
    }

    #[test]
    fn rebuild_snapshot_preserves_unchanged() {
        let original = snap_map(vec![("t1", "Same", false)]);
        let buf = vec![bt(Some("t1"), "Same", false)];
        let result = rebuild_snapshot(&original, &buf, &HashMap::new());
        assert_eq!(result["t1"].content, original["t1"].content);
        assert_eq!(result["t1"].checked, original["t1"].checked);
    }

    fn bt_proj(id: Option<&str>, content: &str, project_id: &str, section_id: Option<&str>, parent_id: Option<&str>) -> BufferTask {
        BufferTask {
            id: id.map(|s| s.to_string()), content: content.to_string(),
            checked: false, indent_level: 0,
            project_id: Some(project_id.to_string()),
            section_id: section_id.map(|s| s.to_string()),
            parent_id: parent_id.map(|s| s.to_string()),
            line_num: 1, due: None,
        }
    }

    fn snap_proj(id: &str, content: &str, project_id: &str, section_id: Option<&str>, parent_id: Option<&str>) -> (String, SnapshotTask) {
        (id.to_string(), SnapshotTask {
            id: id.to_string(), content: content.to_string(),
            project_id: project_id.to_string(),
            section_id: section_id.map(|s| s.to_string()),
            parent_id: parent_id.map(|s| s.to_string()),
            checked: false, child_order: 0, due: None,
        })
    }

    #[test]
    fn detects_project_move() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_proj("t1", "Task", "p1", None, None)].into_iter().collect();
        let buf = vec![bt_proj(Some("t1"), "Task", "p2", None, None)];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert!(ops.iter().any(|(_, op)| matches!(op, SyncOp::Move { project_id, .. } if project_id == "p2")));
    }

    #[test]
    fn detects_section_move() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_proj("t1", "Task", "p1", None, None)].into_iter().collect();
        let buf = vec![bt_proj(Some("t1"), "Task", "p1", Some("s1"), None)];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert!(ops.iter().any(|(_, op)| matches!(op, SyncOp::Move { section_id, .. } if section_id.as_deref() == Some("s1"))));
    }

    #[test]
    fn detects_parent_move() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_proj("t1", "Task", "p1", None, None)].into_iter().collect();
        let buf = vec![bt_proj(Some("t1"), "Task", "p1", None, Some("t0"))];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert!(ops.iter().any(|(_, op)| matches!(op, SyncOp::Move { parent_id, .. } if parent_id.as_deref() == Some("t0"))));
    }

    #[test]
    fn no_move_when_unchanged() {
        let snap: HashMap<String, SnapshotTask> =
            vec![snap_proj("t1", "Task", "p1", None, None)].into_iter().collect();
        let buf = vec![bt_proj(Some("t1"), "Task", "p1", None, None)];
        let ops = compute_ops(&buf, &snap, &mut SyncSummary::default());
        assert!(!ops.iter().any(|(_, op)| matches!(op, SyncOp::Move { .. })));
    }

    #[test]
    fn resolve_parent_id_returns_none_for_dangling() {
        let buf = vec![bt(Some("t1"), "Task", false)];
        let parent_id = Some("pX".to_string());
        let mut summary = SyncSummary::default();
        let result = resolve_parent_id(&parent_id, 1, &buf, &HashMap::new(), &mut summary);
        assert!(result.is_none());
        assert!(!summary.warnings.is_empty());
    }

    #[test]
    fn rebuild_snapshot_applies_move() {
        let original: HashMap<String, SnapshotTask> =
            vec![snap_proj("t1", "Task", "p1", None, None)].into_iter().collect();
        let buf = vec![bt_proj(Some("t1"), "Task", "p2", None, None)];
        let result = rebuild_snapshot(&original, &buf, &HashMap::new());
        assert_eq!(result["t1"].project_id, "p2");
    }
}
