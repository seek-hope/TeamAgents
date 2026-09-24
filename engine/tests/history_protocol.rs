//! 退役（R29，2026-09-24）：本文件覆盖 v1 后端（chat/codex/session/worker/审查树）。
//! v1 入口已不可达；等价覆盖在 v2：`v2_driver`/`v2_supervisor`/`v2_daemon`/`v2_mcp`/`v2_spawn_failure`、
//! `review/eval/r2-p6`（性能）与 `review/eval/r2-p5` 的真实供应商验收。文件待随模块删除。
#![cfg(any())]
//! Production worker history reads: paged, stale-safe and usable after removal.
//! Native history uses a local protocol fixture; no model service is contacted.

mod support;

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use support::isolated_state_home;
use teamagents_engine::sessions::session_paths;

struct Worker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Json>,
    pending: HashMap<u64, Json>,
    next: u64,
}

impl Worker {
    fn spawn(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("serve")
            .env("XDG_STATE_HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(reply) = serde_json::from_str::<Json>(&line) {
                    if reply["id"].is_u64() {
                        let _ = tx.send(reply);
                    }
                }
            }
        });
        Self { child, stdin, replies, pending: HashMap::new(), next: 0 }
    }

    fn send(&mut self, method: &str, params: Json) -> u64 {
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"id":self.next,"method":method,"params":params})).unwrap();
        self.stdin.flush().unwrap();
        self.next
    }

    fn receive(&mut self, id: u64) -> Result<Json, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let reply = loop {
            if let Some(reply) = self.pending.remove(&id) {
                break reply;
            }
            let reply =
                self.replies.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("worker response");
            self.pending.insert(reply["id"].as_u64().unwrap(), reply);
        };
        match reply["error"].as_str() {
            Some(error) => Err(error.into()),
            None => Ok(reply["result"].clone()),
        }
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.send(method, params);
        self.receive(id)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"id":0,"method":"close"}));
        let _ = self.stdin.flush();
        let end = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture(tag: &str) -> (support::TestEnv, PathBuf, String) {
    let env = isolated_state_home(tag);
    std::fs::create_dir_all(env.join("config/teamagents")).unwrap();
    std::fs::write(
        env.join("config/teamagents/config.toml"),
        "[models.leader_main]\nprovider='local'\nmodel='test'\nbase_url='http://127.0.0.1:1/v1'\nmax_retries=0\n",
    )
    .unwrap();
    let project = env.join("project");
    std::fs::create_dir(&project).unwrap();
    let mut worker = Worker::spawn(&env);
    let opened = worker
        .call(
            "open",
            json!({"cwd":project,"initial_spec":{"leader_id":"leader","agents":[
                {"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"leader_main"},
                {"id":"dev","name":"Dev","role":"worker","runtime_kind":"deepagents","model_profile":"leader_main"}
            ],"channels":[{"source":"leader","targets":["dev"],"mode":"message"}]}}),
        )
        .unwrap();
    let session_id = opened["session_id"].as_str().unwrap().to_string();
    // Keep the worker alive only long enough to establish the production
    // session; history reads below use the same worker protocol.
    drop(worker);
    (env, project, session_id)
}

fn write_records(session_id: &str) {
    let member = session_paths(session_id).base.join("members/dev");
    std::fs::create_dir_all(member.join("turns")).unwrap();
    let tree = json!({"ctx:dev:1":{"nodes":[
        {"id":"n1","parent":null,"message":{"role":"user","content":"private request"}},
        {"id":"n2","parent":"n1","message":{"role":"assistant","tool_calls":[{"id":"call-private","function":{"name":"read_file","arguments": "{}"}}]}},
        {"id":"n3","parent":"n2","message":{"role":"tool","tool_call_id":"call-private","content":"PRIVATE_TOOL_RESULT"}},
        {"id":"n4","parent":"n1","message":{"role":"assistant","content":"abandoned branch"}}
    ],"leaf":"n3","rewind_epoch":1}});
    let history = json!({"ctx:dev:1":[
        {"role":"user","content":"private request"},
        {"role":"assistant","content":"reply"}
    ]});
    let checkpoint_history = json!([
        {"role":"user","content":"private request"},
        {"role":"assistant","tool_calls":[{"id":"call-private","function":{"name":"read_file","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call-private","content":"PRIVATE_TOOL_RESULT"}
    ]);
    let checkpoint = json!({"history":checkpoint_history,"model_steps":1,"pending_external":null,
        "outcome":null,"input_events":[],"delivery_ids":[],"tree_base":0,"tree_leaf":"n3",
        "rewind_epoch":1,"tree_pending":[]});
    std::fs::write(member.join("chat_tree.json"), serde_json::to_vec(&tree).unwrap()).unwrap();
    std::fs::write(member.join("chat_history.json"), serde_json::to_vec(&history).unwrap()).unwrap();
    std::fs::write(member.join("turns/run-private.json"), serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    let db = session_paths(session_id).db;
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO turn_runs(run_id,session_id,task_id,goal_id,agent_id,config_revision,topology_revision,status,input_delivery_ids,context_ref,external_turn_id,cancel_requested,waiting_on,created_at,updated_at)
         VALUES(?1,?2,NULL,NULL,'dev',1,1,'COMPLETED','[]','ctx:dev:1',NULL,0,'[]',?3,?3)",
        rusqlite::params!["run-private", session_id, 1.0_f64],
    )
    .unwrap();
}

fn write_events(session_id: &str) -> (i64, i64) {
    let db = session_paths(session_id).db;
    let conn = rusqlite::Connection::open(db).unwrap();
    let unrelated = conn
        .execute(
            "INSERT INTO events(event_id,session_id,actor_id,task_id,kind,payload_json,audience_json,topology_revision,causation_id,created_at)
             VALUES(?1,?2,'leader',NULL,'message',?3,'[\"leader\"]',1,NULL,1.0)",
            rusqlite::params!["history-unrelated", session_id, json!({"text":"dev"}).to_string()],
        )
        .unwrap();
    assert_eq!(unrelated, 1);
    let unrelated_sequence = conn.last_insert_rowid();
    let mut related = vec![];
    for index in 0..45 {
        conn.execute(
            "INSERT INTO events(event_id,session_id,actor_id,task_id,kind,payload_json,audience_json,topology_revision,causation_id,created_at)
             VALUES(?1,?2,'leader',NULL,'message',?3,'[\"leader\"]',1,NULL,?4)",
            rusqlite::params![
                format!("history-related-{index}"),
                session_id,
                json!({"target":"dev","text":format!("event-{index}")}).to_string(),
                index as f64 + 2.0,
            ],
        )
        .unwrap();
        related.push(conn.last_insert_rowid());
        if index % 4 == 0 {
            conn.execute(
                "INSERT INTO events(event_id,session_id,actor_id,task_id,kind,payload_json,audience_json,topology_revision,causation_id,created_at)
                 VALUES(?1,?2,'leader',NULL,'message',?3,'[\"leader\"]',1,NULL,?4)",
                rusqlite::params![
                    format!("history-noise-{index}"),
                    session_id,
                    json!({"text":format!("ordinary-{index}")}).to_string(),
                    index as f64 + 100.0,
                ],
            )
            .unwrap();
        }
    }
    (unrelated_sequence, related[0])
}

#[test]
fn history_is_user_only_paged_and_keeps_removed_member_records() {
    let (env, _project, session_id) = fixture("history-protocol");
    write_records(&session_id);
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":_project,"resume":session_id})).unwrap();

    let before = worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
    let members = worker.call("history", json!({})).unwrap();
    assert!(members["entries"].as_array().unwrap().iter().any(|e| e["id"] == "dev"));
    let sources = worker.call("history", json!({"agent_id":"dev"})).unwrap();
    assert_eq!(sources["entries"].as_array().unwrap().len(), 4);

    let page = worker.call("history", json!({"agent_id":"dev","source":"tree"})).unwrap();
    let revision = page["revision"].as_str().unwrap().to_string();
    assert!(page.to_string().contains("PRIVATE_TOOL_RESULT"), "list preview must expose recorded tool output");
    let detail =
        worker.call("history", json!({"agent_id":"dev","source":"tree","item":"2","revision":revision})).unwrap();
    assert!(detail["text"].to_string().contains("PRIVATE_TOOL_RESULT"));
    assert!(detail["text"].to_string().contains("call-private"));

    let runs = worker.call("history", json!({"agent_id":"dev","source":"turns"})).unwrap();
    assert!(runs["entries"].as_array().unwrap().iter().any(|e| e["id"] == "run-private"));
    let run = worker.call("history", json!({"agent_id":"dev","source":"turns","item":"run-private"})).unwrap();
    assert!(run["text"].to_string().contains("PRIVATE_TOOL_RESULT"));

    let after = worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
    assert_eq!(before["runs"], after["runs"], "history must not start or mutate a member run");
    assert_eq!(before["events"], after["events"], "history must not inject events");

    let revision = page["revision"].as_str().unwrap().to_string();
    std::fs::write(session_paths(&session_id).base.join("members/dev/chat_tree.json"), b"{}\n").unwrap();
    let stale = worker.call("history", json!({"agent_id":"dev","source":"tree","offset":40,"revision":revision}));
    assert!(stale.is_err(), "changed files must reject a stale page");

    let state = worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
    let base_revision = state["revision"].as_i64().unwrap();
    let removed = worker
        .call(
            "submit",
            json!({"action":{"action_id":"history-remove-dev","actor_id":"leader","kind":"apply_topology_patch","payload":{
                "base_revision":base_revision,"operations":[{"op":"remove_agent","agent_id":"dev"}]}}}),
        )
        .unwrap();
    assert_eq!(removed["ok"], true, "remove member: {removed}");
    let retained = worker.call("history", json!({})).unwrap();
    assert!(retained["entries"].as_array().unwrap().iter().any(|e| e["id"] == "dev"));
    drop(worker);
}

#[test]
fn history_rejects_symlinked_records_and_never_reads_outside_session() {
    let (env, project, session_id) = fixture("history-symlink");
    let outside = env.join("outside.json");
    std::fs::write(&outside, "PRIVATE_OUTSIDE").unwrap();
    let member = session_paths(&session_id).base.join("members/dev");
    std::os::unix::fs::symlink(&outside, member.join("chat_tree.json")).unwrap();
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    let result = worker.call("history", json!({"agent_id":"dev","source":"tree"}));
    assert!(result.is_err());
    assert!(!result.unwrap_err().contains("PRIVATE_OUTSIDE"));
    drop(worker);
}

#[test]
fn history_event_pages_stop_at_related_rows_and_use_schema_fields() {
    let (env, project, session_id) = fixture("history-events");
    let (unrelated_sequence, first_related) = write_events(&session_id);
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();

    let first = worker.call("history", json!({"agent_id":"dev","source":"events"})).unwrap();
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 40, "a page must contain exactly 40 related events");
    assert!(first["next_offset"].is_number());
    assert!(!entries.iter().any(|entry| entry["id"].as_i64() == Some(unrelated_sequence)));
    assert_eq!(entries[0]["id"], first_related.to_string());

    let next_offset = first["next_offset"].as_u64().unwrap() as usize;
    let second = worker
        .call("history", json!({"agent_id":"dev","source":"events","offset":next_offset,"through":first["through"]}))
        .unwrap();
    assert_eq!(second["entries"].as_array().unwrap().len(), 5);
    assert!(second["next_offset"].is_null());

    let false_positive =
        worker.call("history", json!({"agent_id":"dev","source":"events","item":unrelated_sequence.to_string()}));
    assert!(false_positive.is_err(), "ordinary text must not identify an unrelated member event");
    drop(worker);
}

fn native_fixture(env: &mut support::TestEnv, session_id: &str, mode: &str) -> Json {
    let bin = env.join("bin");
    std::fs::create_dir(&bin).unwrap();
    // Stable shell executable avoids the freshly-written executable race.
    std::os::unix::fs::symlink("/bin/sh", bin.join("codex")).unwrap();
    let script = env.join("native-history.py");
    std::fs::write(
        &script,
        r#"import json, os, sys, time
for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["TA_NATIVE_REQUESTS"], "a") as log:
        log.write(json.dumps(request) + "\n")
    method, params = request.get("method"), request.get("params", {})
    data = json.load(open(os.environ["TA_NATIVE_DATA"]))
    mode = data["mode"]
    if method == "initialize":
        result = {}
    elif method == "thread/read":
        if mode == "stall":
            with open(os.environ["TA_NATIVE_PID"], "w") as pid:
                pid.write(str(os.getpid()))
            while True:
                time.sleep(0.02)
        result = {"thread":{"id":params["threadId"],"turns":[]}}
        if mode == "wrong-thread":
            result["thread"]["id"] = "PRIVATE_FOREIGN_THREAD"
        if mode in ("rollout", "denied"):
            result["thread"]["path"] = data.get("rollout_path", os.environ["TA_NATIVE_ROLLOUT"])
        if params.get("includeTurns"):
            if mode == "rollout":
                print(json.dumps({"id":request["id"],"error":{"code":-32601,"message":"list_turns is not supported yet"}}), flush=True)
                continue
            result["thread"]["turns"] = [{"id":"turn-native","items":[e["item"] for e in data["entries"]]}]
    elif method == "thread/items/list":
        if mode == "denied":
            print(json.dumps({"id":request["id"],"error":{"code":-32000,"message":"history access denied"}}), flush=True)
            continue
        if mode in ("legacy", "rollout"):
            print(json.dumps({"id":request["id"],"error":{"code":-32601,"message":"legacy rollout storage"}}), flush=True)
            continue
        start = int((params.get("cursor") or "native-0").split("-")[1])
        end = min(start + params["limit"], len(data["entries"]))
        result = {"data":data["entries"][start:end],"nextCursor":"native-"+str(end) if end<len(data["entries"]) else None}
        if mode == "invalid":
            result["data"] = None
        if mode == "oversize":
            result["data"][0]["item"]["text"] = "x" * (33 * 1024 * 1024)
    else:
        raise RuntimeError("history invoked a mutating method: " + str(method))
    print(json.dumps({"id":request["id"],"result":result}), flush=True)
"#,
    )
    .unwrap();
    std::fs::write(
        session_paths(session_id).base.join("app-server"),
        format!("exec python3 -u '{}'\n", script.display()),
    )
    .unwrap();
    env.set("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    env.set("TA_NATIVE_REQUESTS", env.join("native-requests.jsonl"));
    env.set("TA_NATIVE_DATA", env.join("native-data.json"));
    env.set("TA_NATIVE_PID", env.join("native.pid"));
    std::fs::create_dir_all(env.join("codex/sessions/2026/09/20")).unwrap();
    env.set("CODEX_HOME", env.join("codex"));
    env.set("TA_NATIVE_ROLLOUT", native_rollout(env));
    let mut entries: Vec<Json> = (0..45)
        .map(|index| {
            json!({"turnId":"turn-native","item":{
                "type":"agentMessage","id":format!("item-{index}"),"text":format!("native message {index}")
            }})
        })
        .collect();
    entries[1]["item"] = json!({"type":"commandExecution","id":"item-1","command":"printf fixture",
        "aggregatedOutput":format!("{}PRIVATE_CODEX_TOOL_RESULT", "中".repeat(16_000)),
        "exitCode":0,"status":"completed"});
    entries[2]["item"] = json!({"type":"fileChange","id":"item-2","status":"completed",
        "changes":[{"path":"src/lib.rs","kind":{"type":"update"},"diff":"PRIVATE_NATIVE_DIFF"}]});
    entries[3]["item"] = json!({"type":"mcpToolCall","id":"item-3","server":"fixture","tool":"lookup",
        "arguments":{"query":"fixture"},"result":{"content":[{"type":"text","text":"PRIVATE_MCP_RESULT"}]}});
    let data = json!({"mode":mode,"entries":entries});
    std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
    if mode == "rollout" {
        write_native_rollout(env, &data);
    }
    let conn = rusqlite::Connection::open(session_paths(session_id).db).unwrap();
    conn.execute("UPDATE agent_runtime SET external_thread_id='native-thread' WHERE agent_id='dev'", []).unwrap();
    data
}

fn write_native_rollout(env: &support::TestEnv, data: &Json) {
    let mut records = vec![
        json!({"type":"session_meta","payload":{"id":"native-thread"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-native"}}),
    ];
    records.extend(
        data["entries"].as_array().unwrap().iter().map(|entry| json!({"type":"response_item","payload":entry["item"]})),
    );
    let text = records.into_iter().map(|record| record.to_string()).collect::<Vec<_>>().join("\n");
    std::fs::write(native_rollout(env), format!("{text}\n")).unwrap();
}

fn native_rollout(env: &Path) -> PathBuf {
    env.join("codex/sessions/2026/09/20/native-rollout.jsonl")
}

fn native_requests(env: &Path) -> Vec<Json> {
    std::fs::read_to_string(env.join("native-requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn native_codex_history_reads_all_pages_and_tool_details_without_team_mutation() {
    for mode in ["items", "legacy", "rollout"] {
        let (mut env, project, session_id) = fixture("native-history");
        let mut data = native_fixture(&mut env, &session_id, mode);
        let mut worker = Worker::spawn(&env);
        worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
        let before = worker.call("call", json!({"method":"state","params":{}})).unwrap();
        let sources = worker.call("history", json!({"agent_id":"dev"})).unwrap();
        assert_eq!(sources["entries"][0]["id"], "codex");
        let first = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 40);
        let next = worker
            .call(
                "history",
                json!({"agent_id":"dev","source":"codex","offset":first["next_offset"],"cursor":first["next_cursor"]}),
            )
            .unwrap();
        assert_eq!(next["entries"].as_array().unwrap().len(), 5);
        assert!(next["next_offset"].is_null());
        let mut params = json!({"agent_id":"dev","source":"codex","revision":first["revision"],
            "item":first["entries"][1]["id"],"offset":0});
        let detail = worker.call("history", params.clone()).unwrap();
        params["offset"] = detail["next_offset"].clone();
        let tail = worker.call("history", params).unwrap();
        let full = format!("{}{}", detail["text"].as_str().unwrap(), tail["text"].as_str().unwrap());
        let restored: Json = serde_json::from_str(&full).unwrap();
        assert_eq!(restored["entry"]["item"], data["entries"][1]["item"]);
        assert_eq!(restored["entry"]["turnId"], data["entries"][1]["turnId"]);
        for (index, expected) in [(2, "PRIVATE_NATIVE_DIFF"), (3, "PRIVATE_MCP_RESULT")] {
            let detail = worker
                .call(
                    "history",
                    json!({"agent_id":"dev","source":"codex",
                "revision":first["revision"],"item":first["entries"][index]["id"]}),
                )
                .unwrap();
            assert!(detail["text"].as_str().unwrap().contains(expected));
        }
        let after = worker.call("call", json!({"method":"state","params":{}})).unwrap();
        assert_eq!(before, after, "human history reads must not alter team state");
        data["entries"][1]["item"]["aggregatedOutput"] = json!("changed after selection");
        std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
        if mode == "rollout" {
            write_native_rollout(&env, &data);
        }
        let stale = worker
            .call(
                "history",
                json!({"agent_id":"dev","source":"codex",
            "revision":first["revision"],"item":first["entries"][1]["id"]}),
            )
            .unwrap_err();
        assert!(stale.contains("变化"), "{stale}");

        let removed = worker
            .call(
                "submit",
                json!({"action":{"action_id":"native-remove-dev","actor_id":"leader",
            "kind":"apply_topology_patch","payload":{"base_revision":after["revision"],
            "operations":[{"op":"remove_agent","agent_id":"dev"}]}}}),
            )
            .unwrap();
        assert_eq!(removed["ok"], true);
        drop(worker);
        let mut reopened = Worker::spawn(&env);
        reopened.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
        assert_eq!(
            reopened.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            40
        );
        drop(reopened);
        let requests = native_requests(&env);
        assert!(requests.iter().all(|request| matches!(
            request["method"].as_str(),
            Some("initialize" | "thread/read" | "thread/items/list")
        )));
        assert!(requests
            .iter()
            .filter(|request| request["method"] != "initialize")
            .all(|request| request["params"]["threadId"] == "native-thread"));
    }
}

#[test]
fn native_codex_history_rejects_cross_member_cursors_and_invalid_backend_records() {
    let (mut env, project, session_id) = fixture("native-history-integrity");
    let mut data = native_fixture(&mut env, &session_id, "items");
    let conn = rusqlite::Connection::open(session_paths(&session_id).db).unwrap();
    conn.execute("UPDATE agent_runtime SET external_thread_id='another-thread' WHERE agent_id='leader'", []).unwrap();
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    let first = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap();
    let count = native_requests(&env).len();
    let bad = worker
        .call("history", json!({"agent_id":"leader","source":"codex","cursor":first["next_cursor"],"offset":40}))
        .unwrap_err();
    assert!(bad.contains("不属于"), "{bad}");
    assert!(worker.call("history", json!({"agent_id":"absent","source":"codex"})).is_err());
    assert_eq!(native_requests(&env).len(), count);
    for mode in ["wrong-thread", "invalid", "oversize"] {
        data["mode"] = json!(mode);
        std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
        let error = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap_err();
        assert!(!error.contains("PRIVATE_FOREIGN_THREAD"));
        let expected = match mode {
            "wrong-thread" => "身份不符",
            "invalid" => "数组",
            _ => "上限",
        };
        assert!(error.contains(expected), "{mode}: {error}");
    }
    let requests = native_requests(&env);
    assert!(
        !requests.iter().any(|request| request["params"]["includeTurns"] == true),
        "corruption and transport errors must not silently downgrade the read"
    );
    drop(worker);
}

#[test]
fn native_rollout_rejects_paths_outside_codex_sessions_and_symlink_components() {
    let (mut env, project, session_id) = fixture("native-history-paths");
    let mut data = native_fixture(&mut env, &session_id, "rollout");
    let original = native_rollout(&env);
    let outside = env.join("foreign-records.jsonl");
    std::fs::copy(&original, &outside).unwrap();
    let sibling = env.join("codex/sessions-other");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::copy(&original, sibling.join("record.jsonl")).unwrap();
    let sessions = env.join("codex/sessions");
    std::os::unix::fs::symlink(&outside, sessions.join("linked.jsonl")).unwrap();
    std::os::unix::fs::symlink(original.parent().unwrap(), sessions.join("linked-directory")).unwrap();
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    for path in [
        outside,
        sibling.join("record.jsonl"),
        env.join("codex/sessions/../../foreign-records.jsonl"),
        sessions.join("linked.jsonl"),
        sessions.join("linked-directory/native-rollout.jsonl"),
        PathBuf::from("native-rollout.jsonl"),
    ] {
        data["rollout_path"] = json!(path);
        std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
        let result = worker.call("history", json!({"agent_id":"dev","source":"codex"}));
        assert!(result.is_err(), "accepted a forbidden rollout path: {}", path.display());
        assert!(!result.unwrap_err().contains("PRIVATE_CODEX_TOOL_RESULT"));
    }
    data["rollout_path"] = json!(original);
    std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
    assert_eq!(
        worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        40,
        "path errors must not poison a later legitimate read"
    );
    drop(worker);
}

#[test]
fn native_rollout_requires_one_matching_metadata_record_and_bounded_regular_files() {
    let (mut env, project, session_id) = fixture("native-history-rollout-integrity");
    let data = native_fixture(&mut env, &session_id, "rollout");
    let path = native_rollout(&env);
    let good = std::fs::read_to_string(&path).unwrap();
    let (_, items) = good.split_once('\n').unwrap();
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    for (name, text) in [
        ("missing metadata", items.to_string()),
        ("missing identity", format!("{{\"type\":\"session_meta\",\"payload\":{{}}}}\n{items}")),
        (
            "conflicting identity",
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"native-thread\",\"session_id\":\"other\"}}}}\n{items}"),
        ),
        (
            "paginated store is not a complete legacy rollout",
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"native-thread\",\"history_mode\":\"paginated\"}}}}\n{items}"),
        ),
        ("duplicate metadata", format!("{good}{good}")),
        ("truncated JSON", format!("{good}{{\"type\":")),
    ] {
        std::fs::write(&path, text).unwrap();
        let result = worker.call("history", json!({"agent_id":"dev","source":"codex"}));
        assert!(result.is_err(), "accepted invalid rollout: {name}");
        assert!(!result.unwrap_err().contains("PRIVATE_CODEX_TOOL_RESULT"));
    }
    let oversized = std::fs::File::create(&path).unwrap();
    oversized.set_len(33 * 1024 * 1024).unwrap();
    let error = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap_err();
    assert!(error.contains("上限"), "{error}");
    std::fs::remove_file(&path).unwrap();
    assert!(Command::new("mkfifo").arg(&path).status().unwrap().success());
    let started = Instant::now();
    assert!(worker.call("history", json!({"agent_id":"dev","source":"codex"})).is_err());
    assert!(started.elapsed() < Duration::from_secs(2), "opening a FIFO blocked the history worker");
    std::fs::remove_file(&path).unwrap();
    write_native_rollout(&env, &data);
    assert!(worker.call("history", json!({"agent_id":"dev","source":"codex"})).is_ok());
    drop(worker);
}

#[test]
fn native_rollout_browses_response_items_without_ids_and_preserves_record_identity() {
    let (mut env, project, session_id) = fixture("native-history-response-items");
    let mut data = native_fixture(&mut env, &session_id, "rollout");
    // These ResponseItem shapes follow the installed CLI's generated schema;
    // unlike ThreadItem, their id is optional and must not be fabricated.
    let records = [
        json!({"type":"session_meta","payload":{"session_id":"native-thread"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"developer",
            "content":[{"type":"input_text","text":"saved instructions"}]}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-raw"}}),
        json!({"type":"response_item","payload":{"type":"message","id":null,"role":"user",
            "content":[{"type":"input_text","text":"read the fixture"}]}}),
        json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-raw",
            "name":"functions.exec_command","arguments":"{\"cmd\":\"cat fixture\"}"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-raw",
            "output":"PRIVATE_RAW_TOOL_OUTPUT"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"patch-raw",
            "name":"apply_patch","input":"PRIVATE_RAW_PATCH"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"patch-raw",
            "output":"Success"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"done"}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn-raw"}}}),
    ];
    let text = records.iter().map(Json::to_string).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(native_rollout(&env), &text).unwrap();
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    let first = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap();
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 7);
    assert!(first.to_string().contains("functions.exec_command"));
    assert!(first.to_string().contains("PRIVATE_RAW_TOOL_OUTPUT"));
    let ids: std::collections::HashSet<_> = entries.iter().map(|entry| entry["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), entries.len(), "missing native IDs must not collapse distinct records");
    for (entry, (index, record)) in
        entries.iter().zip(records.iter().enumerate().filter(|(_, r)| r["type"] == "response_item"))
    {
        let detail = worker
            .call("history", json!({"agent_id":"dev","source":"codex","revision":first["revision"],"item":entry["id"]}))
            .unwrap();
        let detail: Json = serde_json::from_str(detail["text"].as_str().unwrap()).unwrap();
        assert_eq!(detail["entry"]["item"], record["payload"], "raw items must be preserved verbatim");
        assert_eq!(detail["entry"]["record_line"], index + 1);
        assert_eq!(detail["entry"]["turnId"], if index == 1 { Json::Null } else { json!("turn-raw") });
    }
    let archived = env.join("codex/archived_sessions");
    std::fs::create_dir(&archived).unwrap();
    std::fs::rename(native_rollout(&env), archived.join("native.jsonl")).unwrap();
    data["rollout_path"] = json!(archived.join("native.jsonl"));
    std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
    let archive = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap();
    assert_eq!(first["entries"], archive["entries"]);
    assert_eq!(first["revision"], archive["revision"]);
    data["mode"] = json!("denied");
    std::fs::write(env.join("native-data.json"), data.to_string()).unwrap();
    let error = worker.call("history", json!({"agent_id":"dev","source":"codex"})).unwrap_err();
    assert!(error.contains("denied"), "RPC refusal must not be bypassed using the rollout: {error}");
    drop(worker);
}

#[test]
fn stalled_native_history_keeps_worker_responsive_and_close_kills_the_reader() {
    let (mut env, project, session_id) = fixture("native-history-close");
    native_fixture(&mut env, &session_id, "stall");
    let mut worker = Worker::spawn(&env);
    worker.call("open", json!({"cwd":project,"resume":session_id})).unwrap();
    let history = worker.send("history", json!({"agent_id":"dev","source":"codex"}));
    assert!(support::wait_for(|| env.join("native.pid").exists(), 3000));
    let pid: i32 = std::fs::read_to_string(env.join("native.pid")).unwrap().parse().unwrap();
    let before = Instant::now();
    worker.call("call", json!({"method":"state","params":{"include_events":false}})).unwrap();
    assert!(before.elapsed() < Duration::from_secs(1), "a slow history read blocked state polling");
    worker.call("close", json!({})).unwrap();
    assert!(worker.receive(history).unwrap_err().contains("取消"));
    assert!(support::wait_for(|| !Path::new(&format!("/proc/{pid}")).exists(), 2000));
    drop(worker);
}
