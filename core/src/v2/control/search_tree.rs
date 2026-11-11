//! Session-level exploration navigation. The tree is a model-maintained claim
//! over the Leader-visible conversation, never execution or capability authority.
//! Member-private history is neither read here nor copied into the shared view.

use super::{event, Identity};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};

const LEADER: &str = "i-leader";
const MAX_NODES: usize = 96;
const MAX_OPERATIONS: usize = 32;
const MAX_DEPTH: usize = 32;
const MAX_TITLE: usize = 160;
const MAX_SUMMARY: usize = 1200;
const MAX_REFS: usize = 16;
const MAX_REF_CHARS: usize = 256;
const ROOT: &str = "root";

fn leader(tx: &Connection) -> Result<String, String> {
    Ok(tx
        .query_row("SELECT id FROM instances WHERE is_leader=1 ORDER BY rowid LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(|e| format!("search-tree Leader lookup: {e}"))?
        .unwrap_or_else(|| LEADER.to_string()))
}

fn empty_tree() -> Json {
    json!({
        "version": 1,
        "focus": ROOT,
        "nodes": [{
            "id": ROOT,
            "parent_id": Json::Null,
            "title": "Session exploration",
            "summary": "No exploration has been recorded yet.",
            "kind": "question",
            "status": "active",
            "source_refs": []
        }]
    })
}

fn current_position(tx: &Connection) -> Result<Option<(String, i64, Option<String>)>, String> {
    let leader = leader(tx)?;
    let current: Option<i64> = tx
        .query_row("SELECT context_epoch FROM instances WHERE id = ?1", [&leader], |row| row.get(0))
        .optional()
        .map_err(|e| format!("search-tree Leader epoch: {e}"))?;
    let Some(epoch) = current else { return Ok(None) };
    let entry: Option<String> = tx
        .query_row(
            "SELECT id FROM context_entries WHERE instance_id = ?1 AND epoch = ?2 ORDER BY idx DESC LIMIT 1",
            rusqlite::params![leader, epoch],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("search-tree source watermark: {e}"))?;
    Ok(Some((leader, epoch, entry)))
}

fn null_position() -> Json {
    json!({"instance_id":null,"epoch":null,"entry_id":null})
}

fn position_json(position: Option<(String, i64, Option<String>)>) -> Json {
    position
        .map(|(instance, epoch, entry)| json!({"instance_id":instance,"epoch":epoch,"entry_id":entry}))
        .unwrap_or_else(null_position)
}

#[derive(Debug, Clone)]
struct Head {
    revision: i64,
    tree: Json,
    source: Json,
}

fn head(tx: &Connection, session: &str) -> Result<Head, String> {
    let row = tx
        .query_row(
            "SELECT r.revision, r.tree_json, r.source_instance, r.source_epoch, r.source_entry_id
             FROM search_tree_head h JOIN search_tree_revisions r
               ON r.session_id = h.session_id AND r.revision = h.revision
             WHERE h.session_id = ?1",
            [session],
            |row| {
                let tree: String = row.get(1)?;
                let instance: Option<String> = row.get(2)?;
                let epoch: Option<i64> = row.get(3)?;
                let entry: Option<String> = row.get(4)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    serde_json::from_str::<Json>(&tree).unwrap_or(Json::Null),
                    json!({"instance_id": instance, "epoch": epoch, "entry_id": entry}),
                ))
            },
        )
        .optional()
        .map_err(|e| format!("search-tree head: {e}"))?;
    if let Some((revision, tree, source)) = row {
        if !tree.is_object() {
            return Err("stored search-tree revision is malformed".into());
        }
        validate(&tree)?;
        return Ok(Head { revision, tree, source });
    }
    Ok(Head { revision: 0, tree: empty_tree(), source: json!({"instance_id": null, "epoch": null, "entry_id": null}) })
}

fn depth_map(tree: &Json) -> HashMap<String, usize> {
    let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
    let parents: HashMap<String, Option<String>> = nodes
        .iter()
        .map(|node| (node["id"].as_str().unwrap_or("").to_string(), node["parent_id"].as_str().map(str::to_string)))
        .collect();
    let mut depths = HashMap::new();
    for id in parents.keys() {
        let mut depth = 0;
        let mut current = Some(id.clone());
        let mut seen = HashSet::new();
        while let Some(node) = current {
            if !seen.insert(node.clone()) {
                break;
            }
            match parents.get(&node) {
                Some(Some(parent)) => {
                    depth += 1;
                    current = Some(parent.clone());
                }
                _ => break,
            }
        }
        depths.insert(id.clone(), depth);
    }
    depths
}

fn clipped(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    format!("{}…", text.chars().take(cap.saturating_sub(1)).collect::<String>())
}

/// Literal, single-line text for terminal rendering. JSON retains the original
/// model text; only the human projection escapes controls and embedded breaks.
fn terminal_text(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other
                if other.is_control()
                    || matches!(
                        other as u32,
                        0x061c | 0x200e | 0x200f | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff
                    ) =>
            {
                out.push_str(&format!("\\u{{{:x}}}", other as u32))
            }
            other => out.push(other),
        }
    }
    out
}

fn render(head: &Json) -> String {
    let mut out = format!(
        "Session search tree · revision {} · {}",
        head["revision"].as_i64().unwrap_or(0),
        if head["stale"] == json!(true) { "stale" } else { "current" }
    );
    if let Some(entry) = head["source"]["entry_id"].as_str() {
        out.push_str(&format!(" · source {entry}"));
    }
    let depths = depth_map(head);
    for node in head["nodes"].as_array().into_iter().flatten() {
        let id = node["id"].as_str().unwrap_or("?");
        let depth = depths.get(id).copied().unwrap_or(0);
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!(
            "{} [{}] {} ({}){}",
            if head["focus"].as_str() == Some(id) { "→" } else { "•" },
            node["status"].as_str().unwrap_or("open"),
            terminal_text(node["title"].as_str().unwrap_or("Untitled")),
            id,
            if node["kind"].as_str().is_some_and(|kind| kind != "attempt") {
                format!(" · {}", node["kind"].as_str().unwrap_or(""))
            } else {
                String::new()
            }
        ));
        let summary = node["summary"].as_str().unwrap_or("").trim();
        if !summary.is_empty() {
            out.push_str(&format!("\n{}  {}", "  ".repeat(depth), terminal_text(&clipped(summary, 240))));
        }
        let refs: Vec<String> =
            node["source_refs"].as_array().into_iter().flatten().filter_map(Json::as_str).map(terminal_text).collect();
        if !refs.is_empty() {
            out.push_str(&format!("\n{}  refs: {}", "  ".repeat(depth), refs.join(", ")));
        }
    }
    out
}

fn decorate(tx: &Connection, mut head: Head) -> Result<Json, String> {
    let current = position_json(current_position(tx)?);
    head.tree["revision"] = json!(head.revision);
    head.tree["source"] = head.source;
    let initial_empty = head.revision == 0 && current["entry_id"].is_null();
    head.tree["stale"] = json!(!initial_empty && current != head.tree["source"]);
    head.tree["text"] = Json::Null;
    head.tree["text"] = json!(render(&head.tree));
    Ok(head.tree)
}

/// Read the one session tree for the user or its Leader. Other members cannot
/// use this projection as a side channel into the Leader's private history.
pub(super) fn read(tx: &Connection, session: &str, _params: &Json, identity: &Identity) -> Result<Json, String> {
    match identity {
        Identity::User => {}
        Identity::Instance(id) if *id == leader(tx)? => {}
        Identity::Instance(_) => return Err("only the Leader may read the session search tree".into()),
        Identity::System => return Err("the system does not read the session search tree".into()),
    }
    decorate(tx, head(tx, session)?)
}

/// Trusted read-only daemon/driver projection. Authorization belongs to the
/// caller; the model-facing path still goes through `read` above.
pub fn snapshot(tx: &Connection, session: &str) -> Result<Json, String> {
    decorate(tx, head(tx, session)?)
}

/// Full previous tree for compaction, with bounded per-node prose. Operations
/// preserve nodes unless they explicitly merge them, so omissions cannot prune.
pub fn compaction_snapshot(tx: &Connection, session: &str) -> Result<Json, String> {
    let current = snapshot(tx, session)?;
    let nodes: Vec<Json> = current["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| {
            let mut node = node.clone();
            node["summary"] = json!(clipped(node["summary"].as_str().unwrap_or(""), 300));
            node
        })
        .collect();
    Ok(json!({
        "base_revision": current["revision"],
        "focus": current["focus"],
        "stale": current["stale"],
        "source": current["source"],
        "nodes": nodes,
    }))
}

/// Compact prompt view: the focus path, its ancestors and unresolved branches.
/// The complete tree remains available through search_tree.show.
pub fn prompt(tx: &Connection, session: &str) -> Result<String, String> {
    let current = snapshot(tx, session)?;
    let nodes = current["nodes"].as_array().cloned().unwrap_or_default();
    let parents: HashMap<String, Option<String>> = nodes
        .iter()
        .map(|node| (node["id"].as_str().unwrap_or("").to_string(), node["parent_id"].as_str().map(str::to_string)))
        .collect();
    let mut include = HashSet::new();
    let mut focus = current["focus"].as_str().map(str::to_string);
    while let Some(id) = focus {
        if !include.insert(id.clone()) {
            break;
        }
        focus = parents.get(&id).cloned().flatten();
    }
    for node in &nodes {
        if matches!(node["status"].as_str(), Some("active" | "open" | "blocked")) {
            include.insert(node["id"].as_str().unwrap_or("").to_string());
        }
    }
    let nodes: Vec<Json> = nodes
        .into_iter()
        .filter(|node| include.contains(node["id"].as_str().unwrap_or("")))
        .map(|mut node| {
            node["summary"] = json!(clipped(node["summary"].as_str().unwrap_or(""), 400));
            node
        })
        .collect();
    Ok(json!({
        "revision": current["revision"],
        "focus": current["focus"],
        "stale": current["stale"],
        "source": current["source"],
        "nodes": nodes,
    })
    .to_string())
}

fn text_field(value: &Json, field: &str, cap: usize, required: bool) -> Result<String, String> {
    let text = value[field].as_str().ok_or_else(|| format!("search-tree {field} must be a string"))?.trim();
    if required && text.is_empty() {
        return Err(format!("search-tree {field} must not be empty"));
    }
    if text.chars().count() > cap {
        return Err(format!("search-tree {field} exceeds {cap} characters"));
    }
    Ok(text.to_string())
}

fn refs_field(value: &Json) -> Result<Vec<String>, String> {
    let refs = value["source_refs"].as_array().ok_or("search-tree source_refs must be an array")?;
    if refs.len() > MAX_REFS {
        return Err(format!("a search-tree node carries at most {MAX_REFS} source refs"));
    }
    let mut out = Vec::new();
    for reference in refs {
        let text = reference.as_str().ok_or("search-tree source refs must be strings")?.trim();
        if text.is_empty() || text.chars().count() > MAX_REF_CHARS {
            return Err("search-tree source refs must be non-empty and at most 256 characters".into());
        }
        if !out.iter().any(|existing| existing == text) {
            out.push(text.to_string());
        }
    }
    Ok(out)
}

fn validate(tree: &Json) -> Result<(), String> {
    if tree["version"].as_i64() != Some(1) {
        return Err("search-tree version must be 1".into());
    }
    let nodes = tree["nodes"].as_array().ok_or("search-tree nodes must be an array")?;
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err(format!("search tree must hold 1..{MAX_NODES} nodes"));
    }
    let allowed: HashSet<&str> =
        ["id", "parent_id", "title", "summary", "kind", "status", "source_refs"].into_iter().collect();
    let kinds: HashSet<&str> =
        ["question", "hypothesis", "attempt", "finding", "decision", "obstacle", "next_step"].into_iter().collect();
    let statuses: HashSet<&str> = ["active", "open", "resolved", "blocked", "abandoned"].into_iter().collect();
    let mut ids = HashSet::new();
    let mut parents = HashMap::new();
    for node in nodes {
        let object = node.as_object().ok_or("search-tree nodes must be objects")?;
        if object.keys().any(|key| !allowed.contains(key.as_str())) {
            return Err("search-tree node contains an unknown field".into());
        }
        let id = text_field(node, "id", 64, true)?;
        if !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')) {
            return Err(format!("search-tree node id {id:?} has an unsupported shape"));
        }
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate search-tree node {id}"));
        }
        if (id == ROOT) != node["parent_id"].is_null() {
            return Err("only the root has a null parent, and the root cannot have a parent".into());
        }
        if id != ROOT && node["parent_id"].as_str().unwrap_or("").is_empty() {
            return Err(format!("search-tree node {id} needs a parent"));
        }
        text_field(node, "title", MAX_TITLE, true)?;
        text_field(node, "summary", MAX_SUMMARY, false)?;
        if !kinds.contains(node["kind"].as_str().unwrap_or("")) {
            return Err(format!("search-tree node {id} has an unknown kind"));
        }
        if !statuses.contains(node["status"].as_str().unwrap_or("")) {
            return Err(format!("search-tree node {id} has an unknown status"));
        }
        refs_field(node)?;
        if id == ROOT && node["status"] != json!("active") {
            return Err("the search-tree root stays active".into());
        }
        parents.insert(id, node["parent_id"].as_str().map(str::to_string));
    }
    if !ids.contains(ROOT) {
        return Err("search tree has no root".into());
    }
    let focus = tree["focus"].as_str().ok_or("search-tree focus must be a string")?;
    if !ids.contains(focus) {
        return Err("search-tree focus names no node".into());
    }
    for id in &ids {
        let mut current = Some(id.clone());
        let mut seen = HashSet::new();
        let mut depth = 0;
        let mut reached = false;
        while let Some(node) = current {
            if node == ROOT {
                reached = true;
                break;
            }
            if depth >= MAX_DEPTH || !seen.insert(node.clone()) {
                return Err("search tree contains a cycle or exceeds its depth bound".into());
            }
            let parent = parents.get(&node).cloned().flatten().ok_or("search-tree node is unreachable")?;
            if !ids.contains(&parent) {
                return Err(format!("search-tree node {node} names missing parent {parent}"));
            }
            depth += 1;
            current = Some(parent);
        }
        if !reached {
            return Err(format!("search-tree node {id} is not reachable from the root"));
        }
    }
    Ok(())
}

fn patch_node(node: &Json) -> Result<Json, String> {
    let object = node.as_object().ok_or("search-tree add.node must be an object")?;
    let allowed: HashSet<&str> = ["title", "summary", "kind", "status", "source_refs"].into_iter().collect();
    if object.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err("search-tree add.node contains an unknown field; node ids are assigned by the control plane".into());
    }
    let kind = node.get("kind").cloned().unwrap_or(json!("attempt"));
    let status = node.get("status").cloned().unwrap_or(json!("open"));
    Ok(json!({
        "id": "",
        "parent_id": Json::Null,
        "title": text_field(node, "title", MAX_TITLE, true)?,
        "summary": if node.get("summary").is_some() { text_field(node, "summary", MAX_SUMMARY, false)? } else { String::new() },
        "kind": kind,
        "status": status,
        "source_refs": if node.get("source_refs").is_some() { refs_field(node)? } else { Vec::<String>::new() },
    }))
}

fn index_of(tree: &Json, id: &str) -> Result<usize, String> {
    tree["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().position(|node| node["id"] == json!(id)))
        .ok_or_else(|| format!("search-tree node {id} does not exist"))
}

fn is_descendant(tree: &Json, candidate: &str, ancestor: &str) -> bool {
    let mut current = Some(candidate.to_string());
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return true;
        }
        if id == ancestor {
            return true;
        }
        let Ok(index) = index_of(tree, &id) else { return false };
        current = tree["nodes"][index]["parent_id"].as_str().map(str::to_string);
    }
    false
}

fn apply_operations(mut tree: Json, operations: &[Json], revision: i64) -> Result<(Json, Vec<String>), String> {
    let mut added = Vec::new();
    for (ordinal, operation) in operations.iter().enumerate() {
        let object = operation.as_object().ok_or("search-tree operations must be objects")?;
        let kind = operation["op"].as_str().ok_or("search-tree operation.op required")?;
        let allowed: HashSet<&str> = match kind {
            "add" => ["op", "parent_id", "node"].into_iter().collect(),
            "update" => ["op", "id", "title", "summary", "kind", "status", "source_refs"].into_iter().collect(),
            "move" => ["op", "id", "parent_id"].into_iter().collect(),
            "merge" => ["op", "from_id", "into_id", "summary"].into_iter().collect(),
            "focus" => ["op", "id"].into_iter().collect(),
            other => return Err(format!("unknown search-tree operation {other:?}")),
        };
        if object.keys().any(|key| !allowed.contains(key.as_str())) {
            return Err(format!("search-tree {kind} operation contains an unknown field"));
        }
        match kind {
            "add" => {
                if tree["nodes"].as_array().is_none_or(|nodes| nodes.len() >= MAX_NODES) {
                    return Err(format!(
                        "search tree holds at most {MAX_NODES} nodes; merge or abandon branches first"
                    ));
                }
                let parent = operation["parent_id"].as_str().ok_or("search-tree add.parent_id required")?;
                index_of(&tree, parent)?;
                let mut node = patch_node(&operation["node"])?;
                let id = format!("n{revision}-{}", ordinal + 1);
                node["id"] = json!(id.clone());
                node["parent_id"] = json!(parent);
                tree["nodes"].as_array_mut().unwrap().push(node);
                added.push(id);
            }
            "update" => {
                let id = operation["id"].as_str().ok_or("search-tree update.id required")?;
                let index = index_of(&tree, id)?;
                let mut node = tree["nodes"][index].clone();
                for (field, cap, required) in [
                    ("title", MAX_TITLE, true),
                    ("summary", MAX_SUMMARY, false),
                    ("kind", 32, true),
                    ("status", 32, true),
                ] {
                    if operation.get(field).is_some() {
                        node[field] = json!(text_field(operation, field, cap, required)?);
                    }
                }
                if operation.get("source_refs").is_some() {
                    node["source_refs"] = json!(refs_field(operation)?);
                }
                if object.len() == 2 {
                    return Err("search-tree update changes no fields".into());
                }
                tree["nodes"][index] = node;
            }
            "move" => {
                let id = operation["id"].as_str().ok_or("search-tree move.id required")?;
                let parent = operation["parent_id"].as_str().ok_or("search-tree move.parent_id required")?;
                if id == ROOT {
                    return Err("the search-tree root cannot move".into());
                }
                index_of(&tree, parent)?;
                if is_descendant(&tree, parent, id) {
                    return Err("moving a search-tree node below itself would create a cycle".into());
                }
                let index = index_of(&tree, id)?;
                tree["nodes"][index]["parent_id"] = json!(parent);
            }
            "merge" => {
                let from = operation["from_id"].as_str().ok_or("search-tree merge.from_id required")?;
                let into = operation["into_id"].as_str().ok_or("search-tree merge.into_id required")?;
                if from == ROOT || from == into {
                    return Err("search-tree merge needs distinct nodes and cannot remove the root".into());
                }
                let source_index = index_of(&tree, from)?;
                let target_index = index_of(&tree, into)?;
                if is_descendant(&tree, into, from) {
                    return Err("search-tree merge cannot move a node below its own descendant".into());
                }
                let source = tree["nodes"][source_index].clone();
                let mut target = tree["nodes"][target_index].clone();
                if operation.get("summary").is_some() {
                    target["summary"] = json!(text_field(operation, "summary", MAX_SUMMARY, false)?);
                }
                let mut refs = refs_field(&target)?;
                for reference in refs_field(&source)? {
                    if !refs.contains(&reference) {
                        refs.push(reference);
                    }
                }
                if refs.len() > MAX_REFS {
                    return Err(format!("merged search-tree node exceeds {MAX_REFS} source refs"));
                }
                target["source_refs"] = json!(refs);
                tree["nodes"][target_index] = target;
                for node in tree["nodes"].as_array_mut().unwrap().iter_mut() {
                    if node["parent_id"] == json!(from) {
                        node["parent_id"] = json!(into);
                    }
                }
                if tree["focus"] == json!(from) {
                    tree["focus"] = json!(into);
                }
                tree["nodes"].as_array_mut().unwrap().remove(source_index);
            }
            "focus" => {
                let id = operation["id"].as_str().ok_or("search-tree focus.id required")?;
                index_of(&tree, id)?;
                tree["focus"] = json!(id);
            }
            _ => unreachable!(),
        }
    }
    validate(&tree)?;
    Ok((tree, added))
}

fn apply_patch_inner(
    tx: &Connection,
    session: &str,
    patch: &Json,
    cause: &str,
    cause_ref: &str,
    actor: &str,
) -> Result<Json, String> {
    let canonical = serde_json::to_string(patch).map_err(|e| format!("search-tree patch: {e}"))?;
    if let Some((revision, old_patch)) = tx
        .query_row(
            "SELECT revision, patch_json FROM search_tree_revisions WHERE session_id = ?1 AND cause_ref = ?2",
            rusqlite::params![session, cause_ref],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| format!("search-tree replay lookup: {e}"))?
    {
        if old_patch != canonical {
            return Err(format!("search-tree cause {cause_ref} was already used by a different patch"));
        }
        let tree: Json = tx
            .query_row(
                "SELECT tree_json FROM search_tree_revisions WHERE session_id = ?1 AND revision = ?2",
                rusqlite::params![session, revision],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("search-tree replay: {e}"))
            .and_then(|text| serde_json::from_str(&text).map_err(|e| format!("search-tree replay JSON: {e}")))?;
        let source: Json = tx
            .query_row(
                "SELECT json_object('instance_id',source_instance,'epoch',source_epoch,'entry_id',source_entry_id)
             FROM search_tree_revisions WHERE session_id=?1 AND revision=?2",
                rusqlite::params![session, revision],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("search-tree replay source: {e}"))
            .and_then(|text| serde_json::from_str(&text).map_err(|e| format!("search-tree replay source JSON: {e}")))?;
        let added_ids: Vec<String> = patch["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, operation)| operation["op"] == json!("add"))
            .map(|(ordinal, _)| format!("n{revision}-{}", ordinal + 1))
            .collect();
        let mut result = decorate(tx, Head { revision, tree, source })?;
        result["added_ids"] = json!(added_ids);
        result["replayed"] = json!(true);
        return Ok(result);
    }
    let current = head(tx, session)?;
    let base = patch["base_revision"].as_i64().ok_or("search-tree base_revision must be an integer")?;
    if base != current.revision {
        return Err(format!("stale search-tree revision {base}; current revision is {}", current.revision));
    }
    let operations = patch["operations"].as_array().ok_or("search-tree operations must be an array")?;
    if operations.len() > MAX_OPERATIONS {
        return Err(format!("a search-tree patch carries at most {MAX_OPERATIONS} operations"));
    }
    let revision = current.revision + 1;
    let (tree, added) = apply_operations(current.tree, operations, revision)?;
    let source = position_json(current_position(tx)?);
    tx.execute(
        "INSERT INTO search_tree_revisions
         (session_id,revision,tree_json,patch_json,source_instance,source_epoch,source_entry_id,cause,cause_ref,actor,created)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![
            session,
            revision,
            serde_json::to_string(&tree).map_err(|e| format!("search-tree encode: {e}"))?,
            canonical,
            source["instance_id"].as_str(),
            source["epoch"].as_i64(),
            source["entry_id"].as_str(),
            cause,
            cause_ref,
            actor,
            crate::models::now()
        ],
    )
    .map_err(|e| format!("search-tree revision insert: {e}"))?;
    tx.execute(
        "INSERT INTO search_tree_head(session_id,revision) VALUES(?1,?2)
         ON CONFLICT(session_id) DO UPDATE SET revision=excluded.revision",
        rusqlite::params![session, revision],
    )
    .map_err(|e| format!("search-tree head update: {e}"))?;
    event(
        tx,
        session,
        "search_tree_updated",
        &leader(tx)?,
        &json!({"revision":revision,"cause":cause,"cause_ref":cause_ref,"node_count":tree["nodes"].as_array().map_or(0,Vec::len),
                "source_entry_id":source["entry_id"]}),
    )?;
    let mut result = decorate(tx, Head { revision, tree, source })?;
    result["added_ids"] = json!(added);
    result["replayed"] = json!(false);
    Ok(result)
}

/// A tree patch can be a best-effort part of compaction. Roll back every
/// partial statement on error so catching that error cannot split the tree.
fn apply_patch(
    tx: &Connection,
    session: &str,
    patch: &Json,
    cause: &str,
    cause_ref: &str,
    actor: &str,
) -> Result<Json, String> {
    tx.execute_batch("SAVEPOINT search_tree_patch").map_err(|e| format!("search-tree savepoint: {e}"))?;
    let result = apply_patch_inner(tx, session, patch, cause, cause_ref, actor);
    match result {
        Ok(value) => {
            if let Err(release) = tx.execute_batch("RELEASE search_tree_patch") {
                let _ = tx.execute_batch("ROLLBACK TO search_tree_patch; RELEASE search_tree_patch");
                return Err(format!("search-tree release: {release}"));
            }
            Ok(value)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO search_tree_patch; RELEASE search_tree_patch")
                .map_err(|rollback| format!("{error}; search-tree rollback failed: {rollback}"))?;
            Err(error)
        }
    }
}

pub(super) fn apply(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    match identity {
        Identity::User => {}
        Identity::Instance(id) if *id == leader(tx)? => {}
        Identity::Instance(_) => return Err("only the Leader may change the session search tree".into()),
        Identity::System => return Err("the system changes a search tree only through compaction".into()),
    }
    let operation_id = params["operation_id"].as_str().ok_or("search-tree operation_id required")?;
    let operations = params["operations"].as_array().ok_or("search-tree operations must be an array")?;
    if operations.is_empty() {
        return Err("an explicit search-tree patch needs at least one operation".into());
    }
    let patch = json!({"base_revision":params["base_revision"],"operations":operations});
    apply_patch(tx, session, &patch, "model_operation", operation_id, &identity.actor())
}

pub(super) fn apply_compaction(
    tx: &Connection,
    session: &str,
    instance: &str,
    request: &str,
    patch: &Json,
) -> Result<Json, String> {
    if instance != leader(tx)? {
        return Err("only Leader compaction updates the session search tree".into());
    }
    apply_patch(tx, session, patch, "compaction", request, "compression")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(operations: Json) -> Json {
        json!({"base_revision":0,"operations":operations})
    }

    fn leader_control(tag: &str) -> (crate::v2::Control, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("teamagents-search-tree-{tag}-{}.db", uuid::Uuid::new_v4()));
        let mut control = crate::v2::Control::open(&path, "s1", true).unwrap();
        control
            .submit(
                crate::v2::Command {
                    command_id: "create-leader".into(),
                    method: "create_instance".into(),
                    params: json!({"id":LEADER,"is_leader":true}),
                },
                Identity::User,
            )
            .unwrap();
        (control, path)
    }

    fn cleanup(control: crate::v2::Control, path: &std::path::Path) {
        drop(control);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    fn apply(
        control: &mut crate::v2::Control,
        command: &str,
        operation: &str,
        base: i64,
        operations: Json,
    ) -> Result<Json, String> {
        control.submit(
            crate::v2::Command {
                command_id: command.into(),
                method: "apply_search_tree".into(),
                params: json!({"operation_id":operation,"base_revision":base,"operations":operations}),
            },
            Identity::Instance(LEADER.into()),
        )
    }

    #[test]
    fn operations_preserve_history_and_reject_cycles() {
        let (tree, added) = apply_operations(
            empty_tree(),
            patch(json!([{"op":"add","parent_id":"root","node":{"title":"Try parser","kind":"attempt"}}]))
                ["operations"]
                .as_array()
                .unwrap(),
            1,
        )
        .unwrap();
        assert_eq!(added, vec!["n1-1"]);
        let error = apply_operations(
            tree,
            patch(json!([
                {"op":"add","parent_id":"n1-1","node":{"title":"Child"}},
                {"op":"move","id":"n1-1","parent_id":"n2-1"},
            ]))["operations"]
                .as_array()
                .unwrap(),
            2,
        )
        .unwrap_err();
        assert!(error.contains("cycle"), "{error}");
    }

    #[test]
    fn merge_reparents_children_and_unions_references() {
        let operations = vec![
            json!({"op":"add","parent_id":"root","node":{"title":"A","source_refs":["entry:a"]}}),
            json!({"op":"add","parent_id":"root","node":{"title":"B","source_refs":["entry:b"]}}),
            json!({"op":"add","parent_id":"n1-2","node":{"title":"B child"}}),
            json!({"op":"merge","from_id":"n1-2","into_id":"n1-1"}),
            json!({"op":"focus","id":"n1-1"}),
        ];
        let (tree, _) = apply_operations(empty_tree(), &operations, 1).unwrap();
        assert_eq!(tree["nodes"].as_array().unwrap().len(), 3);
        assert_eq!(tree["nodes"][1]["source_refs"], json!(["entry:a", "entry:b"]));
        assert_eq!(tree["nodes"][2]["parent_id"], json!("n1-1"));
        assert_eq!(tree["focus"], json!("n1-1"));
    }

    #[test]
    fn text_and_node_bounds_fail_closed() {
        let error = apply_operations(
            empty_tree(),
            &[json!({"op":"add","parent_id":"root","node":{"title":"x".repeat(MAX_TITLE+1)}})],
            1,
        )
        .unwrap_err();
        assert!(error.contains("exceeds"), "{error}");
        let error =
            apply_operations(empty_tree(), &[json!({"op":"add","parent_id":"missing","node":{"title":"orphan"}})], 1)
                .unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
    }

    #[test]
    fn session_tree_is_leader_scoped_current_and_replay_safe() {
        let path = std::env::temp_dir().join(format!("teamagents-search-tree-{}.db", uuid::Uuid::new_v4()));
        let mut control = crate::v2::Control::open(&path, "s1", true).unwrap();
        let submit = |control: &mut crate::v2::Control, id: &str, method: &str, params: Json, identity: Identity| {
            control.submit(crate::v2::Command { command_id: id.into(), method: method.into(), params }, identity)
        };
        submit(&mut control, "create-leader", "create_instance", json!({"id":LEADER,"is_leader":true}), Identity::User)
            .unwrap();
        submit(&mut control, "create-member", "create_instance", json!({"id":"i-member"}), Identity::User).unwrap();
        submit(
            &mut control,
            "input-1",
            "submit_input",
            json!({"instance_id":LEADER,"envelope_id":"e1","text":"investigate the parser"}),
            Identity::User,
        )
        .unwrap();
        let initial = submit(&mut control, "read-0", "read_search_tree", json!({}), Identity::User).unwrap();
        assert_eq!(initial["revision"], json!(0));
        assert_eq!(initial["stale"], json!(true));
        let denied =
            submit(&mut control, "member-read", "read_search_tree", json!({}), Identity::Instance("i-member".into()))
                .unwrap_err();
        assert!(denied.contains("only the Leader"), "{denied}");
        let params = json!({"operation_id":"op-tree-1","base_revision":0,"operations":[
            {"op":"add","parent_id":"root","node":{"title":"Inspect lexer","kind":"attempt","source_refs":["i-leader:0:1"]}}
        ]});
        let applied = submit(
            &mut control,
            "tree-command-1",
            "apply_search_tree",
            params.clone(),
            Identity::Instance(LEADER.into()),
        )
        .unwrap();
        assert_eq!(applied["revision"], json!(1));
        assert_eq!(applied["stale"], json!(false));
        assert_eq!(applied["added_ids"], json!(["n1-1"]));
        let replay = submit(
            &mut control,
            "tree-command-1",
            "apply_search_tree",
            params.clone(),
            Identity::Instance(LEADER.into()),
        )
        .unwrap();
        assert_eq!(replay["revision"], json!(1));
        let cause_replay = submit(
            &mut control,
            "tree-command-2",
            "apply_search_tree",
            params.clone(),
            Identity::Instance(LEADER.into()),
        )
        .unwrap();
        assert_eq!(cause_replay["replayed"], json!(true));
        assert_eq!(cause_replay["added_ids"], json!(["n1-1"]));
        let stale = submit(
            &mut control,
            "tree-stale",
            "apply_search_tree",
            json!({"operation_id":"op-stale","base_revision":0,"operations":[{"op":"focus","id":"root"}]}),
            Identity::Instance(LEADER.into()),
        )
        .unwrap_err();
        assert!(stale.contains("current revision is 1"), "{stale}");
        submit(
            &mut control,
            "input-2",
            "submit_input",
            json!({"instance_id":LEADER,"envelope_id":"e2","text":"compare the second parser"}),
            Identity::User,
        )
        .unwrap();
        let stale = submit(&mut control, "read-stale", "read_search_tree", json!({}), Identity::User).unwrap();
        assert_eq!(stale["stale"], json!(true));
        let focused = submit(
            &mut control,
            "tree-user-focus",
            "apply_search_tree",
            json!({"operation_id":"user-focus","base_revision":1,"operations":[{"op":"focus","id":"n1-1"}]}),
            Identity::User,
        )
        .unwrap();
        assert_eq!(focused["focus"], json!("n1-1"));
        assert_eq!(focused["stale"], json!(false));
        drop(control);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn failed_compaction_patch_rolls_back_its_partial_revision() {
        let (control, path) = leader_control("savepoint");
        control
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_search_tree_head BEFORE INSERT ON search_tree_head
                 BEGIN SELECT RAISE(FAIL, 'forced head failure'); END;",
            )
            .unwrap();
        let patch = json!({"base_revision":0,"operations":[]});
        let error = apply_compaction(control.connection(), "s1", LEADER, "comp-1", &patch).unwrap_err();
        assert!(error.contains("forced head failure"), "{error}");
        let revisions: i64 =
            control.connection().query_row("SELECT COUNT(*) FROM search_tree_revisions", [], |row| row.get(0)).unwrap();
        assert_eq!(revisions, 0, "the failed patch must not leave an orphan revision");
        control.connection().execute_batch("DROP TRIGGER fail_search_tree_head").unwrap();
        let applied = apply_compaction(control.connection(), "s1", LEADER, "comp-2", &patch).unwrap();
        assert_eq!(applied["revision"], json!(1));
        cleanup(control, &path);
    }

    #[test]
    fn human_tree_output_escapes_model_terminal_controls() {
        let (tree, _) = apply_operations(
            empty_tree(),
            &[json!({"op":"add","parent_id":"root","node":{
                "title":"bad\u{1b}[2J\nforged","summary":"osc\u{1b}]52;c;ZWNobw==\u{7}",
                "source_refs":["entry\nspoof"]
            }})],
            1,
        )
        .unwrap();
        let text = render(&json!({"revision":1,"stale":false,"source":{},"focus":tree["focus"],"nodes":tree["nodes"]}));
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{7}'), "{text:?}");
        assert!(!text.contains("\nforged"), "{text:?}");
        assert!(text.contains("\\u{1b}"), "{text:?}");
        assert!(text.contains("\\nforged"), "{text:?}");
        assert!(text.contains("entry\\nspoof"), "{text:?}");
    }

    #[test]
    fn empty_epoch_watermarks_keep_the_actual_epoch_and_leader() {
        let (mut control, path) = leader_control("epochs");
        control.connection().execute("UPDATE instances SET context_epoch=5 WHERE id=?1", [LEADER]).unwrap();
        let first = control
            .submit(
                crate::v2::Command {
                    command_id: "epoch-5".into(),
                    method: "apply_search_tree".into(),
                    params: json!({"operation_id":"epoch-5","base_revision":0,"operations":[{"op":"focus","id":"root"}]}),
                },
                Identity::Instance(LEADER.into()),
            )
            .unwrap();
        assert_eq!(first["source"]["epoch"], json!(5));
        assert_eq!(first["source"]["entry_id"], Json::Null);
        assert_eq!(first["stale"], json!(false));
        control.connection().execute("UPDATE instances SET context_epoch=6 WHERE id=?1", [LEADER]).unwrap();
        let stale = snapshot(control.connection(), "s1").unwrap();
        assert_eq!(stale["stale"], json!(true));
        let second = control
            .submit(
                crate::v2::Command {
                    command_id: "epoch-6".into(),
                    method: "apply_search_tree".into(),
                    params: json!({"operation_id":"epoch-6","base_revision":1,"operations":[{"op":"focus","id":"root"}]}),
                },
                Identity::Instance(LEADER.into()),
            )
            .unwrap();
        assert_eq!(second["source"]["epoch"], json!(6));
        assert_eq!(second["stale"], json!(false));
        cleanup(control, &path);
    }

    #[test]
    fn multi_operation_patches_are_ordered_atomic_and_replayable() {
        let (mut control, path) = leader_control("multi-op");
        let operations = json!([
            {"op":"add","parent_id":"root","node":{"title":"A","kind":"hypothesis"}},
            {"op":"add","parent_id":"n1-1","node":{"title":"B","kind":"attempt"}},
            {"op":"update","id":"n1-2","status":"active","summary":"B is current"},
            {"op":"focus","id":"n1-2"}
        ]);
        let applied = apply(&mut control, "multi-1", "op-multi", 0, operations.clone()).unwrap();
        assert_eq!(applied["revision"], json!(1));
        assert_eq!(applied["added_ids"], json!(["n1-1", "n1-2"]));
        assert_eq!(applied["nodes"][2]["parent_id"], json!("n1-1"));
        assert_eq!(applied["nodes"][2]["status"], json!("active"));
        assert_eq!(applied["focus"], json!("n1-2"));
        let replay = apply(&mut control, "multi-replay", "op-multi", 0, operations).unwrap();
        assert_eq!(replay["replayed"], json!(true));
        assert_eq!(replay["added_ids"], json!(["n1-1", "n1-2"]));
        let before = snapshot(control.connection(), "s1").unwrap();
        let error = apply(
            &mut control,
            "multi-invalid",
            "op-multi-invalid",
            1,
            json!([
                {"op":"update","id":"n1-1","title":"must not survive"},
                {"op":"move","id":"n1-1","parent_id":"missing"}
            ]),
        )
        .unwrap_err();
        assert!(error.contains("missing"), "{error}");
        assert_eq!(snapshot(control.connection(), "s1").unwrap(), before);
        let revisions: i64 =
            control.connection().query_row("SELECT COUNT(*) FROM search_tree_revisions", [], |row| row.get(0)).unwrap();
        assert_eq!(revisions, 1);
        cleanup(control, &path);
    }

    #[test]
    fn arbitrary_unicode_and_controls_keep_json_but_escape_human_output() {
        let (mut control, path) = leader_control("unicode");
        let alphabet =
            ['Ω', 'λ', '🙂', '\u{1b}', '\u{7}', '\n', '\r', '\t', '\0', '\u{2028}', '\u{202e}', '"', '\\', 'a'];
        for index in 0..64usize {
            let generated: String = (0..24).map(|offset| alphabet[(index * 7 + offset * 3) % alphabet.len()]).collect();
            let title = format!("branch-{index}-\nFORGED-{generated}");
            let summary = format!("summary-{generated}");
            let reference = format!("ref-{index}\n\u{1b}{generated}");
            let applied = apply(
                &mut control,
                &format!("unicode-command-{index}"),
                &format!("unicode-op-{index}"),
                index as i64,
                json!([{"op":"add","parent_id":"root","node":{"title":title,"summary":summary,"source_refs":[reference]}}]),
            )
            .unwrap();
            let node = applied["nodes"].as_array().unwrap().last().unwrap();
            // text_field normalizes boundary whitespace; all internal Unicode
            // and controls remain exactly as supplied.
            assert_eq!(node["title"], json!(title.trim()));
            assert_eq!(node["summary"], json!(summary.trim()));
            let text = applied["text"].as_str().unwrap();
            for forbidden in ['\u{1b}', '\u{7}', '\u{2028}', '\u{202e}'] {
                assert!(!text.contains(forbidden), "terminal control remained in {text:?}");
            }
            assert!(!text.contains("\nFORGED"), "an injected line survived: {text:?}");
        }
        cleanup(control, &path);
    }

    #[test]
    fn compaction_patch_rolls_back_revision_and_event_failures() {
        for (tag, trigger) in [
            (
                "revision",
                "CREATE TRIGGER fail_tree_revision BEFORE INSERT ON search_tree_revisions
                 BEGIN SELECT RAISE(FAIL, 'forced revision failure'); END;",
            ),
            (
                "event",
                "CREATE TRIGGER fail_tree_event BEFORE INSERT ON events
                 WHEN NEW.kind = 'search_tree_updated'
                 BEGIN SELECT RAISE(FAIL, 'forced event failure'); END;",
            ),
        ] {
            let (control, path) = leader_control(tag);
            control.connection().execute_batch(trigger).unwrap();
            let patch = json!({"base_revision":0,"operations":[]});
            let error =
                apply_compaction(control.connection(), "s1", LEADER, &format!("comp-{tag}"), &patch).unwrap_err();
            assert!(error.contains("forced"), "{error}");
            for table in ["search_tree_revisions", "search_tree_head"] {
                let count: i64 = control
                    .connection()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                    .unwrap();
                assert_eq!(count, 0, "{tag} left partial {table}");
            }
            cleanup(control, &path);
        }
    }
}
