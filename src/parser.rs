// src/parser.rs
//
// Parses the buffer back into Vec<BufferTask>.
//
// Buffer structure (matches fetch.rs output):
//   # ProjectName <!-- project:ID -->   <- H1: project
//   ## SectionName <!-- section:ID -->  <- H2: section
//   - [ ] Task <!-- id:ID -->
//       - [ ] Subtask <!-- id:ID -->    <- 4-space indent per level

use crate::models::BufferTask;
use std::collections::HashMap;

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

        // H1 = project ("# " but NOT "## ")
        if raw_line.starts_with("# ") && !raw_line.starts_with("## ") {
            current_project_id = extract_comment_value(raw_line, "project:");
            current_section_id = None;
            if current_project_id.is_none() {
                warnings.push(format!(
                    "Line {}: H1 has no <!-- project:ID --> — tasks here won't be synced",
                    line_num
                ));
            }
            continue;
        }

        // H2 = section ("## " but NOT "### ")
        if raw_line.starts_with("## ") && !raw_line.starts_with("### ") {
            current_section_id = extract_comment_value(raw_line, "section:");
            if current_section_id.is_none() {
                warnings.push(format!(
                    "Line {}: H2 has no <!-- section:ID --> — tasks here won't be synced",
                    line_num
                ));
            }
            continue;
        }

        // H3+ — ignored (e.g. "### Subtasks" label in single-task view)
        if raw_line.starts_with("### ") {
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
        let content = content_raw.trim().to_string();

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
    fn h1_is_project() {
        let buf = lines("# Work <!-- project:p1 -->\n\n- [ ] Task <!-- id:t1 -->");
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].project_id.as_deref(), Some("p1"));
    }

    #[test]
    fn h2_is_section() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             ## Backend <!-- section:s1 -->\n\
             - [ ] Fix bug <!-- id:t1 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks[0].section_id.as_deref(), Some("s1"));
    }

    #[test]
    fn four_space_indent_resolves_parent() {
        // No indent → siblings
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             - [ ] Parent <!-- id:p1 -->\n\
             - [ ] Child <!-- id:c1 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks[1].parent_id, None);

        // 4-space indent → child
        let buf2 = lines(
            "# Work <!-- project:p1 -->\n\
             - [ ] Parent <!-- id:p1 -->\n\
                 - [ ] Child <!-- id:c1 -->"
        );
        let r2 = parse(&buf2);
        assert_eq!(r2.tasks[1].parent_id.as_deref(), Some("p1"));
    }

    #[test]
    fn checked_task_detected() {
        let buf = lines("# Work <!-- project:p1 -->\n\n- [x] Done <!-- id:t1 -->");
        let r = parse(&buf);
        assert!(r.tasks[0].checked);
    }

    #[test]
    fn new_task_no_id() {
        let buf = lines("# Work <!-- project:p1 -->\n\n- [ ] Brand new task");
        let r = parse(&buf);
        assert_eq!(r.tasks[0].id, None);
    }

    #[test]
    fn section_resets_on_new_project() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             ## Backend <!-- section:s1 -->\n\
             - [ ] A <!-- id:t1 -->\n\
             # Personal <!-- project:p2 -->\n\
             - [ ] B <!-- id:t2 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks[0].section_id.as_deref(), Some("s1"));
        assert_eq!(r.tasks[1].section_id, None);
    }

    #[test]
    fn h3_subtasks_label_ignored() {
        let buf = lines(
            "# Work <!-- project:p1 -->\n\
             - [ ] Task <!-- id:t1 -->\n\
             ### Subtasks\n\
                 - [ ] Sub <!-- id:s1 -->"
        );
        let r = parse(&buf);
        assert_eq!(r.tasks.len(), 2);
        assert_eq!(r.tasks[1].indent_level, 1);
    }
}
