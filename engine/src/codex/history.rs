//! Human-only reads of the stored member thread. This client never resumes a
//! thread or submits a turn; it is separate from the executing member client.

use super::{AppServerOptions, CodexAppServer, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PAGE: usize = 40;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
const NOFOLLOW: i32 = 0x20000;
const DIRECTORY: i32 = 0x10000;
const NONBLOCK: i32 = 0x800;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Cursor {
    Items { thread: String, cursor: String },
    Legacy { thread: String, offset: usize, revision: String },
}

pub(crate) struct HistoryPage {
    pub items: Vec<Json>,
    pub next_cursor: Option<String>,
    pub revision: String,
}

fn digest(value: &Json) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("JSON value")))
}

fn cursor(value: Cursor) -> String {
    serde_json::to_string(&value).expect("history cursor")
}

fn checked_items(items: &[Json]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for entry in items {
        let turn = entry["turnId"].as_str().filter(|id| !id.is_empty()).ok_or("Codex 历史缺少回合 ID")?;
        let item = entry["item"]["id"].as_str().filter(|id| !id.is_empty()).ok_or("Codex 历史缺少条目 ID")?;
        entry["item"]["type"].as_str().filter(|kind| !kind.is_empty()).ok_or("Codex 历史缺少条目类型")?;
        if !ids.insert((turn, item)) {
            return Err("Codex 历史包含重复条目，未混合记录".into());
        }
    }
    Ok(())
}

fn open_rollout(cwd: &Path, path: &Path) -> Result<File, String> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").filter(|value| !value.is_empty()).map(|home| PathBuf::from(home).join(".codex"))
        })
        .ok_or("无法确定 Codex 会话目录")?;
    // A relative CODEX_HOME has the same base as the separate app-server.
    let home = if home.is_absolute() { home } else { cwd.join(home) };
    let root = std::fs::canonicalize(&home).map_err(|error| format!("无法打开 Codex 会话目录：{error}"))?;
    let relative = path
        .strip_prefix(&home)
        .or_else(|_| path.strip_prefix(&root))
        .map_err(|_| "Codex rollout 不在配置的会话目录内")?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() < 2
        || !matches!(parts[0], Component::Normal(name) if name == "sessions" || name == "archived_sessions")
        || parts.iter().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Codex rollout 路径必须位于 sessions 或 archived_sessions 内".into());
    }
    let fd_path = |file: &File| PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    let open = |path: &Path, directory: bool| {
        OpenOptions::new()
            .read(true)
            .custom_flags(NOFOLLOW | NONBLOCK | if directory { DIRECTORY } else { 0 })
            .open(path)
            .map_err(|error| format!("无法安全打开 Codex rollout：{error}"))
    };
    let mut parent = open(&root, true)?;
    if std::fs::canonicalize(fd_path(&parent)).map_err(|error| error.to_string())? != root {
        return Err("Codex 会话目录在读取前已变化".into());
    }
    // Pin each directory before following the next component. NOFOLLOW on only
    // the final file would still follow a replaced or symlinked parent.
    for part in &parts[..parts.len() - 1] {
        parent = open(&fd_path(&parent).join(part.as_os_str()), true)?;
    }
    let file = open(&fd_path(&parent).join(parts.last().unwrap().as_os_str()), false)?;
    let actual = std::fs::canonicalize(fd_path(&file)).map_err(|error| error.to_string())?;
    if !["sessions", "archived_sessions"].iter().any(|directory| actual.starts_with(root.join(directory))) {
        return Err("Codex rollout 在读取前已移出会话目录".into());
    }
    if !file.metadata().map_err(|error| error.to_string())?.is_file() {
        return Err("Codex rollout 不是普通文件".into());
    }
    Ok(file)
}

fn rollout_page(
    server: &CodexAppServer,
    path: &Path,
    thread: &str,
    position: Option<&Cursor>,
    cancelled: &AtomicBool,
    expired: &AtomicBool,
) -> Result<HistoryPage, String> {
    let file = open_rollout(&server.cwd, path)?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_RESPONSE as u64 {
        return Err("Codex rollout 超过 32 MiB 浏览上限".into());
    }
    // ponytail: replay one bounded JSONL file per page. A native paginated store
    // or a read-only index can lift this ceiling without hiding old records.
    let mut reader = BufReader::new(file.take(MAX_RESPONSE as u64 + 1));
    let mut total = 0usize;
    let mut record_line = 0usize;
    let mut hash = Sha256::new();
    let mut items = Vec::new();
    let mut current_turn: Option<String> = None;
    let mut line = String::new();
    loop {
        if cancelled.load(Ordering::SeqCst) || expired.load(Ordering::SeqCst) {
            return Err("Codex 历史读取已取消或超时".into());
        }
        line.clear();
        let bytes = reader.read_line(&mut line).map_err(|error| format!("读取 Codex rollout 失败：{error}"))?;
        if bytes == 0 {
            break;
        }
        total += bytes;
        if total > MAX_RESPONSE {
            return Err("Codex rollout 超过 32 MiB 浏览上限".into());
        }
        record_line += 1;
        hash.update(line.as_bytes());
        let record: Json = serde_json::from_str(&line).map_err(|error| format!("Codex rollout JSON 无效：{error}"))?;
        let kind = record["type"].as_str().filter(|kind| !kind.is_empty()).ok_or("Codex rollout 缺少记录类型")?;
        if record_line == 1 {
            let meta = &record["payload"];
            let identity = meta.get("id").or_else(|| meta.get("session_id"));
            if kind != "session_meta"
                || identity.and_then(Json::as_str) != Some(thread)
                || ["id", "session_id"].iter().any(|key| meta.get(key).is_some_and(|id| id.as_str() != Some(thread)))
            {
                return Err("Codex rollout 首条元数据与该成员的持久线程不符".into());
            }
            if meta.get("history_mode").is_some_and(|mode| mode.as_str() != Some("legacy")) {
                return Err(
                    "Codex 线程使用其他历史存储，不能用 rollout 代替其完整条目；请使用支持原生分页的后端".into()
                );
            }
            continue;
        }
        match kind {
            "session_meta" => return Err("Codex rollout 包含重复线程元数据".into()),
            "turn_context" | "event_msg" if kind == "turn_context" || record["payload"]["type"] == "task_started" => {
                current_turn = record["payload"]["turn_id"].as_str().filter(|id| !id.is_empty()).map(str::to_string);
            }
            "event_msg" if matches!(record["payload"]["type"].as_str(), Some("task_complete" | "turn_aborted")) => {
                current_turn = None;
            }
            "response_item" => {
                let item = &record["payload"];
                item["type"].as_str().filter(|kind| !kind.is_empty()).ok_or("Codex rollout 条目缺少类型")?;
                if item.get("id").is_some_and(|id| !id.is_null() && !id.is_string()) {
                    return Err("Codex rollout 条目 ID 格式无效".into());
                }
                let turn = item["internal_chat_message_metadata_passthrough"]["turn_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .or(current_turn.as_deref());
                // Raw ResponseItem IDs are optional. Identify persisted records
                // by source line without inventing IDs or changing their body.
                items.push(json!({"turnId":turn,"record_line":record_line,"item":item}));
            }
            _ => {}
        }
    }
    if record_line == 0 {
        return Err("Codex rollout 缺少线程元数据".into());
    }
    let revision = format!("{:x}", hash.finalize());
    let offset = match position {
        Some(Cursor::Legacy { offset, revision: expected, .. }) if expected == &revision => *offset,
        Some(Cursor::Legacy { .. }) => return Err("Codex 历史已变化，请按 r 刷新后重新选择".into()),
        Some(Cursor::Items { .. }) => return Err("Codex 历史分页游标与 rollout 回退格式不兼容".into()),
        None => 0,
    };
    if offset > items.len() {
        return Err("Codex 历史分页偏移不存在".into());
    }
    let end = offset.saturating_add(PAGE).min(items.len());
    Ok(HistoryPage {
        items: items[offset..end].to_vec(),
        next_cursor: (end < items.len())
            .then(|| cursor(Cursor::Legacy { thread: thread.into(), offset: end, revision: revision.clone() })),
        revision,
    })
}

fn rollout_path(reply: &Json) -> Option<&Path> {
    reply["thread"]["path"].as_str().map(Path::new)
}

fn load(
    server: &CodexAppServer,
    thread: &str,
    position: Option<&Cursor>,
    cancelled: &AtomicBool,
    expired: &AtomicBool,
) -> Result<HistoryPage, String> {
    let reply = server.call("thread/read", json!({"threadId":thread,"includeTurns":false}), 8_000)?;
    if reply["thread"]["id"].as_str() != Some(thread) {
        return Err("Codex 返回的线程与该成员的持久身份不符".into());
    }
    let rollout = rollout_path(&reply);
    let native = match position {
        Some(Cursor::Items { cursor, .. }) => Some(cursor.as_str()),
        _ => None,
    };
    if !matches!(position, Some(Cursor::Legacy { .. })) {
        let reply = server.request(
            "thread/items/list",
            json!({"threadId":thread,"cursor":native,"limit":PAGE,"sortDirection":"asc"}),
            8_000,
        );
        match reply {
            Ok(reply) => {
                let items = reply["data"].as_array().ok_or("Codex 分页历史缺少条目数组")?;
                if items.len() > PAGE {
                    return Err("Codex 返回的历史页超过请求上限".into());
                }
                checked_items(items)?;
                let next = match &reply["nextCursor"] {
                    Json::Null => None,
                    Json::String(next) if !next.is_empty() && Some(next.as_str()) != native => {
                        Some(cursor(Cursor::Items { thread: thread.into(), cursor: next.clone() }))
                    }
                    _ => return Err("Codex 历史分页游标无效或未前进".into()),
                };
                return Ok(HistoryPage {
                    items: items.clone(),
                    next_cursor: next,
                    revision: digest(&json!({"thread":thread,"position":position,"data":items})),
                });
            }
            // Only an unsupported-method response on the first page permits
            // fallback. Permission refusals and rejected cursors must surface.
            Err(RpcError::Rejected(error)) if position.is_none() && error["code"].as_i64() == Some(-32601) => {
                if let Some(path) = rollout {
                    return rollout_page(server, path, thread, position, cancelled, expired);
                }
            }
            Err(error) => return Err(format!("Codex 历史分页失败：{error}")),
        }
    }
    if let Some(path) = rollout {
        return rollout_page(server, path, thread, position, cancelled, expired);
    }
    let reply = server.call("thread/read", json!({"threadId":thread,"includeTurns":true}), 8_000)?;
    if reply["thread"]["id"].as_str() != Some(thread) {
        return Err("Codex 返回的线程与该成员的持久身份不符".into());
    }
    let turns = reply["thread"]["turns"].as_array().ok_or("Codex 没有返回完整回合历史")?;
    let mut items = vec![];
    for turn in turns {
        let id = turn["id"].as_str().filter(|id| !id.is_empty()).ok_or("Codex 历史缺少回合 ID")?;
        let entries = turn["items"].as_array().ok_or("Codex 回合没有返回条目数组")?;
        items.extend(entries.iter().map(|item| json!({"turnId":id,"item":item})));
    }
    checked_items(&items)?;
    let revision = digest(&json!({"thread":thread,"data":items}));
    let offset = match position {
        Some(Cursor::Legacy { offset, revision: expected, .. }) if expected == &revision => *offset,
        Some(Cursor::Legacy { .. }) => return Err("Codex 历史已变化，请按 r 刷新后重新选择".into()),
        _ => 0,
    };
    if offset > items.len() {
        return Err("Codex 历史分页偏移不存在".into());
    }
    let end = offset.saturating_add(PAGE).min(items.len());
    Ok(HistoryPage {
        items: items[offset..end].to_vec(),
        next_cursor: (end < items.len())
            .then(|| cursor(Cursor::Legacy { thread: thread.into(), offset: end, revision: revision.clone() })),
        revision,
    })
}

pub(crate) fn read_page(
    cwd: &Path,
    thread: &str,
    position: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<HistoryPage, String> {
    let position: Option<Cursor> =
        position.map(serde_json::from_str).transpose().map_err(|_| "无效的 Codex 历史游标")?;
    if position.as_ref().is_some_and(|position| match position {
        Cursor::Items { thread: saved, .. } | Cursor::Legacy { thread: saved, .. } => saved != thread,
    }) {
        return Err("Codex 历史游标不属于该成员线程".into());
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err("历史读取已取消".into());
    }
    let server = CodexAppServer::new(
        cwd,
        AppServerOptions {
            experimental_api: true,
            max_message_bytes: Some(MAX_RESPONSE),
            config_overrides: vec![
                ("approval_policy".into(), json!("never")),
                ("sandbox_mode".into(), json!("read-only")),
            ],
            ..Default::default()
        },
    );
    let done = AtomicBool::new(false);
    let expired = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let guard = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(8);
            while !done.load(Ordering::SeqCst) {
                if cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    expired.store(true, Ordering::SeqCst);
                    // Keep checking until the caller exits, including a close
                    // racing the initial child registration during startup.
                    server.close();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = server.start().and_then(|()| load(&server, thread, position.as_ref(), cancelled, &expired));
        done.store(true, Ordering::SeqCst);
        let _ = guard.join();
        server.close();
        if cancelled.load(Ordering::SeqCst) {
            Err("历史读取已取消".into())
        } else if expired.load(Ordering::SeqCst) {
            Err("Codex 历史读取超时；未启动或恢复回合".into())
        } else {
            result
        }
    })
}
