use crate::models::{CompletedTask, Page, Project, Section, Task};
use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;

const BASE: &str = "https://api.todoist.com/api/v1";
const TIMEOUT: Duration = Duration::from_secs(120);

/// Build a pre-configured client with generous timeout.
pub fn make_client() -> Result<Client, String> {
    Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

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

pub fn fetch_projects(client: &Client, token: &str) -> Result<Vec<Project>, String> {
    get_all(client, token, "projects")
}
pub fn fetch_sections(client: &Client, token: &str) -> Result<Vec<Section>, String> {
    get_all(client, token, "sections")
}
pub fn fetch_tasks(client: &Client, token: &str) -> Result<Vec<Task>, String> {
    get_all(client, token, "tasks")
}

/// Fetch recently completed tasks (last 30 days).
pub fn fetch_completed_tasks(client: &Client, token: &str) -> Result<Vec<CompletedTask>, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let since_secs = now.saturating_sub(30 * 24 * 3600);

    let since = format_date(since_secs);
    let until = format_date(now + 86400);

    let url = format!(
        "{}/tasks/completed/by_completion_date?since={}&until={}",
        BASE, since, until
    );

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (completed tasks): {}", e))?;

    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, "tasks/completed/by_completion_date"));
    }

    #[derive(serde::Deserialize)]
    struct CompletedPage {
        items: Vec<CompletedTask>,
        #[serde(default, rename = "nextCursor")]
        _next_cursor: Option<String>,
    }

    let page: CompletedPage = resp
        .json()
        .map_err(|e| format!("Parse error (completed tasks): {}", e))?;

    Ok(page.items)
}

fn format_date(unix_secs: u64) -> String {
    let days_since_epoch = unix_secs / 86400;
    let mut y = 1970u64;
    let mut d = days_since_epoch;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        y += 1;
    }
    let months = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1u64;
    for &days in &months {
        if d < days {
            break;
        }
        d -= days;
        m += 1;
    }
    format!("{:04}-{:02}-{:02}", y, m, d + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ─── Mutations ────────────────────────────────────────────────────────────────

/// Classification of a due string captured from the buffer.
pub enum DueKind<'a> {
    /// ISO date only: `"YYYY-MM-DD"`
    Date(&'a str),
    /// ISO date + time: `"YYYY-MM-DD HH:MM"`
    DateTime(&'a str),
    /// Natural-language string with an associated language code (`"en"` or `"ru"`).
    Natural { text: &'a str, lang: &'a str },
}

/// Classify a raw due string captured from the buffer.
///
/// - Matches `^\d{4}-\d{2}-\d{2}$`                    → `Date`
/// - Matches `^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$`        → `DateTime`
/// - Otherwise                                          → `Natural` (lang: "ru" if any
///   Cyrillic U+0400-U+04FF char is present, else "en")
pub fn classify_due(s: &str) -> DueKind<'_> {
    if is_iso_date(s) {
        return DueKind::Date(s);
    }
    if is_iso_datetime(s) {
        return DueKind::DateTime(s);
    }
    let lang = detect_lang(s);
    DueKind::Natural { text: s, lang }
}

/// Returns `"ru"` if any character falls in the Cyrillic block (U+0400–U+04FF),
/// otherwise `"en"`.
fn detect_lang(s: &str) -> &'static str {
    if s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
        "ru"
    } else {
        "en"
    }
}

/// Returns true iff `s` is exactly `YYYY-MM-DD` (ASCII digits and dashes only).
fn is_iso_date(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let b = s.as_bytes();
    b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

/// Returns true iff `s` is exactly `YYYY-MM-DD HH:MM`.
fn is_iso_datetime(s: &str) -> bool {
    if s.len() != 16 {
        return false;
    }
    let b = s.as_bytes();
    is_iso_date(&s[..10]) && b[10] == b' ' && is_valid_time_bytes(&b[11..])
}

/// Returns true iff the 5-byte slice is `HH:MM` (digits and colon).
fn is_valid_time_bytes(b: &[u8]) -> bool {
    b.len() >= 5
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b':'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
}

/// Apply due-date fields to a mutable JSON body according to the Todoist API rules:
/// - `None`                               → `"due_string": ""` (clears the due date)
/// - `Some("YYYY-MM-DD")`                 → `"due_date": "YYYY-MM-DD"`
/// - `Some("YYYY-MM-DD HH:MM")`           → `"due_datetime": "YYYY-MM-DDTHH:MM:00"`
/// - `Some(<natural language>)`           → `"due_string": s, "due_lang": lang`
fn apply_due(body: &mut serde_json::Value, due: Option<&str>) {
    match due {
        None => {
            body["due_string"] = json!("");
        }
        Some(s) => match classify_due(s) {
            DueKind::Date(d) => {
                body["due_date"] = json!(d);
            }
            DueKind::DateTime(dt) => {
                // "YYYY-MM-DD HH:MM" → "YYYY-MM-DDTHH:MM:00"
                let iso = format!("{}T{}:00", &dt[..10], &dt[11..16]);
                body["due_datetime"] = json!(iso);
            }
            DueKind::Natural { text, lang } => {
                body["due_string"] = json!(text);
                body["due_lang"] = json!(lang);
            }
        },
    }
}

pub fn create_task(
    client: &Client,
    token: &str,
    content: &str,
    project_id: &str,
    section_id: Option<&str>,
    parent_id: Option<&str>,
    due: Option<&str>,
) -> Result<String, String> {
    let mut body = json!({ "content": content, "project_id": project_id });
    if let Some(sid) = section_id {
        body["section_id"] = json!(sid);
    }
    if let Some(pid) = parent_id {
        body["parent_id"] = json!(pid);
    }
    if due.is_some() {
        apply_due(&mut body, due);
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
    let created: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Parse error (create task): {}", e))?;
    created["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Create task: missing 'id' in response".to_string())
}

pub fn update_task(
    client: &Client,
    token: &str,
    task_id: &str,
    content: &str,
    due: Option<&str>,
) -> Result<(), String> {
    let mut body = json!({ "content": content });
    apply_due(&mut body, due);
    let resp = client
        .post(format!("{}/tasks/{}", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| format!("Network error (update {}): {}", task_id, e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, &format!("tasks/{} [POST]", task_id)));
    }
    Ok(())
}

pub fn close_task(client: &Client, token: &str, task_id: &str) -> Result<(), String> {
    let resp = client
        .post(format!("{}/tasks/{}/close", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (close {}): {}", task_id, e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, &format!("tasks/{}/close", task_id)));
    }
    Ok(())
}

pub fn reopen_task(client: &Client, token: &str, task_id: &str) -> Result<(), String> {
    let resp = client
        .post(format!("{}/tasks/{}/reopen", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (reopen {}): {}", task_id, e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, &format!("tasks/{}/reopen", task_id)));
    }
    Ok(())
}

pub fn create_project(client: &Client, token: &str, name: &str) -> Result<String, String> {
    let body = json!({ "name": name });
    let resp = client
        .post(format!("{}/projects", BASE))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| format!("Network error (create project): {}", e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, "projects [POST]"));
    }
    let created: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Parse error (create project): {}", e))?;
    created["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Create project: missing 'id' in response".to_string())
}

/// Reorder tasks via the Sync API v9 (item_reorder command).
/// Each tuple is (task_id, new_child_order).
pub fn reorder_tasks(
    client: &Client,
    token: &str,
    items: &[(String, i64)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let items_json: Vec<serde_json::Value> = items
        .iter()
        .map(|(id, order)| json!({"id": id, "child_order": order}))
        .collect();
    let commands = json!([{
        "type": "item_reorder",
        "uuid": uuid_v4(),
        "args": { "items": items_json }
    }]);
    let resp = client
        .post("https://api.todoist.com/sync/v9/sync")
        .header("Authorization", format!("Bearer {}", token))
        .form(&[("commands", commands.to_string())])
        .send()
        .map_err(|e| format!("Network error (reorder): {}", e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        return Err(http_err(status, "sync/v9 item_reorder"));
    }
    Ok(())
}

/// Simple UUID v4 generator (random hex, no external crate needed).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (seed & 0xFFFF_FFFF) as u32,
        ((seed >> 32) & 0xFFFF) as u16,
        ((seed >> 48) & 0x0FFF) as u16,
        (((seed >> 60) & 0x3F) | 0x80) as u16,
        ((seed >> 66) & 0xFFFF_FFFF_FFFF) as u64,
    )
}

pub fn delete_task(client: &Client, token: &str, task_id: &str) -> Result<(), String> {
    let resp = client
        .delete(format!("{}/tasks/{}", BASE, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("Network error (delete {}): {}", task_id, e))?;
    let status = resp.status().as_u16();
    if status < 200 || status >= 300 {
        if status == 404 {
            return Ok(());
        }
        return Err(http_err(status, &format!("tasks/{} [DELETE]", task_id)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_iso_date() {
        let kind = classify_due("2026-04-20");
        assert!(matches!(kind, DueKind::Date("2026-04-20")));
    }

    #[test]
    fn classify_iso_datetime() {
        let kind = classify_due("2026-04-20 15:30");
        assert!(matches!(kind, DueKind::DateTime("2026-04-20 15:30")));
    }

    #[test]
    fn classify_natural_english() {
        let kind = classify_due("next Monday");
        match kind {
            DueKind::Natural { text, lang } => {
                assert_eq!(text, "next Monday");
                assert_eq!(lang, "en");
            }
            _ => panic!("expected Natural"),
        }
    }

    #[test]
    fn classify_natural_russian() {
        let kind = classify_due("завтра");
        match kind {
            DueKind::Natural { text, lang } => {
                assert_eq!(text, "завтра");
                assert_eq!(lang, "ru");
            }
            _ => panic!("expected Natural"),
        }
    }

    #[test]
    fn classify_natural_russian_mixed() {
        // A string with both Latin and Cyrillic should still be detected as Russian.
        let kind = classify_due("every понедельник");
        match kind {
            DueKind::Natural { lang, .. } => assert_eq!(lang, "ru"),
            _ => panic!("expected Natural"),
        }
    }

    #[test]
    fn apply_due_none_clears() {
        let mut body = serde_json::json!({});
        apply_due(&mut body, None);
        assert_eq!(body["due_string"], serde_json::json!(""));
    }

    #[test]
    fn apply_due_date() {
        let mut body = serde_json::json!({});
        apply_due(&mut body, Some("2026-04-20"));
        assert_eq!(body["due_date"], serde_json::json!("2026-04-20"));
        assert!(body.get("due_string").is_none());
    }

    #[test]
    fn apply_due_datetime() {
        let mut body = serde_json::json!({});
        apply_due(&mut body, Some("2026-04-20 15:30"));
        assert_eq!(body["due_datetime"], serde_json::json!("2026-04-20T15:30:00"));
        assert!(body.get("due_string").is_none());
    }

    #[test]
    fn apply_due_natural_english() {
        let mut body = serde_json::json!({});
        apply_due(&mut body, Some("tomorrow"));
        assert_eq!(body["due_string"], serde_json::json!("tomorrow"));
        assert_eq!(body["due_lang"], serde_json::json!("en"));
    }

    #[test]
    fn apply_due_natural_russian() {
        let mut body = serde_json::json!({});
        apply_due(&mut body, Some("завтра"));
        assert_eq!(body["due_string"], serde_json::json!("завтра"));
        assert_eq!(body["due_lang"], serde_json::json!("ru"));
    }
}
