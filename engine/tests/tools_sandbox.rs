//! Sandbox regressions: shell output larger than the pipe buffer, artifact
//! spill for long output, the SSRF guard table, web bindings fail-closed, and
//! the isolated workspace note (findings 1/2/6/8/9).

use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use teamagents_core::models::{AgentSpec, ToolBinding, UserConfig};
use teamagents_engine::{tools, workspace};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-sandbox-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn binding(value: serde_json::Value) -> ToolBinding {
    serde_json::from_value(value).unwrap()
}

fn has_bwrap() -> bool {
    if tools::bwrap_available() {
        return true;
    }
    eprintln!("skip: bwrap is not available");
    false
}

/// Multi-file edits land together or not at all.
#[test]
fn batch_edits_are_all_or_nothing() {
    let base = scratch("edit-files");
    let workspace = base.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), "alpha = 1\n").unwrap();
    std::fs::write(workspace.join("b.txt"), "beta = 1\n").unwrap();
    let executor = tools::workspace_executor(workspace.clone(), None);
    let edit = |path: &str, from: &str, to: &str| json!({"path": path, "old_string": from, "new_string": to});

    // one edit cannot match -> neither file changes
    let error = executor("edit_files", &json!({"edits": [
        edit("a.txt", "alpha = 1", "alpha = 2"),
        edit("b.txt", "beta = 99", "beta = 2"),
    ]})).unwrap_err();
    assert!(error.contains("b.txt"), "{error}");
    assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "alpha = 1\n", "nothing was half-applied");
    assert_eq!(std::fs::read_to_string(workspace.join("b.txt")).unwrap(), "beta = 1\n");

    // the same batch with a matching second edit applies both and reports diffs
    let report = executor("edit_files", &json!({"edits": [
        edit("a.txt", "alpha = 1", "alpha = 2"),
        edit("b.txt", "beta = 1", "beta = 2"),
    ]})).unwrap();
    let report = report.as_str().unwrap_or_default().to_string();
    assert!(report.contains("a.txt") && report.contains("b.txt"), "{report}");
    assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "alpha = 2\n");
    assert_eq!(std::fs::read_to_string(workspace.join("b.txt")).unwrap(), "beta = 2\n");

    // two edits to one file in the same call are refused (they would race each other)
    let error = executor("edit_files", &json!({"edits": [
        edit("a.txt", "alpha = 2", "alpha = 3"),
        edit("a.txt", "alpha = 3", "alpha = 4"),
    ]})).unwrap_err();
    assert!(error.contains("one edit per file"), "{error}");
    assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "alpha = 2\n");
    let _ = std::fs::remove_dir_all(&base);
}

/// A terminal keeps `cd` and `export`; so must the sandbox, without writing any
/// of that state into the user's project.
#[test]
fn persistent_shell_keeps_cd_and_exports_between_commands() {
    if !has_bwrap() {
        return;
    }
    let base = scratch("shell-state");
    let workspace = base.join("workspace");
    let state = base.join("state");
    std::fs::create_dir_all(workspace.join("sub")).unwrap();
    let control = teamagents_engine::gateway::TurnControl::default();

    let first = tools::shell_run_stateful("cd sub && export TA_MARK=42 && pwd", &workspace, 30, false, None, Some(&state), &control).unwrap();
    assert!(first.contains("sub"), "first command reports the new cwd: {first}");
    let second = tools::shell_run_stateful("pwd && echo \"mark=$TA_MARK\"", &workspace, 30, false, None, Some(&state), &control).unwrap();
    assert!(second.starts_with("[cwd: "), "the model is told where it is: {second}");
    assert!(second.contains("sub"), "cd persisted: {second}");
    assert!(second.contains("mark=42"), "export persisted: {second}");

    // a state-free run stays at the workdir (no cross-talk with plain calls)
    let plain = tools::shell_run("pwd", &workspace, 30, false, None).unwrap();
    assert!(!plain.contains("sub"), "no state, no persisted cwd: {plain}");

    // and none of that state leaked into the project
    let entries: Vec<String> = std::fs::read_dir(&workspace).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(entries, vec!["sub".to_string()], "workspace holds only what the command created: {entries:?}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_interrupted_command_does_not_advance_the_shell_state() {
    if !has_bwrap() {
        return;
    }
    let base = scratch("shell-interrupt");
    let workspace = base.join("workspace");
    let state = base.join("state");
    std::fs::create_dir_all(workspace.join("sub")).unwrap();
    let control = teamagents_engine::gateway::TurnControl::default();
    tools::shell_run_stateful("cd sub", &workspace, 30, false, None, Some(&state), &control).unwrap();

    // cancel mid-command: the state file must keep the last finished directory
    let cancelling = std::sync::Arc::new(teamagents_engine::gateway::TurnControl::default());
    let handle = cancelling.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        handle.cancel();
    });
    let interrupted = tools::shell_run_stateful("cd /tmp && sleep 5 && echo changed", &workspace, 30, false, None, Some(&state), &cancelling);
    assert!(interrupted.is_err(), "a cancelled command is reported as interrupted");

    let after = tools::shell_run_stateful("pwd", &workspace, 30, false, None, Some(&state), &control).unwrap();
    let expected = workspace.join("sub").to_string_lossy().into_owned();
    assert!(after.starts_with(&format!("[cwd: {expected}]")), "state stays at the last completed command: {after}");
    assert!(!after.contains("[cwd: /tmp]"), "the cancelled cd never took effect: {after}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn file_tools_reject_dangling_links_and_keep_in_root_links_working() {
    use std::os::unix::fs::symlink;
    let base = scratch("dangling-links");
    let root = base.join("workspace");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    symlink(&outside, root.join("leaf")).unwrap();
    symlink(base.join("missing-dir"), root.join("parent")).unwrap();
    let executor = tools::workspace_executor(root.clone(), None);
    for name in ["leaf", "parent/nested/file"] {
        assert!(executor("write_file", &json!({"path":name, "content":"escaped"})).is_err());
    }
    assert!(!outside.exists());
    assert!(!base.join("missing-dir").exists());
    executor("write_file", &json!({"path":"nested/target", "content":"old"})).unwrap();
    symlink(root.join("nested/target"), root.join("safe")).unwrap();
    executor("write_file", &json!({"path":"safe", "content":"new"})).unwrap();
    executor("edit_file", &json!({"path":"safe", "old_string":"new", "new_string":"edited"})).unwrap();
    assert_eq!(executor("read_file", &json!({"path":"safe"})).unwrap(), json!("edited"));
    assert_eq!(std::fs::read_to_string(root.join("nested/target")).unwrap(), "edited");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn paged_reads_preserve_middle_lines_and_multibyte_long_lines() {
    let dir = scratch("paged-read");
    let artifacts = dir.join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let text = format!("first\n{}\nlast\n", "中文🙂".repeat(15_000));
    std::fs::write(dir.join("large.txt"), &text).unwrap();
    let executor = tools::workspace_executor(dir.clone(), Some(artifacts.clone()));
    let first = executor("read_file", &json!({"path":"large.txt", "offset":1, "limit":1, "include_sha256":true})).unwrap();
    assert_eq!(first["content"], "first\n");
    assert_eq!(first["next_offset"], 2);
    assert_eq!(first["sha256"], format!("{:x}", Sha256::digest(text.as_bytes())));
    let mut recovered = first["content"].as_str().unwrap().to_string();
    let mut position = first["next_byte_offset"].as_u64();
    while let Some(offset) = position {
        let page = executor("read_file", &json!({"path":"large.txt", "byte_offset":offset})).unwrap();
        assert!(page["content"].as_str().unwrap().len() <= 32_000);
        recovered.push_str(page["content"].as_str().unwrap());
        position = page["next_byte_offset"].as_u64();
    }
    assert_eq!(recovered, text);
    assert_eq!(executor("read_file", &json!({"path":"large.txt","offset":3,"limit":1})).unwrap()["content"], "last\n");
    for args in [json!({"offset":0}), json!({"limit":0}), json!({"byte_offset":-1}), json!({"offset":2,"byte_offset":6})] {
        let mut args = args;
        args["path"] = json!("large.txt");
        assert!(executor("read_file", &args).is_err());
    }
    // Long artifacts have no whole-file 10 MiB read ceiling.
    let mut file = std::fs::File::create(artifacts.join("huge.log")).unwrap();
    file.set_len(12 * 1024 * 1024).unwrap();
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b"tail\n").unwrap();
    let tail = executor("read_artifact", &json!({"path":"huge.log", "byte_offset":12 * 1024 * 1024})).unwrap();
    assert_eq!(tail["content"], "tail\n");
    assert!(tail["next_byte_offset"].is_null());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn edits_reject_ambiguous_matches_and_writes_preserve_permissions_and_versions() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = scratch("atomic-writes");
    let path = dir.join("source.rs");
    std::fs::write(&path, "aaa\nunique\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
    let executor = tools::workspace_executor(dir.clone(), None);
    for old in ["", "aa", "missing"] {
        assert!(executor("edit_file", &json!({"path":"source.rs", "old_string":old,"new_string":"bad"})).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaa\nunique\n");
    }
    let snapshot = executor("read_file", &json!({"path":"source.rs","include_sha256":true})).unwrap();
    let original = std::fs::File::open(&path).unwrap();
    let old_inode = original.metadata().unwrap().ino();
    let diff = executor("edit_file", &json!({"path":"source.rs", "old_string":"unique","new_string":"changed", "expected_sha256":snapshot["sha256"]})).unwrap();
    assert!(diff.as_str().unwrap().contains("@@ line 2 @@\n-unique\n+changed"));
    assert_ne!(std::fs::metadata(&path).unwrap().ino(), old_inode, "replace the inode, never truncate the original");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o751);
    let mut old = String::new();
    original.take(100).read_to_string(&mut old).unwrap();
    assert_eq!(old, "aaa\nunique\n");
    assert!(executor("write_file", &json!({"path":"source.rs","content":"stale","expected_sha256":snapshot["sha256"]})).unwrap_err().contains("conflict"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaa\nchanged\n");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "failed mutations clean their staging file");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn independent_members_cannot_both_commit_the_same_file_version() {
    let dir = scratch("concurrent-writes");
    std::fs::write(dir.join("shared.txt"), "original").unwrap();
    let hash = format!("{:x}", Sha256::digest(b"original"));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs: Vec<_> = (0..2).map(|index| {
        let executor = tools::workspace_executor(dir.clone(), None);
        let barrier = barrier.clone();
        let hash = hash.clone();
        std::thread::spawn(move || {
            barrier.wait();
            executor("write_file", &json!({"path":"shared.txt", "content":format!("member-{index}"),"expected_sha256":hash}))
        })
    }).collect();
    let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().find_map(|result| result.as_ref().err()).unwrap().contains("conflict"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn searches_respect_ignores_and_workspace_boundaries() {
    if !has_bwrap() || tools::which("rg").is_none() { return; }
    let dir = scratch("search-ignore");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(dir.join("src/visible.rs"), "needle\n").unwrap();
    std::fs::write(dir.join("ignored.rs"), "needle\n").unwrap();
    std::os::unix::fs::symlink("/etc", dir.join("escape")).unwrap();
    let executor = tools::workspace_executor(dir.clone(), None);
    let files = executor("glob", &json!({"pattern":"**/*.rs"})).unwrap();
    assert!(files.as_str().unwrap().contains("src/visible.rs"), "{files}");
    assert!(!files.as_str().unwrap().contains("ignored.rs"), "{files}");
    let hits = executor("grep", &json!({"pattern":"needle"})).unwrap();
    assert!(hits.as_str().unwrap().contains("src/visible.rs"), "{hits}");
    assert!(!hits.as_str().unwrap().contains("ignored.rs"), "{hits}");
    assert!(executor("grep", &json!({"pattern":"root","path":"escape/passwd"})).is_err());
    assert!(executor("grep", &json!({"pattern":"root","path":"/etc/passwd"})).is_err());
    assert!(!executor("glob", &json!({"pattern":"escape/**"})).unwrap().as_str().unwrap().contains("passwd"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// finding 1: a child that fills the 64KiB pipe buffer must not be mistaken
/// for a hang, and a real hang must still be killed.
#[test]
fn shell_survives_output_larger_than_the_pipe_buffer() {
    if !has_bwrap() {
        return;
    }
    let dir = scratch("big-output");
    let small = tools::shell_run("echo hello", &dir, 20, false, None).unwrap();
    assert_eq!(small, "hello\n");

    let started = Instant::now();
    let large = tools::shell_run("seq 1 20000", &dir, 20, false, None).expect("large output must not time out");
    assert!(started.elapsed() < Duration::from_secs(15), "readers drain while the child runs");
    let lines: Vec<&str> = large.lines().collect();
    assert_eq!(lines.len(), 20000, "every line arrives");
    assert_eq!(lines[0], "1");
    assert_eq!(lines[19_999], "20000");

    let failed = tools::shell_run("echo boom; exit 3", &dir, 20, false, None).unwrap();
    assert!(failed.contains("boom") && failed.contains("(exit 3)"), "{failed}");

    // a timed-out command is killed, and what it already printed is kept
    let started = Instant::now();
    let killed = tools::shell_run("echo before-hang; sleep 30; touch escaped.txt", &dir, 2, false, None)
        .expect_err("timeout must be an error");
    assert!(started.elapsed() < Duration::from_secs(30), "kill must be prompt");
    assert!(killed.contains("timed out"), "{killed}");
    assert!(killed.contains("before-hang"), "partial output survives the kill: {killed}");
    std::thread::sleep(Duration::from_secs(3));
    assert!(!dir.join("escaped.txt").exists(), "the killed command must not keep running");
    let _ = std::fs::remove_dir_all(&dir);
}

/// finding 2: output past the cap is preserved under /artifacts and readable
/// through the member's file tools.
#[test]
fn long_shell_output_is_stored_as_a_readable_artifact() {
    if !has_bwrap() {
        return;
    }
    let dir = scratch("artifacts");
    let artifacts = dir.join("artifacts");
    let output = tools::shell_run("seq 1 40000", &dir, 20, false, Some(&artifacts)).expect("must complete");
    assert!(output.len() < 220_000, "the inline text stays capped");
    let reference = output
        .split("full output: ")
        .nth(1)
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| panic!("no artifact reference in {}", &output[..200.min(output.len())]))
        .to_string();
    assert!(reference.starts_with("/artifacts/exec-"), "{reference}");
    let stored = std::fs::read_to_string(artifacts.join(reference.trim_start_matches("/artifacts/"))).expect("artifact file");
    assert_eq!(stored.lines().count(), 40000, "the artifact holds the full output");
    assert!(stored.ends_with("40000\n"));

    // the member reads it back: `/artifacts/...` through read_file, or read_artifact
    let executor =
        tools::member_executor(dir.clone(), UserConfig::default(), vec!["files".into()], Some(artifacts.clone()));
    let via_read_file = executor("read_file", &json!({"path": reference})).expect("read_file /artifacts/...");
    assert_eq!(via_read_file["content"].as_str().unwrap(), stored.lines().take(2000).map(|line| format!("{line}\n")).collect::<String>());
    assert_eq!(via_read_file["next_offset"], 2001);
    let via_tool = executor("read_artifact", &json!({"path": reference.trim_start_matches("/artifacts/")})).unwrap();
    assert_eq!(via_tool, via_read_file);
    let middle = executor("read_artifact", &json!({"path":reference,"offset":25000,"limit":2})).unwrap();
    assert_eq!(middle["content"], "25000\n25001\n");
    // members may also deliver their own artifacts under the same prefix
    executor("write_file", &json!({"path": "/artifacts/member-note.txt", "content": "done"})).unwrap();
    assert_eq!(std::fs::read_to_string(artifacts.join("member-note.txt")).unwrap(), "done");
    assert!(
        executor("read_artifact", &json!({"path": "../../etc/passwd"})).is_err(),
        "traversal out of artifacts is refused"
    );
    assert!(executor("read_file", &json!({"path": "/artifacts/../team.db"})).is_err());
    assert!(executor("write_file", &json!({"path": "/artifacts/../escape.txt", "content": "x"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The guard refuses the not-globally-reachable address table (and keeps
/// public targets usable).
#[test]
fn guard_url_matches_the_blocked_range_table() {
    for blocked in [
        "http://127.0.0.1/x",
        "http://10.0.0.5/x",
        "http://172.20.0.1/",
        "http://192.168.1.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://0.0.0.0/",
        "http://224.0.0.1/",
        "http://239.1.2.3/",
        "http://198.18.0.1/",
        "http://192.0.2.1/",
        "http://198.51.100.7/",
        "http://203.0.113.9/",
        "http://240.0.0.1/",
        "http://255.255.255.255/",
        "http://user:pass@127.0.0.1/x",
        "http://localhost/x",
        "http://[::1]:8000/x",
        "http://[::ffff:127.0.0.1]:8000/x",
        "http://[fe80::1]/",
        "http://[fc00::1]/",
        "http://[2001:db8::1]/",
        "http://[2002::1]/",
    ] {
        assert!(tools::guard_url(blocked).is_err(), "{blocked} must be refused");
    }
    for allowed in ["http://93.184.216.34/x", "https://example.com/x", "https://[2606:4700::1]/x"] {
        assert_eq!(tools::guard_url(allowed).unwrap(), allowed, "{allowed} is public");
    }
    // bracketed literals are parsed, not reported as unresolvable
    let err = tools::guard_url("http://[::1]:8000/x").unwrap_err();
    assert!(err.contains("private address"), "{err}");
}

/// finding 9: a member only gets the web tools it bound, and multiple
/// bindings resolve in member binding order.
#[test]
fn web_tools_are_fail_closed_and_ordered_by_member_binding() {
    let dir = scratch("web-bindings");
    let mut catalog = UserConfig::default();
    catalog.tools.insert("search_unknown".into(), binding(json!({"kind": "web_search", "provider": "nope"})));
    catalog.tools.insert(
        "search_anysearch".into(),
        binding(json!({"kind": "web_search", "url": "http://127.0.0.1:1/search"})),
    );
    catalog.tools.insert("fetch_private_ok".into(), binding(json!({"kind": "web_fetch", "env": {"allow_private": "1"}})));
    catalog.tools.insert("fetch_guarded".into(), binding(json!({"kind": "web_fetch"})));

    // nothing bound → explicit refusal, not a catalog-wide lookup
    let unbound = tools::member_executor(dir.clone(), catalog.clone(), vec!["files".into()], None);
    for tool in ["web_search", "web_fetch"] {
        let err = unbound(tool, &json!({"query": "q", "url": "http://127.0.0.1:1/"})).unwrap_err();
        assert!(err.contains("not bound to this member"), "{tool}: {err}");
    }

    // binding order decides which provider serves web_search
    let first = tools::member_executor(dir.clone(), catalog.clone(), vec!["search_unknown".into()], None);
    let err = first("web_search", &json!({"query": "q"})).unwrap_err();
    assert!(err.contains("unsupported web_search provider"), "{err}");
    let reordered = tools::member_executor(
        dir.clone(),
        catalog.clone(),
        vec!["search_anysearch".into(), "search_unknown".into()],
        None,
    );
    let err = reordered("web_search", &json!({"query": "q"})).unwrap_err();
    assert!(!err.contains("unsupported web_search provider"), "the first binding wins: {err}");

    // web_fetch: the first binding's allow_private decides whether loopback works
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let page = std::thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let body = "<html><head><title>Bound</title></head><body>served</body></html>";
            let _ = socket.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                    .as_bytes(),
            );
        }
    });
    let url = format!("http://127.0.0.1:{port}/");
    let allowed = tools::member_executor(dir.clone(), catalog.clone(), vec!["fetch_private_ok".into()], None)
        ("web_fetch", &json!({"url": url}))
    .expect("the allow_private binding is used");
    assert_eq!(allowed["title"], "Bound", "the allow_private binding is used");
    let guarded = tools::member_executor(dir.clone(), catalog.clone(), vec!["fetch_guarded".into()], None)
        ("web_fetch", &json!({"url": url}))
    .expect_err("the guarded binding refuses loopback");
    assert!(guarded.contains("private address"), "{guarded}");
    let _ = page.join();

    // a required web service that cannot work fails at load time
    catalog.tools.insert(
        "search_required".into(),
        binding(json!({"kind": "web_search", "provider": "nope", "required": true})),
    );
    let err = tools::validate_web_bindings(&catalog, &["search_required".into()]).unwrap_err();
    assert!(err.contains("required tool service"), "{err}");
    assert!(tools::validate_web_bindings(&catalog, &["search_unknown".into()]).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

/// finding 8: reopening an isolated workspace keeps the member's notes.
#[test]
fn isolated_workspace_inputs_note_is_not_truncated() {
    let root = scratch("isolated");
    let project = root.join("project");
    let member = root.join("members/iso");
    std::fs::create_dir_all(&project).unwrap();
    let agent: AgentSpec = serde_json::from_value(json!({
        "id": "iso", "name": "iso", "role": "worker", "runtime_kind": "deepagents",
        "model_profile": "m", "workspace_policy": "isolated",
    }))
    .unwrap();
    let first = workspace::prepare(&agent, &project, &member).unwrap();
    std::fs::write(first.path.join("INPUTS.md"), "notes to keep").unwrap();
    std::fs::write(first.path.join("draft.txt"), "work in progress").unwrap();
    let second = workspace::prepare(&agent, &project, &member).unwrap();
    assert_eq!(std::fs::read_to_string(second.path.join("INPUTS.md")).unwrap(), "notes to keep");
    assert!(second.path.join("draft.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}
