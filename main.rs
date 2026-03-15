// src/main.rs
//
// todoist-nvim — Rust binary for the Neovim Todoist plugin.
//
// API target: Todoist Unified API v1
//   https://developer.todoist.com/api/v1/
//
// Key differences vs the deprecated REST v2 API:
//   • Base URL  : https://api.todoist.com/api/v1/   (was /rest/v2/)
//   • JSON keys : camelCase                         (were snake_case)
//   • List responses: { "results": […], "next_cursor": "…" | null }
//                                                   (were bare arrays)
//   • Sort key on tasks/projects: childOrder        (was order)
//   • Sort key on sections      : sectionOrder      (was order)
//
// Responsibilities:
//   1. Read TODOIST_API_TOKEN from the environment.
//   2. Exhaust all cursor-paginated pages for projects, sections, and tasks.
//   3. Build a project → section → task hierarchy (with sub-task nesting).
//   4. Render the hierarchy as Markdown and print it to stdout.
//
// All errors are written to stderr; the process exits with code 1, letting
// the Lua layer surface them via vim.notify.

use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;

// ─── Todoist API v1 data model ───────────────────────────────────────────────
//
// All JSON field names are camelCase in API v1.
// Serde's `rename_all = "camelCase"` maps them to idiomatic Rust snake_case.

/// Paginated list response returned by every GET-all endpoint in API v1.
/// `next_cursor` is `None` (JSON `null`) on the final page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    results: Vec<T>,
    next_cursor: Option<String>,
}

/// A Todoist project (top-level container).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    name: String,
    /// Display order within its level.
    #[serde(default)]
    child_order: i64,
    /// `true` for the built-in Inbox project — we still show it.
    #[serde(default)]
    inbox_project: bool,
}

/// A section inside a project.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Section {
    id: String,
    project_id: String,
    name: String,
    /// Display order within the project.
    #[serde(default)]
    section_order: i64,
}

/// A single active task.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Task {
    id: String,
    content: String,
    project_id: String,
    /// `null` when there is no section.
    section_id: Option<String>,
    /// Set only for sub-tasks; `null` for root tasks.
    parent_id: Option<String>,
    /// Display order within its parent container.
    #[serde(default)]
    child_order: i64,
}

impl Task {
    /// Normalised section key: treat `None` and `""` the same way.
    fn section_key(&self) -> Option<&str> {
        self.section_id.as_deref().filter(|s| !s.is_empty())
    }

    /// Normalised parent key: treat `None` and `""` the same way.
    fn parent_key(&self) -> Option<&str> {
        self.parent_id.as_deref().filter(|s| !s.is_empty())
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() {
    match run() {
        Ok(output) => print!("{}", output),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

// ─── Orchestration ───────────────────────────────────────────────────────────

fn run() -> Result<String, String> {
    let token = env::var("TODOIST_API_TOKEN").map_err(|_| {
        "TODOIST_API_TOKEN is not set.\n\
         Add it to your shell profile or Neovim config, e.g.:\n\
         \n\
         export TODOIST_API_TOKEN=\"your_token_here\"\n\
         \n\
         Get your token at: https://app.todoist.com/app/settings/integrations/developer"
            .to_string()
    })?;

    let client = Client::new();

    let projects = api_get_all::<Project>(
        &client,
        &token,
        "https://api.todoist.com/api/v1/projects",
    )?;
    let sections = api_get_all::<Section>(
        &client,
        &token,
        "https://api.todoist.com/api/v1/sections",
    )?;
    let tasks = api_get_all::<Task>(
        &client,
        &token,
        "https://api.todoist.com/api/v1/tasks",
    )?;

    if tasks.is_empty() {
        return Ok(
            "# 🎉 No active tasks\n\n\
             Your Todoist is empty — enjoy the peace!\n"
                .to_string(),
        );
    }

    render(projects, sections, tasks)
}

// ─── HTTP / pagination helpers ────────────────────────────────────────────────

/// Fetch every page of a cursor-paginated API v1 endpoint and collect all items.
///
/// API v1 returns `{ "results": [...], "next_cursor": "<opaque>" | null }`.
/// When `next_cursor` is null the final page has been reached.
/// We send `cursor=<value>` as a query parameter to advance to the next page.
fn api_get_all<T>(client: &Client, token: &str, base_url: &str) -> Result<Vec<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let mut all_items: Vec<T> = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut req = client
            .get(base_url)
            .header("Authorization", format!("Bearer {}", token));

        if let Some(ref c) = cursor {
            req = req.query(&[("cursor", c.as_str())]);
        }

        let resp = req
            .send()
            .map_err(|e| format!("Network error while fetching {}:\n{}", base_url, e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(http_error_message(status.as_u16(), base_url));
        }

        let page: Page<T> = resp.json().map_err(|e| {
            format!("Failed to parse Todoist API response from {}:\n{}", base_url, e)
        })?;

        all_items.extend(page.results);

        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    Ok(all_items)
}

/// Build a human-readable error message for a non-2xx HTTP response.
fn http_error_message(status: u16, url: &str) -> String {
    match status {
        401 => "Todoist API returned 401 Unauthorized.\n\
                Check that TODOIST_API_TOKEN is valid."
            .to_string(),
        403 => format!(
            "Todoist API returned 403 Forbidden for {}.\n\
             Your token may lack the required scope (data:read).",
            url
        ),
        429 => "Todoist API returned 429 Too Many Requests.\n\
                Wait a moment and try again."
            .to_string(),
        500..=599 => format!(
            "Todoist API returned a server error ({}) for {}.\n\
             Try again in a few minutes.",
            status, url
        ),
        code => format!("Todoist API error {} for {}", code, url),
    }
}

// ─── Hierarchy construction & rendering ──────────────────────────────────────

fn render(
    mut projects: Vec<Project>,
    sections: Vec<Section>,
    mut tasks: Vec<Task>,
) -> Result<String, String> {
    // Sort projects by their display order.
    projects.sort_by_key(|p| p.child_order);

    // Sections grouped by project_id, sorted by section_order.
    let mut sections_by_project: HashMap<&str, Vec<&Section>> = HashMap::new();
    for sec in &sections {
        sections_by_project
            .entry(sec.project_id.as_str())
            .or_default()
            .push(sec);
    }
    for v in sections_by_project.values_mut() {
        v.sort_by_key(|s| s.section_order);
    }

    // Sort tasks before partitioning so the partition preserves order.
    tasks.sort_by_key(|t| t.child_order);

    // Partition into root tasks (no parent) and sub-tasks.
    let (top_level, sub_tasks): (Vec<&Task>, Vec<&Task>) =
        tasks.iter().partition(|t| t.parent_key().is_none());

    // sub-task map: parent_id → sorted Vec<child>
    let mut subtask_map: HashMap<&str, Vec<&Task>> = HashMap::new();
    for task in &sub_tasks {
        if let Some(pid) = task.parent_key() {
            subtask_map.entry(pid).or_default().push(task);
        }
    }
    // Already sorted by child_order from the tasks sort above, but ensure it.
    for v in subtask_map.values_mut() {
        v.sort_by_key(|t| t.child_order);
    }

    // Group root tasks: project_id → Option<section_id> → Vec<task>.
    // `None` section key = "not assigned to any section".
    let mut by_project: HashMap<&str, HashMap<Option<&str>, Vec<&Task>>> = HashMap::new();
    for task in &top_level {
        by_project
            .entry(task.project_id.as_str())
            .or_default()
            .entry(task.section_key())
            .or_default()
            .push(task);
    }

    // ── Render ───────────────────────────────────────────────────────────────
    let mut out = String::with_capacity(4096);

    for project in &projects {
        let pid = project.id.as_str();

        // Skip projects with no active tasks at all.
        let Some(section_map) = by_project.get(pid) else {
            continue;
        };

        out.push_str(&format!("# {}\n\n", project.name));

        // 1. Root tasks not in any section.
        if let Some(unsectioned) = section_map.get(&None) {
            for task in unsectioned {
                render_task(&mut out, task, &subtask_map, 0);
            }
            out.push('\n');
        }

        // 2. Tasks grouped under sections, in section display order.
        if let Some(secs) = sections_by_project.get(pid) {
            for sec in secs {
                let sid = sec.id.as_str();
                let Some(tasks_in_section) = section_map.get(&Some(sid)) else {
                    continue;
                };
                if tasks_in_section.is_empty() {
                    continue;
                }
                out.push_str(&format!("## {}\n\n", sec.name));
                for task in tasks_in_section {
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

/// Recursively render a task and its children at the given indent `depth`.
///
/// depth 0  →  `- [ ] Task title`
/// depth 1  →  `    - [ ] Sub-task`   (4-space indent per level)
fn render_task(
    out: &mut String,
    task: &Task,
    subtask_map: &HashMap<&str, Vec<&Task>>,
    depth: usize,
) {
    let indent = "    ".repeat(depth);
    out.push_str(&format!("{}- [ ] {}\n", indent, task.content));

    if let Some(children) = subtask_map.get(task.id.as_str()) {
        for child in children {
            render_task(out, child, subtask_map, depth + 1);
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Builders ─────────────────────────────────────────────────────────────

    fn make_project(id: &str, name: &str, order: i64) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            child_order: order,
            inbox_project: false,
        }
    }

    fn make_section(id: &str, project_id: &str, name: &str, order: i64) -> Section {
        Section {
            id: id.to_string(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            section_order: order,
        }
    }

    fn make_task(id: &str, content: &str, project_id: &str) -> Task {
        Task {
            id: id.to_string(),
            content: content.to_string(),
            project_id: project_id.to_string(),
            section_id: None,
            parent_id: None,
            child_order: 0,
        }
    }

    fn make_subtask(id: &str, content: &str, project_id: &str, parent_id: &str) -> Task {
        Task {
            id: id.to_string(),
            content: content.to_string(),
            project_id: project_id.to_string(),
            section_id: None,
            parent_id: Some(parent_id.to_string()),
            child_order: 0,
        }
    }

    fn make_sectioned_task(
        id: &str,
        content: &str,
        project_id: &str,
        section_id: &str,
    ) -> Task {
        Task {
            id: id.to_string(),
            content: content.to_string(),
            project_id: project_id.to_string(),
            section_id: Some(section_id.to_string()),
            parent_id: None,
            child_order: 0,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn render_handles_all_task_list() {
        // render() itself is still callable; an empty project map just returns
        // the "all empty" message.
        let out = render(
            vec![make_project("p1", "Work", 1)],
            vec![],
            vec![],
        );
        assert!(out.is_ok());
    }

    #[test]
    fn unsectioned_tasks_render_directly_under_project() {
        let out = render(
            vec![make_project("p1", "Work", 1)],
            vec![],
            vec![make_task("t1", "Fix bug", "p1")],
        )
        .unwrap();

        assert!(out.contains("# Work"));
        assert!(out.contains("- [ ] Fix bug"));
        // No H2 heading expected
        assert!(!out.contains("## "));
    }

    #[test]
    fn sectioned_tasks_render_under_h2() {
        let out = render(
            vec![make_project("p1", "Work", 1)],
            vec![make_section("s1", "p1", "Backend", 1)],
            vec![make_sectioned_task("t1", "Fix auth", "p1", "s1")],
        )
        .unwrap();

        assert!(out.contains("# Work"));
        assert!(out.contains("## Backend"));
        assert!(out.contains("- [ ] Fix auth"));
    }

    #[test]
    fn subtasks_are_indented() {
        let out = render(
            vec![make_project("p1", "Work", 1)],
            vec![],
            vec![
                make_task("t1", "Parent task", "p1"),
                make_subtask("t2", "Child task", "p1", "t1"),
            ],
        )
        .unwrap();

        assert!(out.contains("- [ ] Parent task"));
        assert!(out.contains("    - [ ] Child task"));
    }

    #[test]
    fn projects_with_no_tasks_are_omitted() {
        let out = render(
            vec![
                make_project("p1", "Empty Project", 1),
                make_project("p2", "Active Project", 2),
            ],
            vec![],
            vec![make_task("t1", "A task", "p2")],
        )
        .unwrap();

        assert!(!out.contains("# Empty Project"));
        assert!(out.contains("# Active Project"));
    }

    #[test]
    fn section_id_null_treated_as_no_section() {
        let task = Task {
            id: "t1".to_string(),
            content: "Task".to_string(),
            project_id: "p1".to_string(),
            section_id: None, // null from API v1
            parent_id: None,
            child_order: 0,
        };
        assert_eq!(task.section_key(), None);
    }

    #[test]
    fn section_id_empty_string_treated_as_no_section() {
        // Defensive: guard against any stale empty-string responses.
        let task = Task {
            id: "t1".to_string(),
            content: "Task".to_string(),
            project_id: "p1".to_string(),
            section_id: Some("".to_string()),
            parent_id: None,
            child_order: 0,
        };
        assert_eq!(task.section_key(), None);
    }

    #[test]
    fn pagination_page_deserialises() {
        // Verify the Page<T> shape matches what the API actually returns.
        let json = r#"{"results":[{"id":"t1","content":"Task","projectId":"p1","sectionId":null,"parentId":null,"childOrder":1}],"nextCursor":null}"#;
        let page: Page<Task> = serde_json::from_str(json).expect("deserialise failed");
        assert_eq!(page.results.len(), 1);
        assert_eq!(page.results[0].content, "Task");
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn pagination_cursor_deserialises() {
        let json = r#"{"results":[],"nextCursor":"abc123"}"#;
        let page: Page<Task> = serde_json::from_str(json).expect("deserialise failed");
        assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
    }
}
