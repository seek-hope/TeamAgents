//! Bounded human-readable projection of the durable work tree returned by status.
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write;

const PREVIEW: usize = 24;

fn label(value: &Value) -> String {
    let mut chars = value.as_str().unwrap_or("?").chars();
    let mut out: String = chars.by_ref().take(70).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

pub(crate) fn render(task: &Value) -> Option<String> {
    let rows = task["tree_nodes"].as_array().filter(|rows| rows.len() > 1)?;
    let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
    let mut root = None;
    for row in rows {
        if let Some(parent) = row["parent"].as_str() {
            children.entry(parent).or_default().push(row);
        } else if root.is_none() {
            root = Some(row);
        }
    }

    let mut out = String::from("Work tree (stage status, task ID)");
    let mut stack = vec![(root?, String::new(), true, true)];
    let mut displayed = 0;
    while let Some((row, prefix, last, first)) = stack.pop() {
        let id = row["task"].as_str().or_else(|| row["id"].as_str())?;
        let branch = if first {
            ""
        } else if last {
            "└─ "
        } else {
            "├─ "
        };
        let status = row["stage_status"].as_str().or_else(|| row["status"].as_str()).unwrap_or("?");
        write!(out, "\n    {prefix}{branch}{} [{status}] {id}", label(&row["agent"])).unwrap();
        displayed += 1;
        if displayed == PREVIEW {
            break;
        }
        if let Some(next) = children.get(id) {
            let child_prefix = prefix
                + if first {
                    ""
                } else if last {
                    "   "
                } else {
                    "│  "
                };
            for (i, child) in next.iter().enumerate().rev() {
                stack.push((child, child_prefix.clone(), i + 1 == next.len(), false));
            }
        }
    }
    if displayed < rows.len() {
        write!(out, "\n    … {} more (use status --json)", rows.len() - displayed).unwrap();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shows_completed_parent_and_running_leaf_without_permitting_extra_lines() {
        let task = json!({"tree_nodes":[
            {"task":"c","parent":"b","agent":"late\nchild","status":"running"},
            {"task":"r","parent":null,"agent":"default","status":"done"},
            {"task":"b","parent":"r","agent":"reader","status":"done"},
            {"task":"s","parent":"r","agent":"tester","status":"queued"}
        ]});
        let view = render(&task).unwrap();
        assert!(view.contains(
            "default [done] r\n    ├─ reader [done] b\n    │  └─ late child [running] c\n    └─ tester [queued] s"
        ));
        assert!(render(&json!({"status":"done"})).is_none());
    }
}
