//! Member removal must retire backend resources, not only the TeamSpec row.

mod support;

use serde_json::{json, Value as Json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use support::*;
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::gateway::ToolGateway;
use teamagents_engine::mcp::McpClient;
use teamagents_engine::runtime::AgentRunner;
use teamagents_engine::session::{open_session, OpenOptions};
use teamagents_engine::sessions::session_paths;

fn alive(pid: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next()) != Some("Z")
}

struct McpProcessProbe {
    script: std::path::PathBuf,
    pids: std::path::PathBuf,
    waiting: std::path::PathBuf,
}

impl McpProcessProbe {
    fn new(root: &std::path::Path) -> Self {
        let script = root.join("mcp.py");
        std::fs::write(
            &script,
            r#"
import json, os, pathlib, subprocess, sys, time
root = pathlib.Path(sys.argv[1])
child_file = root / 'child.pid'
child = subprocess.Popen([sys.executable, '-c', '''
import os, pathlib, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path(sys.argv[1]).write_text(str(os.getpid()))
time.sleep(30)
''', str(child_file)])
deadline = time.monotonic() + 3
while not child_file.exists():
    if time.monotonic() > deadline: raise RuntimeError('child startup timeout')
    time.sleep(.01)
tmp = root / 'pids.tmp'
tmp.write_text(json.dumps([os.getpid(), child.pid]))
tmp.rename(root / 'pids.json')
for line in sys.stdin:
    message = json.loads(line)
    if 'id' not in message: continue
    method = message.get('method')
    if method == 'initialize':
        if len(sys.argv) > 2 and sys.argv[2] == 'hang': continue
        result = {'protocolVersion':'2025-06-18', 'capabilities':{'tools':{}}}
    elif method == 'tools/list':
        result = {'tools':[{'name':'echo','description':'echo','inputSchema':{'type':'object'}}]}
    elif method == 'tools/call':
        (root / 'waiting').write_text('waiting')
        continue
    else: result = {}
    print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result}), flush=True)
"#,
        )
        .unwrap();
        Self { script, pids: root.join("pids.json"), waiting: root.join("waiting") }
    }

    fn args(&self) -> Vec<String> {
        vec![self.script.to_string_lossy().into_owned(), self.script.parent().unwrap().to_string_lossy().into_owned()]
    }

    fn pids(&self) -> Vec<u64> {
        serde_json::from_slice(&std::fs::read(&self.pids).expect("server must have started")).unwrap()
    }

    fn stopped(&self) -> bool {
        self.pids().into_iter().all(|pid| !alive(pid))
    }
}

impl Drop for McpProcessProbe {
    fn drop(&mut self) {
        // A failing regression must not leave its intentionally stubborn child.
        if let Ok(bytes) = std::fs::read(&self.pids) {
            if let Ok(pids) = serde_json::from_slice::<Vec<u64>>(&bytes) {
                for pid in pids.into_iter().filter(|pid| alive(*pid)) {
                    let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
                }
            }
        }
    }
}

#[derive(Default)]
struct TrackedRunner {
    entered: AtomicBool,
    finish: AtomicBool,
    closing: AtomicBool,
    release_close: AtomicBool,
    close_count: AtomicUsize,
}

impl AgentRunner for TrackedRunner {
    fn start_or_resume(&self, _: &TurnRun, _: &Json, _: &ToolGateway, _: &Json) -> TurnOutcome {
        self.entered.store(true, Ordering::SeqCst);
        assert!(wait_for(|| self.finish.load(Ordering::SeqCst), 10_000));
        TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }
    }

    fn request_interrupt(&self, _: &str) -> TurnStatus {
        self.finish.store(true, Ordering::SeqCst);
        TurnStatus::Cancelled
    }

    fn query_state(&self, _: &str) -> Option<TurnStatus> {
        None
    }

    fn deliver_mid_turn(&self, _: &str, _: Vec<Json>) {}

    fn close(&self) {
        self.finish.store(true, Ordering::SeqCst);
        self.closing.store(true, Ordering::SeqCst);
        assert!(wait_for(|| self.release_close.load(Ordering::SeqCst), 10_000));
        self.close_count.fetch_add(1, Ordering::SeqCst);
    }
}

fn spec() -> Json {
    json!({"leader_id":"leader", "agents":[member("leader","leader"), member("b","worker")]})
}

#[test]
fn idle_member_is_closed_and_evicted_even_while_session_is_paused() {
    let _env = isolated_state_home("removed-idle");
    let core = core_with_spec("removed-idle", spec());
    let worker = Arc::new(TrackedRunner::default());
    worker.release_close.store(true, Ordering::SeqCst);
    let h = harness_with(core.clone(), vec![("b", worker.clone())]);
    h.runtime.start();
    assert!(submit(&core, "pause", "user", "pause_session", json!({})).ok);
    let receipt = submit(
        &core,
        "remove",
        "leader",
        "apply_topology_patch",
        json!({"base_revision":1, "operations":[{"op":"remove_agent", "agent_id":"b"}]}),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["status"], "APPLIED");
    // Capture the pre-shutdown state: session.close() alone must not make the
    // test pass, and a failure must still clean up the runtime.
    let closed = wait_for(|| worker.close_count.load(Ordering::SeqCst) == 1, 2000);
    let evicted = h.runtime.runner("b").is_none();
    h.runtime.close();
    assert!(closed, "removed idle backend survives until session shutdown");
    assert!(evicted, "removed backend remains in the runner cache");
    assert_eq!(worker.close_count.load(Ordering::SeqCst), 1, "retirement is exactly once");
}

#[test]
fn removal_waits_for_boundary_and_slow_close_does_not_block_other_members() {
    let _env = isolated_state_home("removed-boundary");
    let core = core_with_spec("removed-boundary", spec());
    let worker = Arc::new(TrackedRunner::default());
    let leader = scripted(
        "leader",
        &json!([["inbox"], ["end"], ["inbox"], ["end"], ["inbox"], ["end"], ["inbox"], ["end"]]),
        barriers(),
    );
    let h = harness_with(core.clone(), vec![("leader", leader.clone()), ("b", worker.clone())]);
    h.runtime.start();
    assert!(submit(&core, "assign", "leader", "assign_task", json!({"assignee":"b", "description":"work"})).ok);
    assert!(wait_for(|| worker.entered.load(Ordering::SeqCst), 5000));
    let receipt = submit(
        &core,
        "remove",
        "leader",
        "apply_topology_patch",
        json!({"base_revision":1, "operations":[{"op":"remove_agent", "agent_id":"b"}]}),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.result["status"], "WAITING_BOUNDARY");
    h.runtime.user_message("still before the boundary", false).unwrap();
    assert!(wait_for(
        || leader.observed_inbox.lock().unwrap().iter().any(|item| item.to_string().contains("still before")),
        2000,
    ));
    assert!(!worker.closing.load(Ordering::SeqCst));
    assert!(h.runtime.runner("b").is_some(), "draining backend remains reachable");
    worker.finish.store(true, Ordering::SeqCst);
    let closing = wait_for(|| worker.closing.load(Ordering::SeqCst), 2000);
    h.runtime.user_message("continue after removing worker", false).unwrap();
    let progressed = wait_for(
        || leader.observed_inbox.lock().unwrap().iter().any(|item| item.to_string().contains("continue after")),
        2000,
    );
    let before_release = worker.close_count.load(Ordering::SeqCst);
    worker.release_close.store(true, Ordering::SeqCst);
    h.runtime.close();
    assert!(closing, "backend cleanup never started after the safe boundary");
    assert!(progressed, "backend cleanup blocked dispatch to the Leader");
    assert_eq!(before_release, 0, "the cleanup probe was actually held");
    assert_eq!(worker.close_count.load(Ordering::SeqCst), 1, "shutdown joins pending cleanup");
}

#[test]
fn shutdown_joins_a_removal_cleanup_that_has_not_finished_yet() {
    let _env = isolated_state_home("removed-shutdown");
    let core = core_with_spec("removed-shutdown", spec());
    let worker = Arc::new(TrackedRunner::default());
    let leader = Arc::new(TrackedRunner::default());
    leader.finish.store(true, Ordering::SeqCst);
    leader.release_close.store(true, Ordering::SeqCst);
    let h = harness_with(core.clone(), vec![("leader", leader.clone()), ("b", worker.clone())]);
    h.runtime.start();
    let receipt = submit(
        &core,
        "remove",
        "leader",
        "apply_topology_patch",
        json!({"base_revision":1, "operations":[{"op":"remove_agent", "agent_id":"b"}]}),
    );
    assert!(receipt.ok, "{receipt:?}");
    assert!(wait_for(|| worker.closing.load(Ordering::SeqCst), 3000));
    let (tx, rx) = std::sync::mpsc::channel();
    let runtime = h.runtime.clone();
    let shutdown = std::thread::spawn(move || {
        runtime.close();
        tx.send(()).unwrap();
    });
    assert!(wait_for(|| leader.close_count.load(Ordering::SeqCst) == 1, 3000));
    let waiting = rx.recv_timeout(std::time::Duration::from_millis(100)).is_err();
    worker.release_close.store(true, Ordering::SeqCst);
    shutdown.join().unwrap();
    assert!(waiting, "shutdown returned while a removed member was still closing");
    assert_eq!(worker.close_count.load(Ordering::SeqCst), 1);
}

#[test]
fn mcp_close_reaps_descendants_and_releases_pending_calls() {
    let env = isolated_state_home("mcp-close-tree");
    let probe = McpProcessProbe::new(&env);
    let client = McpClient::connect_stdio_in("python3", &probe.args(), &[], &env, "host", false, 3, 30).unwrap();
    assert!(probe.pids().iter().all(|pid| alive(*pid)), "both parent and descendant are live controls");
    let calling = client.clone();
    let handle = std::thread::spawn(move || calling.call_tool("echo", &json!({})));
    assert!(wait_for(|| probe.waiting.is_file(), 3000));
    client.close();
    let stopped = wait_for(|| probe.stopped(), 2000);
    let released = wait_for(|| handle.is_finished(), 2000);
    // Clean up the fixture before joining, even against the broken client.
    drop(probe);
    assert!(handle.join().unwrap().is_err(), "an interrupted call cannot report success");
    assert!(stopped, "MCP descendants survive close (including a TERM-ignoring child)");
    assert!(released, "an inherited stdout keeps the pending request parked");
    assert!(client.tools().is_err(), "closed clients cannot issue more work");
}

#[test]
fn failed_mcp_handshake_reaps_the_whole_process_group() {
    let env = isolated_state_home("mcp-handshake-tree");
    let probe = McpProcessProbe::new(&env);
    let mut args = probe.args();
    args.push("hang".into());
    let result = McpClient::connect_stdio_in("python3", &args, &[], &env, "host", false, 1, 1);
    assert!(result.is_err(), "the fixture never replies to initialize");
    assert!(wait_for(|| probe.stopped(), 2000), "failed initialization leaked an MCP descendant");
}

#[test]
fn removing_a_production_chat_member_closes_mcp_without_erasing_work() {
    let env = isolated_state_home("removed-chat-mcp");
    let project = env.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let probe = McpProcessProbe::new(&env);
    let mut worker = member("b", "worker");
    worker["tool_bindings"] = json!(["mcp"]);
    worker["workspace_policy"] = json!("isolated");
    let mut catalog = test_catalog();
    catalog["tools"]["mcp"] = json!({
        "kind":"mcp", "mcp_transport":"stdio", "mcp_execution":"host", "mcp_server":"probe",
        "command":"python3", "args":probe.args(), "required":true,
    });
    let session = open_session(OpenOptions {
        session_id: Some("removed-chat".into()),
        cwd: Some(project),
        catalog: Some(serde_json::from_value(catalog).unwrap()),
        initial_spec: Some(json!({"leader_id":"leader", "agents":[member("leader","leader"),worker]})),
        ..Default::default()
    })
    .unwrap();
    let paths = session_paths("removed-chat");
    let workspace = paths.base.join("members/b/work");
    let result_path = workspace.join("result.txt");
    std::fs::write(&result_path, "preserve member output").unwrap();
    let backend = Arc::downgrade(&session.runtime.runner("b").unwrap());
    session.runtime.start();
    let receipt = submit(
        &session.core,
        "remove",
        "leader",
        "apply_topology_patch",
        json!({"base_revision":1, "operations":[{"op":"remove_agent", "agent_id":"b"}]}),
    );
    assert!(receipt.ok, "{receipt:?}");
    let stopped = wait_for(|| probe.stopped(), 2000);
    let released = wait_for(|| backend.upgrade().is_none(), 2000);
    let evicted = session.runtime.runner("b").is_none();
    session.close();
    assert!(stopped, "removal must terminate the real MCP process tree before session.close");
    assert!(evicted);
    assert!(released, "usage probes must not retain removed backend histories in memory");
    assert_eq!(std::fs::read_to_string(result_path).unwrap(), "preserve member output");
}
