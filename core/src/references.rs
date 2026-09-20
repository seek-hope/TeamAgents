//! Validate references at publication, without opening or copying their content.
//! Workspace/artifact references remain useful; private transcripts do not
//! become deliverables just because a model puts their address in a string.

use crate::models::{Json, TeamSpec, WorkspacePolicy};
use crate::storage::Store;
use std::path::{Component, Path, PathBuf};

const PRIVATE: &str = "引用指向成员私有上下文或运行状态；请先将可共享成果写入工作目录或 /artifacts/。";
const INVALID: &str = "附件引用必须是非空字符串；result_refs 必须是字符串数组。";

/// Ok(Some(reason)) is an invalid reference, Err is a storage failure. Keeping
/// those separate lets finalization roll back on unavailable validation data.
pub(crate) fn validate(
    store: &Store,
    session_id: &str,
    spec: &TeamSpec,
    actor: &str,
    payload: &Json,
    field: &str,
) -> Result<Option<String>, String> {
    let refs: Vec<&Json> = match payload.get(field) {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::Array(values)) if field == "result_refs" => values.iter().collect(),
        Some(value @ Json::String(_)) if field == "ref" => vec![value],
        _ => return Ok(Some(INVALID.into())),
    };
    if refs.is_empty() {
        return Ok(None);
    }
    let session = store.get_session(session_id).map_err(|e| e.to_string())?.ok_or("unknown reference session")?;
    let cwd = PathBuf::from(session["cwd"].as_str().ok_or("missing reference workspace")?);
    let database = store.conn.path().filter(|path| !path.is_empty()).map(PathBuf::from);
    let base = database.as_deref().and_then(Path::parent);
    let member_work = base.map(|base| base.join("members").join(actor).join("work"));
    let workspace = match spec.agent(actor).map(|a| a.workspace_policy) {
        Some(WorkspacePolicy::Isolated) => member_work.as_deref().unwrap_or(&cwd),
        Some(WorkspacePolicy::GitWorktree) => member_work.as_deref().filter(|work| work.is_dir()).unwrap_or(&cwd),
        _ => &cwd,
    };

    for value in refs {
        let Some(reference) = value.as_str() else { return Ok(Some(INVALID.into())) };
        if reference.trim().is_empty() || reference.chars().any(char::is_control) {
            return Ok(Some(INVALID.into()));
        }
        let address = reference.split('#').next().unwrap_or(reference);
        if address.is_empty() {
            return Ok(Some(INVALID.into()));
        }
        let scheme = address.split_once(':').filter(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        });
        if scheme.is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("ctx") || scheme.eq_ignore_ascii_case("codex"))
            || store.is_private_context_reference(address).map_err(|e| e.to_string())?
            || (source_path(address) != address
                && store.is_private_context_reference(source_path(address)).map_err(|e| e.to_string())?)
        {
            return Ok(Some(PRIVATE.into()));
        }
        let file_path;
        let mut paths = if let Some((scheme, rest)) = scheme.filter(|(scheme, rest)| {
            // A bare filename with :line[:column] is a local source reference,
            // even when the filename also satisfies the URI scheme grammar.
            scheme.eq_ignore_ascii_case("file")
                || !rest.split(':').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        }) {
            if !scheme.eq_ignore_ascii_case("file") {
                // Remote evidence URLs and opaque external artifact schemes
                // are not local files. Do not fetch them during a transaction.
                continue;
            }
            file_path = match file_uri_path(rest) {
                Ok(path) => path,
                Err(reason) => return Ok(Some(reason)),
            };
            vec![file_path.as_str(), source_path(&file_path)]
        } else {
            // Files can literally contain spaces, `#` and `:`. Check both the
            // exact spelling and the decorated reference before publishing it.
            vec![reference, address, source_path(address)]
        };
        paths.dedup();
        for path in paths {
            if let Some(reason) = validate_path(path, workspace, base, database.as_deref())? {
                return Ok(Some(reason));
            }
        }
    }
    Ok(None)
}

fn source_path(mut path: &str) -> &str {
    while let Some((head, tail)) = path.rsplit_once(':') {
        if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        path = head;
    }
    path
}

fn validate_path(
    path: &str,
    workspace: &Path,
    base: Option<&Path>,
    database: Option<&Path>,
) -> Result<Option<String>, String> {
    let path = Path::new(path);
    if path.starts_with("/tool-output") || lexical(path).starts_with("/tool-output") {
        return Ok(Some(PRIVATE.into()));
    }
    let path = path.strip_prefix(".").unwrap_or(path);
    let artifact = path.strip_prefix("/artifacts").or_else(|_| path.strip_prefix("artifacts")).ok();
    let candidate = if let Some(name) = artifact {
        let relative = lexical(name);
        if relative.is_absolute() || relative.starts_with("..") || legacy_log(&relative) {
            return Ok(Some(PRIVATE.into()));
        }
        // Keep symlinks before `..` until resolution; lexical normalization
        // here would erase a private target behind an artifact alias.
        base.map(|base| base.join("artifacts").join(name)).unwrap_or_else(|| workspace.join(path))
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    // Compare the spelling and the actual target. In particular, resolving
    // a symlink before `..` must not accidentally turn a private path public.
    for path in [Ok(lexical(&candidate)), resolve(&candidate)] {
        let path = match path {
            Ok(path) => path,
            Err(error) => return Ok(Some(format!("无法确认附件引用的目标：{error}"))),
        };
        if private_path(&path, database)? {
            return Ok(Some(PRIVATE.into()));
        }
    }
    Ok(None)
}

fn file_uri_path(rest: &str) -> Result<String, String> {
    let path = if let Some(authority_and_path) = rest.strip_prefix("//") {
        let (authority, path) = authority_and_path.split_once('/').ok_or("file 引用缺少绝对路径")?;
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            return Err("file 引用仅支持本机绝对路径".into());
        }
        format!("/{path}")
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        return Err("file 引用必须使用绝对路径".into());
    };
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            let pair = raw.get(index + 1..index + 3).ok_or("file 引用包含无效百分号编码")?;
            let digit = |b: u8| (b as char).to_digit(16).ok_or("file 引用包含无效百分号编码");
            bytes.push((digit(pair[0])? * 16 + digit(pair[1])?) as u8);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let path = String::from_utf8(bytes).map_err(|_| "file 引用必须使用 UTF-8 路径")?;
    if path.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    Ok(path)
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolve existing symlinks, also when the referenced output is not yet there.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        out.push(part);
        match std::fs::canonicalize(&out) {
            Ok(real) => out = real,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&out).is_ok() {
                    return Err("符号链接目标不可用".into());
                }
                out = lexical(&out);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(out)
}

fn legacy_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("exec-") && name.ends_with(".log"))
}

fn session_private(path: &Path) -> bool {
    let parts: Vec<_> = path.components().map(|part| part.as_os_str()).collect();
    if parts.first().is_some_and(|part| *part == "artifacts") {
        return legacy_log(path);
    }
    !(parts.len() >= 3 && parts[0] == "members" && parts[2] == "work")
}

fn state_private(path: &Path) -> bool {
    let Ok(sessions) = path.strip_prefix("sessions") else { return true };
    let sessions = sessions.strip_prefix("archived").unwrap_or(sessions);
    let mut parts = sessions.components();
    if parts.next().is_none() {
        return true;
    }
    session_private(parts.as_path())
}

fn env_dir(key: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or(fallback)
}

fn private_path(path: &Path, database: Option<&Path>) -> Result<bool, String> {
    let home = env_dir("HOME", PathBuf::from("/"));
    let config = env_dir("XDG_CONFIG_HOME", home.join(".config")).join("teamagents");
    let codex = env_dir("CODEX_HOME", home.join(".codex"));
    for root in [config, codex] {
        let root = resolve(&root)?;
        if path.starts_with(&root) || root.starts_with(path) {
            return Ok(true);
        }
    }
    let state = resolve(&env_dir("XDG_STATE_HOME", home.join(".local/state")).join("teamagents"))?;
    if state.starts_with(path) {
        return Ok(true);
    }
    if let Ok(relative) = path.strip_prefix(state) {
        return Ok(state_private(relative));
    }
    if let Some(database) = database {
        let database = resolve(database)?;
        if database.starts_with(path)
            || path == database.with_file_name(format!("{}-wal", database.file_name().unwrap().to_string_lossy()))
            || path == database.with_file_name(format!("{}-shm", database.file_name().unwrap().to_string_lossy()))
        {
            return Ok(true);
        }
        if let Some(base) = database.parent() {
            if let Ok(relative) = path.strip_prefix(base) {
                return Ok(session_private(relative));
            }
            // Normal production stores have one database per session, with
            // sibling and archived sessions under the same private state root.
            if let Some(sessions) = base.parent().filter(|p| p.file_name().is_some_and(|n| n == "sessions")) {
                if let Some(state) = sessions.parent() {
                    if let Ok(relative) = path.strip_prefix(state) {
                        return Ok(state_private(relative));
                    }
                }
            }
        }
    }
    Ok(false)
}
