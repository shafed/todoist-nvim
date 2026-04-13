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

/// Extract a `due:` date from the end of a content string.
///
/// Looks for the literal marker `due:` near the end of `s`, then validates
/// the trailing substring as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM` using only
/// char-level checks (no regex crate needed). An optional single space
/// between `due:` and the date is accepted (`due:2026-04-20` and
/// `due: 2026-04-20` both parse).
///
/// Returns `(clean_content, Option<due_string>)`.
/// - If a valid date is found: returns content with the `due:…` segment stripped,
///   and the captured date string.
/// - If `due:` is present but the text after it is not a valid date: returns
///   the original content unchanged and `None` (caller should push a warning).
fn extract_due(s: &str) -> (String, Option<String>) {
    let Some(marker_pos) = s.rfind(DUE_MARKER) else {
        return (s.to_string(), None);
    };

    // Everything after the marker.
    let after_marker = &s[marker_pos + DUE_MARKER.len()..];

    // Allow an optional leading space between `due:` and the date.
    let date_str = after_marker.trim_start_matches(' ');

    // Validate: must start with YYYY-MM-DD (10 chars of digits/dashes)
    if !is_valid_date_prefix(date_str) {
        // marker present but unparseable — signal the caller via None
        return (s.to_string(), None);
    }

    let rest = &date_str[10..]; // after YYYY-MM-DD
    let due = if rest.is_empty() || rest.trim().is_empty() {
        // Date only
        date_str[..10].to_string()
    } else if let Some(time_part) = rest.strip_prefix(' ') {
        // Possible "HH:MM" suffix
        if is_valid_time_prefix(time_part) && (time_part.len() == 5 || time_part[5..].trim().is_empty()) {
            format!("{} {}", &date_str[..10], &time_part[..5])
        } else {
            // Not a valid time — treat whole thing as unparseable
            return (s.to_string(), None);
        }
    } else {
        // Extra non-space characters immediately after date
        return (s.to_string(), None);
    };

    // Strip from content: everything from the marker position back (trim trailing spaces too)
    let clean = s[..marker_pos].trim_end().to_string();
    (clean, Some(due))
}

/// Returns true if `s` starts with `DDDD-DD-DD` where D is a decimal digit
/// and the separators are `-`.
fn is_valid_date_prefix(s: &str) -> bool {
    if s.len() < 10 {
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

/// Returns true if `s` starts with `HH:MM` (5 chars, digits and colon).
fn is_valid_time_prefix(s: &str) -> bool {
    if s.len() < 5 {
        return false;
    }
    let b = s.as_bytes();
    b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b':' && b[3].is_ascii_digit() && b[4].is_ascii_digit()
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

        // Extract optional due date from content (due:YYYY-MM-DD or due:YYYY-MM-DD HH:MM)
        let (clean_content, due) = extract_due(content_trimmed);
        if content_trimmed.contains(DUE_MARKER) && due.is_none() {
            warnings.push(format!(
                "Line {}: 'due:' found but date is unparseable — ignoring due date",
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
}
