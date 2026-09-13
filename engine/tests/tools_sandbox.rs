//! Sandbox regressions: shell output larger than the pipe buffer, artifact
//! spill for long output, the SSRF guard table, web bindings fail-closed, and
//! the isolated workspace note (findings 1/2/6/8/9).

use serde_json::json;
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
    assert_eq!(via_read_file.as_str().unwrap(), stored);
    let via_tool = executor("read_artifact", &json!({"path": reference.trim_start_matches("/artifacts/")})).unwrap();
    assert_eq!(via_tool, via_read_file);
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

/// finding 6: the guard refuses what Python's ipaddress refuses (and keeps
/// public targets usable).
#[test]
fn guard_url_matches_the_python_guard_table() {
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
