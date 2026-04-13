// src/parser.rs
//
// Parses the buffer back into Vec<BufferTask>.
//
// Buffer structure (matches fetch.rs output):
//   ## ProjectName <!-- project:ID -->   <- H2: project
//   ### SectionName <!-- section:ID -->  <- H3: section
//   - [ ] Task <!-- id:ID -->
//       - [ ] Subtask <!-- id:ID -->    <- 4-space indent per level

use crate::models::BufferTask;
use std::collections::HashMap;

/// Marker that prefixes a due date in the buffer (e.g. `due:2026-04-20`).
pub const DUE_MARKER: &str = "due:";

/// Extract a `due:` value from the end of a content string.
///
/// Looks for the last `due:` in `s`, then captures everything after it
/// (after stripping at most one optional leading space) until end of input.
/// The captured string is stored verbatim — no format validation is performed,
/// so natural-language strings like `"tomorrow"` or `"завтра"` are accepted.
///
/// Returns `(clean_content, Option<due_string>)`.
/// - If a non-empty value is found: returns content with the `due:…` segment
///   stripped, and the raw due string.
/// - If `due:` is present but nothing follows (or only whitespace): returns
///   the original content unchanged and `None` (caller should push a warning).
fn extract_due(s: &str) -> (String, Option<String>) {
    let Some(marker_pos) = s.rfind(DUE_MARKER) else {
        return (s.to_string(), None);
    };

    // Everything after the marker.
    let after_marker = &s[marker_pos + DUE_MARKER.len()..];

    // Strip at most one optional leading space, then trim trailing whitespace.
    let due_str = after_marker
        .strip_prefix(' ')
        .unwrap_or(after_marker)
        .trim_end();

    if due_str.is_empty() {
        // marker present but nothing after it
        return (s.to_string(), None);
    }

    // Strip from content: everything from the marker position back (trim trailing spaces too)
    let clean = s[..marker_pos].trim_end().to_string();
    (clean, Some(due_str.to_string()))
}

fn extract_comment_value(line: &str, key: &str) -> Option<String> {
    let start = line.find("<!--")?;
    let end   = line[start..].find("-->")?;
    let comment = line[start + 4..start + end].trim();
    if comment.starts_with(key) {
        return comment[key.len()..].trim().split_whitespace().next()
            .map(|s| s.to_string());
    }
    None
}

/// Strip the last `<!-- ... -->` comment and return (clean_text, comment_value).
fn strip_comment(line: &str) -> (&str, Option<String>) {
    if let Some(start) = line.rfind("<!--") {
        if let Some(end) = line[start..].find("-->") {
            let comment = line[start + 4..start + end].trim().to_string();
            let text    = line[..start].trim_end();
            let value   = comment.splitn(2, ':').nth(1).map(|v| v.trim().to_string());
            return (text, value);
        }
    }
    (line, None)
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

pub struct ParseResult {
    pub tasks: Vec<BufferTask>,
    pub warnings: Vec<String>,
}

pub fn parse(lines: &[String]) -> ParseResult {
    let mut tasks: Vec<BufferTask> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let mut current_project_id: Option<String> = None;
    let mut current_section_id: Option<String> = None;

    for (i, raw_line) in lines.iter().enumerate() {
        let line_num = i + 1;

        // H2 = project ("## " but NOT "### ")
        if raw_line.starts_with("## ") && !raw_line.starts_with("### ") {
            current_project_id = extract_comment_value(raw_line, "project:");
            current_section_id = None;
            if current_project_id.is_none() {
                warnings.push(format!(
                    "Line {}: H2 has no <!-- project:ID --> — tasks here won't be synced",
                    line_num
                ));
            }
            continue;
        }

        // H3 = section ("### " but NOT "#### ")
        if raw_line.starts_with("### ") && !raw_line.starts_with("#### ") {
            current_section_id = extract_comment_value(raw_line, "section:");
            if current_section_id.is_none() {
                warnings.push(format!(
                    "Line {}: H3 has no <!-- section:ID --> — tasks here won't be synced",
                    line_num
                ));
            }
            continue;
        }

        // H4+ — ignored (e.g. "#### Subtasks" label in single-task view)
        if raw_line.starts_with("#### ") {
            continue;
        }

        // Task line
        let trimmed = raw_line.trim_start();
        let is_unchecked = trimmed.starts_with("- [ ] ");
        let is_checked   = trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ");
        if !is_unchecked && !is_checked { continue; }

        // 4 spaces per indent level
        let indent_spaces = leading_spaces(raw_line);
        let indent_level  = indent_spaces / 4;

        let after_checkbox = &trimmed[6..]; // "- [ ] " or "- [x] " = 6 chars
        let (content_raw, task_id) = strip_comment(after_checkbox);
        let content_trimmed = content_raw.trim();

        // Extract optional due date/string from content.
        let (clean_content, due) = extract_due(content_trimmed);
        if content_trimmed.contains(DUE_MARKER) && due.is_none() {
            warnings.push(format!(
                "Line {}: 'due:' is empty — ignoring due date",
                line_num
            ));
        }
        let content = clean_content.trim().to_string();

        if content.is_empty() {
            warnings.push(format!("Line {}: empty task content — skipped", line_num));
            continue;
        }
        if current_project_id.is_none() {
            warnings.push(format!(
                "Line {}: task '{}' has no project context — skipped",
                line_num, content
            ));
            continue;
        }

        tasks.push(BufferTask {
            id: task_id,
            content,
            checked: is_checked,
            indent_level,
            project_id: current_project_id.clone(),
            section_id: current_section_id.clone(),
            parent_id: None,
            line_num,
            due,
        });
    }

    // Second pass: resolve parent IDs via indent stack.
    let mut indent_stack: HashMap<usize, Option<String>> = HashMap::new();

    for task in &mut tasks {
        let level = task.indent_level;
        if level == 0 {
            task.parent_id = None;
        } else {
            task.parent_id = indent_stack.get(&(level - 1)).cloned().flatten();
            if task.parent_id.is_none() && level > 0 {
                warnings.push(format!(
                    "Line {}: subtask '{}' has no resolvable parent — will be a root task",
                    task.line_num, task.content
                ));
            }
        }
        indent_stack.insert(level, task.id.clone());
        indent_stack.retain(|&k, _| k <= level);
    }

    ParseResult { tasks, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn h2_is_project() {
        let buf = lines("## Work <!-- project:p1 -->\n\n- [ ] Task <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].project_id.as_deref(), Some("p1"));
    }

    #[test]
    fn h3_is_section() {
        let buf = lines(
            "## Work <!-- project:p1 -->\n\
             ### Backend <!-- section:s1 -->\n\
             - [ ] Fix bug <!-- id:t1 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks[0].section_id.as_deref(), Some("s1"));
    }

    #[test]
    fn four_space_indent_resolves_parent() {
        // No indent → siblings
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Parent <!-- id:p1 -->\n- [ ] Child <!-- id:c1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[1].parent_id, None);

        // 4-space indent → child
        let buf2 = lines("## Work <!-- project:p1 -->\n- [ ] Parent <!-- id:p1 -->\n    - [ ] Child <!-- id:c1 -->");
        let r2 = parse(&buf2);
        assert_eq!(r2.tasks[1].parent_id.as_deref(), Some("p1"));
    }

    #[test]
    fn checked_task_detected() {
        let buf = lines("## Work <!-- project:p1 -->\n\n- [x] Done <!-- id:t1 -->");
        let r = parse(&buf);
        assert!(r.tasks[0].checked);
    }

    #[test]
    fn new_task_no_id() {
        let buf = lines("## Work <!-- project:p1 -->\n\n- [ ] Brand new task");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].id, None);
    }

    #[test]
    fn section_resets_on_new_project() {
        let buf = lines(
            "## Work <!-- project:p1 -->\n\
             ### Backend <!-- section:s1 -->\n\
             - [ ] A <!-- id:t1 -->\n\
             ## Personal <!-- project:p2 -->\n\
             - [ ] B <!-- id:t2 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks[0].section_id.as_deref(), Some("s1"));
        assert_eq!(r.tasks[1].section_id, None);
    }

    #[test]
    fn h4_subtasks_label_ignored() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task <!-- id:t1 -->\n#### Subtasks\n    - [ ] Sub <!-- id:s1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 2);
        assert_eq!(r.tasks[1].indent_level, 1);
    }

    #[test]
    fn parses_date_only() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task due:2026-04-20 <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].due.as_deref(), Some("2026-04-20"));
        assert_eq!(r.tasks[0].content, "Task");
    }

    #[test]
    fn parses_datetime() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task due:2026-04-20 15:30 <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].due.as_deref(), Some("2026-04-20 15:30"));
    }

    #[test]
    fn parses_date_with_space_after_marker() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task due: 2026-04-20 <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].due.as_deref(), Some("2026-04-20"));
        assert_eq!(r.tasks[0].content, "Task");
    }

    #[test]
    fn no_date_when_missing() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].due, None);
    }

    #[test]
    fn date_stripped_from_content() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Buy milk due:2026-05-01 <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].content, "Buy milk");
        assert!(!r.tasks[0].content.contains("due:"));
    }

    #[test]
    fn parses_natural_language_english() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Call Bob due:next Monday <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].due.as_deref(), Some("next Monday"));
        assert_eq!(r.tasks[0].content, "Call Bob");
    }

    #[test]
    fn parses_natural_language_with_space_after_marker() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Call Bob due: tomorrow <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].due.as_deref(), Some("tomorrow"));
        assert_eq!(r.tasks[0].content, "Call Bob");
    }

    #[test]
    fn parses_natural_language_russian() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Позвонить due:завтра <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].due.as_deref(), Some("завтра"));
        assert_eq!(r.tasks[0].content, "Позвонить");
    }

    #[test]
    fn empty_due_produces_warning_and_none() {
        let buf = lines("## Work <!-- project:p1 -->\n- [ ] Task due: <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].due, None);
        assert!(r.warnings.iter().any(|w| w.contains("'due:' is empty")));
    }
}
