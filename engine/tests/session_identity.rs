//! T24: production session paths preserve identity, never display-name identity.
//! Local protocol fixtures only; these are not real-provider acceptance tests.

mod support;

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;
use teamagents_engine::session::{open_session, OpenOptions, OpenedSession};
use teamagents_engine::sessions::session_paths;

struct LocalModel {
    url: String,
    requests: Arc<Mutex<Vec<Json>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LocalModel {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, done) = (requests.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => Self::respond(stream, &seen),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("local model accept: {e}"),
                }
            }
        });
        Self { url, requests, stop, handle: Some(handle) }
    }

    fn respond(mut stream: TcpStream, seen: &Mutex<Vec<Json>>) {
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        let mut len = 0;
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("content-length") {
                    len = value.trim().parse::<usize>().unwrap();
                }
            }
        }
        assert!(len > 0 && len < 2_000_000);
        let mut bytes = vec![0; len];
        reader.read_exact(&mut bytes).unwrap();
        let request: Json = serde_json::from_slice(&bytes).unwrap();
        let index = {
            let mut seen = seen.lock().unwrap();
            seen.push(request.clone());
            seen.len()
        };
        let messages = request["messages"].as_array().unwrap();
        let worker = messages[0]["content"].as_str().unwrap().contains("Member id:");
        let message = if worker && messages.last().unwrap()["role"] != "tool" {
            json!({"role":"assistant", "content":null, "tool_calls":[{
                "id":format!("read-{index}"), "type":"function",
                "function":{"name":"read_file", "arguments":json!({"path":"notes.txt"}).to_string()}
            }]})
        } else {
            json!({"role":"assistant", "content":"acknowledged without publishing private notes"})
        };
        let body = json!({"choices":[{"message":message}],
            "usage":{"prompt_tokens":100, "completion_tokens":10, "total_tokens":110}})
        .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    }

    fn catalog(&self) -> Json {
        json!({"models":{"m":{"provider":"openai", "protocol":"openai", "model":"local-only",
            "base_url":self.url, "max_retries":0, "timeout":3}}})
    }
}

impl Drop for LocalModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn worker(id: &str, kind: &str) -> Json {
    json!({"id":id, "name":"同名工程师", "role":"worker", "runtime_kind":kind,
        "model_profile":"m", "workspace_policy":"isolated", "tool_bindings":["files"]})
}

fn spec(kind: &str) -> Json {
    json!({"leader_id":"leader", "agents":[member("leader","leader"),worker("b",kind)],
        "channels":[message_channel("leader", &["b"])]})
}

fn open(project: &Path, id: &str, catalog: Json, spec: Json) -> Arc<OpenedSession> {
    open_session(OpenOptions {
        cwd: Some(project.to_path_buf()),
        session_id: Some(id.into()),
        catalog: Some(serde_json::from_value(catalog).unwrap()),
        initial_spec: Some(spec),
        ..Default::default()
    })
    .unwrap()
}

fn patch(session: &OpenedSession, operations: Json) -> teamagents_core::models::Receipt {
    let revision = session.core.state_brief().unwrap()["revision"].clone();
    submit(
        &session.core,
        &uuid::Uuid::new_v4().to_string(),
        "leader",
        "apply_topology_patch",
        json!({"base_revision":revision, "operations":operations}),
    )
}

fn send(session: &OpenedSession, target: &str, phase: &str) {
    let receipt = submit(&session.core, phase, "leader", "send_message", json!({"target":target,"text":phase}));
    assert!(receipt.ok, "{receipt:?}");
    assert!(session.runtime.settle(10));
    let state = session.core.state_brief().unwrap();
    assert!(state["runs"].as_array().unwrap().iter().all(|r| r["status"] == "COMPLETED"), "{state}");
}

fn chat_turn(session: &OpenedSession, model: &LocalModel, target: &str, phase: &str) -> Vec<Json> {
    let before = model.requests.lock().unwrap().len();
    send(session, target, phase);
    let calls = model.requests.lock().unwrap()[before..].to_vec();
    assert_eq!(calls.len(), 2, "one model request, real read_file, then continuation");
    assert!(calls[1]["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool"));
    calls
}

fn notes(session: &str, member: &str) -> std::path::PathBuf {
    session_paths(session).base.join("members").join(member).join("work/notes.txt")
}

#[test]
fn t24_chat_resume_same_name_replacement_and_new_session_are_isolated() {
    let env = isolated_state_home("identity-chat");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session = open(&project, "identity-one", model.catalog(), spec("deepagents"));
    std::fs::write(notes("identity-one", "b"), "PRIVATE_OLD_MEMBER_TOOL_OUTPUT").unwrap();
    session.runtime.start();
    let first = chat_turn(&session, &model, "b", "first-private-turn");
    assert!(!first[0].to_string().contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));
    assert!(first[1].to_string().contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));
    let usage = session.usage_report();
    assert_eq!(usage["agents"][1]["usage"]["calls"], 2);
    session.set_model_override("b", Some("old-member-override".into()), None).unwrap();
    // A model selection must not erase usage while waiting for the next turn.
    assert_eq!(session.usage_report()["agents"][1]["usage"]["calls"], 2);
    session.close();
    drop(session);

    let resumed = open(&project, "identity-one", model.catalog(), spec("deepagents"));
    resumed.runtime.start();
    let resumed_calls = chat_turn(&resumed, &model, "b", "resume-private-turn");
    assert_eq!(resumed_calls[0]["model"], "old-member-override");
    assert!(resumed_calls[0].to_string().contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));
    let original = session_paths("identity-one").base.join("members/b/chat_tree.json");
    let original_bytes = std::fs::read(&original).unwrap();
    let tree: Json = serde_json::from_slice(&original_bytes).unwrap();
    assert!(tree.get("ctx:b:1").is_some(), "stable epoch and member identity across restart");

    // The human state may show progress, but neither the model request nor its
    // user-facing agent_view automatically acquires another member's tool data.
    assert!(!resumed
        .core
        .call_in_session("agent_view", json!({"agent_id":"leader"}))
        .unwrap()
        .to_string()
        .contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));
    let before = model.requests.lock().unwrap().len();
    resumed.runtime.user_message("leader isolation check", false).unwrap();
    assert!(resumed.runtime.settle(10));
    let leader_requests = model.requests.lock().unwrap()[before..].to_vec();
    assert!(!leader_requests.is_empty());
    assert!(!serde_json::to_string(&leader_requests).unwrap().contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));

    let removed = patch(&resumed, json!([{"op":"remove_agent","agent_id":"b"}]));
    assert!(removed.ok, "{removed:?}");
    assert!(wait_for(|| resumed.runtime.runner("b").is_none(), 3000));
    let reused = patch(&resumed, json!([{"op":"add_agent","agent":worker("b","deepagents")}]));
    assert!(!reused.ok && reused.error.unwrap().contains("cannot be reused"));
    let added = patch(
        &resumed,
        json!([{"op":"add_agent","agent":worker("replacement","deepagents"),
        "channels":[message_channel("leader", &["replacement"])]}]),
    );
    assert!(added.ok, "{added:?}");
    // Dynamic member workspaces are prepared lazily; populate explicit inputs.
    let fresh_notes = notes("identity-one", "replacement");
    std::fs::create_dir_all(fresh_notes.parent().unwrap()).unwrap();
    std::fs::write(fresh_notes, "PRIVATE_REPLACEMENT_INPUT").unwrap();
    let replacement = chat_turn(&resumed, &model, "replacement", "replacement-private-turn");
    assert!(!serde_json::to_string(&replacement).unwrap().contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT"));
    assert!(replacement[1].to_string().contains("PRIVATE_REPLACEMENT_INPUT"));
    assert_eq!(replacement[0]["model"], "local-only", "display names must not inherit model overrides");
    assert_eq!(std::fs::read(&original).unwrap(), original_bytes, "retained history is not overwritten");
    assert_eq!(std::fs::read_to_string(notes("identity-one", "b")).unwrap(), "PRIVATE_OLD_MEMBER_TOOL_OUTPUT");
    resumed.close();
    drop(resumed);

    let fresh = open(&project, "identity-two", model.catalog(), spec("deepagents"));
    std::fs::write(notes("identity-two", "b"), "PRIVATE_NEW_SESSION_INPUT").unwrap();
    fresh.runtime.start();
    let calls = chat_turn(&fresh, &model, "b", "new-session-private-turn");
    let wire = serde_json::to_string(&calls).unwrap();
    assert!(!wire.contains("PRIVATE_OLD_MEMBER_TOOL_OUTPUT") && !wire.contains("PRIVATE_REPLACEMENT_INPUT"));
    assert!(wire.contains("PRIVATE_NEW_SESSION_INPUT"));
    assert_eq!(calls[0]["model"], "local-only");
    assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
    fresh.close();
}

#[test]
fn damaged_legacy_history_stops_before_model_calls_and_preserves_evidence() {
    let env = isolated_state_home("damaged-legacy-history");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session_id = "damaged-legacy";
    let session =
        open(&project, session_id, model.catalog(), json!({"leader_id":"leader","agents":[member("leader","leader")]}));
    session.close();
    drop(session);
    let member_dir = session_paths(session_id).base.join("members/leader");
    std::fs::create_dir_all(&member_dir).unwrap();
    let original = br#"{"ctx:leader:1":{"damaged_messages":[{"role":"user","content":"keep this task"}]}}"#;
    let history = member_dir.join("chat_history.json");
    std::fs::write(&history, original).unwrap();

    let restored =
        open(&project, session_id, model.catalog(), json!({"leader_id":"leader","agents":[member("leader","leader")]}));
    restored.runtime.start();
    restored.runtime.user_message("Continue my previous task.", false).unwrap();
    let settled = restored.runtime.settle(5);
    let state = restored.core.state().unwrap();
    restored.close();
    drop(restored);
    assert!(settled);
    assert!(model.requests.lock().unwrap().is_empty(), "damaged history must not silently start a fresh model request");
    assert_eq!(state["runs"][0]["status"], "OUTCOME_UNKNOWN", "{state}");
    assert!(
        state["events"].as_array().unwrap().iter().any(|event| {
            event["kind"] == "run_failed"
                && event["payload"]["error"].as_str().unwrap_or("").contains("chat_history.json")
        }),
        "{state}"
    );
    assert_eq!(std::fs::read(&history).unwrap(), original);
    assert!(!member_dir.join("chat_tree.json").exists(), "the invalid legacy source must not migrate to an empty tree");
}

#[test]
fn cyclic_history_refuses_resume_and_rewind_without_losing_the_source() {
    let env = isolated_state_home("cyclic-history");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session_id = "cyclic-history";
    let team = json!({"leader_id":"leader","agents":[member("leader","leader")]});
    let session = open(&project, session_id, model.catalog(), team.clone());
    session.runtime.start();
    session.runtime.user_message("Remember the original task.", false).unwrap();
    assert!(session.runtime.settle(5));
    session.close();
    drop(session);
    let member_dir = session_paths(session_id).base.join("members/leader");
    let path = member_dir.join("chat_tree.json");
    let original = std::fs::read(&path).unwrap();
    let linear = std::fs::read(member_dir.join("chat_history.json")).unwrap();
    let mut tree: Json = serde_json::from_slice(&original).unwrap();
    let tip = tree["ctx:leader:1"]["leaf"].clone();
    tree["ctx:leader:1"]["nodes"][0]["parent"] = tip;
    let damaged = tree.to_string();
    std::fs::write(&path, &damaged).unwrap();
    let calls_before = model.requests.lock().unwrap().len();

    let restored = open(&project, session_id, model.catalog(), team.clone());
    let points = restored.rewind_points();
    let rewind = restored.rewind(None);
    restored.runtime.start();
    restored.runtime.user_message("Continue after restart.", false).unwrap();
    let settled = restored.runtime.settle(5);
    let state = restored.core.state().unwrap();
    restored.close();
    drop(restored);
    assert!(settled);
    assert!(points.unwrap_err().contains("chat_tree.json"));
    assert!(rewind.unwrap_err().contains("chat_tree.json"), "even rewind-to-empty must preserve corrupt evidence");
    assert_eq!(model.requests.lock().unwrap().len(), calls_before, "no new model request or tool side effect");
    assert!(state["runs"].as_array().unwrap().iter().any(|run| run["status"] == "OUTCOME_UNKNOWN"));
    assert!(state["events"].as_array().unwrap().iter().any(|event| {
        event["kind"] == "run_failed" && event["payload"]["error"].as_str().unwrap_or("").contains("chat_tree.json")
    }));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    assert_eq!(std::fs::read(member_dir.join("chat_history.json")).unwrap(), linear);

    // An explicit restore of the original file makes the history readable
    // again; there is no automatic reset or guessed repair.
    std::fs::write(&path, &original).unwrap();
    let repaired = open(&project, session_id, model.catalog(), team);
    let points = repaired.rewind_points().unwrap();
    repaired.close();
    drop(repaired);
    assert!(!points["points"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(model.requests.lock().unwrap().len(), calls_before);
}

#[test]
fn valid_legacy_history_migrates_while_other_threads_remain_intact() {
    let env = isolated_state_home("valid-legacy-history");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session_id = "valid-legacy";
    let team = json!({"leader_id":"leader","agents":[member("leader","leader")]});
    let session = open(&project, session_id, model.catalog(), team.clone());
    session.close();
    drop(session);
    let member_dir = session_paths(session_id).base.join("members/leader");
    std::fs::create_dir_all(&member_dir).unwrap();
    let legacy = json!({
        "ctx:leader:1":[{"role":"user","content":"LEGACY_TASK_TO_CONTINUE"}],
        "ctx:leader:0":[{"role":"user","content":"RETIRED_EPOCH"}]
    });
    let old_tree = json!({"nodes":[
        {"id":"old-root","parent":null,"message":{"role":"user","content":"old root"}},
        {"id":"old-leaf","parent":"old-root","message":{"role":"assistant","content":"abandoned branch"}}
    ],"leaf":null});
    std::fs::write(member_dir.join("chat_history.json"), legacy.to_string()).unwrap();
    std::fs::write(member_dir.join("chat_tree.json"), json!({"ctx:leader:0":old_tree}).to_string()).unwrap();
    let restored = open(&project, session_id, model.catalog(), team);
    let points = restored.rewind_points().unwrap();
    assert_eq!(points["points"][0]["preview"], "LEGACY_TASK_TO_CONTINUE");
    restored.runtime.start();
    restored.runtime.user_message("Continue.", false).unwrap();
    let settled = restored.runtime.settle(5);
    let state = restored.core.state_brief().unwrap();
    restored.close();
    drop(restored);
    assert!(settled);
    assert_eq!(state["runs"][0]["status"], "COMPLETED");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].to_string().contains("LEGACY_TASK_TO_CONTINUE"));
    assert!(!requests[0].to_string().contains("RETIRED_EPOCH"));
    let saved: Json = serde_json::from_slice(&std::fs::read(member_dir.join("chat_tree.json")).unwrap()).unwrap();
    assert_eq!(saved["ctx:leader:0"], old_tree, "unselected tree and abandoned branch remain intact");
    assert!(saved["ctx:leader:1"].to_string().contains("LEGACY_TASK_TO_CONTINUE"));
    let linear: Json = serde_json::from_slice(&std::fs::read(member_dir.join("chat_history.json")).unwrap()).unwrap();
    assert_eq!(linear["ctx:leader:0"], legacy["ctx:leader:0"]);
}

fn records(path: &Path) -> Vec<Json> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn process_alive(pid: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next()) != Some("Z")
}

#[test]
fn t24_codex_resumes_only_the_same_session_member_and_retires_its_server() {
    use std::os::unix::fs::PermissionsExt;
    let mut env = isolated_state_home("identity-codex");
    let bin = env.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("codex");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, os, sys, uuid
log = os.environ['TA_IDENTITY_CODEX_LOG']
for line in sys.stdin:
    message = json.loads(line)
    with open(log, 'a') as file:
        file.write(json.dumps({'pid':os.getpid(), **message}) + '\n')
    if 'id' not in message: continue
    method, params = message.get('method'), message.get('params', {})
    if method == 'thread/start':
        result = {'thread':{'id':'thread-' + uuid.uuid4().hex}}
    elif method == 'thread/resume':
        result = {'thread':{'id':params['threadId']}}
    elif method == 'turn/start':
        turn = 'turn-' + uuid.uuid4().hex
        result = {'turn':{'id':turn, 'status':'inProgress'}}
    else: result = {}
    print(json.dumps({'id':message['id'], 'result':result}), flush=True)
    if method == 'turn/start':
        print(json.dumps({'method':'item/completed', 'params':{
            'threadId':params['threadId'], 'turnId':turn,
            'item':{'type':'agentMessage', 'text':'local identity fixture completed'}}}), flush=True)
        print(json.dumps({'method':'turn/completed', 'params':{
            'threadId':params['threadId'], 'turn':{'id':turn,'status':'completed'}}}), flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(bin).chain(std::env::split_paths(&path));
    env.set("PATH", std::env::join_paths(paths).unwrap());
    let log = env.join("requests.jsonl");
    env.set("TA_IDENTITY_CODEX_LOG", &log);
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session = open(&project, "codex-one", model.catalog(), spec("codex"));
    std::fs::write(notes("codex-one", "b"), "original codex work").unwrap();
    session.runtime.start();
    send(&session, "b", "first-codex-turn");
    let thread =
        session.core.call_in_session("get_codex_thread", json!({"agent_id":"b"})).unwrap()["thread_id"].clone();
    assert!(thread.as_str().is_some_and(|id| id.starts_with("thread-")));
    let first = records(&log);
    assert_eq!(first.iter().filter(|r| r["method"] == "thread/start").count(), 1);
    assert!(!first.iter().any(|r| r["method"] == "thread/resume"));
    let first_pid = first[0]["pid"].as_u64().unwrap();
    assert!(process_alive(first_pid));
    session.close();
    assert!(wait_for(|| !process_alive(first_pid), 3000));
    drop(session);

    let resumed = open(&project, "codex-one", model.catalog(), spec("codex"));
    resumed.runtime.start();
    send(&resumed, "b", "resumed-codex-turn");
    let after_resume = records(&log);
    let resumed_request = after_resume.iter().find(|r| r["method"] == "thread/resume").unwrap();
    assert_eq!(resumed_request["params"]["threadId"], thread);
    assert_eq!(after_resume.iter().filter(|r| r["method"] == "thread/start").count(), 1);
    let resumed_pid = resumed_request["pid"].as_u64().unwrap();
    let weak = Arc::downgrade(&resumed.runtime.runner("b").unwrap());
    assert!(patch(&resumed, json!([{"op":"remove_agent", "agent_id":"b"}])).ok);
    let stopped = wait_for(|| !process_alive(resumed_pid) && weak.upgrade().is_none(), 3000);
    let added = patch(
        &resumed,
        json!([{"op":"add_agent","agent":worker("replacement","codex"),
        "channels":[message_channel("leader", &["replacement"])]}]),
    );
    assert!(added.ok, "{added:?}");
    send(&resumed, "replacement", "replacement-codex-turn");
    let replacement_thread =
        resumed.core.call_in_session("get_codex_thread", json!({"agent_id":"replacement"})).unwrap()["thread_id"]
            .clone();
    assert_ne!(replacement_thread, thread);
    assert_eq!(records(&log).iter().filter(|r| r["method"] == "thread/start").count(), 2);
    assert_eq!(std::fs::read_to_string(notes("codex-one", "b")).unwrap(), "original codex work");
    resumed.close();
    assert!(stopped, "the removed Codex server and runner must retire before session shutdown");
    drop(resumed);

    // The tombstone also survives restarting the session after removal.
    let reopened = open(&project, "codex-one", model.catalog(), spec("codex"));
    let reused = patch(&reopened, json!([{"op":"add_agent", "agent":worker("b","codex")}]));
    assert!(!reused.ok && reused.error.unwrap().contains("cannot be reused"));
    reopened.close();
    drop(reopened);

    let before = records(&log).len();
    let fresh = open(&project, "codex-two", model.catalog(), spec("codex"));
    fresh.runtime.start();
    send(&fresh, "b", "new-session-codex-turn");
    let fresh_thread =
        fresh.core.call_in_session("get_codex_thread", json!({"agent_id":"b"})).unwrap()["thread_id"].clone();
    assert!(fresh_thread.is_string() && fresh_thread != thread && fresh_thread != replacement_thread);
    let last = records(&log);
    assert!(last[before..].iter().any(|r| r["method"] == "thread/start"));
    assert!(!last[before..].iter().any(|r| r["method"] == "thread/resume"));
    fresh.close();
    assert!(last.iter().all(|r| !process_alive(r["pid"].as_u64().unwrap())));
    assert!(model.requests.lock().unwrap().is_empty(), "no Leader model request was necessary");
}

#[test]
fn codex_member_receives_the_same_bounded_skills_and_project_instructions_as_chat() {
    use std::os::unix::fs::PermissionsExt;

    let mut env = isolated_state_home("codex-skills");
    let bin = env.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("codex");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, os, sys

log = os.environ['TA_CODEX_SKILLS_LOG']
for line in sys.stdin:
    message = json.loads(line)
    with open(log, 'a') as file:
        file.write(json.dumps(message) + '\n')
    if 'id' not in message:
        continue
    method, params = message.get('method'), message.get('params', {})
    if method == 'thread/start':
        result = {'thread': {'id': 'skills-thread'}}
    elif method == 'turn/start':
        result = {'turn': {'id': 'skills-turn', 'status': 'inProgress'}}
    else:
        result = {}
    print(json.dumps({'id': message['id'], 'result': result}), flush=True)
    if method == 'turn/start':
        print(json.dumps({'method': 'turn/completed', 'params': {
            'threadId': params['threadId'],
            'turn': {'id': 'skills-turn', 'status': 'completed'}
        }}), flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(bin).chain(std::env::split_paths(&path));
    env.set("PATH", std::env::join_paths(paths).unwrap());

    let project = env.join("project");
    let registry = env.join("skills");
    std::fs::create_dir_all(registry.join("review")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(registry.join("review/SKILL.md"), "selected review skill").unwrap();
    std::fs::write(project.join("AGENTS.md"), "project-only instructions").unwrap();
    let log = env.join("codex-requests.jsonl");
    env.set("TA_CODEX_SKILLS_LOG", &log);

    let catalog = json!({
        "models": {"m": {"provider": "openai", "protocol": "openai", "model": "local-only"}},
        "skills_paths": [registry.to_string_lossy()],
        "instruction_files": [],
    });
    let mut team = spec("codex");
    team["agents"][1]["instructions"] = json!("member-specific instructions");
    team["agents"][1]["skills"] = json!(["review"]);

    let session = open(&project, "codex-skills", catalog.clone(), team.clone());
    session.runtime.start();
    send(&session, "b", "inspect injected context");

    let requests = records(&log);
    let start = requests.iter().find(|request| request["method"] == "thread/start").unwrap();
    let instructions = start["params"]["developerInstructions"].as_str().unwrap();
    assert!(instructions.contains("<teamagents_worker>"));
    assert!(instructions.contains("member-specific instructions"));
    assert!(instructions.contains("selected review skill"));
    assert!(instructions.contains("project-only instructions"));
    assert!(
        instructions.find("member-specific instructions").unwrap()
            < instructions.find("selected review skill").unwrap()
    );
    assert!(
        instructions.find("selected review skill").unwrap() < instructions.find("project-only instructions").unwrap()
    );
    assert!(!instructions.contains("<member_context source=\"escape\">"));

    session.close();

    let resumed = open(&project, "codex-skills", catalog, team);
    resumed.runtime.start();
    send(&resumed, "b", "inspect injected context after resume");
    let resumed_request = records(&log)
        .into_iter()
        .find(|request| request["method"] == "thread/resume")
        .expect("the same session/member must resume its Codex thread");
    let resumed_instructions = resumed_request["params"]["developerInstructions"].as_str().unwrap();
    assert!(resumed_instructions.contains("selected review skill"));
    assert!(resumed_instructions.contains("project-only instructions"));
    resumed.close();
}
