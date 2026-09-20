//! Human-only inspection of persisted member records. Native Codex history is
//! read by a separate read-only client; never run a member, materialize a model
//! context, migrate files, or acknowledge deliveries here.

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PAGE: usize = 40;
const TEXT_PAGE: usize = 12_000;
// ponytail: parse one bounded persisted JSON file per request, not all member
// checkpoints. A streaming index can lift this ceiling without hiding records.
const MAX_FILE: u64 = 32 * 1024 * 1024;
const NOFOLLOW: i32 = 0x20000;
const DIRECTORY: i32 = 0x10000;
const NONBLOCK: i32 = 0x800;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub agent_id: Option<String>,
    pub source: Option<String>,
    pub item: Option<String>,
    #[serde(default)]
    pub offset: usize,
    pub revision: Option<String>,
    pub through: Option<i64>,
    pub cursor: Option<String>,
}

fn error(e: impl std::fmt::Display) -> String {
    format!("历史读取失败：{e}")
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("历史读取已取消".into())
    } else {
        Ok(())
    }
}

fn valid_id(id: &str) -> Result<(), String> {
    crate::sessions::validate_session_id(id)
}

fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn directory(parent: &File, name: &str) -> Result<File, String> {
    directory_optional(parent, name)?.ok_or_else(|| format!("历史目录不存在：{name}"))
}

fn directory_optional(parent: &File, name: &str) -> Result<Option<File>, String> {
    match OpenOptions::new().read(true).custom_flags(DIRECTORY | NOFOLLOW | NONBLOCK).open(fd_path(parent).join(name)) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error(e)),
    }
}

fn member_file(session: &File, agent: &str, run: Option<&str>, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(members) = directory_optional(session, "members")? else { return Ok(None) };
    let Some(member) = directory_optional(&members, agent)? else { return Ok(None) };
    let parent = if run.is_some() {
        let Some(turns) = directory_optional(&member, "turns")? else { return Ok(None) };
        turns
    } else {
        member
    };
    let file = match OpenOptions::new().read(true).custom_flags(NOFOLLOW | NONBLOCK).open(fd_path(&parent).join(name)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    if !file.metadata().map_err(error)?.is_file() {
        return Err("历史记录不是普通文件".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("历史文件超过 32 MiB 浏览上限；未截断冒充完整记录，请在本地检查原文件".into());
    }
    Ok(Some(bytes))
}

fn revision(bytes: &[u8], expected: Option<&str>) -> Result<String, String> {
    let revision = format!("{:x}", Sha256::digest(bytes));
    if expected.is_some_and(|expected| expected != revision) {
        return Err("记录已变化，请按 r 刷新后重新选择；未混合不同版本的页面".into());
    }
    Ok(revision)
}

fn page(entries: Vec<Json>, offset: usize, next: Option<usize>, revision: Option<&str>, through: Option<i64>) -> Json {
    json!({"entries":entries,"offset":offset,"next_offset":next,"revision":revision,"through":through})
}

fn missing_page(message: &str) -> Json {
    let mut page = page(Vec::new(), 0, None, None, None);
    page["warning"] = json!(message);
    page
}

fn text(value: &Json, request: &Request, expected: Option<&str>) -> Result<Json, String> {
    let text = serde_json::to_string_pretty(value).map_err(error)?;
    let rev = revision(text.as_bytes(), expected)?;
    let total = text.chars().count();
    if request.offset > total || (request.offset > 0 && request.revision.is_none()) {
        return Err("正文分页需要有效版本和偏移".into());
    }
    let chunk: String = text.chars().skip(request.offset).take(TEXT_PAGE).collect();
    let end = request.offset.saturating_add(TEXT_PAGE).min(total);
    Ok(json!({"text":chunk,"offset":request.offset,"total_chars":total,
        "next_offset":(end < total).then_some(end),"revision":rev}))
}

fn detail_page(value: &Json, request: &Request, source_revision: &str) -> Result<Json, String> {
    let rendered = serde_json::to_string_pretty(value).map_err(error)?;
    if request.offset > 0 && request.revision != Some(source_revision.to_string()) {
        return Err("记录已变化，请按 r 刷新后重新选择；未混合不同版本的页面".into());
    }
    let total = rendered.chars().count();
    if request.offset > total {
        return Err("正文分页偏移不存在".into());
    }
    let end = request.offset.saturating_add(TEXT_PAGE).min(total);
    Ok(json!({"text":rendered.chars().skip(request.offset).take(TEXT_PAGE).collect::<String>(),
        "offset":request.offset,"total_chars":total,
        "next_offset":(end < total).then_some(end),"revision":source_revision}))
}

fn detail_page_raw(value: &Json, request: &Request, source_revision: &str) -> Result<Json, String> {
    if request.revision.as_deref().is_some_and(|revision| revision != source_revision) {
        return Err("记录已变化，请按 r 刷新后重新选择；未混合不同版本的页面".into());
    }
    let rendered = serde_json::to_string_pretty(value).map_err(error)?;
    let total = rendered.chars().count();
    if request.offset > total {
        return Err("正文分页偏移不存在".into());
    }
    let end = request.offset.saturating_add(TEXT_PAGE).min(total);
    Ok(json!({"text":rendered.chars().skip(request.offset).take(TEXT_PAGE).collect::<String>(),
        "offset":request.offset,"total_chars":total,
        "next_offset":(end < total).then_some(end),"revision":source_revision}))
}

fn preview(message: &Json) -> String {
    let content = message["content"].as_str().unwrap_or("");
    let preview: String = content.chars().take(100).collect();
    let calls: Vec<&str> = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["function"]["name"].as_str())
        .take(5)
        .collect();
    format!(
        "{} {} {} {}",
        message["role"].as_str().unwrap_or("?"),
        message["tool_call_id"].as_str().unwrap_or(""),
        calls.join(","),
        preview
    )
}

/// Match only fields whose schema can identify a member. Searching every
/// string in a payload would make an ordinary message such as "dev" look
/// like an event about the `dev` member.
fn contains_member_reference(value: &Json, wanted: &str) -> bool {
    const MEMBER_KEYS: &[&str] = &[
        "agent_id",
        "agent_ids",
        "assignee",
        "author",
        "affected_agents",
        "decided_by",
        "from",
        "leader_id",
        "member_id",
        "members",
        "owner",
        "proposer",
        "recipient",
        "recipients",
        "requester",
        "source",
        "subject",
        "subjects",
        "target",
        "targets",
        "to",
    ];
    match value {
        Json::Array(values) => values.iter().any(|value| contains_member_reference(value, wanted)),
        Json::Object(values) => values.iter().any(|(key, value)| {
            if MEMBER_KEYS.contains(&key.as_str()) {
                match value {
                    Json::String(value) => value == wanted,
                    Json::Array(values) => values.iter().any(|value| match value {
                        Json::String(value) => value == wanted,
                        Json::Object(_) | Json::Array(_) => contains_member_reference(value, wanted),
                        _ => false,
                    }),
                    Json::Object(_) => contains_member_reference(value, wanted),
                    _ => false,
                }
            } else {
                // Nested topology operations and observer descriptions may
                // carry one of the explicit fields above.
                contains_member_reference(value, wanted)
            }
        }),
        _ => false,
    }
}

fn conversation(session: &File, request: &Request, cancelled: &AtomicBool) -> Result<Json, String> {
    let tree = request.source.as_deref() == Some("tree");
    let name = if tree { "chat_tree.json" } else { "chat_history.json" };
    let agent = request.agent_id.as_deref().unwrap();
    let (value, rev) = if tree {
        if let Some(bytes) = member_file(session, agent, None, "chat_tree.json")? {
            let rev = revision(&bytes, request.revision.as_deref())?;
            (serde_json::from_slice::<Json>(&bytes).map_err(error)?, rev)
        } else if let Some(bytes) = member_file(session, agent, None, "chat_history.json")? {
            // Legacy sessions have no tree file. Build the equivalent linear
            // node shape in memory; browsing must not trigger lazy migration.
            let legacy_revision = format!("legacy:{:x}", Sha256::digest(&bytes));
            if request.revision.as_deref().is_some_and(|value| value != legacy_revision) {
                return Err("记录已变化，请按 r 刷新后重新选择；未混合不同版本的页面".into());
            }
            let legacy: Json = serde_json::from_slice(&bytes).map_err(error)?;
            let object = legacy.as_object().ok_or("无效的成员历史映射")?;
            let mut converted = serde_json::Map::new();
            for (thread, messages) in object {
                let messages = messages.as_array().ok_or("无效的历史记录数组")?;
                let mut nodes = Vec::with_capacity(messages.len());
                let mut parent = Json::Null;
                for (index, message) in messages.iter().enumerate() {
                    let id = format!("n{}", index + 1);
                    nodes.push(json!({"id":id,"parent":parent,"skip_to":Json::Null,"message":message}));
                    parent = json!(id);
                }
                converted.insert(thread.clone(), json!({"nodes":nodes,"leaf":parent,"rewind_epoch":0}));
            }
            (Json::Object(converted), legacy_revision)
        } else {
            return Ok(missing_page("该成员没有本地对话树或线性历史；后端未提供可读的完整记录"));
        }
    } else {
        let Some(bytes) = member_file(session, agent, None, "chat_history.json")? else {
            return Ok(missing_page("该成员没有本地线性历史；后端未提供可读的完整记录"));
        };
        let rev = revision(&bytes, request.revision.as_deref())?;
        (serde_json::from_slice::<Json>(&bytes).map_err(error)?, rev)
    };
    if request.offset > 0 && request.revision.is_none() {
        return Err("对话分页需要记录版本".into());
    }
    let threads = value.as_object().ok_or("无效的成员历史映射")?;
    let selected = request.item.as_deref().map(|id| id.parse::<usize>().map_err(error)).transpose()?;
    let mut index = 0usize;
    let mut entries = vec![];
    let mut detail = None;
    for (thread, history) in threads {
        check(cancelled)?;
        let records = if tree { &history["nodes"] } else { history };
        let records = records.as_array().ok_or("无效的历史记录数组")?;
        for record in records {
            if selected == Some(index) {
                detail = Some(if tree {
                    json!({"source":name,"context_ref":thread,"leaf":history["leaf"],
                        "rewind_epoch":history["rewind_epoch"],"node":record})
                } else {
                    json!({"source":name,"context_ref":thread,"message_index":index,"message":record})
                });
            }
            if selected.is_none() && index >= request.offset && entries.len() < PAGE {
                let message = if tree { &record["message"] } else { record };
                let node = record["id"].as_str().unwrap_or("");
                let leaf = tree && record["id"].is_string() && record["id"] == history["leaf"];
                entries.push(json!({"id":index.to_string(),
                    "label":format!("{thread} {node}{} · {}", if leaf { " [leaf]" } else { "" }, preview(message))}));
            }
            index += 1;
        }
    }
    if let Some(detail) = detail {
        // Use the entire source revision for every page of a node, so appends,
        // rewinds and compaction never silently relabel a stale node index.
        let mut result = detail_page_raw(&detail, request, &rev)?;
        result["revision"] = json!(rev);
        Ok(result)
    } else if selected.is_some() || request.offset > index {
        Err("历史记录或偏移不存在".into())
    } else {
        Ok(page(
            entries,
            request.offset,
            (request.offset.saturating_add(PAGE) < index).then_some(request.offset + PAGE),
            Some(&rev),
            None,
        ))
    }
}

fn ceiling(conn: &Connection, session: &str, request: &Request, table: &str, key: &str) -> Result<i64, String> {
    if request.offset > 0 && request.through.is_none() {
        return Err("数据库分页需要快照上界".into());
    }
    match request.through {
        Some(n) if n >= 0 => Ok(n),
        Some(_) => Err("无效的历史快照上界".into()),
        None => conn
            .query_row(&format!("SELECT coalesce(max({key}),0) FROM {table} WHERE session_id=?"), [session], |r| {
                r.get(0)
            })
            .map_err(error),
    }
}

fn events(conn: &Connection, session: &str, request: &Request) -> Result<Json, String> {
    let agent = request.agent_id.as_deref().unwrap();
    let through = ceiling(conn, session, request, "events", "sequence")?;
    let selected = request.item.as_deref().map(|v| v.parse::<i64>().map_err(error)).transpose()?;

    if let Some(sequence) = selected {
        if sequence <= 0 || sequence > through {
            return Err("无效事件序号".into());
        }
        let row = conn
            .query_row(
                "SELECT sequence,event_id,actor_id,task_id,kind,payload_json,audience_json,topology_revision,causation_id,created_at
                 FROM events WHERE session_id=?1 AND sequence=?2",
                params![session, sequence],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, f64>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(error)?
            .ok_or("成员相关事件不存在")?;
        if row.5.len() as u64 > MAX_FILE || row.6.len() as u64 > MAX_FILE {
            return Err("单条事件超过 32 MiB 浏览上限".into());
        }
        let payload: Json = serde_json::from_str(&row.5).map_err(error)?;
        let audience: Json = serde_json::from_str(&row.6).map_err(error)?;
        let related = row.2 == agent
            || contains_member_reference(&payload, agent)
            || audience.as_array().is_some_and(|values| values.iter().any(|id| id == agent));
        if !related {
            return Err("成员相关事件不存在".into());
        }
        let value = json!({"sequence":row.0,"event_id":row.1,"actor_id":row.2,
            "task_id":row.3,"kind":row.4,"payload":payload,"audience":audience,
            "topology_revision":row.7,"causation_id":row.8,"created_at":row.9});
        return text(&value, request, request.revision.as_deref());
    }

    // Scan a bounded sequence window, rather than an unbounded search for a
    // sparse member. Stop when a full page is assembled; the sequence we
    // stopped at is the next cursor, so interleaved unrelated events are not
    // skipped.
    let mut stmt = conn.prepare("SELECT sequence,event_id,actor_id,task_id,kind,payload_json,audience_json,topology_revision,causation_id,created_at FROM events WHERE session_id=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 100").map_err(error)?;
    let start = i64::try_from(request.offset).map_err(error)?;
    let mut rows = stmt.query(params![session, start, through]).map_err(error)?;
    let mut entries = vec![];
    let mut last = start;
    while let Some(row) = rows.next().map_err(error)? {
        let sequence: i64 = row.get(0).map_err(error)?;
        last = sequence;
        let actor: String = row.get(2).map_err(error)?;
        let parse = |i| -> Result<Json, String> {
            let value = row.get_ref(i).map_err(error)?.as_str().map_err(error)?;
            if value.len() as u64 > MAX_FILE {
                return Err("单条事件超过 32 MiB 浏览上限".into());
            }
            serde_json::from_str(value).map_err(error)
        };
        let payload = parse(5)?;
        let audience = parse(6)?;
        let related = actor == agent
            || contains_member_reference(&payload, agent)
            || audience.as_array().is_some_and(|a| a.iter().any(|id| id == agent));
        if !related {
            continue;
        }
        let kind: String = row.get(4).map_err(error)?;
        entries.push(json!({"id":sequence.to_string(),"label":format!("#{sequence} {kind} · {actor}")}));
        if entries.len() == PAGE {
            break;
        }
    }
    Ok(page(entries, request.offset, (last < through).then_some(last as usize), None, Some(through)))
}

fn runs(conn: &Connection, session_id: &str, session: &File, request: &Request) -> Result<Json, String> {
    let agent = request.agent_id.as_deref().unwrap();
    if let Some(run) = &request.item {
        valid_id(run)?;
        let metadata = conn.query_row("SELECT context_ref,status,external_turn_id,created_at,updated_at FROM turn_runs WHERE session_id=? AND agent_id=? AND run_id=?", params![session_id,agent,run], |r| {
            Ok(json!({"run_id":run,"context_ref":r.get::<_,Option<String>>(0)?,"status":r.get::<_,String>(1)?,"external_turn_id":r.get::<_,Option<String>>(2)?,"created_at":r.get::<_,f64>(3)?,"updated_at":r.get::<_,f64>(4)?}))
        }).optional().map_err(error)?.ok_or("成员回合不存在")?;
        let bytes = member_file(session, agent, Some(run), &format!("{run}.json"))?;
        let (value, source_revision) = match bytes {
            Some(bytes) => {
                let source_revision = format!("{:x}", Sha256::digest(&bytes));
                (
                    json!({"run":metadata,"checkpoint":serde_json::from_slice::<Json>(&bytes).map_err(error)?}),
                    source_revision,
                )
            }
            None => {
                let source = serde_json::to_vec(&metadata).map_err(error)?;
                let source_revision = format!("{:x}", Sha256::digest(&source));
                (
                    json!({"run":metadata,"checkpoint":Json::Null,
                        "warning":"检查点不可用（该成员后端未在本地保存 Chat 检查点）"}),
                    source_revision,
                )
            }
        };
        return detail_page(&value, request, &source_revision);
    }
    let through = ceiling(conn, session_id, request, "turn_runs", "rowid")?;
    let offset = i64::try_from(request.offset).map_err(error)?;
    let mut stmt = conn.prepare("SELECT rowid,run_id,status,context_ref FROM turn_runs WHERE session_id=? AND agent_id=? AND rowid>? AND rowid<=? ORDER BY rowid LIMIT 41").map_err(error)?;
    let rows = stmt.query_map(params![session_id,agent,offset,through], |r| Ok((r.get::<_,i64>(0)?,json!({"id":r.get::<_,String>(1)?,"label":format!("{} {} · {}",r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?.unwrap_or_default())})))).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    let next = (rows.len() > PAGE).then(|| rows[PAGE - 1].0 as usize);
    Ok(page(rows.into_iter().take(PAGE).map(|(_, v)| v).collect(), request.offset, next, None, Some(through)))
}

fn codex(session: &File, thread: Option<&str>, request: &Request, cancelled: &AtomicBool) -> Result<Json, String> {
    let Some(thread) = thread.filter(|id| !id.is_empty()) else {
        return Ok(missing_page("该成员尚未保存 Codex 线程 ID，无法读取原生记录"));
    };
    if request.item.is_none() && request.offset > 0 && request.cursor.is_none() {
        return Err("Codex 历史分页需要有效游标".into());
    }
    let root = std::fs::canonicalize(fd_path(session)).map_err(error)?;
    let source = crate::codex::read_history_page(&root, thread, request.cursor.as_deref(), cancelled)?;
    if request.revision.as_deref().is_some_and(|expected| expected != source.revision) {
        return Err("Codex 记录已变化，请按 r 刷新后重新选择；未混合不同版本的页面".into());
    }
    let item_id = |entry: &Json| {
        if let Some(line) = entry["record_line"].as_u64() {
            json!(["rollout", line]).to_string()
        } else {
            json!(["native", entry["turnId"], entry["item"]["id"]]).to_string()
        }
    };
    if let Some(id) = &request.item {
        if request.revision.is_none() {
            return Err("Codex 条目详情需要列表版本，请先刷新并选择条目".into());
        }
        let entry = source.items.iter().find(|entry| item_id(entry) == *id).ok_or("Codex 条目不在所选历史页中")?;
        return detail_page_raw(&json!({"source":"codex","thread_id":thread,"entry":entry}), request, &source.revision);
    }
    let entries: Vec<Json> = source
        .items
        .iter()
        .map(|entry| {
            let item = &entry["item"];
            let text = item["text"]
                .as_str()
                .or_else(|| item["command"].as_str())
                .or_else(|| item["output"].as_str())
                .or_else(|| item["arguments"].as_str())
                .or_else(|| item["input"].as_str())
                .or_else(|| item["content"].as_array().and_then(|parts| parts.first()?.get("text")?.as_str()))
                .unwrap_or("");
            let role_or_tool = item["name"].as_str().or_else(|| item["role"].as_str()).unwrap_or("");
            json!({"id":item_id(entry),"label":format!("{} · {} {} · {}",
                entry["turnId"].as_str().unwrap_or("—"), item["type"].as_str().unwrap(),
                role_or_tool,
                text.chars().take(100).collect::<String>())})
        })
        .collect();
    let next = source
        .next_cursor
        .as_ref()
        .map(|_| request.offset.checked_add(entries.len()).ok_or("历史偏移过大"))
        .transpose()?;
    let mut result = page(entries, request.offset, next, Some(&source.revision), None);
    result["next_cursor"] = json!(source.next_cursor);
    Ok(result)
}

fn read_inner(
    conn: &Connection,
    session_id: &str,
    session: &File,
    request: &Request,
    cancelled: &AtomicBool,
) -> Result<Json, String> {
    check(cancelled)?;
    if request.cursor.is_some() && request.source.as_deref() != Some("codex") {
        return Err("该历史来源不接受 Codex 分页游标".into());
    }
    let Some(agent) = &request.agent_id else {
        if request.source.is_some() || request.item.is_some() {
            return Err("历史请求缺少成员 ID".into());
        }
        let mut stmt = conn
            .prepare("SELECT agent_id,status FROM agent_runtime WHERE session_id=? ORDER BY agent_id")
            .map_err(error)?;
        let all = stmt
            .query_map([session_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(error)?;
        let snapshot = serde_json::to_vec(&all).map_err(error)?;
        let rev = revision(&snapshot, request.revision.as_deref())?;
        if request.offset > all.len() || (request.offset > 0 && request.revision.is_none()) {
            return Err("无效的成员分页".into());
        }
        let entries = all
            .iter()
            .skip(request.offset)
            .take(PAGE)
            .map(|(id, status)| json!({"id":id,"label":format!("{id} · {status}")}))
            .collect();
        return Ok(page(
            entries,
            request.offset,
            (request.offset + PAGE < all.len()).then_some(request.offset + PAGE),
            Some(&rev),
            None,
        ));
    };
    valid_id(agent)?;
    let thread: Option<Option<String>> = conn
        .query_row(
            "SELECT external_thread_id FROM agent_runtime WHERE session_id=? AND agent_id=?",
            params![session_id, agent],
            |r| r.get(0),
        )
        .optional()
        .map_err(error)?;
    let Some(thread) = thread else {
        return Err("成员不存在（包括已移除记录）".into());
    };
    match request.source.as_deref() {
        None if request.item.is_none() && request.offset == 0 => {
            let mut sources = vec![
                json!({"id":"tree","label":"chat_tree.json"}),
                json!({"id":"snapshot","label":"chat_history.json"}),
                json!({"id":"turns","label":"turn_runs / turns/*.json"}),
                json!({"id":"events","label":"events"}),
            ];
            if thread.as_deref().is_some_and(|id| !id.is_empty()) {
                sources.insert(0, json!({"id":"codex","label":"Codex 原生对话与工具记录"}));
            }
            Ok(page(sources, 0, None, None, None))
        }
        Some("tree" | "snapshot") => conversation(session, request, cancelled),
        Some("turns") => runs(conn, session_id, session, request),
        Some("events") => events(conn, session_id, request),
        Some("codex") => codex(session, thread.as_deref(), request, cancelled),
        _ => Err("无效的历史来源".into()),
    }
}

/// Separate read-only SQLite connection: large history reads never hold the
/// core's control mutex. No model-facing tool or core method exposes this API.
pub fn read(session_id: &str, request: &Request, cancelled: &AtomicBool) -> Result<Json, String> {
    valid_id(session_id)?;
    check(cancelled)?;
    let root = File::open(crate::config::sessions_dir()).map_err(error)?;
    let session = directory(&root, session_id)?;
    let base = std::fs::canonicalize(fd_path(&session)).map_err(error)?;
    let db = base.join("team.db");
    if !std::fs::symlink_metadata(&db).map_err(error)?.is_file() {
        return Err("会话数据库不是普通文件".into());
    }
    let conn = Connection::open_with_flags(
        Path::new(&db),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    conn.busy_timeout(Duration::from_millis(200)).map_err(error)?;
    conn.execute_batch("PRAGMA query_only=ON; BEGIN").map_err(error)?;
    let interrupt = conn.get_interrupt_handle();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let guard = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done.load(Ordering::SeqCst) {
                if cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    interrupt.interrupt();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let result = read_inner(&conn, session_id, &session, request, cancelled).map(|mut value| {
            value["session_id"] = json!(session_id);
            value["agent_id"] = json!(request.agent_id);
            value["source"] = json!(request.source);
            value["item"] = json!(request.item);
            value["cursor"] = json!(request.cursor);
            value
        });
        done.store(true, Ordering::SeqCst);
        let _ = guard.join();
        check(cancelled)?;
        result
    })
}
