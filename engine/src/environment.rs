//! D-479: explicit development-environment inspection, never a model turn or an environment repair.

use serde_json::{json, Value as Json};
use std::io::Read;
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use teamagents_core::models::UserConfig;

/// Boot-selected paths, not a claim about the last model request. File contents are read per turn.
pub(crate) fn sources(catalog: &UserConfig) -> Json {
    json!({"instruction_files": catalog.instruction_files, "skills_paths": catalog.skills_paths,
        "semantics": "Paths selected at daemon boot; instruction contents are read per turn. Skills load on demand."})
}

/// Keep configuration bytes private; readers only learn whether the startup snapshot changed.
#[derive(Clone)]
pub(crate) struct ConfigWatch(Vec<(PathBuf, Result<String, String>)>);

impl ConfigWatch {
    pub(crate) fn new(workspace: &Path) -> Self {
        Self::paths(vec![crate::config::user_config_path(), crate::config::project_config_path(workspace)])
    }

    fn paths(paths: Vec<PathBuf>) -> Self {
        Self(
            paths
                .into_iter()
                .map(|path| {
                    let signature = signature(&path);
                    (path, signature)
                })
                .collect(),
        )
    }

    pub(crate) fn report(&self, workspace: &Path) -> Json {
        let mut rows: Vec<Json> = self
            .0
            .iter()
            .map(|(path, before)| {
                let now = signature(path);
                let state = if now.is_err() || before.is_err() {
                    "unavailable"
                } else if &now == before {
                    "unchanged"
                } else {
                    "changed"
                };
                json!({"path": path, "state": state})
            })
            .collect();
        let project = crate::config::project_config_path(workspace);
        if !self.0.iter().any(|(path, _)| path == &project) {
            rows.push(json!({"path": project, "state": "changed"}));
        }
        json!({"files": rows, "restart_required": rows.iter().any(|r| r["state"] == "changed"),
            "comparison_available": rows.iter().all(|r| r["state"] != "unavailable"),
            "note": "Configuration is not hot-reloaded. Stop this daemon explicitly and reconnect to apply changes; CLI overrides remain boot-local."})
    }
}

fn signature(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("missing".into()),
        Err(e) => return Err(e.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = vec![];
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("configuration exceeds diagnostic limit".into());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Hints supplement the original error. They never reinterpret an operation as successful or retry it.
pub(crate) fn recovery_hint(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("cd:") || reason.contains("workdir") || reason.contains("working directory") {
        "Workspace: verify that the host directory exists and is accessible."
    } else if reason.contains("not found") || reason.contains("not available") {
        "Missing executable: inspect the host PATH and install the required tool explicitly."
    } else {
        "Inspect the host shell error. No dependency installation or environment repair was attempted."
    }
}

const TOOL_NAMES: [&str; 7] = ["rustc", "cargo", "node", "npm", "python3", "pip3", "git"];

// Only shell builtins in the probe itself: a discovered shim is not executed.
const PROBE: &str = "printf 'workspace\\t%s\\n' \"$PWD\"; for tool in rustc cargo node npm python3 pip3 git; do if location=$(command -v \"$tool\"); then printf '%s\\t%s\\n' \"$tool\" \"$location\"; else printf '%s\\tmissing\\n' \"$tool\"; fi; done";

/// User-invoked host discovery, without reading or writing persistent shell state.
pub fn doctor(workspace: &Path, json_out: bool) -> i32 {
    let result = (|| {
        crate::cli::require_workspace_dir(workspace)?;
        let workspace = workspace.canonicalize().map_err(|e| e.to_string())?;
        let _ = crate::config::load_user_config_for(&workspace)?;
        let host = inspect(&workspace);
        Ok::<_, String>(json!({"workspace": workspace, "execution": "host", "host": host,
            "scope": "Fresh host shell; not a running member's saved exports/cwd. No model calls. Executable discovery only, not a build test."}))
    })();
    match result {
        Ok(report) => {
            let ok = report["host"]["ok"] == true;
            if json_out {
                println!("{report}");
            } else {
                print_report(&report);
            }
            if ok {
                0
            } else {
                1
            }
        }
        Err(error) => {
            if json_out {
                println!("{}", json!({"ok": false, "error": error}));
            } else {
                eprintln!("environment: {error}");
            }
            1
        }
    }
}

fn print_report(report: &Json) {
    println!("Workspace: {}\nExecution: host", report["workspace"].as_str().unwrap_or("?"));
    if report["host"]["ok"] == true {
        println!("{:<10} Host path", "Tool");
        for row in report["host"]["tools"].as_array().into_iter().flatten() {
            println!("{:<10} {}", row["name"].as_str().unwrap_or("?"), row["path"].as_str().unwrap_or("?"));
        }
    } else {
        println!(
            "{}\n{}",
            report["host"]["error"].as_str().unwrap_or("unavailable"),
            report["host"]["hint"].as_str().unwrap_or("")
        );
    }
    println!("Executable discovery only; verify actual build/test commands separately. Saved member exports are not inspected.");
}

fn inspect(workspace: &Path) -> Json {
    let result = crate::tools::shell_command_spec(PROBE, workspace, None)
        .map_err(|e| e.reason)
        .and_then(|spec| capture(&spec, Duration::from_secs(20)))
        .and_then(|output| {
            for name in TOOL_NAMES.into_iter().chain(["workspace"]) {
                if output
                    .lines()
                    .filter(|line| line.split_once('\t').is_some_and(|(key, value)| key == name && !value.is_empty()))
                    .count()
                    != 1
                {
                    return Err(format!("incomplete environment probe: missing or duplicate {name} result"));
                }
            }
            Ok(output)
        });
    match result {
        Ok(output) => {
            let tools: Vec<Json> = output
                .lines()
                .filter_map(|line| {
                    let (name, path) = line.split_once('\t')?;
                    TOOL_NAMES
                        .contains(&name)
                        .then(|| json!({"name": name, "path": path, "available": path != "missing"}))
                })
                .collect();
            json!({"ok": true, "tools": tools, "note": "Missing tools must be installed explicitly in this environment; a PATH entry is not proof of a working toolchain."})
        }
        Err(error) => json!({"ok": false, "hint": recovery_hint(&error), "error": error}),
    }
}

/// Bounded capture with no reader threads that can be held open by a descendant. Never inherits model keys.
fn capture(spec: &crate::tools::ShellCommandSpec, timeout: Duration) -> Result<String, String> {
    let (mut reader, writer) = UnixStream::pair().map_err(|e| e.to_string())?;
    reader.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stderr = writer.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::os::fd::OwnedFd::from(writer)))
        .stderr(Stdio::from(std::os::fd::OwnedFd::from(stderr)))
        .process_group(0);
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", spec.program))?;
    drop(command);
    let start = Instant::now();
    let mut output = Vec::new();
    let mut reaped = false;
    let outcome = loop {
        let mut buf = [0u8; 4096];
        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                output.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => break Err(e.to_string()),
            _ => {}
        }
        if output.len() > 32 * 1024 {
            break Err("diagnostic output exceeded 32 KiB".into());
        }
        if start.elapsed() >= timeout {
            break Err("environment probe timed out; remote processes may require backend-specific cleanup".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                reaped = true;
                // Drain the bounded tail already available, never wait for inherited descriptors.
                let mut tail = [0u8; 4096];
                while output.len() <= 32 * 1024 {
                    match reader.read(&mut tail) {
                        Ok(n) if n > 0 => output.extend_from_slice(&tail[..n]),
                        _ => break,
                    }
                }
                if output.len() > 32 * 1024 {
                    break Err("diagnostic output exceeded 32 KiB".into());
                }
                let text: String = String::from_utf8_lossy(&output)
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                    .collect();
                break if status.success() { Ok(text) } else { Err(format!("exit {status}: {text}")) };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(e.to_string()),
        }
    };
    if outcome.is_err() && !reaped {
        // This is our just-spawned process group, not a PID guessed from a name.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_watch_reports_changes_without_exposing_contents() {
        let root = std::env::temp_dir().join(format!("ta-env-watch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, "secret-before").unwrap();
        let watch = ConfigWatch::paths(vec![path.clone(), crate::config::project_config_path(&root)]);
        assert_eq!(watch.report(&root)["restart_required"], false);
        std::fs::write(&path, "secret-after").unwrap();
        let report = watch.report(&root);
        assert_eq!(report["restart_required"], true);
        assert!(!report.to_string().contains("secret-"));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(watch.report(&root)["restart_required"], true);
        std::fs::create_dir(&path).unwrap();
        let unavailable = watch.report(&root);
        assert_eq!(unavailable["files"][0]["state"], "unavailable");
        assert_eq!(unavailable["comparison_available"], false);
        assert!(!unavailable.to_string().contains("secret-"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_is_bounded_and_does_not_inherit_credentials() {
        let mut spec =
            crate::tools::shell_command_spec("printf '%s' \"${TA_ENV_SECRET-unset}\"", Path::new("/tmp"), None)
                .unwrap();
        spec.env.retain(|(key, _)| key != "TA_ENV_SECRET");
        assert_eq!(capture(&spec, Duration::from_secs(2)).unwrap(), "unset");
        spec.args = vec!["-c".into(), "sleep 10".into()];
        assert!(capture(&spec, Duration::from_millis(50)).unwrap_err().contains("timed out"));
        spec.args = vec!["-c".into(), "while :; do printf '%01000d' 0; done".into()];
        assert!(capture(&spec, Duration::from_secs(2)).unwrap_err().contains("32 KiB"));
    }
}
