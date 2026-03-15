// src/fetch.rs
//
// # Buffer structure (updated)
//
//   # Todoist Tasks                    ← H1: fixed buffer title
//
//   ## Inbox <!-- project:ID -->       ← H2: project name
//
//   - [ ] Task <!-- id:ID -->
//       - [ ] Subtask <!-- id:ID -->
//
//   ## Work <!-- project:ID -->
//
//   ### Backend <!-- section:ID -->    ← H3: section (optional)
//
//   - [ ] Fix bug <!-- id:ID -->
//
// H1 is a fixed title — never changes.
// H2 = project.  H3 = section (omitted when project has no sections).
// IDs are in HTML comments; Lua hides them with extmarks.
//
// # Subtask indentation
//
// Each indent level uses exactly 2 spaces (renders cleanly in markdown
// and is unambiguous to parse: 2 spaces = 1 level).

use crate::api;
use crate::models::{Snapshot, SnapshotTask, Task};
use crate::snapshot;
use reqwest::blocking::Client;
use std::collections::HashMap;
use std::thread;

pub fn run() -> Result<(), String> {
    let token = read_token()?;

    // Spawn three independent threads — projects / sections / tasks are
    // completely independent, so we fetch them in parallel.
    // Each thread creates its own Client (cheap: no shared connection pool
    // with blocking clients, but we avoid contention).
    //
    // Wall-clock time: max(t_projects, t_sections, t_tasks)
    // instead of t_projects + t_sections + t_tasks  (~3× faster).

    let t1 = token.clone();
    let projects_handle = thread::spawn(move || {
        api::fetch_projects(&Client::new(), &t1)
    });

    let t2 = token.clone();
    let sections_handle = thread::spawn(move || {
        api::fetch_sections(&Client::new(), &t2)
    });

    let t3 = token.clone();
    let tasks_handle = thread::spawn(move || {
        api::fetch_tasks(&Client::new(), &t3)
    });

    let mut projects = projects_handle
        .join()
        .map_err(|_| "projects fetch thread panicked".to_string())??;
    let sections = sections_handle
        .join()
        .map_err(|_| "sections fetch thread panicked".to_string())??;
    let tasks = tasks_handle
        .join()
        .map_err(|_| "tasks fetch thread panicked".to_string())??;

    if tasks.is_empty() {
        println!("# Todoist Tasks\n\n*No active tasks — enjoy the peace!*");
        return Ok(());
    }

    // Save snapshot before rendering.
    let snap_tasks: HashMap<String, SnapshotTask> = tasks.iter().map(|t| {
        (t.id.clone(), SnapshotTask {
            id: t.id.clone(),
            content: t.content.clone(),
            project_id: t.project_id.clone(),
            section_id: t.section_id.clone(),
            parent_id: t.parent_id.clone(),
            checked: false, // active tasks are unchecked
        })
    }).collect();

    if let Err(e) = snapshot::save(&Snapshot::new(snap_tasks)) {
        eprintln!("Warning: could not save snapshot: {}", e);
    }

    projects.sort_by_key(|p| p.child_order);
    let output = render(&projects, &sections, &tasks)?;
    print!("{}", output);
    Ok(())
}

/// Fetch and render the completed tasks buffer.
pub fn run_completed() -> Result<(), String> {
    let token = read_token()?;

    // projects and completed tasks are independent — fetch in parallel.
    let t1 = token.clone();
    let projects_handle  = thread::spawn(move || api::fetch_projects(&Client::new(), &t1));

    let t2 = token.clone();
    let completed_handle = thread::spawn(move || api::fetch_completed_tasks(&Client::new(), &t2));

    let projects  = projects_handle.join()
        .map_err(|_| "projects thread panicked".to_string())??;
    let completed = completed_handle.join()
        .map_err(|_| "completed tasks thread panicked".to_string())??;

    let project_names: HashMap<&str, &str> = projects.iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    if completed.is_empty() {
        println!("# Completed Tasks\n\n*No completed tasks in the last 30 days.*");
        return Ok(());
    }

    // Group by project.
    let mut by_project: HashMap<&str, Vec<&crate::models::CompletedTask>> = HashMap::new();
    for task in &completed {
        by_project.entry(task.project_id.as_str()).or_default().push(task);
    }

    let mut out = String::from("# Completed Tasks\n\n");

    for project in &projects {
        let pid = project.id.as_str();
        let Some(tasks) = by_project.get(pid) else { continue; };
        let proj_name = project_names.get(pid).unwrap_or(&project.name.as_str());
        out.push_str(&format!("## {} <!-- project:{} -->\n\n", proj_name, pid));
        for task in tasks {
            out.push_str(&format!("- [x] {} <!-- id:{} -->\n", task.content, task.id));
        }
        out.push('\n');
    }

    print!("{}", out);
    Ok(())
}

pub fn read_token() -> Result<String, String> {
    std::env::var("TODOIST_API_TOKEN").map_err(|_| {
        "TODOIST_API_TOKEN is not set.\n\
         export TODOIST_API_TOKEN=\"your_token_here\"\n\
         Get your token: https://app.todoist.com/app/settings/integrations/developer"
            .to_string()
    })
}

// ─── Renderer ────────────────────────────────────────────────────────────────

fn render(
    projects: &[crate::models::Project],
    sections: &[crate::models::Section],
    tasks: &[Task],
) -> Result<String, String> {
    let mut sections_by_project: HashMap<&str, Vec<&crate::models::Section>> = HashMap::new();
    for sec in sections {
        sections_by_project.entry(sec.project_id.as_str()).or_default().push(sec);
    }
    for v in sections_by_project.values_mut() {
        v.sort_by_key(|s| s.section_order);
    }

    let mut sorted_tasks = tasks.to_vec();
    sorted_tasks.sort_by_key(|t| t.child_order);

    let (top_level, sub_tasks): (Vec<&Task>, Vec<&Task>) =
        sorted_tasks.iter().partition(|t| t.parent_key().is_none());

    let mut subtask_map: HashMap<&str, Vec<&Task>> = HashMap::new();
    for task in &sub_tasks {
        if let Some(pid) = task.parent_key() {
            subtask_map.entry(pid).or_default().push(task);
        }
    }
    for v in subtask_map.values_mut() {
        v.sort_by_key(|t| t.child_order);
    }

    let mut by_project: HashMap<&str, HashMap<Option<&str>, Vec<&Task>>> = HashMap::new();
    for task in &top_level {
        by_project
            .entry(task.project_id.as_str())
            .or_default()
            .entry(task.section_key())
            .or_default()
            .push(task);
    }

    // H1 = fixed buffer title
    let mut out = String::from("# Todoist Tasks\n\n");

    for project in projects {
        let pid = project.id.as_str();
        let Some(section_map) = by_project.get(pid) else { continue; };

        // H2 = project name
        out.push_str(&format!("## {} <!-- project:{} -->\n\n", project.name, project.id));

        // Unsectioned tasks directly under the project.
        if let Some(unsectioned) = section_map.get(&None) {
            for task in unsectioned {
                render_task(&mut out, task, &subtask_map, 0);
            }
            out.push('\n');
        }

        // H3 = section name (only when the project actually has sections).
        if let Some(secs) = sections_by_project.get(pid) {
            for sec in secs {
                let sid = sec.id.as_str();
                let Some(tasks_in_sec) = section_map.get(&Some(sid)) else { continue; };
                if tasks_in_sec.is_empty() { continue; }
                out.push_str(&format!("### {} <!-- section:{} -->\n\n", sec.name, sec.id));
                for task in tasks_in_sec {
                    render_task(&mut out, task, &subtask_map, 0);
                }
                out.push('\n');
            }
        }
    }

    if out.trim() == "# Todoist Tasks" {
        return Ok("# Todoist Tasks\n\n*All projects appear to be empty.*\n".to_string());
    }

    Ok(out)
}

/// Render a task recursively.
/// Indent: 2 spaces per level — unambiguous and markdown-friendly.
fn render_task(
    out: &mut String,
    task: &Task,
    subtask_map: &HashMap<&str, Vec<&Task>>,
    depth: usize,
) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{}- [ ] {} <!-- id:{} -->\n", indent, task.content, task.id));
    if let Some(children) = subtask_map.get(task.id.as_str()) {
        for child in children {
            render_task(out, child, subtask_map, depth + 1);
        }
    }
}
