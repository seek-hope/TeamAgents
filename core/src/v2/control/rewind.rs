//! A conversation checkpoint names an immutable epoch and the prefix just before a delivered
//! Leader prompt. The first compaction covering each row is stored once, so its summary's index
//! recovers visibility at any checkpoint without copying the entire context on every input.
use super::*;
use sha2::{Digest, Sha256};

/// Host-captured regular files keyed by workspace-relative path. Only the storage worker supplies this.
pub struct FileSnapshot {
    pub root: String,
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
}

pub(super) fn may_deliver(conn: &Connection, method: &str, params: &Json, identity: &Identity) -> Result<bool, String> {
    if !matches!(method, "submit_input" | "send_rewind_draft" | "drain_inbox") {
        return Ok(false);
    }
    let Some((leader, epoch)) = conn
        .query_row("SELECT id,context_epoch FROM instances WHERE is_leader=1", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .optional()
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    Ok(match method {
        "drain_inbox" => {
            params["instance_id"] == leader
                && (matches!(identity,Identity::User) || matches!(identity,Identity::Instance(id) if *id==leader))
                && conn.query_row("SELECT EXISTS(SELECT 1 FROM envelopes WHERE recipient=?1 AND epoch=?2 AND kind='user_input' AND state IN ('ACCEPTED','AFTER_WORK'))",rusqlite::params![leader,epoch],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?
                && inbox_ready(conn, &leader, epoch)?
        }
        "send_rewind_draft" => matches!(identity, Identity::User),
        _ => matches!(identity, Identity::User) && params["instance_id"] == leader && params["after"] != "turn",
    })
}

pub(super) fn bind_snapshot(tx: &Connection, start: i64, snapshot: Result<FileSnapshot, String>) -> Result<(), String> {
    let exists: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM rewind_points WHERE rowid>?1)", [start], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(());
    }
    let manifest = match snapshot {
        Err(error) => json!({"error":error}),
        Ok(snapshot) => {
            let mut files = std::collections::BTreeMap::new();
            for (path, bytes) in snapshot.files {
                let digest = format!("{:x}", Sha256::digest(&bytes));
                tx.execute(
                    "INSERT OR IGNORE INTO rewind_blobs(digest,body) VALUES(?1,?2)",
                    rusqlite::params![digest, bytes],
                )
                .map_err(|e| format!("store rewind bytes: {e}"))?;
                files.insert(path, digest);
            }
            json!({"root":snapshot.root,"files":files})
        }
    };
    tx.execute(
        "INSERT INTO rewind_manifests(envelope_id,manifest) SELECT envelope_id,?2 FROM rewind_points WHERE rowid>?1",
        rusqlite::params![start, manifest.to_string()],
    )
    .map_err(|e| format!("bind delivery snapshot: {e}"))?;
    Ok(())
}

pub(super) fn capture(
    tx: &Connection,
    instance: &str,
    epoch: i64,
    envelope: &str,
    payload: &Json,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO rewind_points(instance_id,epoch,prompt_idx,envelope_id,payload_json)
        SELECT i.id,?2,c.idx,?3,?4 FROM instances i JOIN context_entries c ON c.instance_id=i.id
        WHERE i.is_leader=1 AND i.context_epoch=?2 AND i.id=?1 AND c.epoch=?2 AND c.envelope_id=?3",
        rusqlite::params![instance, epoch, envelope, payload.to_string()],
    )
    .map_err(|e| format!("capture Leader rewind point: {e}"))?;
    Ok(())
}

pub fn points(conn: &Connection, before: i64) -> Result<Json, String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.epoch,p.prompt_idx,p.envelope_id,substr(json_extract(p.payload_json,'$.text'),1,120)
        FROM rewind_points p JOIN instances i ON i.id=p.instance_id AND i.is_leader=1 AND i.context_epoch=p.epoch
        WHERE p.prompt_idx<?1 ORDER BY p.prompt_idx DESC LIMIT 25",
        )
        .map_err(|e| e.to_string())?;
    let rows=stmt.query_map([before],|r|Ok(json!({"epoch":r.get::<_,i64>(0)?,"position":r.get::<_,i64>(1)?,"envelope":r.get::<_,String>(2)?,"preview":r.get::<_,String>(3)?}))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    Ok(json!({"points":rows,"next_before":rows.last().map(|r| &r["position"])}))
}

fn point(conn: &Connection, key: &str) -> Result<(String, i64, i64, i64, String), String> {
    conn.query_row(
        "SELECT p.instance_id,p.epoch,p.prompt_idx,i.revision,p.payload_json FROM rewind_points p
        JOIN instances i ON i.id=p.instance_id AND i.is_leader=1 AND i.context_epoch=p.epoch
        WHERE p.envelope_id=?1",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "no such applied Leader input in the current epoch".into())
}

pub fn preview(conn: &Connection, key: &str) -> Result<Json, String> {
    let (instance, epoch, position, revision, payload) = point(conn, key)?;
    let head = context_head(conn, &instance)?;
    let payload: Json = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
    Ok(
        json!({"mode":"conversation","epoch":epoch,"revision":revision,"head":head,"position":position,"envelope":key,"draft":payload,
        "files_changed":false}),
    )
}

pub(super) fn apply(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    apply_mode(tx, session, params, false)
}

pub(super) fn check_attempt(tx: &Connection, id: &str) -> Result<(), String> {
    let seen: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM rewind_attempts WHERE command_id=?1)", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if seen {
        Err("file rewind already attempted without a completion receipt; inspect files and preview again".into())
    } else {
        Ok(())
    }
}

type Point = (String, i64, i64, i64, String);
pub(super) fn validate(tx: &Connection, session: &str, params: &Json, combined: bool) -> Result<Point, String> {
    if !combined && params.get("mode").is_some_and(|m| m != "conversation") {
        return Err("this rewind only changes the conversation; files were not restored".into());
    }
    let key = params["envelope"].as_str().ok_or("select a rewind point")?;
    let point = point(tx, key)?;
    let (ref id, epoch, _, revision, _) = point;
    let (owner, _, lifecycle, phase, _) = load_instance(tx, id)?;
    if params["head"].as_i64() != Some(context_head(tx, id)?)
        || owner != session
        || params["epoch"].as_i64() != Some(epoch)
        || params["revision"].as_i64() != Some(revision)
    {
        return Err("rewind preview is stale; preview again".into());
    }
    let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM work_scopes WHERE status='ACTIVE') OR
        EXISTS(SELECT 1 FROM tasks WHERE status NOT IN ('SUCCEEDED','FAILED','CANCELLED')) OR
        EXISTS(SELECT 1 FROM model_requests WHERE status='PENDING') OR
        EXISTS(SELECT 1 FROM operations WHERE status IN ('PREPARED','DISPATCH_COMMITTED','RUNNING','OUTCOME_UNKNOWN')) OR
        EXISTS(SELECT 1 FROM approvals WHERE status='PENDING') OR EXISTS(SELECT 1 FROM capability_requests WHERE status='PENDING') OR
        EXISTS(SELECT 1 FROM waits WHERE status='PENDING') OR EXISTS(SELECT 1 FROM meta WHERE key='manual_compaction') OR
        EXISTS(SELECT 1 FROM work_inputs WHERE instance_id=?1 AND epoch=?2 AND state='PENDING') OR
        EXISTS(SELECT 1 FROM envelopes e JOIN instances i ON i.id=e.recipient AND i.context_epoch=e.epoch
        WHERE i.lifecycle!='TERMINATED' AND e.state IN ('ACCEPTED','DEFERRED','AFTER_WORK'))",rusqlite::params![id,epoch],|r|r.get(0)).map_err(|e|e.to_string())?;
    if busy || phase != "READY" || lifecycle == "TERMINATED" {
        return Err(
            "rewind needs a quiet session: settle pending work, deliveries, requests and uncertain effects first"
                .into(),
        );
    }
    if combined && !workspace_takeover_candidates(tx, id)?.is_empty() {
        return Err("confirm execution has stopped before restoring workspace files".into());
    }
    Ok(point)
}

pub(super) fn apply_mode(tx: &Connection, session: &str, params: &Json, combined: bool) -> Result<Json, String> {
    let (id, epoch, pos, _, payload) = validate(tx, session, params, combined)?;
    let key = params["envelope"].as_str().ok_or("select a rewind point")?;
    let next = epoch.checked_add(1).ok_or("context epoch exhausted")?;
    // A row is visible if it wasn't yet covered at the prompt. Keep the original indexes: the
    // driver orders summaries first and the remaining rows by idx, including call/result pairs.
    // Refuse missing cover records rather than silently inventing part of the model's context.
    let orphan:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM context_entries c WHERE c.instance_id=?1 AND c.epoch=?2 AND c.idx<?3 AND c.compressed_by IS NOT NULL AND NOT EXISTS(SELECT 1 FROM context_entries s WHERE s.instance_id=c.instance_id AND s.epoch=c.epoch AND s.id=c.compressed_by AND s.kind='summary'))",rusqlite::params![id,epoch,pos],|r|r.get(0)).map_err(|e|e.to_string())?;
    if orphan {
        return Err("rewind point references a missing summary".into());
    }
    // Keep earlier covered rows and their point identities too: a second rewind can move
    // further back along the retained prefix without ever traversing the abandoned suffix.
    let restored=tx.execute("INSERT INTO context_entries(instance_id,epoch,idx,id,kind,message_json,payload_ref,envelope_id,refs_json,created,compressed_by)
        SELECT c.instance_id,?4,c.idx,printf('%s:%d:%d',c.instance_id,?4,c.idx),c.kind,c.message_json,c.payload_ref,c.envelope_id,c.refs_json,c.created,
          CASE WHEN s.idx<?3 THEN printf('%s:%d:%d',c.instance_id,?4,s.idx) END FROM context_entries c
        LEFT JOIN context_entries s ON s.instance_id=c.instance_id AND s.epoch=c.epoch AND s.id=c.compressed_by
        WHERE c.instance_id=?1 AND c.epoch=?2 AND c.idx<?3",rusqlite::params![id,epoch,pos,next]).map_err(|e|format!("restore conversation prefix: {e}"))?;
    tx.execute("INSERT INTO rewind_points(instance_id,epoch,prompt_idx,envelope_id,payload_json)
        SELECT instance_id,?4,prompt_idx,envelope_id,payload_json FROM rewind_points WHERE instance_id=?1 AND epoch=?2 AND prompt_idx<?3",rusqlite::params![id,epoch,pos,next]).map_err(|e|format!("retain earlier rewind points: {e}"))?;
    let head: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(idx),0) FROM context_entries WHERE instance_id=?1 AND epoch=?2",
            rusqlite::params![id, next],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE instances SET context_epoch=?2,context_head=?3,revision=revision+1 WHERE id=?1",
        rusqlite::params![id, next, head],
    )
    .map_err(|e| e.to_string())?;
    let position = json!({"instance":id,"epoch":next,"head":head,"envelope":key});
    tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('rewind_draft',?1)", [position.to_string()])
        .map_err(|e| e.to_string())?;
    event(
        tx,
        session,
        "conversation_rewound",
        &id,
        &json!({"old_epoch":epoch,"new_epoch":next,"envelope":key,"position":pos,"restored":restored,"mode":if combined {"conversation_and_files"} else {"conversation"}}),
    )?;
    let payload: Json = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
    Ok(json!({"epoch":next,"envelope":key,"draft":payload,"submitted":false,"files_changed":false}))
}

fn held_draft(conn: &Connection) -> Result<Option<(String, String, String)>, String> {
    conn.query_row("SELECT i.id,p.envelope_id,p.payload_json FROM meta m JOIN instances i ON i.id=json_extract(m.value,'$.instance') AND i.is_leader=1 AND i.context_epoch=json_extract(m.value,'$.epoch') AND i.context_head=json_extract(m.value,'$.head') JOIN rewind_points p ON p.instance_id=i.id AND p.epoch=i.context_epoch-1 AND p.envelope_id=json_extract(m.value,'$.envelope') WHERE m.key='rewind_draft'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())
}

pub fn draft(conn: &Connection) -> Result<Json, String> {
    let Some((_, key, raw)) = held_draft(conn)? else { return Ok(Json::Null) };
    let payload: Json = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    Ok(json!({"envelope":key,"draft":payload}))
}

/// An explicitly resubmitted edited prompt may reuse only the just-restored prompt's immutable
/// snapshots. Never resolve its labels in the live filesystem or accept client-supplied descriptors.
pub(super) fn send(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    let source = params["source"].as_str().ok_or("rewind source required")?;
    let envelope = params["envelope_id"].as_str().ok_or("new envelope required")?;
    let text = params["text"].as_str().ok_or("edited text required")?;
    let (id, _, raw) = held_draft(tx)?
        .filter(|(_, key, _)| key == source)
        .ok_or("rewind draft is no longer current; no input was sent")?;
    let mut arg = json!({"instance_id":id,"envelope_id":envelope,"text":text});
    let payload: Json = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    copy_input_attachments(&payload, &mut arg);
    if let Some(checks) = payload.get("checks") {
        arg["checks"] = checks.clone();
    }
    let result = submit_input(tx, session, &arg, identity)?;
    tx.execute("INSERT INTO input_attachments(envelope_id,ordinal,artifact_id) SELECT ?1,ordinal,artifact_id FROM input_attachments WHERE envelope_id=?2",rusqlite::params![envelope,source]).map_err(|e|e.to_string())?;
    tx.execute("DELETE FROM meta WHERE key='rewind_draft'", []).map_err(|e| e.to_string())?;
    Ok(result)
}
