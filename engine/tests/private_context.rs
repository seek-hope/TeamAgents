//! T6: inspect actual model requests through the production session/tool path.
//! The model is a local protocol fixture, not a real-provider acceptance run.

mod support;

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;
use teamagents_engine::session::{open_session, OpenOptions, OpenedSession};
use teamagents_engine::sessions::session_paths;

const MEMBERS: [&str; 4] = ["leader", "a", "b", "c"];
type Responses = Mutex<HashMap<String, VecDeque<Json>>>;

struct LocalModel {
    url: String,
    requests: Arc<Mutex<Vec<Json>>>,
    responses: Arc<Responses>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalModel {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let responses = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, replies, done) = (requests.clone(), responses.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => Self::respond(stream, &seen, &replies),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("local model accept: {e}"),
                }
            }
        });
        Self { url, requests, responses, stop, thread: Some(thread) }
    }

    fn respond(mut stream: TcpStream, seen: &Mutex<Vec<Json>>, replies: &Responses) {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut len = 0;
        loop {
            let mut line = String::new();
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
        let model = request["model"].as_str().unwrap();
        let message = replies
            .lock()
            .unwrap()
            .get_mut(model)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| json!({"role":"assistant","content":"Acknowledged."}));
        seen.lock().unwrap().push(request);
        let body = json!({"choices":[{"message":message}],
            "usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}})
        .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    }

    fn tools(&self, member: &str, calls: Vec<Json>) {
        self.responses
            .lock()
            .unwrap()
            .entry(member.into())
            .or_default()
            .push_back(json!({"role":"assistant","content":null,"tool_calls":calls}));
    }

    fn catalog(&self) -> Json {
        let models: serde_json::Map<String, Json> = MEMBERS
            .iter()
            .map(|id| {
                (
                    id.to_string(),
                    json!({"provider":"openai","protocol":"openai","model":id,
                        "base_url":self.url,"max_retries":0,"timeout":5}),
                )
            })
            .collect();
        json!({"models":models})
    }

    fn calls(&self, member: &str, after: usize) -> Vec<Json> {
        self.requests.lock().unwrap()[after..].iter().filter(|r| r["model"] == member).cloned().collect()
    }

    fn position(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for LocalModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

// Close all execution before TestEnv restores the process environment, including
// when a privacy assertion deliberately fails during a regression's red phase.
struct Session(Arc<OpenedSession>);

impl Deref for Session {
    type Target = OpenedSession;
    fn deref(&self) -> &OpenedSession {
        &self.0
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn spec() -> Json {
    let agents: Vec<Json> = MEMBERS
        .iter()
        .map(|id| {
            json!({"id":id,"name":id,"role":if *id == "leader" {"leader"} else {"worker"},
                "runtime_kind":"deepagents","model_profile":id,"workspace_policy":"isolated",
                "instructions":format!("PRIVATE_INSTRUCTIONS_{id}"),"tool_bindings":["files","shell"]})
        })
        .collect();
    json!({"leader_id":"leader","agents":agents,
        "channels":[message_channel("leader",&["a","b","c"]),message_channel("a",&["b"])],
        "shared_spaces":[{"id":"results","readers":MEMBERS,"writers":["a"]}]})
}

fn open(project: &Path, id: &str, model: &LocalModel) -> Session {
    open_with_spec(project, id, model, spec())
}

fn open_with_spec(project: &Path, id: &str, model: &LocalModel, spec: Json) -> Session {
    Session(
        open_session(OpenOptions {
            cwd: Some(project.to_path_buf()),
            session_id: Some(id.into()),
            catalog: Some(serde_json::from_value(model.catalog()).unwrap()),
            initial_spec: Some(spec),
            ..Default::default()
        })
        .unwrap(),
    )
}

#[test]
fn t6_observer_scope_and_authorized_forwarding_do_not_grant_private_history() {
    let env = isolated_state_home("t6-observers");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    for scope in ["status", "public_message", "result"] {
        let id = format!("t6-observer-{scope}");
        let mut configured = spec();
        configured["observers"] = json!([{"agent_id":"c","subjects":["a"],"event_types":["message"],
            "payload_scope":scope,"wake_policy":"on_event"}]);
        configured["channels"].as_array_mut().unwrap().push(message_channel("b", &["leader"]));
        let session = open_with_spec(&project, &id, &model, configured);
        std::fs::write(session_paths(&id).base.join("members/a/work/notes.txt"), "PRIVATE_OBSERVER_TOOL_DATA").unwrap();
        model.tools(
            "a",
            vec![
                call("observer-a-notes", "read_file", json!({"path":"notes.txt"})),
                call("observer-a-message", "send_message", json!({"target":"b","text":"SCOPED_MESSAGE_A_TO_B"})),
            ],
        );
        model.tools(
            "c",
            vec![
                call("observer-history", "read_history", json!({"tool_call_id":"observer-a-notes"})),
                call("observer-reply", "send_message", json!({"target":"a","text":"UNAUTHORIZED_REPLY"})),
            ],
        );
        session.runtime.start();
        let before = model.position();
        wake(&session, "a");
        let calls = model.calls("c", before);
        assert_eq!(serde_json::to_string(&calls).unwrap().contains("SCOPED_MESSAGE_A_TO_B"), scope != "status");
        assert!(result(&calls, "observer-history")["error"].is_string());
        assert!(result(&calls, "observer-reply")["error"].is_string());
        absent(&calls, &["PRIVATE_OBSERVER_TOOL_DATA", "PRIVATE_INSTRUCTIONS_a"]);
        absent(&model.calls("a", before), &["UNAUTHORIZED_REPLY"]);
        wake(&session, "leader");
        absent(&model.calls("leader", before), &["SCOPED_MESSAGE_A_TO_B", "PRIVATE_OBSERVER_TOOL_DATA"]);

        model.tools(
            "b",
            vec![call("explicit-forward", "send_message", json!({"target":"leader","text":"SCOPED_MESSAGE_A_TO_B"}))],
        );
        let forward = model.position();
        wake(&session, "b");
        let leader = model.calls("leader", forward);
        assert!(serde_json::to_string(&leader).unwrap().contains("SCOPED_MESSAGE_A_TO_B"));
        absent(&leader, &["PRIVATE_OBSERVER_TOOL_DATA", "PRIVATE_INSTRUCTIONS_a"]);
    }
}

#[test]
fn t6_shared_workspace_cannot_expose_the_runtime_state_tree() {
    let env = isolated_state_home("t6-overlap");
    let model = LocalModel::start();
    let mut configured = spec();
    configured["agents"][3]["workspace_policy"] = json!("shared");
    let opened = open_session(OpenOptions {
        cwd: Some(env.to_path_buf()),
        session_id: Some("t6-overlap".into()),
        catalog: Some(serde_json::from_value(model.catalog()).unwrap()),
        initial_spec: Some(configured),
        ..Default::default()
    });
    match opened {
        Err(error) => {
            assert!(error.contains("工作目录与私有运行数据重叠"), "{error}");
            assert_eq!(model.position(), 0, "reject an unsafe root before any model execution");
        }
        Ok(session) => {
            let session = Session(session);
            session.runtime.start();
            wake(&session, "a");
            model.tools(
                "c",
                vec![call(
                    "nested-state",
                    "read_file",
                    json!({"path":session_paths("t6-overlap").base.join("members/a/chat_tree.json")}),
                )],
            );
            wake(&session, "c");
            let calls = model.calls("c", 0);
            assert!(result(&calls, "nested-state")["error"].is_string(), "the workspace exposes private state");
            absent(&calls, &["PRIVATE_INSTRUCTIONS_a"]);
        }
    }
}

#[test]
fn t6_shared_workspace_rejects_symlinked_or_not_yet_created_private_roots() {
    let mut env = isolated_state_home("t6-root-aliases");
    let project = env.join("project");
    std::fs::create_dir_all(project.join("config")).unwrap();
    std::os::unix::fs::symlink(project.join("config"), env.join("config-link")).unwrap();
    env.set("XDG_CONFIG_HOME", env.join("config-link"));
    let model = LocalModel::start();
    for phase in ["config-alias", "codex-missing", "state-alias"] {
        let mut configured = spec();
        configured["agents"][0]["workspace_policy"] = json!("shared");
        let opened = open_session(OpenOptions {
            cwd: Some(project.clone()),
            session_id: Some(format!("t6-{phase}")),
            catalog: Some(serde_json::from_value(model.catalog()).unwrap()),
            initial_spec: Some(configured),
            ..Default::default()
        });
        match opened {
            Err(error) => assert!(error.contains("工作目录与私有运行数据重叠"), "{phase}: {error}"),
            Ok(session) => {
                session.close();
                panic!("unsafe private directory was accepted: {phase}");
            }
        }
        match phase {
            "config-alias" => {
                env.set("XDG_CONFIG_HOME", env.join("safe-config"));
                env.set("CODEX_HOME", project.join("not-created-yet"));
            }
            "codex-missing" => {
                env.set("CODEX_HOME", env.join("safe-codex"));
                std::fs::create_dir_all(project.join("state")).unwrap();
                std::os::unix::fs::symlink(project.join("state"), env.join("state-link")).unwrap();
                env.set("XDG_STATE_HOME", env.join("state-link"));
            }
            _ => {}
        }
    }
    assert_eq!(model.position(), 0);
}

#[test]
fn t6_legacy_logs_and_private_image_references_cannot_bypass_file_permissions() {
    let env = isolated_state_home("t6-legacy");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session = open(&project, "t6-legacy", &model);
    let paths = session_paths("t6-legacy");
    let legacy = paths.artifacts.join("exec-old.log");
    std::fs::write(&legacy, "PRIVATE_LEGACY_OUTPUT").unwrap();
    std::os::unix::fs::symlink(&legacy, paths.artifacts.join("innocent.txt")).unwrap();
    let private = paths.base.join("members/a/tool-output");
    std::fs::create_dir_all(&private).unwrap();
    let image = b"\x89PNG\r\n\x1a\nPRIVATE_IMAGE_BYTES";
    std::fs::write(private.join("exec-image.log"), image).unwrap();
    std::fs::write(paths.artifacts.join("shared.png"), image).unwrap();
    session.runtime.start();
    for id in ["a", "c", "leader"] {
        model.tools(
            id,
            vec![
                call("legacy-ref", "read_artifact", json!({"path":"/artifacts/./exec-old.log"})),
                call("legacy-alias", "read_file", json!({"path":"/artifacts/innocent.txt"})),
                call("output-escape", "read_file", json!({"path":"/tool-output/../chat_tree.json"})),
                call("output-write", "write_file", json!({"path":"/tool-output/new.log","content":"tampered"})),
                call("private-image", "view_image", json!({"path":"/tool-output/exec-image.log"})),
            ],
        );
        let before = model.position();
        wake(&session, id);
        let calls = model.calls(id, before);
        for tool in ["legacy-ref", "legacy-alias", "output-escape", "output-write"] {
            assert!(result(&calls, tool)["error"].is_string(), "{id}: {tool}");
        }
        let encoded = "iVBORw0KGgpQUklWQVRFX0lNQUdFX0JZVEVT";
        let wire = serde_json::to_string(&calls).unwrap();
        assert_eq!(wire.contains(&format!("data:image/png;base64,{encoded}")), id == "a");
        if id != "a" {
            assert!(result(&calls, "private-image")["error"].is_string());
        }
        absent(&calls, &["PRIVATE_LEGACY_OUTPUT"]);
    }
    // Shared image reload still uses the shared root after the private root was
    // added; check the actual encoded request, not just view_image's receipt.
    model.tools("c", vec![call("shared-image", "view_image", json!({"path":"/artifacts/shared.png"}))]);
    let before = model.position();
    wake(&session, "c");
    assert!(serde_json::to_string(&model.calls("c", before)).unwrap().contains("data:image/png;base64,"));
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "PRIVATE_LEGACY_OUTPUT");
    assert_eq!(std::fs::read(private.join("exec-image.log")).unwrap(), image);
}

#[test]
fn private_output_references_are_refused_and_corrected_through_model_tools_in_both_modes() {
    let mut env = isolated_state_home("output-references");
    let project = env.join("project");
    let codex = env.join("codex-private");
    env.set("CODEX_HOME", &codex);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("history.jsonl"), "PRIVATE_CODEX_REFERENCE_DATA").unwrap();
    let model = LocalModel::start();
    for mode in ["approved_scope", "full_auto"] {
        let id = format!("output-refs-{mode}");
        let mut configured = spec();
        configured["channels"].as_array_mut().unwrap().push(task_channel("leader", &["a"]));
        let session = open_with_spec(&project, &id, &model, configured);
        let paths = session_paths(&id);
        let alias = paths.artifacts.join("private-alias.jsonl");
        std::os::unix::fs::symlink(codex.join("history.jsonl"), &alias).unwrap();
        assert!(submit(&session.core, "mode", "user", "set_permission_mode", json!({"mode":mode})).ok);
        let assigned = submit(
            &session.core,
            "assign",
            "leader",
            "assign_task",
            json!({"assignee":"a","description":"Write and share the final report."}),
        );
        assert!(assigned.ok, "{assigned:?}");
        let task = assigned.result["task_id"].as_str().unwrap();
        let private_refs = [
            "ctx:a:PRIVATE_CONTEXT_REFERENCE".to_string(),
            "/tool-output/exec-private-reference.log".into(),
            "/artifacts/private-alias.jsonl".into(),
            paths.base.join("members/a/chat_tree.json").to_string_lossy().into_owned(),
            env.join("config/teamagents/config.toml").to_string_lossy().into_owned(),
        ];
        let mut refused: Vec<Json> = private_refs
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                call(
                    &format!("private-ref-{index}"),
                    "publish_shared",
                    json!({"space_id":"results","ref":reference,"content":"PRIVATE_REJECTED_ENTRY"}),
                )
            })
            .collect();
        refused.push(call(
            "private-task-result",
            "complete_task",
            json!({"task_id":task,"result_refs":["/artifacts/private-alias.jsonl"],"summary":"PRIVATE_REJECTED_SUMMARY"}),
        ));
        refused.push(call(
            "malformed-task-result",
            "complete_task",
            json!({"task_id":task,"result_refs":["/artifacts/report.txt",42]}),
        ));
        model.tools("a", refused);
        // A second model step sees the refusals and supplies an ordinary
        // deliverable. The original private files remain intact.
        model.tools(
            "a",
            vec![
                call(
                    "write-deliverable",
                    "write_file",
                    json!({"path":"/artifacts/report.txt","content":"PUBLIC_REFERENCE_DELIVERABLE"}),
                ),
                call(
                    "share-deliverable",
                    "publish_shared",
                    json!({"space_id":"results","ref":"/artifacts/report.txt"}),
                ),
                call(
                    "finish-deliverable",
                    "complete_task",
                    json!({"task_id":task,"result_refs":["/artifacts/report.txt"],"summary":"The report is ready."}),
                ),
            ],
        );
        let before = model.position();
        session.runtime.start();
        assert!(session.runtime.settle(15));
        let calls = model.calls("a", before);
        for index in 0..private_refs.len() {
            assert!(result(&calls, &format!("private-ref-{index}"))["error"].is_string(), "{mode}");
        }
        for tool in ["private-task-result", "malformed-task-result"] {
            assert!(result(&calls, tool)["error"].is_string(), "{mode}: {tool}");
        }
        for tool in ["write-deliverable", "share-deliverable", "finish-deliverable"] {
            let receipt = result(&calls, tool);
            assert!(!receipt["error"].is_string(), "{mode}: {tool}: {receipt}");
        }
        let state = session.core.state_brief().unwrap();
        let finished = state["tasks"].as_array().unwrap().iter().find(|t| t["task_id"] == task).unwrap();
        assert_eq!(finished["status"], "SUCCEEDED", "{state}");
        assert_eq!(finished["result_refs"], json!(["/artifacts/report.txt"]));
        let entries = session.core.call_in_session("shared_entries", json!({"space_ids":["results"]})).unwrap();
        assert_eq!(entries["entries"].as_array().unwrap().len(), 1, "{entries}");
        assert_eq!(entries["entries"][0]["ref"], "/artifacts/report.txt");
        for member in ["b", "c", "leader"] {
            model.tools(
                member,
                vec![
                    call("shared-entries", "read_shared", json!({"space_id":"results"})),
                    call("read-deliverable", "read_artifact", json!({"path":"/artifacts/report.txt"})),
                ],
            );
            wake(&session, member);
            let received = model.calls(member, before);
            assert_eq!(result(&received, "read-deliverable")["output"], "PUBLIC_REFERENCE_DELIVERABLE");
            let mut markers: Vec<&str> = private_refs.iter().map(String::as_str).collect();
            markers.extend(["PRIVATE_REJECTED_ENTRY", "PRIVATE_REJECTED_SUMMARY", "PRIVATE_CODEX_REFERENCE_DATA"]);
            absent(&received, &markers);
        }
        assert_eq!(std::fs::read_to_string(codex.join("history.jsonl")).unwrap(), "PRIVATE_CODEX_REFERENCE_DATA");
    }
}

fn call(id: &str, tool: &str, args: Json) -> Json {
    json!({"id":id,"type":"function","function":{"name":tool,"arguments":args.to_string()}})
}

fn wake(session: &Session, member: &str) {
    if member == "leader" {
        session.runtime.user_message("Check your own context.", false).unwrap();
    } else {
        let receipt = submit(
            &session.core,
            &uuid::Uuid::new_v4().to_string(),
            "leader",
            "send_message",
            json!({"target":member,"text":"Continue your own work."}),
        );
        assert!(receipt.ok, "{receipt:?}");
    }
    assert!(session.runtime.settle(15));
    let state = session.core.state_brief().unwrap();
    assert!(state["runs"].as_array().unwrap().iter().all(|r| r["status"] == "COMPLETED"), "{state}");
}

fn tool_text<'a>(calls: &'a [Json], id: &str) -> &'a str {
    let message = calls
        .iter()
        .rev()
        .flat_map(|call| call["messages"].as_array().unwrap())
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("missing tool result {id}"));
    message["content"].as_str().unwrap()
}

fn result(calls: &[Json], id: &str) -> Json {
    serde_json::from_str(tool_text(calls, id)).unwrap()
}

fn absent(calls: &[Json], markers: &[&str]) {
    assert!(!calls.is_empty(), "privacy must be checked on an actual model request");
    let wire = serde_json::to_string(calls).unwrap();
    for marker in markers {
        assert!(!wire.contains(marker), "private marker {marker} reached another member");
    }
}

#[test]
fn t6_private_messages_history_and_new_sessions_are_isolated_on_the_wire() {
    let env = isolated_state_home("t6-wire");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session = open(&project, "t6-one", &model);
    for id in ["a", "b"] {
        std::fs::write(
            session_paths("t6-one").base.join(format!("members/{id}/work/notes.txt")),
            format!("PRIVATE_TOOL_OUTPUT_{id}"),
        )
        .unwrap();
    }
    model.tools(
        "a",
        vec![
            call("a-notes", "read_file", json!({"path":"notes.txt"})),
            call("a-message", "send_message", json!({"target":"b","text":"PRIVATE_MESSAGE_A_TO_B"})),
        ],
    );
    model.tools("b", vec![call("b-notes", "read_file", json!({"path":"notes.txt"}))]);
    session.runtime.start();
    wake(&session, "a");
    assert_eq!(result(&model.calls("a", 0), "a-notes")["output"], "PRIVATE_TOOL_OUTPUT_a");
    let b_calls = model.calls("b", 0);
    assert!(serde_json::to_string(&b_calls).unwrap().contains("PRIVATE_MESSAGE_A_TO_B"));
    assert_eq!(result(&b_calls, "b-notes")["output"], "PRIVATE_TOOL_OUTPUT_b");
    absent(&b_calls, &["PRIVATE_TOOL_OUTPUT_a", "PRIVATE_INSTRUCTIONS_a"]);

    for id in ["c", "leader"] {
        model.tools(
            id,
            vec![
                call(
                    "foreign-history",
                    "read_history",
                    json!({"tool_call_id":"a-notes","agent_id":"a","session_id":"t6-one","thread":"ctx:a:1"}),
                ),
                call(
                    "foreign-tree",
                    "read_file",
                    json!({"path":session_paths("t6-one").base.join("members/a/chat_tree.json")}),
                ),
                call("database", "read_file", json!({"path":session_paths("t6-one").db})),
                call("traversal", "read_file", json!({"path":"../../a/chat_history.json"})),
            ],
        );
        let before = model.position();
        wake(&session, id);
        let calls = model.calls(id, before);
        for tool in ["foreign-history", "foreign-tree", "database", "traversal"] {
            assert!(result(&calls, tool)["error"].is_string(), "{id}: {tool}");
        }
        absent(
            &calls,
            &["PRIVATE_MESSAGE_A_TO_B", "PRIVATE_TOOL_OUTPUT_a", "PRIVATE_TOOL_OUTPUT_b", "PRIVATE_INSTRUCTIONS_a"],
        );
    }
    let old_tree = session_paths("t6-one").base.join("members/a/chat_tree.json");
    let original = std::fs::read(&old_tree).unwrap();
    drop(session);

    let resumed = open(&project, "t6-one", &model);
    model.tools("a", vec![call("own-history", "read_history", json!({"tool_call_id":"a-notes"}))]);
    resumed.runtime.start();
    let before = model.position();
    wake(&resumed, "a");
    let calls = model.calls("a", before);
    assert!(result(&calls, "own-history").to_string().contains("PRIVATE_TOOL_OUTPUT_a"));
    assert!(!std::fs::read(&old_tree).unwrap().is_empty());
    drop(resumed);
    let retained = std::fs::read(&old_tree).unwrap();
    assert_ne!(original, retained, "the resumed turn adds its own history");

    let fresh = open(&project, "t6-two", &model);
    fresh.runtime.start();
    for id in MEMBERS {
        model.tools(
            id,
            vec![call(
                "old-session-history",
                "read_history",
                json!({"tool_call_id":"a-notes","agent_id":"a","session_id":"t6-one","thread":"ctx:a:1"}),
            )],
        );
        let before = model.position();
        wake(&fresh, id);
        let calls = model.calls(id, before);
        assert!(result(&calls, "old-session-history")["error"].is_string());
        absent(&calls, &["PRIVATE_MESSAGE_A_TO_B", "PRIVATE_TOOL_OUTPUT_a", "PRIVATE_TOOL_OUTPUT_b"]);
    }
    assert_eq!(std::fs::read(&old_tree).unwrap(), retained);
}

#[test]
fn t6_automatic_shell_output_is_private_but_explicit_artifacts_remain_shared() {
    let mut env = isolated_state_home("t6-output");
    env.set("T6_PRIVATE_ENV_KEY", "PRIVATE_MODEL_CREDENTIAL");
    if !teamagents_engine::tools::bwrap_available() {
        eprintln!("skip: bwrap is not available; automatic output isolation was not executed");
        return;
    }
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let model = LocalModel::start();
    let session = open(&project, "t6-output-one", &model);
    model.tools(
        "a",
        vec![call(
            "a-shell",
            "shell",
            json!({"command":"printf 'PRIVATE_SHELL_OUTPUT_A\\n'; seq 1 40000","timeout":10}),
        )],
    );
    session.runtime.start();
    wake(&session, "a");
    let calls = model.calls("a", 0);
    // Wire previews of a long result are bounded text, not a complete JSON
    // object; the full tool receipt remains in this member's private history.
    let output = tool_text(&calls, "a-shell");
    assert!(output.contains("PRIVATE_SHELL_OUTPUT_A"));
    let reference = output
        .split("full output: ")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .unwrap_or_else(|| panic!("missing long-output reference: {output}"));
    model.tools("a", vec![call("own-output", "read_artifact", json!({"path":reference,"limit":1}))]);
    wake(&session, "a");
    assert!(result(&model.calls("a", 0), "own-output").to_string().contains("PRIVATE_SHELL_OUTPUT_A"));

    // Use the exact known reference: an unguessable filename is not an ACL.
    let paths = session_paths("t6-output-one");
    let hidden = [
        paths.db,
        paths.base.join("members/a/chat_tree.json"),
        paths.base.join("members/a/shell/state.sh"),
        paths.base.join("members/a/tool-output").join(reference.rsplit('/').next().unwrap()),
    ];
    assert!(hidden.iter().all(|path| path.is_file()), "probe actual files, not nonexistent fixture paths");
    let quoted =
        hidden.iter().map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))).collect::<Vec<_>>();
    let shell_probe = format!(
        "for path in {}; do if test -e \"$path\"; then printf 'private path visible\\n'; exit 1; fi; done; \
         test -z \"${{T6_PRIVATE_ENV_KEY-}}\" || exit 1; printf 'private paths and environment hidden\\n'",
        quoted.join(" ")
    );
    for id in ["b", "c", "leader"] {
        model.tools(
            id,
            vec![
                call("foreign-output", "read_artifact", json!({"path":reference,"limit":1})),
                call("foreign-output-file", "read_file", json!({"path":reference,"limit":1})),
                call("foreign-output-shell", "shell", json!({"command":shell_probe,"timeout":10})),
            ],
        );
        let before = model.position();
        wake(&session, id);
        let calls = model.calls(id, before);
        assert!(result(&calls, "foreign-output")["error"].is_string(), "{id} can read another member's shell log");
        assert!(result(&calls, "foreign-output-file")["error"].is_string());
        let shell = result(&calls, "foreign-output-shell");
        assert!(!shell["error"].is_string(), "{shell}");
        assert!(shell["output"].as_str().unwrap().contains("private paths and environment hidden"));
        absent(&calls, &["PRIVATE_SHELL_OUTPUT_A", "PRIVATE_MODEL_CREDENTIAL"]);
    }
    model.tools(
        "a",
        vec![
            call("write-result", "write_file", json!({"path":"/artifacts/result.txt","content":"PUBLIC_DELIVERABLE"})),
            call(
                "publish-result",
                "publish_shared",
                json!({"space_id":"results","ref":"/artifacts/result.txt","content":"The result is ready."}),
            ),
        ],
    );
    wake(&session, "a");
    model.tools("c", vec![call("shared-result", "read_artifact", json!({"path":"/artifacts/result.txt"}))]);
    wake(&session, "c");
    assert_eq!(result(&model.calls("c", 0), "shared-result")["output"], "PUBLIC_DELIVERABLE");
    drop(session);

    let resumed = open(&project, "t6-output-one", &model);
    model.tools("a", vec![call("resumed-output", "read_artifact", json!({"path":reference,"limit":1}))]);
    resumed.runtime.start();
    wake(&resumed, "a");
    assert!(result(&model.calls("a", 0), "resumed-output").to_string().contains("PRIVATE_SHELL_OUTPUT_A"));
    drop(resumed);

    let fresh = open(&project, "t6-output-two", &model);
    model.tools("a", vec![call("old-output", "read_artifact", json!({"path":reference,"limit":1}))]);
    fresh.runtime.start();
    let before = model.position();
    wake(&fresh, "a");
    let calls = model.calls("a", before);
    assert!(result(&calls, "old-output")["error"].is_string());
    absent(&calls, &["PRIVATE_SHELL_OUTPUT_A", "PUBLIC_DELIVERABLE"]);
}
