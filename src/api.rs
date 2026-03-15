// src/api.rs
//
// All Todoist API v1 HTTP calls.
//
// Fetch helpers return deserialized data.
// Mutation helpers return Ok(()) or Err(String).
// The token is never logged — errors only mention the endpoint path, not the
// Authorization header value.

use crate::models::{Page, Project, Section, Task};
use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde_json::json;

const BASE: &str = "https://api.todoist.com/api/v1";

// ─── Shared error formatter ───────────────────────────────────────────────────

fn http_err(status: u16, path: &str) -> String {
    match status {
        401 => "Todoist API: 401 Unauthorized — check TODOIST_API_TOKEN".to_string(),
        403 => format!("Todoist API: 403 Forbidden — {}", path),
        404 => format!("Todoist API: 404 Not Found — {}", path),
        429 => "Todoist API: 429 Too Many Requests — wait a moment".to_string(),
        s if s >= 500 => format!("Todoist API: server error {} — {}", s, path),
        s => format!("Todoist API: error {} — {}", s, path),
    }
}

// ─── Paginated GET ─────────────────────────────────────────────────────────────

/// Exhaust all cursor pages for a list endpoint and return collected items.
pub fn get_all<T: DeserializeOwned>(
    client: &Client,
    token: &str,
    path: &str,
) -> Result<Vec<T>, String> {
    let url = format!("{}/{}", BASE, path);
    let mut items: Vec<T> = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut req = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token));
        if let Some(ref c) = cursor {
            req = req.query(&[("cursor", c.as_str())]);
        }

        let resp = req
            .send()
            .map_err(|e| format!("Network error ({}): {}", path, e))?;

        let status = resp.status().as_u16();
        if status < 200 || status >= 300 {
            return Err(http_err(status, path));
        }

        let page: Page<T> = resp
            .json()
            .map_err(|e| format!("Parse error ({}): {}", path, e))?;

        items.extend(page.results);

        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    Ok(items)
}

// ─── High-level fetch helpers ─────────────────────────────────────────────────

pub fn fetch_projects(client: &Client, token: &str) -> Result<Vec<Project>, String> {
    get_all(client, token, "projects")
}

pub fn fetch_sections(client: &Client, token: &str) -> Result<Vec<Section>, String> {
    get_all(client, token, "sections")
}

pub fn fetch_tasks(client: &Client, token: &str) -> Result<Vec<Task>, String> {
    get_all(client, token, "tasks")
}

// ─── Mutations ────────────────────────────────────────────────────────────────

/// Create a new task. Returns the created task's ID.
pub fn create_task(
    client: &Client,
    token: &str,
    content: &str,
    project_id: &str,
    section_id: Option<&str>,
    parent_id: Option<&str>,
) -> Result<String, String> {
    let mut body = json!({
        "content": content,
        "project_id": project_id,
    });

    if let Some(sid) = section_id {
        body["section_id"] = json!(sid);
    }
    if let Some(pid) = parent_id {
        body["parent_id"] = json!(pid);
    }

    let resp = client
        .post(format!("{}/tasks", BASE))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| format!("Network error (create task): {}", e))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, "tasks [POST]"));
    }

    // Extract the new task ID from the response.
    let created: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Parse error (create task response): {}", e))?;

    created["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Create task: response missing 'id' field".to_string())
}

/// Update a task's content (title).
pub fn update_task(
    client: &Client,
    token: &str,
    task_id: &str,
    content: &str,
) -> Result<(), String> {
    let resp = client
        .post(format!("{}/tasks/{}", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "content": content }))
        .send()
        .map_err(|e| format!("Network error (update {}): {}", task_id, e))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, &format!("tasks/{} [POST]", task_id)));
    }
    Ok(())
}

/// Mark a task as complete (close it).
pub fn close_task(client: &Client, token: &str, task_id: &str) -> Result<(), String> {
    let resp = client
        .post(format!("{}/tasks/{}/close", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (close {}): {}", task_id, e))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, &format!("tasks/{}/close [POST]", task_id)));
    }
    Ok(())
}

/// Delete a task permanently.
pub fn delete_task(client: &Client, token: &str, task_id: &str) -> Result<(), String> {
    let resp = client
        .delete(format!("{}/tasks/{}", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (delete {}): {}", task_id, e))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        // 404 means already deleted — treat as success.
        if status == 404 {
            return Ok(());
        }
        return Err(http_err(status, &format!("tasks/{} [DELETE]", task_id)));
    }
    Ok(())
}
