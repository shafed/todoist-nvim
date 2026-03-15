// src/parser.rs
//
// Parses the Neovim buffer (as a list of lines) back into a Vec<BufferTask>.
//
// # Expected format
//
// ```markdown
// # Project Name <!-- project:PROJECT_ID -->
//
// ## Section Name <!-- section:SECTION_ID -->
//
// - [ ] Task title <!-- id:TASK_ID -->
//     - [x] Subtask <!-- id:TASK_ID -->
//         - [ ] Nested subtask <!-- id:TASK_ID -->
// ```
//
// Lines not matching any of these patterns are silently ignored.
// Missing project/section IDs produce a warning stored in the error list.
//
// # Parent resolution
//
// After collecting all tasks, a second pass walks the indent stack to assign
// `parent_id`.  The stack maps indent_level → most_recent_task_id_at_that_level.

use crate::models::BufferTask;
use std::collections::HashMap;

/// Regex-free extraction helpers.

fn strip_comment(line: &str) -> (&str, Option<String>) {
    if let Some(start) = line.rfind("<!--") {
        if let Some(end) = line[start..].find("-->") {
            let comment = &line[start + 4..start + end].trim().to_string();
            let text = line[..start].trim_end();
            // Extract key:value from comment.
            let value = comment
                .splitn(2, ':')
                .nth(1)
                .map(|v| v.trim().to_string());
            return (text, value);
        }
    }
    (line, None)
}

fn extract_comment_value(line: &str, key: &str) -> Option<String> {
    if let Some(start) = line.find("<!--") {
        if let Some(end) = line[start..].find("-->") {
            let comment = line[start + 4..start + end].trim();
            if comment.starts_with(key) {
                return comment[key.len()..].trim().split_whitespace().next()
                    .map(|s| s.to_string());
            }
        }
    }
    None
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

        // ── H1: project heading ────────────────────────────────────────────
        if raw_line.starts_with("# ") && !raw_line.starts_with("## ") {
            current_section_id = None;
            current_project_id =
                extract_comment_value(raw_line, "project:");
            if current_project_id.is_none() {
                warnings.push(format!(
                    "Line {}: H1 heading has no <!-- project:ID --> — \
                     new tasks under this project will not be synced",
                    line_num
                ));
            }
            continue;
        }

        // ── H2: section heading ────────────────────────────────────────────
        if raw_line.starts_with("## ") {
            current_section_id =
                extract_comment_value(raw_line, "section:");
            // Sections without IDs are fine — tasks under them will be
            // placed in the project root (section_id = None).
            continue;
        }

        // ── Task line ──────────────────────────────────────────────────────
        let trimmed = raw_line.trim_start();
        let is_unchecked = trimmed.starts_with("- [ ] ");
        let is_checked   = trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ");

        if !is_unchecked && !is_checked {
            continue;
        }

        let indent_spaces = leading_spaces(raw_line);
        // 4 spaces per level; round down so any alignment works.
        let indent_level = indent_spaces / 4;

        // Strip checkbox prefix.
        let after_checkbox = if is_unchecked {
            trimmed[6..].to_string() // "- [ ] " is 6 chars
        } else {
            trimmed[6..].to_string() // "- [x] " is 6 chars
        };

        // Separate content from <!-- id:XXX --> comment.
        let (content_raw, task_id) = strip_comment(&after_checkbox);
        let content = content_raw.trim().to_string();

        if content.is_empty() {
            warnings.push(format!("Line {}: empty task content — skipped", line_num));
            continue;
        }

        if current_project_id.is_none() {
            warnings.push(format!(
                "Line {}: task '{}' is not under any project heading — skipped",
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
            parent_id: None, // resolved in second pass
            line_num,
        });
    }

    // ── Second pass: resolve parent IDs ───────────────────────────────────
    // indent_stack maps indent_level → most recent task ID at that level.
    let mut indent_stack: HashMap<usize, Option<String>> = HashMap::new();

    for task in &mut tasks {
        let level = task.indent_level;

        if level == 0 {
            task.parent_id = None;
        } else {
            // Parent is the most recent task at (level - 1).
            task.parent_id = indent_stack
                .get(&(level - 1))
                .cloned()
                .flatten();

            if task.parent_id.is_none() {
                warnings.push(format!(
                    "Line {}: subtask '{}' has no resolvable parent — \
                     will be created as a root task",
                    task.line_num, task.content
                ));
            }
        }

        // Register this task as the most recent at its indent level, and
        // clear deeper levels so they don't bleed into unrelated subtrees.
        indent_stack.insert(level, task.id.clone());
        indent_stack.retain(|&k, _| k <= level);
    }

    ParseResult { tasks, warnings }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn parses_basic_structure() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             ## Backend <!-- section:s1 -->\n\
             \n\
             - [ ] Fix bug <!-- id:t1 -->\n\
             - [x] Done task <!-- id:t2 -->",
        );
        let result = parse(&buf);
        assert_eq!(result.tasks.len(), 2);
        assert_eq!(result.tasks[0].id.as_deref(), Some("t1"));
        assert_eq!(result.tasks[0].content, "Fix bug");
        assert!(!result.tasks[0].checked);
        assert_eq!(result.tasks[1].id.as_deref(), Some("t2"));
        assert!(result.tasks[1].checked);
    }

    #[test]
    fn assigns_project_and_section_ids() {
        let buf = lines(
            "# Personal <!-- project:p99 -->\n\
             \n\
             ## Goals <!-- section:s99 -->\n\
             \n\
             - [ ] Run a marathon <!-- id:t99 -->",
        );
        let result = parse(&buf);
        assert_eq!(result.tasks[0].project_id.as_deref(), Some("p99"));
        assert_eq!(result.tasks[0].section_id.as_deref(), Some("s99"));
    }

    #[test]
    fn resolves_parent_ids() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             - [ ] Parent <!-- id:parent1 -->\n\
             - [ ] Child <!-- id:child1 -->\n",
        );
        // The second task is NOT indented, so it's a sibling, not a child.
        let result = parse(&buf);
        assert_eq!(result.tasks[1].parent_id, None);
    }

    #[test]
    fn resolves_indented_parent() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             - [ ] Parent <!-- id:parent1 -->\n\
             - [ ] Child <!-- id:child1 -->\n",
        );
        // Indent the child with 4 spaces.
        let buf2 = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             - [ ] Parent <!-- id:parent1 -->\n\
                 - [ ] Child <!-- id:child1 -->\n",
        );
        let result = parse(&buf2);
        assert_eq!(result.tasks[1].parent_id.as_deref(), Some("parent1"));
    }

    #[test]
    fn new_task_has_no_id() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             - [ ] Brand new task\n",
        );
        let result = parse(&buf);
        assert_eq!(result.tasks[0].id, None);
    }

    #[test]
    fn section_reset_on_new_project() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             \n\
             ## Backend <!-- section:s1 -->\n\
             \n\
             - [ ] Task A <!-- id:t1 -->\n\
             \n\
             # Personal <!-- project:p2 -->\n\
             \n\
             - [ ] Task B <!-- id:t2 -->\n",
        );
        let result = parse(&buf);
        assert_eq!(result.tasks[0].section_id.as_deref(), Some("s1"));
        // After a new H1, section should reset to None.
        assert_eq!(result.tasks[1].section_id, None);
    }
}
