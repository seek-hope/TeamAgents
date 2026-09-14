//! Sandboxed tool executors (execution.py / tools.py): file tools confined to
//! the member workspace, shell via bwrap when present, web fetch with an SSRF
//! guard.

use serde_json::{json, Value as Json};
use crate::gateway::TurnControl;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_OUTPUT: usize = 200_000;
/// Virtual prefix members use to read long shell output back (execution.py
/// routes `/artifacts/` to the session artifact directory).
const ARTIFACTS_PREFIX: &str = "/artifacts/";

/// Resolve `key` inside root; reject traversal and symlinks escaping root.
pub fn resolve_in_root(root: &Path, key: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let joined = if Path::new(key).is_absolute() { PathBuf::from(key) } else { root.join(key) };
    let normalized = normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(format!("path escapes workspace: {key}"));
    }
    if let Ok(real) = std::fs::canonicalize(&normalized) {
        if !real.starts_with(&root) {
            return Err(format!("path escapes workspace: {key}"));
        }
        return Ok(real);
    }
    // the file may not exist yet: the deepest existing ancestor must stay inside
    let mut probe = normalized.as_path();
    loop {
        // canonicalize also fails for dangling/cyclic links. They are not a
        // missing ordinary path: a later create would follow them on the host.
        match std::fs::symlink_metadata(probe) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(format!("unresolved symlink: {key}")),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
        let Some(parent) = probe.parent() else { break };
        if let Ok(real) = std::fs::canonicalize(parent) {
            if !real.starts_with(&root) {
                return Err(format!("path escapes workspace: {key}"));
            }
            break;
        }
        probe = parent;
    }
    Ok(normalized)
}

/// Pin the parent directory before opening a member file. Linux /proc fd paths
/// keep a concurrent symlink replacement from redirecting creates or writes.
fn open_member_file(root: &Path, path: &Path, write: bool) -> Result<std::fs::File, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let check = |file: &std::fs::File| -> Result<(), String> {
        let real = std::fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|e| e.to_string())?;
        if !real.starts_with(&root) { return Err("path escapes workspace".into()); }
        Ok(())
    };
    let relative = path.strip_prefix(&root).map_err(|_| "path escapes workspace")?;
    let name = relative.file_name().ok_or("file path required")?;
    let mut parent = std::fs::File::open(&root).map_err(|e| e.to_string())?;
    check(&parent)?;
    for part in relative.parent().unwrap_or(Path::new("")).components() {
        let Component::Normal(part) = part else { return Err("invalid file path".into()) };
        let next = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(part);
        if write && !next.exists() {
            // create_dir never follows/replaces an existing dangling symlink.
            match std::fs::create_dir(&next) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        parent = std::fs::File::open(next).map_err(|e| e.to_string())?;
        check(&parent)?;
    }
    let leaf = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
    let opened = std::fs::OpenOptions::new().read(!write).write(write).open(&leaf);
    let file = match opened {
        Ok(file) => file,
        Err(e) if write && e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&leaf).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    check(&file)?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err("not a regular file".into()); }
    Ok(file)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn cap_read(file: std::fs::File) -> Result<String, String> {
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file too large ({} bytes)", meta.len()));
    }
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text).map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_FILE_BYTES { return Err("file too large".into()); }
    Ok(text)
}

/// Resolve a member-visible artifact reference (`/artifacts/<name>`) inside the
/// session artifact directory; traversal out of it is refused like any root.
fn resolve_artifact(artifacts: Option<&PathBuf>, key: &str) -> Result<PathBuf, String> {
    let root = artifacts.ok_or_else(|| "no artifact directory for this member".to_string())?;
    let name = key.strip_prefix(ARTIFACTS_PREFIX).unwrap_or(key);
    resolve_in_root(root, name)
}

/// File/tool executor for one member's workspace. `artifacts` is the session
/// artifact directory: long shell output lands there (execution.py::_save_artifact)
/// and members read it back through read_file/read_artifact.
pub fn workspace_executor(
    root: PathBuf,
    artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    let executor = workspace_executor_with_control(root, artifacts);
    move |tool, args| executor(tool, args, &TurnControl::default())
}

fn workspace_executor_with_control(
    root: PathBuf, artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync + 'static {
    move |tool: &str, args: &Json, control: &TurnControl| -> Result<Json, String> {
        control.check()?;
        let arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let arg_or = |args: &Json, key: &str, default: &str| -> String {
            args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
        };
        // `/artifacts/...` is a virtual path for this member, not a host path
        // (execution.py routes that prefix to the session artifact directory)
        let member_path = |key: &str| -> Result<PathBuf, String> {
            if key.starts_with(ARTIFACTS_PREFIX) {
                resolve_artifact(artifacts.as_ref(), key)
            } else {
                resolve_in_root(&root, key)
            }
        };
        let member_file = |key: &str, write: bool| {
            let path = member_path(key)?;
            let file_root = if key.starts_with(ARTIFACTS_PREFIX) { artifacts.as_ref().unwrap() } else { &root };
            open_member_file(file_root, &path, write)
        };
        match tool {
            "ls" => {
                let key = if arg("path").is_empty() { ".".to_string() } else { arg("path") };
                let path = resolve_in_root(&root, &key)?;
                let mut names: Vec<String> = std::fs::read_dir(&path)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if e.path().is_dir() {
                            format!("{name}/")
                        } else {
                            name
                        }
                    })
                    .collect();
                names.sort();
                Ok(json!(names.join("\n")))
            }
            "read_file" => Ok(json!(cap_read(member_file(&arg("path"), false)?)?)),
            "read_artifact" => {
                let key = arg("path");
                let key = if key.is_empty() { arg("name") } else { key };
                let path = resolve_artifact(artifacts.as_ref(), &key)?;
                Ok(json!(cap_read(open_member_file(artifacts.as_ref().unwrap(), &path, false)?)?))
            }
            "write_file" => {
                let path = member_path(&arg("path"))?;
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if content.len() as u64 > MAX_FILE_BYTES {
                    return Err("content too large".into());
                }
                let mut file = member_file(&arg("path"), true)?;
                file.set_len(0).map_err(|e| e.to_string())?;
                file.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
                Ok(json!(format!("wrote {}", path.display())))
            }
            "edit_file" => {
                let path = member_path(&arg("path"))?;
                let text = cap_read(member_file(&arg("path"), false)?)?;
                let old = arg("old_string");
                let new = arg("new_string");
                if !text.contains(&old) {
                    return Err("old_string not found".into());
                }
                let mut file = member_file(&arg("path"), true)?;
                file.set_len(0).map_err(|e| e.to_string())?;
                file.write_all(text.replacen(&old, &new, 1).as_bytes()).map_err(|e| e.to_string())?;
                Ok(json!(format!("edited {}", path.display())))
            }
            "delete" => {
                let path = member_path(&arg("path"))?;
                let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
                let result = if recursive {
                    std::fs::remove_dir_all(&path)
                } else if path.is_dir() {
                    std::fs::remove_dir(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                result.map_err(|e| e.to_string())?;
                Ok(json!(format!("deleted {}", path.display())))
            }
            "glob" => {
                let pattern = if arg("pattern").is_empty() { "*".to_string() } else { arg("pattern") };
                let mut hits = vec![];
                glob_walk(&root, &root, &pattern, &mut hits);
                hits.truncate(500);
                Ok(json!(hits.join("\n")))
            }
            "grep" => {
                let pattern = shell_quote(&arg("pattern"));
                let path = shell_quote(&arg_or(args, "path", "."));
                shell_run_with_control(&format!("grep -rn -- {pattern} {path} | head -100"), &root, 30, false, artifacts.as_deref(), control)
                    .map(Json::String)
            }
            "shell" => {
                let command = arg("command");
                let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);
                let network = args.get("network").and_then(|v| v.as_bool()).unwrap_or(false);
                shell_run_with_control(&command, &root, timeout, network, artifacts.as_deref(), control).map(Json::String)
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}

/// One member's bound web providers. Binding a service *is* the authorization
/// (tools.py::build_bound_tools): a member that did not bind a web service has
/// no web tool, and the executor says so instead of failing open.
#[derive(Default, Clone)]
pub(crate) struct WebTools {
    pub(crate) search: Option<teamagents_core::models::ToolBinding>,
    pub(crate) fetch: Option<teamagents_core::models::ToolBinding>,
}

fn is_web_kind(kind: &str) -> bool {
    matches!(kind, "web_search" | "web_fetch")
}

/// Resolve the member's web bindings: explicitly bound names win, in binding
/// order, then `web` expands to every configured web binding. The catalog is a
/// HashMap (unstable order), so the expansion is sorted by name.
pub(crate) fn web_tools(
    catalog: &teamagents_core::models::UserConfig,
    bindings: &[String],
) -> Result<WebTools, String> {
    let mut candidates: Vec<(String, &teamagents_core::models::ToolBinding)> = vec![];
    for name in bindings {
        if name == "web" {
            continue;
        }
        if let Some(binding) = catalog.tools.get(name) {
            if is_web_kind(&binding.kind) {
                candidates.push((name.clone(), binding));
            }
        }
    }
    if candidates.is_empty() && bindings.iter().any(|b| b == "web") {
        candidates = catalog
            .tools
            .iter()
            .filter(|(_, binding)| is_web_kind(&binding.kind))
            .map(|(name, binding)| (name.clone(), binding))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let mut tools = WebTools::default();
    for (name, binding) in candidates {
        // a required service that cannot load fails the member at load time
        // (tools.py::build_bound_tools raises ToolServiceUnavailable)
        if binding.required && binding.kind == "web_search" {
            let provider = binding.provider.clone().unwrap_or_else(|| "anysearch".into());
            if provider != "anysearch" {
                return Err(format!(
                    "required tool service {name:?} is unavailable: unsupported web_search provider {provider:?}"
                ));
            }
        }
        match binding.kind.as_str() {
            "web_search" if tools.search.is_none() => tools.search = Some(binding.clone()),
            "web_fetch" if tools.fetch.is_none() => tools.fetch = Some(binding.clone()),
            _ => {}
        }
    }
    Ok(tools)
}

/// Load-time check for the web half of a member's bindings (session.rs calls
/// this while building the member's runner).
pub fn validate_web_bindings(
    catalog: &teamagents_core::models::UserConfig,
    bindings: &[String],
) -> Result<(), String> {
    web_tools(catalog, bindings).map(|_| ())
}


/// Skills registry roots: user-configured `skills_paths` only (project and
/// member skill dirs live inside the workspace and are readable with `files`).
/// Read-only by construction (plan §12.2: selected skills are pre-authorized reads).
fn skill_roots(catalog: &teamagents_core::models::UserConfig) -> Vec<PathBuf> {
    catalog
        .skills_paths
        .iter()
        .map(|p| crate::config::expand_home(p))
        .filter(|p| p.is_dir())
        .collect()
}

/// (name, canonical SKILL.md path) pairs under one registry root; candidates
/// whose symlink chain resolves outside the canonical root are rejected (P2-6).
pub fn skill_candidates(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(root) = std::fs::canonicalize(root) else { return vec![] };
    let mut candidates = vec![root.join("SKILL.md")];
    if let Ok(entries) = std::fs::read_dir(&root) {
        candidates.extend(entries.flatten().map(|e| e.path().join("SKILL.md")));
    }
    let mut out: Vec<(String, PathBuf)> = vec![];
    for path in candidates {
        let Ok(real) = std::fs::canonicalize(&path) else { continue };
        if !real.starts_with(&root) || !real.is_file() {
            continue;
        }
        let name = real
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.is_empty() && !out.iter().any(|(n, _)| *n == name) {
            out.push((name, real));
        }
    }
    out
}

/// (name, SKILL.md path) pairs across all registry roots; first root wins on
/// a name clash (user order = priority).
fn skill_index(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = vec![];
    for root in roots {
        for (name, path) in skill_candidates(root) {
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, path));
            }
        }
    }
    out
}

/// Frontmatter `description:` line, capped; enough for search hits without
/// parsing full YAML (ponytail: line scan, upgrade to a YAML parse if skills
/// start needing nested frontmatter).
fn skill_blurb(path: &Path) -> String {
    let mut text = String::new();
    if std::fs::File::open(path).and_then(|f| f.take(4_096).read_to_string(&mut text)).is_err() {
        return String::new();
    }
    let line = text.lines().find(|l| l.starts_with("description:")).unwrap_or("");
    line.trim_start_matches("description:").trim().trim_matches('"').chars().take(200).collect()
}

/// `skill` tool: action "search" (keyword match over name+blurb; empty query
/// lists, capped) or "read" (exact name -> full SKILL.md, capped).
fn skill_tool(catalog: &teamagents_core::models::UserConfig, args: &Json) -> Result<Json, String> {
    const READ_CAP: usize = 32_000;
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let index = skill_index(&skill_roots(catalog));
    if index.is_empty() {
        return Err("no skills configured (user config skills_paths)".into());
    }
    match action {
        "read" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let Some((_, path)) = index.iter().find(|(n, _)| n == name) else {
                let known: Vec<&str> = index.iter().map(|(n, _)| n.as_str()).collect();
                return Err(format!("unknown skill {name:?}; known: {}", known.join(", ")));
            };
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Ok(Json::String(text.chars().take(READ_CAP).collect()))
        }
        "search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let words: Vec<&str> = query.split_whitespace().collect();
            let mut hits: Vec<(usize, String)> = index
                .iter()
                .map(|(name, path)| {
                    let blurb = skill_blurb(path);
                    let hay = format!("{} {}", name.to_lowercase(), blurb.to_lowercase());
                    let score = words.iter().filter(|w| hay.contains(**w)).count();
                    (score, format!("{name} — {blurb}"))
                })
                .filter(|(score, _)| words.is_empty() || *score > 0)
                .collect();
            hits.sort_by(|a, b| b.0.cmp(&a.0));
            let total = hits.len();
            let mut lines: Vec<String> = hits.into_iter().take(10).map(|(_, line)| line).collect();
            if total > 10 {
                lines.push(format!("… {} more; refine the query", total - 10));
            }
            Ok(Json::String(if lines.is_empty() { "no matching skills".into() } else { lines.join("
") }))
        }
        other => Err(format!("unknown skill action {other:?} (use search|read)")),
    }
}

/// Executor for one member root: file/shell tools rooted there plus the web
/// tools that member actually bound (tools.py::build_bound_tools).
pub fn member_executor(
    root: PathBuf,
    catalog: teamagents_core::models::UserConfig,
    bindings: Vec<String>,
    artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    let executor = member_executor_with_control(root, catalog, bindings, artifacts);
    move |tool, args| executor(tool, args, &TurnControl::default())
}

pub(crate) fn member_executor_with_control(
    root: PathBuf, catalog: teamagents_core::models::UserConfig, bindings: Vec<String>, artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync + 'static {
    let workspace = workspace_executor_with_control(root, artifacts);
    let web: OnceLock<Result<WebTools, String>> = OnceLock::new();
    move |tool: &str, args: &Json, control: &TurnControl| match tool {
        "web_search" => {
            let binding = web
                .get_or_init(|| web_tools(&catalog, &bindings))
                .as_ref()
                .map_err(|e| e.clone())?
                .search
                .clone()
                .ok_or_else(|| format!("tool {tool} is not bound to this member"))?;
            let provider = binding.provider.clone().unwrap_or_else(|| "anysearch".into());
            if provider != "anysearch" {
                return Err(format!("unsupported web_search provider {provider:?}"));
            }
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let max = args.get("max_results").and_then(|v| v.as_i64()).unwrap_or(5);
            let include = args.get("include_content").and_then(|v| v.as_bool()).unwrap_or(false);
            web_search(query, max, binding.url.as_deref(), binding.api_key_env.as_deref(), include)
        }
        "web_fetch" => {
            let binding = web
                .get_or_init(|| web_tools(&catalog, &bindings))
                .as_ref()
                .map_err(|e| e.clone())?
                .fetch
                .clone()
                .ok_or_else(|| format!("tool {tool} is not bound to this member"))?;
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let max = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(2_000_000) as usize;
            let allow_private = binding.env.get("allow_private").map(|v| v == "1").unwrap_or(false);
            web_fetch(url, max, allow_private)
        }
        "skill" => {
            if !bindings.iter().any(|b| b == "skills") {
                return Err("tool skill is not bound to this member".into());
            }
            skill_tool(&catalog, args)
        }
        other => {
            let capability = if other == "shell" { "shell" } else { "files" };
            if !bindings.iter().any(|b| b == capability) {
                return Err(format!("tool {other} is not bound to this member"));
            }
            workspace(other, args, control)
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Tiny glob: `*` (within a segment), `?`, `**` (any depth). Enough for the
/// patterns members send; a full glob crate is not worth the dependency yet.
fn glob_walk(root: &Path, dir: &Path, pattern: &str, out: &mut Vec<String>) {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, index)) = stack.pop() {
        if out.len() >= 500 || index > segments.len() {
            continue;
        }
        if index == segments.len() {
            if let Ok(rel) = current.strip_prefix(root) {
                if !rel.as_os_str().is_empty() {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
            continue;
        }
        let segment = segments[index];
        if segment == "**" {
            stack.push((current.clone(), index + 1));
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        stack.push((entry.path(), index));
                    }
                }
            }
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !segment_match(segment, &name) {
                    continue;
                }
                let path = entry.path();
                if index + 1 == segments.len() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().into_owned());
                    }
                } else if path.is_dir() {
                    stack.push((path, index + 1));
                }
            }
        }
    }
}

fn segment_match(pattern: &str, name: &str) -> bool {
    // iterative wildcard match over bytes (patterns are ASCII globs)
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn bwrap_available() -> bool {
    which("bwrap").is_some()
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// execution.py::bwrap_argv — read-only system mounts, sanitized env, private
/// /tmp, no network unless the call was approved for it.
pub fn bwrap_argv(workdir: &Path, network: bool, command: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["bwrap".into()];
    for path in ["/usr", "/etc", "/opt"] {
        if Path::new(path).exists() {
            argv.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }
    for (target, link) in [("usr/lib", "/lib"), ("usr/lib64", "/lib64"), ("usr/bin", "/bin"), ("usr/bin", "/sbin")] {
        // the host's /bin is itself a symlink, so only the target is checked
        if Path::new(&format!("/{target}")).exists() {
            argv.extend(["--symlink".into(), target.into(), link.into()]);
        }
    }
    let workdir = workdir.to_string_lossy().into_owned();
    argv.extend(["--proc".into(), "/proc".into(), "--dev".into(), "/dev".into(), "--tmpfs".into(), "/tmp".into()]);
    argv.extend(["--bind".into(), workdir.clone(), workdir.clone()]);
    argv.extend(["--chdir".into(), workdir]);
    argv.extend([
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
    ]);
    if !network {
        argv.push("--unshare-net".into());
    }
    argv.extend(["--".into(), "/bin/bash".into(), "-lc".into(), command.into()]);
    argv
}

/// Drain one pipe into a shared buffer. Reading happens on its own thread so a
/// child that fills the pipe buffer (>64KiB on Linux) never blocks the wait
/// loop (execution.py uses communicate() for the same reason).
fn drain(mut pipe: impl Read + Send + 'static) -> (Arc<Mutex<Vec<u8>>>, std::thread::JoinHandle<()>) {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let target = sink.clone();
    let handle = std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut chunk) {
            if read == 0 {
                break;
            }
            target.lock().unwrap().extend_from_slice(&chunk[..read]);
        }
    });
    (sink, handle)
}

/// Wait for reader threads, but never block the caller forever: if a pipe is
/// still open after the grace period the partial buffer is all we can use.
/// ponytail: a reader that never sees EOF is leaked (its data is already
/// collected); kill the process group instead if that ever shows up.
fn join_bounded(handles: Vec<std::thread::JoinHandle<()>>, grace: Duration) {
    let deadline = Instant::now() + grace;
    for handle in handles {
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
        // else: leak the reader thread rather than hang the engine; its data is
        // already in the buffer (kill is instantaneous with --die-with-parent)
    }
}

/// execution.py::_save_artifact — long output is preserved under the session
/// artifact directory and referenced by `/artifacts/<name>`.
fn save_artifact(dir: &Path, output: &str) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let name = format!("exec-{}-{}.log", compact_timestamp(), std::process::id());
    std::fs::write(dir.join(&name), output).ok()?;
    Some(format!("{ARTIFACTS_PREFIX}{name}"))
}

/// "YYYYMMDD-HHMMSS" (UTC), the Python artifact name shape.
fn compact_timestamp() -> String {
    let digits: String = iso8601(teamagents_core::models::now() as i64)
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    format!("{}-{}", &digits[..8], &digits[8..14])
}

/// execution.py::run_isolated — bwrap-only: missing isolation is an error,
/// never a silent fallback to unsandboxed execution (plan §12.2).
pub fn shell_run(
    command: &str,
    workdir: &Path,
    timeout_s: u64,
    network: bool,
    artifacts: Option<&Path>,
) -> Result<String, String> {
    shell_run_with_control(command, workdir, timeout_s, network, artifacts, &TurnControl::default())
}

fn shell_run_with_control(
    command: &str, workdir: &Path, timeout_s: u64, network: bool, artifacts: Option<&Path>, control: &TurnControl,
) -> Result<String, String> {
    control.check()?;
    if !bwrap_available() {
        return Err("IsolationUnavailable: bwrap is not available: refusing to run commands without isolation".into());
    }
    let workdir = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let argv = bwrap_argv(&workdir, network, command);
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // whitelist environment: no model keys, no credentials (plan §12.2)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", &workdir)
        .env("LANG", std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()))
        .env("TERM", "dumb")
        .env("TMPDIR", "/tmp")
        .env("PYTHONIOENCODING", "utf-8")
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (stdout_sink, stdout_reader) = drain(stdout);
    let (stderr_sink, stderr_reader) = drain(stderr);
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut timed_out = false;
    let mut status = None;
    loop {
        if control.check().is_err() {
            // Killing bwrap tears down its private PID namespace and command
            // tree (--unshare-pid + --die-with-parent).
            child.kill().map_err(|e| e.to_string())?;
            child.wait().map_err(|e| e.to_string())?;
            join_bounded(vec![stdout_reader, stderr_reader], Duration::from_secs(5));
            return Err("turn interrupted".into());
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) if Instant::now() > deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
    }
    join_bounded(vec![stdout_reader, stderr_reader], Duration::from_secs(5));
    let mut text = String::from_utf8_lossy(&stdout_sink.lock().unwrap()).into_owned();
    text.push_str(&String::from_utf8_lossy(&stderr_sink.lock().unwrap()));
    if text.len() > MAX_OUTPUT {
        // never cut inside a UTF-8 sequence
        let mut cut = MAX_OUTPUT;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let full = text;
        let mut head = full[..cut].to_string();
        match artifacts.and_then(|dir| save_artifact(dir, &full)) {
            Some(reference) => {
                head.push_str(&format!("\n[output truncated at {MAX_OUTPUT} bytes; full output: {reference}]"))
            }
            None => head.push_str(&format!("\n[output truncated at {MAX_OUTPUT} bytes]")),
        }
        text = head;
    }
    if timed_out {
        let note = format!("command timed out after {timeout_s}s");
        return Err(if text.is_empty() { note } else { format!("{note}\n{text}") });
    }
    let status = status.ok_or("no exit status")?;
    Ok(if status.success() { text } else { format!("{text}\n(exit {})", status.code().unwrap_or(-1)) })
}

/// Host part of `rest` ("host[:port]/path..."), brackets and userinfo included
/// (urlparse(url).hostname parity).
fn url_host(rest: &str) -> &str {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    match authority.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

/// tools.py::guard_url — block private/loopback/reserved/multicast targets.
/// The blocks mirror Python's `ipaddress` (which the Python guard trusts), so
/// the two builds refuse the same addresses.
pub fn guard_url(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| format!("unsupported url {url}"))?;
    let host = url_host(rest).to_string();
    if host.is_empty() {
        return Err(format!("unsupported url {url}"));
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(format!("refusing private address for {host}"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_private_addr(ip) {
            return Err(format!("refusing private address for {host}"));
        }
        return Ok(url.to_string());
    }
    // resolve: every address must be public
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 443u16)
        .to_socket_addrs()
        .or_else(|_| (host.as_str(), 80u16).to_socket_addrs())
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("cannot resolve {host}"));
    }
    for addr in addrs {
        if is_private_addr(addr.ip()) {
            return Err(format!("refusing private address for {host}"));
        }
    }
    Ok(url.to_string())
}

// `ipaddress`'s not-globally-reachable blocks (iana special registries), which
// tools.py::guard_url rejects through is_private/is_loopback/is_link_local/
// is_reserved/is_multicast. Keep in sync with the Python module.
const IPV4_BLOCKED: &[(u128, u8)] = &[
    (0x0000_0000, 8),   // 0.0.0.0/8
    (0x0a00_0000, 8),   // 10.0.0.0/8
    (0x7f00_0000, 8),   // 127.0.0.0/8
    (0xa9fe_0000, 16),  // 169.254.0.0/16
    (0xac10_0000, 12),  // 172.16.0.0/12
    (0xc000_0000, 24),  // 192.0.0.0/24
    (0xc000_00aa, 31),  // 192.0.0.170/31
    (0xc000_0200, 24),  // 192.0.2.0/24
    (0xc0a8_0000, 16),  // 192.168.0.0/16
    (0xc612_0000, 15),  // 198.18.0.0/15
    (0xc633_6400, 24),  // 198.51.100.0/24
    (0xcb00_7100, 24),  // 203.0.113.0/24
    (0xe000_0000, 4),   // 224.0.0.0/4 multicast
    (0xf000_0000, 4),   // 240.0.0.0/4 reserved
    (0xffff_ffff, 32),  // 255.255.255.255
];
/// Addresses inside a blocked block that are globally reachable (IANA exceptions).
const IPV4_ALLOWED: &[(u128, u8)] = &[(0xc000_0009, 32), (0xc000_000a, 32)];

fn in_block(bits: u128, block: &[(u128, u8)], width: u32) -> bool {
    block.iter().any(|(net, prefix)| {
        let prefix = *prefix as u32;
        match prefix {
            0 => true,
            _ => (bits >> (width - prefix)) == (*net >> (width - prefix)),
        }
    })
}

fn ipv6_bits(addr: &std::net::Ipv6Addr) -> u128 {
    addr.segments().iter().fold(0u128, |acc, segment| (acc << 16) | u128::from(*segment))
}

fn is_private_v4(addr: std::net::Ipv4Addr) -> bool {
    let bits = u128::from(u32::from(addr));
    in_block(bits, IPV4_BLOCKED, 32) && !in_block(bits, IPV4_ALLOWED, 32)
}

/// Private, loopback, link-local, unique-local, reserved and multicast space
/// (ipaddress.IPv6Address: _private_networks + _reserved_networks + multicast).
const IPV6_BLOCKED: &[([u16; 8], u8)] = &[
    ([0, 0, 0, 0, 0, 0, 0, 1], 128),             // ::1
    ([0, 0, 0, 0, 0, 0, 0, 0], 128),             // ::
    ([0, 0, 0, 0, 0, 0xffff, 0, 0], 96),         // ::ffff:0:0/96 (v4-mapped)
    ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),      // 64:ff9b:1::/48
    ([0x100, 0, 0, 0, 0, 0, 0, 0], 64),          // 100::/64
    ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),         // 2001::/23
    ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),     // 2001:db8::/32
    ([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),         // 2002::/16
    ([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),         // 3fff::/20
    ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),          // fc00::/7 unique local
    ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),         // fe80::/10 link local
    ([0, 0, 0, 0, 0, 0, 0, 0], 8),               // ::/8
    ([0x100, 0, 0, 0, 0, 0, 0, 0], 8),           // 100::/8
    ([0x200, 0, 0, 0, 0, 0, 0, 0], 7),           // 200::/7
    ([0x400, 0, 0, 0, 0, 0, 0, 0], 6),           // 400::/6
    ([0x800, 0, 0, 0, 0, 0, 0, 0], 5),           // 800::/5
    ([0x1000, 0, 0, 0, 0, 0, 0, 0], 4),          // 1000::/4
    ([0x4000, 0, 0, 0, 0, 0, 0, 0], 3),          // 4000::/3
    ([0x6000, 0, 0, 0, 0, 0, 0, 0], 3),          // 6000::/3
    ([0x8000, 0, 0, 0, 0, 0, 0, 0], 3),          // 8000::/3
    ([0xa000, 0, 0, 0, 0, 0, 0, 0], 3),          // a000::/3
    ([0xc000, 0, 0, 0, 0, 0, 0, 0], 3),          // c000::/3
    ([0xe000, 0, 0, 0, 0, 0, 0, 0], 4),          // e000::/4
    ([0xf000, 0, 0, 0, 0, 0, 0, 0], 5),          // f000::/5
    ([0xf800, 0, 0, 0, 0, 0, 0, 0], 6),          // f800::/6
    ([0xfe00, 0, 0, 0, 0, 0, 0, 0], 9),          // fe00::/9
    ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),          // ff00::/8 multicast
];
/// Globally reachable exceptions inside the blocked v6 blocks.
const IPV6_ALLOWED: &[([u16; 8], u8)] = &[
    ([0x2001, 1, 0, 0, 0, 0, 0, 1], 128),
    ([0x2001, 1, 0, 0, 0, 0, 0, 2], 128),
    ([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),
    ([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48),
    ([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    ([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28),
];

fn ipv6_blocks() -> &'static (Vec<(u128, u8)>, Vec<(u128, u8)>) {
    static BLOCKS: OnceLock<(Vec<(u128, u8)>, Vec<(u128, u8)>)> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        let convert = |table: &[([u16; 8], u8)]| -> Vec<(u128, u8)> {
            table.iter().map(|(segments, prefix)| (ipv6_bits(&std::net::Ipv6Addr::from(*segments)), *prefix)).collect()
        };
        (convert(IPV6_BLOCKED), convert(IPV6_ALLOWED))
    })
}

pub fn is_private_addr(addr: std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // v4-mapped addresses keep the IPv4 semantics (ipaddress does this too)
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let bits = ipv6_bits(&v6);
            let (blocked, allowed) = ipv6_blocks();
            in_block(bits, blocked, 128) && !in_block(bits, allowed, 128)
        }
    }
}

/// AnySearch provider (tools.py::_web_search_tool): POST {query, max_results}.
pub fn web_search(
    query: &str,
    max_results: i64,
    url: Option<&str>,
    api_key_env: Option<&str>,
    include_content: bool,
) -> Result<Json, String> {
    let url = url.unwrap_or("https://api.anysearch.com/v1/search");
    let key = api_key_env.and_then(|env| std::env::var(env).ok());
    let count = max_results.clamp(1, 20);
    let mut request = ureq::post(url)
        .timeout(std::time::Duration::from_secs(30))
        .set("content-type", "application/json");
    if let Some(key) = key {
        request = request.set("authorization", &format!("Bearer {key}"));
    }
    let response = request
        .send_string(&json!({"query": query, "max_results": count}).to_string())
        .map_err(|e| format!("web_search failed: {e}"))?;
    let payload: Json = response.into_json().map_err(|e| format!("web_search bad json: {e}"))?;
    Ok(parse_search_response(&payload, query, count, include_content))
}

fn parse_search_response(payload: &Json, query: &str, count: i64, include_content: bool) -> Json {
    let results: Vec<Json> = payload
        .get("data")
        .and_then(|d| d.get("results"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(count as usize)
        .map(|item| {
            let mut entry = json!({
                "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "snippet": item.get("snippet").and_then(|v| v.as_str()).unwrap_or(""),
                "fetched_at": iso_now(),
            });
            if include_content {
                if let Some(content) = item.get("content") {
                    entry["content"] = content.clone();
                }
            }
            entry
        })
        .collect();
    json!({"query": query, "provider": "anysearch", "results": results})
}

/// GET with redirects followed manually so every hop passes the SSRF guard
/// (ureq's built-in following would only guard the first URL, P1-2).
fn http_get_guarded(url: &str, guard: &dyn Fn(&str) -> Result<String, String>) -> Result<ureq::Response, String> {
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut current = guard(url)?;
    for _ in 0..10 {
        let response = agent
            .get(&current)
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| format!("web_fetch failed: {e}"))?;
        if !(300..400).contains(&response.status()) {
            return Ok(response);
        }
        let location = response.header("location").ok_or("web_fetch: redirect without location")?;
        let next = url::Url::parse(&current)
            .and_then(|base| base.join(location))
            .map_err(|e| format!("web_fetch: bad redirect target {location:?}: {e}"))?;
        current = guard(next.as_str())?;
    }
    Err("web_fetch: too many redirects".into())
}

/// tools.py::_web_fetch_tool — guarded GET, title + readable text body.
pub fn web_fetch(url: &str, max_bytes: usize, allow_private: bool) -> Result<Json, String> {
    let guard = |u: &str| if allow_private { Ok(u.to_string()) } else { guard_url(u) };
    let response = http_get_guarded(url, &guard)?;
    let content_type = response.header("content-type").unwrap_or("").to_string();
    let final_url = response.get_url().to_string();
    let body = response.into_string().map_err(|e| format!("web_fetch failed: {e}"))?;
    if !content_type.contains("html") && !content_type.contains("text/") {
        return Err(format!("unsupported content type {content_type:?} for {url:?}"));
    }
    let truncated_by_bytes = body.len() > max_bytes;
    let body: String = body.chars().take(max_bytes).collect();
    let (title, text) = strip_html(&body);
    let capped: String = text.chars().take(200_000).collect();
    Ok(json!({
        "title": if title.is_empty() { url.to_string() } else { title },
        "url": final_url,
        "fetched_at": iso_now(),
        "content": capped,
        "truncated": truncated_by_bytes || text.chars().count() > 200_000,
    }))
}

/// Minimal HTML → (title, visible text), skipping script/style/noscript.
fn strip_html(html: &str) -> (String, String) {
    let mut out = String::new();
    let mut rest = html;
    loop {
        // earliest of the skipped-element openers, not just the first match found
        let Some((start, opener)) = ["<script", "<style", "<noscript"]
            .iter()
            .filter_map(|opener| rest.find(opener).map(|index| (index, *opener)))
            .min_by_key(|(index, _)| *index)
        else {
            break;
        };
        out.push_str(&rest[..start]);
        let close = format!("</{}>", &opener[1..]);
        match rest[start..].find(&close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => break,
        }
    }
    out.push_str(rest);
    let mut text = String::new();
    let mut title = String::new();
    let mut in_tag = false;
    let mut in_title = false;
    let mut tag = String::new();
    for ch in out.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
                if !in_title {
                    in_title = false;
                }
            }
            '>' => {
                in_tag = false;
                if tag.trim().eq_ignore_ascii_case("title") {
                    in_title = true;
                } else if tag.trim().eq_ignore_ascii_case("/title") {
                    in_title = false;
                }
                text.push(' ');
            }
            c if in_tag => tag.push(c),
            c if in_title => title.push(c),
            c => text.push(c),
        }
    }
    (title.split_whitespace().collect::<Vec<_>>().join(" "), text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn iso_now() -> String {
    iso8601(teamagents_core::models::now() as i64)
}

/// "YYYY-MM-DDTHH:MM:SSZ" (UTC), no date library needed.
pub fn iso8601(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let (hour, minute, second) = (
        (seconds.rem_euclid(86_400)) / 3600,
        (seconds.rem_euclid(3600)) / 60,
        seconds.rem_euclid(60),
    );
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve `response` to the first connection, then report our URL.
    fn serve_once(response: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        url
    }

    #[test]
    fn web_fetch_guards_every_redirect_hop() {
        // P1-2: a redirect to a target the guard refuses must stop the fetch
        let second = serve_once("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 6\r\n\r\nsecret".into());
        let first = serve_once(format!("HTTP/1.1 302 Found\r\nlocation: {second}\r\ncontent-length: 0\r\n\r\n"));
        let guard = |u: &str| {
            if u == first { Ok(u.to_string()) } else { Err(format!("private address refused: {u}")) }
        };
        let err = http_get_guarded(&first, &guard).unwrap_err();
        assert!(err.contains("private address"), "{err}");
    }

    #[test]
    fn web_fetch_follows_guarded_redirect_chain() {
        // both hops pass the guard -> the final hop's body and URL are returned
        let second = serve_once("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello".into());
        let first = serve_once(format!("HTTP/1.1 302 Found\r\nlocation: {second}\r\ncontent-length: 0\r\n\r\n"));
        let result = web_fetch(&first, 1_000_000, true).expect("allow_private serves the local chain");
        assert_eq!(result["content"], json!("hello"));
        assert_eq!(result["url"], json!(second));
    }

    #[test]
    fn web_fetch_stops_after_ten_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/loop", listener.local_addr().unwrap());
        let location = url.clone();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    format!("HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\n\r\n").as_bytes(),
                );
            }
        });
        let err = web_fetch(&url, 1_000_000, true).unwrap_err();
        assert!(err.contains("too many redirects"), "{err}");
    }

    #[test]
    fn skill_tool_searches_and_reads_registry() {
        let root = std::env::temp_dir().join(format!("ta-skilltool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ponytail")).unwrap();
        std::fs::create_dir_all(root.join("scanpy")).unwrap();
        std::fs::write(
            root.join("ponytail/SKILL.md"),
            "---\nname: ponytail\ndescription: laziest solution that works\n---\nponytail body",
        )
        .unwrap();
        std::fs::write(
            root.join("scanpy/SKILL.md"),
            "---\nname: scanpy\ndescription: single-cell analysis\n---\nscanpy body",
        )
        .unwrap();
        let mut catalog = teamagents_core::models::UserConfig::default();
        catalog.skills_paths = vec![root.to_string_lossy().into_owned()];

        // search hits name and description, ranks multi-word matches first
        let hits = skill_tool(&catalog, &json!({"action": "search", "query": "single-cell"})).unwrap();
        let hits = hits.as_str().unwrap();
        assert!(hits.contains("scanpy") && !hits.contains("ponytail"), "{hits}");
        // empty query lists everything
        let all = skill_tool(&catalog, &json!({"action": "search", "query": ""})).unwrap();
        assert!(all.as_str().unwrap().contains("ponytail"));
        // read returns the full body; unknown names fail with the known list
        let body = skill_tool(&catalog, &json!({"action": "read", "name": "ponytail"})).unwrap();
        assert!(body.as_str().unwrap().contains("ponytail body"));
        let err = skill_tool(&catalog, &json!({"action": "read", "name": "nope"})).unwrap_err();
        assert!(err.contains("scanpy"), "{err}");
        // an empty registry and a bad action are explicit errors
        let empty = teamagents_core::models::UserConfig::default();
        assert!(skill_tool(&empty, &json!({"action": "search"})).is_err());
        assert!(skill_tool(&catalog, &json!({"action": "delete"})).is_err());
        // executor enforces the binding (bound = authorized, like web tools)
        let executor = member_executor(root.clone(), catalog, vec![], None);
        assert!(executor("skill", &json!({"action": "search"})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_paths_stay_inside_root() {
        let root = std::env::temp_dir().join(format!("ta-tools-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file.txt"), "hello").unwrap();
        assert!(resolve_in_root(&root, "sub/file.txt").is_ok());
        assert!(resolve_in_root(&root, "../etc/passwd").is_err());
        assert!(resolve_in_root(&root, "/etc/passwd").is_err());
        let executor = workspace_executor(root.clone(), None);
        assert_eq!(executor("read_file", &json!({"path": "sub/file.txt"})).unwrap(), json!("hello"));
        assert!(executor("read_file", &json!({"path": "../../etc/hosts"})).is_err());
        let listing = executor("ls", &json!({"path": "."})).unwrap();
        assert!(listing.as_str().unwrap().contains("sub/"));
        let globbed = executor("glob", &json!({"pattern": "**/*.txt"})).unwrap();
        assert!(globbed.as_str().unwrap().contains("sub/file.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_url_blocks_private_targets() {
        assert!(guard_url("http://127.0.0.1/x").is_err());
        assert!(guard_url("http://10.0.0.5/x").is_err());
        assert!(guard_url("http://localhost/x").is_err());
        assert!(guard_url("ftp://example.com/x").is_err());
        assert_eq!(guard_url("https://93.184.216.34/x").unwrap(), "https://93.184.216.34/x");
        // ipaddress parity: reserved/multicast/documentation/benchmarking blocks
        for blocked in [
            "http://224.0.0.1/",
            "http://198.18.0.1/",
            "http://192.0.2.1/",
            "http://198.51.100.7/",
            "http://203.0.113.9/",
            "http://240.0.0.1/",
            "http://0.0.0.0/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert!(guard_url(blocked).is_err(), "{blocked} must be refused");
        }
        // bracketed IPv6 literals are parsed, not rejected as unresolvable
        assert!(guard_url("http://[::1]:8000/x").unwrap_err().contains("private address"));
        assert!(guard_url("http://[fe80::1]/").is_err());
        assert!(guard_url("http://[fc00::1]/").is_err());
        assert!(guard_url("http://[::ffff:127.0.0.1]:8000/x").is_err());
        // …and public v6 targets stay usable
        assert_eq!(guard_url("https://[2606:4700::1]/x").unwrap(), "https://[2606:4700::1]/x");
        // userinfo does not confuse host extraction
        assert!(guard_url("http://user:pass@127.0.0.1/x").is_err());
    }

    #[test]
    fn bwrap_argv_matches_python_and_runs_isolated() {
        let dir = std::env::temp_dir();
        let argv = bwrap_argv(&dir, false, "echo hi");
        assert!(argv.iter().any(|a| a == "--unshare-net"), "network off by default");
        assert!(argv.windows(3).any(|w| w == ["--ro-bind", "/usr", "/usr"]));
        assert!(argv.windows(3).any(|w| w == ["--symlink", "usr/bin", "/bin"]));
        assert!(argv.windows(2).any(|w| w[0] == "--chdir" && w[1] == dir.to_string_lossy()));
        assert_eq!(&argv[argv.len() - 4..], ["--", "/bin/bash", "-lc", "echo hi"]);
        assert!(!bwrap_argv(&dir, true, "x").iter().any(|a| a == "--unshare-net"));
        if bwrap_available() {
            let out = shell_run("echo isolated-ok && id -u", &dir, 30, false, None).unwrap();
            assert!(out.contains("isolated-ok"), "{out}");
        }
    }

    #[test]
    fn web_shapes_match_python_and_bindings_are_required() {
        let payload = json!({"code": 0, "data": {"results": [
            {"title": "T", "url": "https://x", "snippet": "S", "content": "BODY"},
        ]}});
        let parsed = parse_search_response(&payload, "q", 5, false);
        assert_eq!(parsed["provider"], "anysearch");
        assert_eq!(parsed["results"][0]["title"], "T");
        assert!(parsed["results"][0].get("content").is_none());
        assert_eq!(parse_search_response(&payload, "q", 5, true)["results"][0]["content"], "BODY");

        let (title, text) = strip_html(
            "<html><head><title>Hi</title><style>x{}</style></head><body><p>Hello  world</p><script>evil()</script></body></html>",
        );
        assert_eq!(title, "Hi");
        assert_eq!(text, "Hello world");
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");

        // web tools resolve their binding from the catalog ("no binding" = no capability)
        let catalog = teamagents_core::models::UserConfig::default();
        let executor = member_executor(std::env::temp_dir(), catalog.clone(), vec![], None);
        assert!(executor("web_search", &json!({"query": "q"})).is_err());
        // a member that only binds web still needs a configured provider
        let executor = member_executor(std::env::temp_dir(), catalog.clone(), vec!["web".into()], None);
        assert!(executor("web_search", &json!({"query": "q"})).unwrap_err().contains("not bound"));
        let mut catalog = catalog;
        catalog.tools.insert(
            "web".into(),
            serde_json::from_value(json!({"kind": "web_search", "provider": "unknown"})).unwrap(),
        );
        let executor = member_executor(std::env::temp_dir(), catalog, vec!["web".into()], None);
        assert!(executor("web_search", &json!({"query": "q"})).unwrap_err().contains("unsupported"));
    }


    // -- P1-2 / P2-6 regression tests (review 2026-09-14) --

    fn tiny_server(reply: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            while let Ok((mut conn, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 2048];
                let _ = conn.read(&mut buf);
                if conn.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        base
    }

    #[test]
    fn redirect_chain_is_guarded_at_every_hop() {
        let hop_b = tiny_server("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nhi".into());
        let hop_a = tiny_server(format!(
            "HTTP/1.1 302 Found\r\nlocation: {hop_b}/secret\r\ncontent-length: 0\r\n\r\n"
        ));
        let hop_blocked = tiny_server(
            "HTTP/1.1 302 Found\r\nlocation: http://blocked.invalid/\r\ncontent-length: 0\r\n\r\n".into(),
        );

        // happy path: an allowed redirect is followed to its 200
        let (a, b) = (hop_a.clone(), hop_b.clone());
        let allow_local = move |u: &str| {
            if u.starts_with(&a) || u.starts_with(&b) {
                Ok(u.to_string())
            } else {
                Err(format!("blocked {u}"))
            }
        };
        let response = http_get_guarded(&format!("{hop_a}/x"), &allow_local).unwrap();
        assert_eq!(response.status(), 200);

        // the guard must also run on the redirect target, not just hop one
        let blocked_base = hop_blocked.clone();
        let allow_first_only = move |u: &str| {
            if u.starts_with(&blocked_base) {
                Ok(u.to_string())
            } else {
                Err(format!("blocked {u}"))
            }
        };
        let err = http_get_guarded(&format!("{hop_blocked}/x"), &allow_first_only).unwrap_err();
        assert!(err.contains("blocked"), "redirect target escaped the guard: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn skill_candidates_reject_symlink_escapes() {
        let base = std::env::temp_dir().join(format!("ta-skill-test-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("real/SKILL.md"), "real").unwrap();
        std::fs::write(outside.join("SKILL.md"), "outside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linkdir")).unwrap();
        // a symlinked SKILL.md inside a real directory
        std::fs::create_dir_all(root.join("linked")).unwrap();
        std::os::unix::fs::symlink(outside.join("SKILL.md"), root.join("linked/SKILL.md")).unwrap();

        let names: Vec<String> = skill_candidates(&root).into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"real".to_string()), "real skill missing: {names:?}");
        assert!(!names.contains(&"linkdir".to_string()), "symlinked dir escaped: {names:?}");
        assert!(!names.contains(&"linked".to_string()), "symlinked SKILL.md escaped: {names:?}");
        std::fs::remove_dir_all(&base).ok();
    }

}
