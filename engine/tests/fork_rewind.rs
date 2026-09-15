//! D-26 fork/rewind over the worker protocol (pi-style tree history).

use serde_json::{json, Value as Json};
use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

fn state_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-fork-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The worker's default leader spec names `leader_main`: give it a config of our
/// own instead of depending on the developer's ~/.config/teamagents/config.toml.
fn config_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-forkcfg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("teamagents")).unwrap();
    std::fs::write(
        dir.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nprotocol = \"openai\"\nmodel = \"test\"\n",
    )
    .unwrap();
    dir
}

fn call(child: &mut std::process::Child, stdin: &mut impl Write, id: u64, method: &str, params: Json) -> Json {
    writeln!(stdin, "{}", json!({"id": id, "method": method, "params": params})).unwrap();
    stdin.flush().unwrap();
    let stdout = child.stdout.as_mut().unwrap();
    let mut line = String::new();
    loop {
        line.clear();
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(&mut *stdout);
        // read until the response with our id (skip pushes)
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                panic!("worker exited");
            }
            let message: Json = serde_json::from_str(line.trim()).unwrap_or(json!({}));
            if message.get("push").is_some() {
                continue;
            }
            if message.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(error) = message.get("error").and_then(|v| v.as_str()) {
                    panic!("{method} failed: {error}");
                }
                return message.get("result").cloned().unwrap_or(Json::Null);
            }
        }
    }
}

#[test]
fn fork_carries_spec_and_leader_tree_but_not_team_facts() {
    let home = state_home("fork");
    let cfg = config_home("fork");
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("serve")
        .env("XDG_STATE_HOME", &home)
        .env("XDG_CONFIG_HOME", &cfg)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn engine");
    let mut stdin = child.stdin.take().unwrap();

    let opened = call(&mut child, &mut stdin, 1, "open", json!({"cwd": "/tmp", "scripts": {"leader": [["end"]]}}));
    let old_id = opened.get("session_id").and_then(|v| v.as_str()).unwrap().to_string();

    // fabricate a leader history tree as a finished turn would have left it
    let tree = json!({
        "user-dialog-ignore": {"nodes": [], "leaf": null},
        "ctx:leader:1": {"nodes": [
            {"id": "n1", "parent": null, "message": {"role": "user", "content": "u1"}},
            {"id": "n2", "parent": "n1", "message": {"role": "assistant", "content": "a1"}}
        ], "leaf": "n2"}
    });
    let member_dir = home.join("teamagents/sessions").join(&old_id).join("members/leader");
    std::fs::create_dir_all(&member_dir).unwrap();
    std::fs::write(member_dir.join("chat_tree.json"), tree.to_string()).unwrap();

    // rewind without a live runner is a clean error, not a panic
    let rewind = {
        let id = 2u64;
        writeln!(stdin, "{}", json!({"id": id, "method": "rewind", "params": {"node_id": null}})).unwrap();
        stdin.flush().unwrap();
        read_reply(&mut child, id)
    };
    assert!(rewind.get("error").and_then(|v| v.as_str()).is_some(), "rewind without runner must fail: {rewind}");

    let forked = call(&mut child, &mut stdin, 3, "fork_session", json!({}));
    let new_id = forked.get("session_id").and_then(|v| v.as_str()).unwrap().to_string();
    assert_ne!(new_id, old_id);
    assert_eq!(forked.get("forked_from").and_then(|v| v.as_str()), Some(old_id.as_str()));

    // the leader tree file came along; team facts (tasks/runs) did not
    let copied = std::fs::read_to_string(home.join("teamagents/sessions").join(&new_id).join("members/leader/chat_tree.json")).unwrap();
    assert!(copied.contains("u1"), "tree not copied: {copied}");
    let state = call(&mut child, &mut stdin, 4, "call", json!({"method": "state", "params": {"session_id": new_id}}));
    let tasks = state.get("tasks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert!(tasks.is_empty(), "fork must start with fresh team state");

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}

fn read_reply(child: &mut std::process::Child, id: u64) -> Json {
    use std::io::BufRead;
    let stdout = child.stdout.as_mut().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            panic!("worker exited");
        }
        let message: Json = serde_json::from_str(line.trim()).unwrap_or(json!({}));
        if message.get("push").is_none() && message.get("id").and_then(|v| v.as_u64()) == Some(id) {
            return message;
        }
    }
}

#[test]
fn fork_preserves_model_files_and_legacy_history() {
    let home = state_home("files");
    let cfg = config_home("files");
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents")).arg("serve")
        .env("XDG_STATE_HOME", &home)
        .env("XDG_CONFIG_HOME", &cfg).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let opened = call(&mut child, &mut stdin, 1, "open", json!({"cwd":"/tmp", "scripts":{"leader":[["end"]]}}));
    let old = opened["session_id"].as_str().unwrap().to_string();
    let base = home.join("teamagents/sessions").join(&old);
    std::fs::write(base.join("profiles.json"), r#"{"leader":{"model":"m1"}}"#).unwrap();
    std::fs::write(base.join("model_overrides.json"), r#"{"leader":{"model":"m2"}}"#).unwrap();
    let member = base.join("members/leader"); std::fs::create_dir_all(&member).unwrap();
    std::fs::write(member.join("chat_history.json"), r#"{"ctx:leader:1":[{"role":"user","content":"legacy"}]}"#).unwrap();
    let fork = call(&mut child, &mut stdin, 2, "fork_session", json!({}));
    let new = fork["session_id"].as_str().unwrap();
    let dest = home.join("teamagents/sessions").join(new);
    assert_eq!(std::fs::read_to_string(dest.join("profiles.json")).unwrap(), r#"{"leader":{"model":"m1"}}"#);
    assert_eq!(std::fs::read_to_string(dest.join("model_overrides.json")).unwrap(), r#"{"leader":{"model":"m2"}}"#);
    assert!(std::fs::read_to_string(dest.join("members/leader/chat_history.json")).unwrap().contains("legacy"));
    let _ = child.kill(); let _ = child.wait();
}

#[test]
fn failed_switch_keeps_current_session_open() {
    let home = state_home("failed-switch");
    let cfg = config_home("failed-switch");
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents")).arg("serve")
        .env("XDG_STATE_HOME", &home)
        .env("XDG_CONFIG_HOME", &cfg).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let opened = call(&mut child, &mut stdin, 1, "open", json!({"cwd":"/tmp", "scripts":{"leader":[["end"]]}}));
    let old = opened["session_id"].as_str().unwrap().to_string();
    writeln!(stdin, "{}", json!({"id":2,"method":"switch_session","params":{"session_id":"../escape"}})).unwrap(); stdin.flush().unwrap();
    let failure = read_reply(&mut child, 2); assert!(failure.get("error").is_some());
    let state = call(&mut child, &mut stdin, 3, "call", json!({"method":"state","params":{}}));
    assert_eq!(state["session"]["session_id"].as_str(), Some(old.as_str()));
    let _ = child.kill(); let _ = child.wait();
}
