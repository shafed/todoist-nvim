use crate::api;
use crate::models::{Snapshot, SnapshotTask, Task};
use crate::snapshot;
use std::collections::HashMap;
use std::thread;

pub fn run() -> Result<(), String> {
    let token = read_token()?;

    // ── Parallel fetch: 3 threads, ~3× faster ──────────────────────────────
    let t = token.clone();
    let projects_handle = thread::spawn(move || {
        let client = api::make_client()?;
        api::fetch_projects(&client, &t)
    });

    let t = token.clone();
    let sections_handle = thread::spawn(move || {
        let client = api::make_client()?;
        api::fetch_sections(&client, &t)
    });

    let t = token.clone();
    let tasks_handle = thread::spawn(move || {
        let client = api::make_client()?;
        api::fetch_tasks(&client, &t)
    });

    let mut projects = projects_handle
        .join()
        .map_err(|_| "projects thread panicked".to_string())??;
    let sections = sections_handle
        .join()
        .map_err(|_| "sections thread panicked".to_string())??;
    let tasks = tasks_handle
        .join()
        .map_err(|_| "tasks thread panicked".to_string())??;

    // ── Empty check ─────────────────────────────────────────────────────────
    if tasks.is_empty() {
        println!("# 🎉 No active tasks\n\nYour Todoist is empty — enjoy the peace!");
        return Ok(());
    }

    // ── Snapshot ─────────────────────────────────────────────────────────────
    let snap_tasks: HashMap<String, SnapshotTask> = tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                SnapshotTask {
                    id: t.id.clone(),
                    content: t.content.clone(),
                    project_id: t.project_id.clone(),
                    section_id: t.section_id.clone(),
                    parent_id: t.parent_id.clone(),
                },
            )
        })
        .collect();
    let snap = Snapshot::new(snap_tasks);
    if let Err(e) = snapshot::save(&snap) {
        eprintln!("Warning: could not save snapshot: {}", e);
    }

    // ── Render ───────────────────────────────────────────────────────────────
    projects.sort_by_key(|p| p.child_order);
    let output = render(&projects, &sections, &tasks)?;
    print!("{}", output);
    Ok(())
}

pub fn read_token() -> Result<String, String> {
    std::env::var("TODOIST_API_TOKEN").map_err(|_| {
        "TODOIST_API_TOKEN is not set.\n\
         Add it to your shell profile or Neovim config:\n\
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
        sections_by_project
            .entry(sec.project_id.as_str())
            .or_default()
            .push(sec);
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

    let mut out = String::with_capacity(4096);

    for project in projects {
        let pid = project.id.as_str();
        let Some(section_map) = by_project.get(pid) else {
            continue;
        };

        out.push_str(&format!(
            "# {} <!-- project:{} -->\n\n",
            project.name, project.id
        ));

        if let Some(unsectioned) = section_map.get(&None) {
            for task in unsectioned {
                render_task(&mut out, task, &subtask_map, 0);
            }
            out.push('\n');
        }

        if let Some(secs) = sections_by_project.get(pid) {
            for sec in secs {
                let sid = sec.id.as_str();
                let Some(tasks_in_sec) = section_map.get(&Some(sid)) else {
                    continue;
                };
                if tasks_in_sec.is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    "## {} <!-- section:{} -->\n\n",
                    sec.name, sec.id
                ));
                for task in tasks_in_sec {
                    render_task(&mut out, task, &subtask_map, 0);
                }
                out.push('\n');
            }
        }
    }

    if out.is_empty() {
        return Ok("# No active tasks\n\nAll projects appear to be empty.\n".to_string());
    }

    Ok(out)
}

fn render_task(
    out: &mut String,
    task: &Task,
    subtask_map: &HashMap<&str, Vec<&Task>>,
    depth: usize,
) {
    let indent = "    ".repeat(depth);
    out.push_str(&format!(
        "{}- [ ] {} <!-- id:{} -->\n",
        indent, task.content, task.id
    ));
    if let Some(children) = subtask_map.get(task.id.as_str()) {
        for child in children {
            render_task(out, child, subtask_map, depth + 1);
        }
    }
}
