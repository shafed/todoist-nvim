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

    let ops = compute_ops(&buffer_tasks, &snapshot_tasks, &mut summary);

    if ops.is_empty() && summary.warnings.is_empty() {
        println!("No changes detected.");
        return Ok(());
    }

    let token  = read_token()?;
    let client = Client::new();
    let mut new_id_map: HashMap<usize, String> = HashMap::new();

    execute_ops(ops, &client, &token, &buffer_tasks, &mut new_id_map, &mut summary);
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
                        if task.content != snap.content {
                            ops.push((idx, SyncOp::Update {
                                id: id.clone(),
                                old_content: snap.content.clone(),
                                new_content: task.content.clone(),
                            }));
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
            SyncOp::Create { content, project_id, section_id, parent_id } => {
                let resolved = resolve_parent_id(&parent_id, buf_idx, buffer_tasks, new_id_map);
                match api::create_task(client, token, &content, &project_id,
                                       section_id.as_deref(), resolved.as_deref()) {
                    Ok(new_id) => { new_id_map.insert(buf_idx, new_id); summary.created += 1; }
                    Err(e)     => summary.errors.push(format!("Create '{}': {}", content, e)),
                }
            }
            SyncOp::Update { id, old_content, new_content } => {
                match api::update_task(client, token, &id, &new_content) {
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
        }
    }
}

fn resolve_parent_id(
    parent_id: &Option<String>,
    current_idx: usize,
    buffer_tasks: &[BufferTask],
    new_id_map: &HashMap<usize, String>,
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
    Some(pid.clone())
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
            parent_id: None, line_num: 1,
        }
    }
    fn snap(id: &str, content: &str, checked: bool) -> (String, SnapshotTask) {
        (id.to_string(), SnapshotTask {
            id: id.to_string(), content: content.to_string(),
            project_id: "p1".to_string(), section_id: None,
            parent_id: None, checked,
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
}
