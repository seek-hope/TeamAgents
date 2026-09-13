//! Sandboxed tool executors (execution.py / tools.py): file tools confined to
//! the member workspace, shell via bwrap when present, web fetch with an SSRF
//! guard.

use serde_json::{json, Value as Json};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_OUTPUT: usize = 200_000;

/// Resolve `key` inside root; reject traversal and symlinks escaping root.
pub fn resolve_in_root(root: &Path, key: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
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
    while let Some(parent) = probe.parent() {
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

fn cap_read(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file too large ({} bytes)", meta.len()));
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

/// File/tool executor for one member's workspace.
pub fn workspace_executor(root: PathBuf) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    move |tool: &str, args: &Json| -> Result<Json, String> {
        let arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let arg_or = |args: &Json, key: &str, default: &str| -> String {
            args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
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
            "read_file" => Ok(json!(cap_read(&resolve_in_root(&root, &arg("path"))?)?)),
            "write_file" => {
                let path = resolve_in_root(&root, &arg("path"))?;
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if content.len() as u64 > MAX_FILE_BYTES {
                    return Err("content too large".into());
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, content).map_err(|e| e.to_string())?;
                Ok(json!(format!("wrote {}", path.display())))
            }
            "edit_file" => {
                let path = resolve_in_root(&root, &arg("path"))?;
                let text = cap_read(&path)?;
                let old = arg("old_string");
                let new = arg("new_string");
                if !text.contains(&old) {
                    return Err("old_string not found".into());
                }
                std::fs::write(&path, text.replacen(&old, &new, 1)).map_err(|e| e.to_string())?;
                Ok(json!(format!("edited {}", path.display())))
            }
            "delete" => {
                let path = resolve_in_root(&root, &arg("path"))?;
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
                shell_run(&format!("grep -rn -- {pattern} {path} | head -100"), &root, 30, false).map(Json::String)
            }
            "shell" => {
                let command = arg("command");
                let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);
                let network = args.get("network").and_then(|v| v.as_bool()).unwrap_or(false);
                shell_run(&command, &root, timeout, network).map(Json::String)
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}

/// Executor for one session: file/shell tools rooted at the session workspace,
/// plus the user's configured web tools (tools.py::build_bound_tools).
pub fn session_executor(
    root: PathBuf,
    catalog: teamagents_core::models::UserConfig,
) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    let workspace = workspace_executor(root);
    move |tool: &str, args: &Json| match tool {
        "web_search" => {
            let binding = catalog
                .tools
                .values()
                .find(|b| b.kind == "web_search")
                .ok_or("web_search is not configured in the user config")?;
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
            let binding = catalog
                .tools
                .values()
                .find(|b| b.kind == "web_fetch")
                .ok_or("web_fetch is not configured in the user config")?;
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let max = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(2_000_000) as usize;
            let allow_private = binding.env.get("allow_private").map(|v| v == "1").unwrap_or(false);
            web_fetch(url, max, allow_private)
        }
        other => workspace(other, args),
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

/// execution.py::run_isolated — bwrap-only: missing isolation is an error,
/// never a silent fallback to unsandboxed execution (plan §12.2).
pub fn shell_run(command: &str, workdir: &Path, timeout_s: u64, network: bool) -> Result<String, String> {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                let truncated = text.len() > MAX_OUTPUT;
                if truncated {
                    text.truncate(MAX_OUTPUT);
                    text.push_str(&format!("\n[output truncated at {MAX_OUTPUT} bytes]"));
                }
                return Ok(if status.success() { text } else { format!("{text}\n(exit {})", status.code().unwrap_or(-1)) });
            }
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("command timed out after {timeout_s}s"));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// tools.py::guard_url — block private/loopback targets.
pub fn guard_url(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| format!("unsupported url {url}"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(rest.split(['/', '?', '#']).next().unwrap_or(""))
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
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
    let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 443u16).to_socket_addrs()
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

fn is_private_addr(addr: std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 10
                || o[0] == 127
                || o[0] == 0
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00
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

/// tools.py::_web_fetch_tool — guarded GET, title + readable text body.
pub fn web_fetch(url: &str, max_bytes: usize, allow_private: bool) -> Result<Json, String> {
    let url = if allow_private { url.to_string() } else { guard_url(url)? };
    let response = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| format!("web_fetch failed: {e}"))?;
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
        "title": if title.is_empty() { url.clone() } else { title },
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

    #[test]
    fn workspace_paths_stay_inside_root() {
        let root = std::env::temp_dir().join(format!("ta-tools-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file.txt"), "hello").unwrap();
        assert!(resolve_in_root(&root, "sub/file.txt").is_ok());
        assert!(resolve_in_root(&root, "../etc/passwd").is_err());
        assert!(resolve_in_root(&root, "/etc/passwd").is_err());
        let executor = workspace_executor(root.clone());
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
            let out = shell_run("echo isolated-ok && id -u", &dir, 30, false).unwrap();
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
        let executor = session_executor(std::env::temp_dir(), catalog.clone());
        assert!(executor("web_search", &json!({"query": "q"})).is_err());
        let mut catalog = catalog;
        catalog.tools.insert(
            "web".into(),
            serde_json::from_value(json!({"kind": "web_search", "provider": "unknown"})).unwrap(),
        );
        let executor = session_executor(std::env::temp_dir(), catalog);
        assert!(executor("web_search", &json!({"query": "q"})).unwrap_err().contains("unsupported"));
    }
}
