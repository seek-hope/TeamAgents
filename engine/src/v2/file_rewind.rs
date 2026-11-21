//! Preview and restore the Git-tracked working bytes captured at prompt delivery.
//! A check against native edit receipts can detect conflicting later writes.
mod tracked;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

use teamagents_core::v2::{control::rewind, store};

pub(super) async fn enable(storage: &super::storage::Storage, root: &Path, state: &Path) -> Result<(), String> {
    let root = root.to_owned();
    let state = state.to_owned();
    storage.call(move |c| c.set_file_capture(move || tracked::capture(&root, &state))).await
}

struct Edit {
    offset: usize,
    old: String,
    new: String,
}

const MAX_CHANGES: i64 = 256;
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
const READ_BUDGET: u64 = 64 * 1024 * 1024;

struct Change {
    first: String,
    last: String,
    linked: bool,
    edits: Vec<Edit>,
}

type Stamp = (u64, u64, u64, i64, i64, i64, i64);
fn stamp(file: &File) -> Result<Stamp, String> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    Ok((m.len(), m.dev(), m.ino(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()))
}

fn current_bytes(workspace: &Path, path: &str, budget: &mut u64) -> Result<(Vec<u8>, Stamp), String> {
    let file = super::attachments::open_confined(workspace, Path::new(path), false)?;
    let before = stamp(&file)?;
    let limit = FILE_LIMIT.min(*budget);
    if before.0 > limit {
        return Err("file preview byte limit exceeded".into());
    }
    let mut bytes = Vec::new();
    let size = (&file).take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())? as u64;
    *budget = budget.saturating_sub(size);
    if size != before.0 || stamp(&file)? != before {
        return Err("file changed during preview".into());
    }
    Ok((bytes, before))
}

impl Change {
    /// Reverse the recorded edits in memory. Verify both ends of the chain so an unrelated
    /// edit between two native writes cannot disappear from a future restoration.
    fn original_size(&self, mut bytes: Vec<u8>, budget: &mut u64) -> Result<Option<usize>, String> {
        if !self.linked || format!("{:x}", Sha256::digest(&bytes)) != self.last {
            return Ok(None);
        }
        for edit in self.edits.iter().rev() {
            let end = edit.offset.checked_add(edit.new.len()).ok_or("invalid native edit range")?;
            if bytes.get(edit.offset..end) != Some(edit.new.as_bytes()) {
                return Ok(None);
            }
            let size = bytes.len() - edit.new.len() + edit.old.len();
            if size as u64 > FILE_LIMIT {
                return Err("file preview size limit exceeded".into());
            }
            // Bound copying as well as live-file reads for edits near the start of a long file.
            *budget = budget.checked_sub(size as u64).ok_or("file preview byte budget exceeded")?;
            bytes.splice(edit.offset..end, edit.old.bytes());
        }
        Ok((format!("{:x}", Sha256::digest(&bytes)) == self.first).then_some(bytes.len()))
    }
}

fn changes(conn: &Connection, point: &Json) -> Result<(BTreeMap<String, Change>, usize), String> {
    let mut query = conn
        .prepare(
            "SELECT json_extract(o.receipt_json,'$.native_edit.path'),
        json_extract(o.receipt_json,'$.native_edit.before'),json_extract(o.receipt_json,'$.native_edit.after'),
        json_extract(o.receipt_json,'$.native_edit.offset'),
        CASE WHEN length(CAST(o.intent_json AS BLOB))<=?4 THEN json_extract(o.intent_json,'$.args.old_string') END,
        CASE WHEN length(CAST(o.intent_json AS BLOB))<=?4 THEN json_extract(o.intent_json,'$.args.new_string') END
        FROM instances i JOIN context_entries c ON c.instance_id=i.id AND c.epoch=i.context_epoch
        JOIN operations o ON o.operation_id=c.envelope_id
        WHERE i.is_leader=1 AND c.epoch=?1 AND c.idx>?2 AND c.kind='tool_result'
        AND json_extract(o.intent_json,'$.name') IN ('edit_file','edit_files','write_file','delete')
        AND o.status='SUCCEEDED' ORDER BY c.idx LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = query
        .query(rusqlite::params![
            point["epoch"].as_i64(),
            point["position"].as_i64(),
            MAX_CHANGES + 1,
            FILE_LIMIT as i64
        ])
        .map_err(|e| e.to_string())?;
    let mut changes = BTreeMap::<String, Change>::new();
    let mut untracked = 0;
    let mut total = 0;
    let mut remaining = READ_BUDGET;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        total += 1;
        if total > MAX_CHANGES {
            return Err("too many file changes for a single preview".into());
        }
        let read = |n| row.get::<_, Option<String>>(n).map_err(|e| e.to_string());
        let (Some(path), Some(before), Some(after)) = (read(0)?, read(1)?, read(2)?) else {
            untracked += 1;
            continue;
        };
        let (Some(old), Some(new), Some(offset)) =
            (read(4)?, read(5)?, row.get::<_, Option<i64>>(3).map_err(|e| e.to_string())?)
        else {
            return Err("incomplete or oversized native edit record".into());
        };
        let offset = usize::try_from(offset).map_err(|_| "invalid native edit offset")?;
        remaining =
            remaining.checked_sub((old.len() + new.len()) as u64).ok_or("file preview input budget exceeded")?;
        let change = changes.entry(path).or_insert_with(|| Change {
            first: before.clone(),
            last: before.clone(),
            linked: true,
            edits: Vec::new(),
        });
        change.linked &= change.last == before;
        change.last = after;
        change.edits.push(Edit { offset, old, new });
    }
    Ok((changes, untracked))
}

fn manifest(conn: &Connection, key: &str) -> Result<Option<Json>, String> {
    let raw: Option<String> = conn
        .query_row("SELECT manifest FROM rewind_manifests WHERE envelope_id=?1", [key], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    raw.map(|s| serde_json::from_str(&s).map_err(|e| e.to_string())).transpose()
}

struct Plan {
    report: Json,
    observed: BTreeMap<String, Vec<u8>>,
}

fn plan(
    workspace: &Path,
    state: &Path,
    point: &Json,
    manifest: &Json,
    changes: &BTreeMap<String, Change>,
) -> Result<Plan, String> {
    let base = manifest["files"].as_object().ok_or("workspace snapshot is unavailable for this delivery")?;
    let live = tracked::capture(workspace, state)?;
    if manifest["root"] != live.root
        || live.files.len() != base.len()
        || live.files.keys().any(|k| !base.contains_key(k))
    {
        return Err("workspace or Git tracked file set changed since delivery".into());
    }
    let mut files = Vec::new();
    let mut observed = BTreeMap::new();
    let mut applicable = true;
    let mut budget = READ_BUDGET;
    for (path, bytes) in live.files {
        let before = base[&path].as_str().ok_or("invalid workspace manifest digest")?;
        let after = format!("{:x}", Sha256::digest(&bytes));
        if before == after {
            continue;
        }
        if files.len() >= MAX_CHANGES as usize {
            return Err("too many modified files for a single rewind".into());
        }
        let status = if let Some(change) = changes.get(&path) {
            if change.first == before && change.original_size(bytes.clone(), &mut budget)?.is_some() {
                "matches_recorded_edit"
            } else {
                applicable = false;
                "conflict"
            }
        } else {
            "snapshot_difference"
        };
        files.push(json!({"path":path,"status":status,"before":before,"after":after}));
        observed.insert(path, bytes);
    }
    let mut report = json!({"mode":"conversation_and_files","scope":"git_tracked_workspace","envelope":point["envelope"],"epoch":point["epoch"],"head":point["head"],"revision":point["revision"],"files":files,"apply_available":applicable});
    report["preview_digest"] = json!(format!("{:x}", Sha256::digest(report.to_string().as_bytes())));
    Ok(Plan { report, observed })
}

pub(super) fn apply(
    control: &mut teamagents_core::v2::Control,
    cmd: teamagents_core::v2::Command,
    workspace: &Path,
    state: &Path,
) -> Result<Json, String> {
    use teamagents_core::v2::Identity;
    if let Some(receipt) = control.command_receipt(&cmd, &Identity::User)? {
        return Ok(receipt);
    }
    control.check_file_rewind(&cmd)?;
    let key = cmd.params["envelope"].as_str().ok_or("select a rewind point")?;
    let conn = control.connection();
    let point = rewind::preview(conn, key)?;
    let baseline = manifest(conn, key)?.ok_or("no workspace snapshot for this delivery")?;
    let (changes, _) = changes(conn, &point)?;
    let plan = plan(workspace, state, &point, &baseline, &changes)?;
    if plan.report["apply_available"] != true || cmd.params["preview_digest"] != plan.report["preview_digest"] {
        return Err("workspace rewind preview is stale or conflicted; preview again".into());
    }
    let mut restore = Vec::new();
    for (path, current) in plan.observed {
        let digest = baseline["files"][&path].as_str().ok_or("invalid snapshot digest")?;
        let original: Vec<u8> = conn
            .query_row(
                "SELECT body FROM rewind_blobs WHERE digest=?1 AND length(body)<=?2",
                rusqlite::params![digest, FILE_LIMIT as i64],
                |r| r.get(0),
            )
            .map_err(|e| format!("read delivery snapshot: {e}"))?;
        if format!("{:x}", Sha256::digest(&original)) != digest {
            return Err("workspace snapshot integrity check failed".into());
        }
        restore.push((path, current, original));
    }
    let leader: String = conn
        .query_row("SELECT id FROM instances WHERE is_leader=1", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if Path::new(&leader).file_name() != Some(leader.as_ref()) {
        return Err("invalid Leader workspace owner".into());
    }
    let staged = crate::tools::stage_rewind(workspace, &state.join("instances").join(leader).join("locks"), &restore)?;
    control.submit_file_rewind(cmd, staged)
}

pub(super) fn preview(db: &Path, workspace: &Path, state: &Path, envelope: &str) -> Result<Json, String> {
    let conn = store::open_read_only(db)?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let point = rewind::preview(&tx, envelope)?;
    let (changes, untracked) = changes(&tx, &point)?;
    let manifest = manifest(&tx, envelope)?;
    tx.commit().map_err(|e| e.to_string())?;
    if let Some(manifest) = &manifest {
        if manifest.get("files").is_some() {
            return Ok(plan(workspace, state, &point, manifest, &changes)?.report);
        }
    }
    let mut budget = READ_BUDGET;
    let mut files = Vec::new();
    for (path, change) in changes {
        let (status, original_size, reason) = match current_bytes(workspace, &path, &mut budget) {
            Err(error) => ("unavailable", None, Some(error)),
            Ok((bytes, _)) => match change.original_size(bytes, &mut budget) {
                Ok(Some(size)) => ("matches_recorded_edit", Some(size), None),
                Ok(None) => ("conflict", None, None),
                Err(error) => ("unavailable", None, Some(error)),
            },
        };
        files.push(json!({"path":path,"before":change.first,"after":change.last,"edits":change.edits.len(),
            "status":status,"reconstructed_bytes":original_size,"reason":reason}));
    }
    Ok(
        json!({"mode":"conversation_and_files","scope":"recorded_leader_edits","envelope":envelope,"position":point["position"],
        "epoch":point["epoch"],"head":point["head"],"revision":point["revision"],"files":files,
        "untracked_file_operations":untracked,"snapshot_error":manifest.and_then(|v|v["error"].as_str().map(str::to_owned)).unwrap_or_else(||"no workspace snapshot for this delivery".into()),"apply_available":false}),
    )
}
