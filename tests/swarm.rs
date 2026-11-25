#![cfg(target_os = "linux")]
//! Exercise the default tree through the CLI and real loopback Chat Completions. Keeping a
//! provider request open lets the test inspect a running stage before deciding its response.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const BIN: &str = env!("CARGO_BIN_EXE_teamagents");
const MODEL: &str = "tree-fixture";
const POLL: Duration = Duration::from_millis(15);
const LIMIT: Duration = Duration::from_secs(20);

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap_or_else(|| panic!("no string {name} in {value}"))
}
fn handoff(text: &str, jobs: &[(&str, &str)]) -> Value {
    let children: Vec<_> = jobs.iter().map(|(agent, text)| json!({"agent":agent, "text":text})).collect();
    json!({"op":"handoff", "text":text, "children":children})
}
fn finish(text: &str) -> Value {
    json!({"op":"finish", "text":text})
}
fn shell(command: &str) -> Value {
    json!({"op":"shell", "command":command})
}

struct Call {
    socket: TcpStream,
    agent: String,
    id: String,
    messages: Vec<Value>,
}
impl Call {
    fn read(socket: TcpStream, model: &str, effort: Option<&str>) -> Self {
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = BufReader::new(&socket);
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            assert!(reader.read_line(&mut headers).unwrap() > 0 && headers.len() < 8192);
        }
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(!headers.to_ascii_lowercase().contains("\r\nauthorization:"));
        let len: usize = headers
            .lines()
            .filter_map(|h| h.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .unwrap()
            .1
            .trim()
            .parse()
            .unwrap();
        assert!(len <= 1024 * 1024);
        let mut buf = vec![0; len];
        reader.read_exact(&mut buf).unwrap();
        let body: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(body["model"], model);
        assert_eq!(body.get("reasoning_effort"), effort.map(Value::from).as_ref(), "model {model}");
        let messages = body["messages"].as_array().unwrap().clone();
        let (agent, task) =
            field(&messages[1], "content").strip_prefix("Agent: ").unwrap().split_once("\nTask: ").unwrap();
        Self { socket, agent: agent.into(), id: task.into(), messages }
    }
    fn users(&self) -> impl Iterator<Item = &str> {
        self.messages.iter().filter(|m| m["role"] == "user").map(|m| field(m, "content"))
    }
    fn contains(&self, text: &str) -> bool {
        self.users().any(|s| s.contains(text))
    }
    fn records(&self) -> impl Iterator<Item = Value> + '_ {
        self.users().filter_map(|s| serde_json::from_str(s).ok())
    }
    fn observed(&self, tool: &str, text: &str) {
        assert!(
            self.records().any(|v| v["tool"] == tool && v["ok"] == true && field(&v, "result").contains(text)),
            "missing {tool} response containing {text:?}: {:?}",
            self.messages
        );
    }
    fn report_input(&self) -> Value {
        assert_eq!(self.agent, "reporter");
        assert!(field(&self.messages[0], "content").starts_with("You are the reporter"));
        self.records()
            .find(|r| r["leaf_results"].is_array())
            .unwrap_or_else(|| panic!("missing reporter input: {:?}", self.messages))
    }
    fn reply(mut self, action: Value) {
        let body =
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":action.to_string()}}]})
                .to_string();
        write!(self.socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    }
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Option<Child>);
impl Process {
    fn spawn(mut cmd: Command) -> Self {
        Self(Some(cmd.spawn().unwrap()))
    }
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    fn output(mut self) -> Output {
        let until = Instant::now() + LIMIT;
        while self.child().try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "CLI did not exit");
            thread::sleep(POLL);
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
    fn json(self) -> Value {
        let out = self.output();
        assert!(
            out.status.success(),
            "{}: {} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Lab {
    daemon: Option<Process>,
    socket: TcpListener,
    tmp: Temp,
    ws: PathBuf,
    session: PathBuf,
}
impl Lab {
    fn new() -> Self {
        Self::with_budget(None)
    }
    fn with_budget(budget: Option<u32>) -> Self {
        let tmp = Temp(std::env::temp_dir().join(format!("tree-{}", Uuid::new_v4())));
        fs::create_dir(&tmp.0).unwrap();
        let ws = tmp.0.join("repo");
        let session = tmp.0.join("session");
        fs::create_dir(&ws).unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut lab = Self { daemon: None, socket, tmp, ws, session };
        lab.start(budget);
        lab
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(&self.tmp.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.tmp.0)
            .env("OPENAI_BASE_URL", format!("http://{}/v1", self.socket.local_addr().unwrap()))
            .env("TEAMAGENTS_MODEL", MODEL)
            .arg("--session")
            .arg(&self.session)
            .arg("--workspace")
            .arg(&self.ws)
            .stdin(Stdio::null());
        if let Some(helper) = std::env::var_os("TEAMAGENTS_SLIRP4NETNS") {
            c.env("TEAMAGENTS_SLIRP4NETNS", helper);
        }
        c
    }
    fn start(&mut self, budget: Option<u32>) {
        assert!(self.daemon.is_none());
        let mut c = self.cmd();
        if let Some(n) = budget {
            c.env("TEAMAGENTS_TURN_BUDGET", n.to_string());
        }
        c.arg("daemon").stdout(Stdio::null()).stderr(Stdio::inherit());
        let mut p = Process::spawn(c);
        let until = Instant::now() + LIMIT;
        while UnixStream::connect(self.session.join("swarm.sock")).is_err() {
            assert!(p.child().try_wait().unwrap().is_none() && Instant::now() < until, "daemon failed to start");
            thread::sleep(POLL);
        }
        self.daemon = Some(p);
    }
    fn restart(&mut self) {
        drop(self.daemon.take());
        self.start(None);
    }
    fn client(&self, args: &[&str]) -> Process {
        let mut c = self.cmd();
        c.arg("--json").args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        Process::spawn(c)
    }
    fn chat(&self, input: &str) -> Output {
        let mut c = self.cmd();
        c.args(["--json", "chat"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut chat = Process::spawn(c);
        writeln!(chat.child().stdin.take().unwrap(), "{input}").unwrap();
        let out = chat.output();
        assert!(out.status.success(), "chat {input:?}: {}", String::from_utf8_lossy(&out.stderr));
        out
    }
    fn model_settings(&self) -> (String, Value) {
        let out = self.chat("/model");
        let settings: Value =
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("invalid /model output: {e}; {out:?}"));
        assert_eq!(settings["provider"], "api");
        let effort = settings.get("effort").expect("/model must include the effective effort");
        (field(&settings, "model").to_owned(), effort.clone())
    }
    fn json(&self, args: &[&str]) -> Value {
        self.client(args).json()
    }
    fn status(&self, id: &str) -> Value {
        self.json(&["status", id])
    }
    fn wait(&self, id: &str, expected: &str) -> Value {
        let until = Instant::now() + LIMIT;
        loop {
            let t = self.status(id);
            if t["status"] == expected {
                return t;
            }
            assert!(
                !matches!(t["status"].as_str(), Some("done" | "failed" | "unknown" | "cancelled"))
                    && Instant::now() < until,
                "expected {expected}: {t}"
            );
            thread::sleep(POLL);
        }
    }
    fn call(&self) -> Call {
        self.call_with_model(MODEL, None)
    }
    fn call_with_model(&self, model: &str, effort: Option<&str>) -> Call {
        let until = Instant::now() + LIMIT;
        loop {
            match self.socket.accept() {
                Ok((s, _)) => return Call::read(s, model, effort),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => thread::sleep(POLL),
                Err(e) => panic!("expected provider request ({e}); tasks={}", self.json(&["tasks"])),
            }
        }
    }
    fn next(&self, id: &str) -> Call {
        let call = self.call();
        assert_eq!(call.id, id, "wrong task for provider request: {:?}", call.messages);
        call
    }
    fn pending(&self, id: &str, action: &Value) -> String {
        let task = self.wait(id, "approval");
        assert_eq!(task["policy"], "review");
        assert_eq!(&task["action"], action);
        let approval = field(&task, "approval_id");
        assert_ne!(approval, id);
        approval.to_owned()
    }
}

fn verify_receipts(root: &Value, command: &str, outcomes: &[bool]) {
    let attempts: Vec<_> =
        root["evidence"].as_array().unwrap().iter().filter(|v| v["action"]["op"] == "verify").collect();
    assert_eq!(attempts.len(), outcomes.len(), "{root}");
    for (attempt, ok) in attempts.iter().zip(outcomes) {
        assert_eq!(attempt["action"], json!({"op":"verify", "command":command}));
        assert_eq!(attempt["ok"], *ok);
    }
}

#[test]
fn siblings_overlap_grow_a_lower_branch_and_report_partial_failure_once() {
    let lab = Lab::new();
    let receipt = lab.json(&["exec", "--detach", "Review both areas"]);
    let root = field(&receipt, "id");
    let start = lab.next(root);
    assert_eq!(start.agent, "default");
    start.reply(handoff("root handoff", &[("layout", "Map the layout"), ("api", "Inspect the API")]));
    // Both requests must arrive before the server sends either response.
    let first = lab.call();
    let second = lab.call();
    let (layout, api) = if first.agent == "layout" { (first, second) } else { (second, first) };
    assert_eq!(layout.agent, "layout");
    assert_eq!(api.agent, "api");
    assert!(layout.contains("Map the layout") && api.contains("Inspect the API"));
    assert!(!layout.contains("Inspect the API") && !api.contains("Map the layout"));
    let layout_id = layout.id.clone();
    let api_id = api.id.clone();
    for id in [&layout_id, &api_id] {
        let task = lab.status(id);
        assert_eq!(task["status"], "running");
        assert_eq!(task["parent"], root);
    }
    assert_eq!(lab.status(root)["status"], "waiting");
    layout.reply(handoff("split layout work", &[("assets", "Inspect assets")]));
    let assets = lab.call();
    let assets_id = assets.id.clone();
    assert!(assets.contains("Inspect assets"));
    assert_eq!(lab.status(&assets_id)["parent"], layout_id);
    drop(api); // Fail this provider exchange while the other leaf remains in flight.
    lab.wait(&api_id, "failed");
    assert_eq!(lab.status(root)["status"], "waiting");
    assert_eq!(lab.status(&assets_id)["status"], "running");
    assets.reply(finish("assets catalogued"));
    let reporter = lab.call();
    assert!(![root, &layout_id, &api_id, &assets_id].contains(&reporter.id.as_str()));
    let input = reporter.report_input();
    let leaves = input["leaf_results"].as_array().unwrap();
    assert_eq!(leaves.len(), 2, "{leaves:?}");
    assert!(leaves.iter().any(|l| l["task"] == api_id && l["status"] == "failed" && l["result"].is_null()));
    assert!(leaves
        .iter()
        .any(|l| l["task"] == assets_id && l["status"] == "done" && l["result"] == "assets catalogued"));
    let waiting = lab.status(root);
    assert_eq!(waiting["status"], "waiting");
    assert_eq!(waiting["stage_status"], "done");
    assert_eq!(lab.status(&layout_id)["status"], "done");
    let nodes = waiting["tree_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    assert!(nodes.iter().all(|node| node["task"] != reporter.id));
    assert_eq!(waiting["leaf_results"], input["leaf_results"]);
    reporter.reply(finish("Assets catalogued. API inspection failed."));
    let outcome = lab.wait(root, "failed");
    assert_eq!(outcome["result"], "Assets catalogued. API inspection failed.");
    assert_eq!(outcome["turns_used"], 5); // Root, siblings, lower leaf and one outside reporter.
}

fn assert_exec_model(lab: &Lab, prompt: &str, model: &str, effort: Option<&str>) {
    let exec = lab.client(&["exec", prompt]);
    let worker = lab.call_with_model(model, effort);
    assert!(worker.contains(prompt));
    worker.reply(finish("Work complete"));
    let reporter = lab.call_with_model(model, effort);
    assert_eq!(reporter.report_input()["intent"], prompt);
    reporter.reply(finish("Reported"));
    assert_eq!(exec.json()["result"], "Reported");
}

#[test]
fn model_commands_show_reset_and_preserve_settings_on_errors() {
    let mut lab = Lab::new();
    let defaults = lab.model_settings();
    assert_eq!(defaults.0, MODEL);

    lab.chat("/model selected high");
    let selected = ("selected".to_owned(), json!("high"));
    assert_eq!(lab.model_settings(), selected);
    for invalid in [
        "/model effort",
        "/model default high",
        "/model effort low extra",
        "/model replacement low extra",
        "/model effort invalid-effort",
        "/model replacement invalid-effort",
    ] {
        assert!(!lab.chat(invalid).stderr.is_empty(), "missing diagnostic for {invalid}");
        assert_eq!(lab.model_settings(), selected, "{invalid} changed the session settings");
    }
    assert_eq!(lab.json(&["tasks"]), json!([]));

    lab.chat("/model replacement");
    assert_eq!(lab.model_settings(), ("replacement".into(), defaults.1.clone()));
    assert_exec_model(&lab, "Use the new model with its default effort", "replacement", None);

    lab.chat("/model effort medium");
    assert_eq!(lab.model_settings(), ("replacement".into(), json!("medium")));
    lab.chat("/model default");
    assert_eq!(lab.model_settings(), defaults);
    lab.restart();
    assert_eq!(lab.model_settings(), defaults);
    assert_exec_model(&lab, "Use the session defaults", MODEL, None);
}

#[test]
fn model_selection_survives_restart_and_stays_with_the_chat_tree() {
    let mut lab = Lab::new();
    let prompt = "Review the release notes";
    lab.chat("/model reviewer low\n/model effort high");
    assert_eq!(lab.model_settings(), ("reviewer".into(), json!("high")));
    let submitted = lab.chat(prompt);
    let receipt: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    let root = field(&receipt, "id");
    let original = lab.call_with_model("reviewer", Some("high"));
    assert_eq!(original.id, root);
    assert!(original.contains(prompt));

    // Change the session while the original root's first provider request is still open.
    lab.chat("/model editor low");
    original.reply(json!({"op":"ask", "text":"Which version?"}));
    lab.wait(root, "input");
    lab.restart();
    assert_exec_model(&lab, "Edit the changelog", "editor", Some("low"));

    lab.json(&["answer", root, "Version 2"]);
    let resumed = lab.call_with_model("reviewer", Some("high"));
    assert_eq!(resumed.id, root);
    assert!(resumed.contains("Version 2"));
    resumed.reply(handoff("Review version 2", &[("reader", "Read the notes")]));
    let child = lab.call_with_model("reviewer", Some("high"));
    assert_eq!(child.agent, "reader");
    assert_eq!(lab.status(&child.id)["parent"], root);
    child.reply(finish("Notes reviewed"));
    let reporter = lab.call_with_model("reviewer", Some("high"));
    assert_eq!(reporter.report_input()["intent"], prompt);
    reporter.reply(finish("Review complete"));
    assert_eq!(lab.wait(root, "done")["result"], "Review complete");
}

#[test]
fn root_only_waits_for_answer_and_reporter_across_reconnect() {
    let mut lab = Lab::new();
    let attached = lab.client(&["exec", "Choose a platform"]);
    let question = "Which distribution?";
    lab.call().reply(json!({"op":"ask", "text":question}));
    let paused = attached.output();
    assert!(!paused.status.success());
    let task: Value = serde_json::from_slice(&paused.stdout).unwrap();
    let root = field(&task, "id");
    assert_eq!(task["status"], "input");
    assert_eq!(task["question"], question);
    assert!(String::from_utf8_lossy(&paused.stderr).contains(&format!("answer {root}")));
    lab.restart();
    assert_eq!(lab.wait(root, "input")["question"], question);
    let answer = Uuid::new_v4().to_string();
    lab.json(&["answer", root, &answer]);
    assert!(!lab.client(&["answer", root, "late"]).output().status.success());
    let resumed = lab.next(root);
    assert!(resumed.contains(&answer));
    resumed.reply(finish("unverified leaf answer"));
    let reporter = lab.call();
    assert_ne!(reporter.id, root);
    let input = reporter.report_input();
    assert_eq!(input["intent"], "Choose a platform");
    assert!(input["clarifications"].to_string().contains(&answer));
    assert_eq!(input["leaf_results"].as_array().unwrap().len(), 1);
    assert_eq!(input["leaf_results"][0]["result"], "unverified leaf answer");
    assert_eq!(lab.status(root)["status"], "waiting");
    reporter.reply(finish("Final report about the selected platform"));
    let result = lab.wait(root, "done");
    assert_eq!(result["result"], "Final report about the selected platform");
    assert_eq!(result["policy"], "auto");
    assert_eq!(result["verification"], "not_requested");
    assert_eq!(result["evidence"], json!([]));
    assert_eq!(result["turns_used"], 3);
}

#[test]
fn review_survives_disconnect_and_restart_before_exact_shell_and_check() {
    let mut lab = Lab::new();
    let token = Uuid::new_v4().to_string();
    fs::write(lab.ws.join("fixture.txt"), &token).unwrap();
    let script = "cp fixture.txt answer.txt && printf x >> executions && cat answer.txt";
    let edit = json!({"op":"edit_file", "path":"build.sh", "old":"", "new":script});
    let build = "sh build.sh";
    let check = "cmp -s fixture.txt answer.txt && printf checked > accepted";
    let mut client = lab.client(&["exec", "--review", "--check", check, "Build the artifact"]);
    let work = lab.call();
    let root = work.id.clone();
    work.reply(handoff("assign build", &[("builder", "Build from fixture.txt")]));
    let builder = lab.call();
    let leaf = builder.id.clone();
    assert_eq!(lab.status(&leaf)["parent"], root);
    builder.reply(edit.clone());
    let edit_approval = lab.pending(&leaf, &edit);
    assert!(!lab.ws.join("build.sh").exists());
    lab.json(&["approve", &edit_approval]);
    let built = lab.next(&leaf);
    built.observed("edit_file", "");
    built.reply(shell(build));
    let shell_approval = lab.pending(&leaf, &shell(build));
    // The root's agent is free to accept a second submission while its first tree awaits review.
    let second = lab.client(&["exec", "Summarize the independent request"]);
    let other_root = lab.call();
    let other_id = other_root.id.clone();
    assert_ne!(other_id, root);
    other_root.reply(finish("independent answer"));
    let other_report = lab.call();
    let input = other_report.report_input();
    assert_eq!(input["leaf_results"].as_array().unwrap().len(), 1);
    assert_eq!(input["leaf_results"][0]["task"], other_id);
    other_report.reply(finish("Independent report"));
    let other_result = second.json();
    assert_eq!(other_result["id"], other_id);
    assert_eq!(other_result["result"], "Independent report");
    assert!(client.child().try_wait().unwrap().is_none());
    drop(client);
    lab.restart();
    assert_eq!(lab.pending(&leaf, &shell(build)), shell_approval);
    assert!(!lab.client(&["approve", &leaf]).output().status.success());
    assert!(!lab.ws.join("answer.txt").exists());
    lab.json(&["approve", &shell_approval]);
    assert!(!lab.client(&["approve", &shell_approval]).output().status.success());
    let completed = lab.next(&leaf);
    completed.observed("shell", &token);
    completed.reply(finish(&token));
    let reporter = lab.call();
    assert_eq!(reporter.report_input()["leaf_results"][0]["result"], token);
    assert_eq!(fs::read(lab.ws.join("accepted")).unwrap(), b"checked");
    reporter.reply(finish("Artifact verified"));
    let result = lab.wait(&root, "done");
    assert_eq!(result["result"], "Artifact verified");
    assert_eq!(result["verification"], "passed");
    verify_receipts(&result, check, &[true]);
    let receipts = result["subtree_receipts"].as_array().unwrap();
    for op in ["edit_file", "shell"] {
        assert!(receipts.iter().any(|r| r["task"] == leaf && r["op"] == op && r["ok"] == true), "{receipts:?}");
    }
    assert_eq!(fs::read(lab.ws.join("executions")).unwrap(), b"x");
    assert_eq!(fs::read_to_string(lab.ws.join("answer.txt")).unwrap(), token);
}

#[test]
fn failed_authorized_check_creates_distinct_repair_before_reporting() {
    let lab = Lab::new();
    let token = Uuid::new_v4().to_string();
    fs::write(lab.ws.join("required.txt"), &token).unwrap();
    let check = "printf x >> checks; cmp -s required.txt answer.txt || { printf 'expected: ' >&2; cat required.txt >&2; exit 7; }; printf accepted";
    let expired = lab.client(&["exec", "--timeout", "0", "--check", check, "Produce the answer"]).output();
    assert!(!expired.status.success());
    let receipt: Value = serde_json::from_slice(&expired.stdout).unwrap();
    let root = field(&receipt, "id");
    assert!(String::from_utf8_lossy(&expired.stderr).contains(&format!("timed out waiting for {root}")));
    lab.next(root).reply(handoff("assign artifact", &[("writer", "Create answer.txt")]));
    let writer = lab.call();
    let writer_id = writer.id.clone();
    let writer_agent = writer.agent.clone();
    assert!(!lab.ws.join("checks").exists());
    writer.reply(finish("first proposal"));
    let repair = lab.call();
    let repair_id = repair.id.clone();
    assert_ne!(repair_id, writer_id);
    assert_ne!(repair_id, root);
    assert_ne!(repair.agent, writer_agent);
    assert!(repair.contains(&format!("expected: {token}")), "missing check diagnostics: {:?}", repair.messages);
    assert_eq!(fs::read(lab.ws.join("checks")).unwrap(), b"x");
    let edit = json!({"op":"edit_file", "path":"answer.txt", "old":"", "new":token});
    repair.reply(edit.clone());
    let repaired = lab.next(&repair_id);
    repaired.observed("edit_file", "");
    repaired.reply(finish("answer corrected"));
    let reporter = lab.call();
    assert!(![root, &writer_id, &repair_id].contains(&reporter.id.as_str()));
    assert!(reporter.report_input()["leaf_results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["result"] == "answer corrected"));
    assert_eq!(fs::read(lab.ws.join("checks")).unwrap(), b"xx");
    reporter.reply(finish("Corrected answer verified"));
    let task = lab.wait(root, "done");
    assert_eq!(task["verification"], "passed");
    verify_receipts(&task, check, &[false, true]);
    assert_eq!(lab.status(&repair_id)["evidence"][0]["action"], edit);
    assert_eq!(fs::read_to_string(lab.ws.join("answer.txt")).unwrap(), token);
}

#[test]
fn denied_and_cancelled_review_actions_never_execute() {
    let lab = Lab::new();
    let refused = shell("printf unexpected > refused");
    for decision in ["deny", "cancel"] {
        let receipt = lab.json(&["exec", "--review", "--detach", "Probe review"]);
        let root = field(&receipt, "id");
        lab.next(root).reply(refused.clone());
        let approval = lab.pending(root, &refused);
        lab.json(&[decision, if decision == "cancel" { root } else { &approval }]);
        if decision == "deny" {
            let continued = lab.next(root);
            assert!(continued.records().any(|v| v["tool_denied"] == true));
            continued.reply(finish("permission denied"));
            let reporter = lab.call();
            assert_eq!(reporter.report_input()["leaf_results"][0]["result"], "permission denied");
            reporter.reply(finish("No command was authorized"));
            lab.wait(root, "done");
        } else {
            lab.wait(root, "cancelled");
        }
        assert!(!lab.client(&["approve", &approval]).output().status.success());
        assert!(!lab.ws.join("refused").exists());
    }
}

#[test]
fn attempt_cap_and_shared_budget_limit_checked_repairs() {
    let check = "printf x >> attempts; printf 'still missing' >&2; test -f missing.txt";
    // A budget of three pays for root + one repair + reporter. More budget permits
    // another forward repair, but cannot buy a fourth check.
    for (budget, attempts) in [(3, 2), (8, 3)] {
        let lab = Lab::with_budget(Some(budget));
        let receipt = lab.json(&["exec", "--detach", "--check", check, "Produce the missing file"]);
        let root = field(&receipt, "id");
        lab.next(root).reply(finish("first claim"));
        let mut stages = vec![root.to_string()];
        for _ in 1..attempts {
            let repair = lab.call();
            assert_ne!(repair.agent, "reporter");
            assert!(!stages.contains(&repair.id));
            assert!(repair.contains("still missing"));
            stages.push(repair.id.clone());
            repair.reply(finish("repair claim"));
        }
        let reporter = lab.call();
        reporter.report_input();
        reporter.reply(finish("No artifact passed the check"));
        let task = lab.wait(root, "failed");
        assert_eq!(task["verification"], "failed");
        assert_eq!(task["turns_used"], attempts + 1);
        verify_receipts(&task, check, &vec![false; attempts]);
        assert_eq!(fs::read(lab.ws.join("attempts")).unwrap(), vec![b'x'; attempts]);
    }
}

#[test]
fn root_and_handoff_can_fix_the_same_checkout_installer() {
    const INSTALLER: &str = include_str!("../install.sh");
    const CHECK: &str = "sh install.sh --help";
    let broken = INSTALLER.replacen("-h|--help)", "-h|--hlep)", 1);
    assert_ne!(broken, INSTALLER);
    for team in [false, true] {
        let lab = Lab::new();
        let target = lab.ws.join("install.sh");
        fs::write(&target, &broken).unwrap();
        let attached = lab.client(&["exec", "--check", CHECK, "Restore the installer help option"]);
        let mut editor = lab.call();
        if team {
            editor.reply(handoff("assign installer", &[("fixer", "Restore install.sh --help")]));
            editor = lab.call();
        }
        let id = editor.id.clone();
        editor.reply(json!({"op":"read_file", "path":"install.sh"}));
        let read = lab.next(&id);
        read.observed("read_file", "-h|--hlep)");
        read.reply(json!({"op":"edit_file", "path":"install.sh", "old":"-h|--hlep)", "new":"-h|--help)"}));
        let edited = lab.next(&id);
        edited.observed("edit_file", "");
        edited.reply(finish("installer fixed"));
        let reporter = lab.call();
        assert_eq!(reporter.report_input()["leaf_results"][0]["result"], "installer fixed");
        reporter.reply(finish("Installer help restored"));
        let root = attached.json();
        assert_eq!(root["result"], "Installer help restored");
        assert_eq!(root["verification"], "passed");
        verify_receipts(&root, CHECK, &[true]);
        assert_eq!(fs::read_to_string(target).unwrap(), INSTALLER);
    }
}

#[test]
fn reporter_cannot_run_a_workspace_tool() {
    let lab = Lab::new();
    let receipt = lab.json(&["exec", "--detach", "Identify a platform"]);
    let root = field(&receipt, "id");
    lab.next(root).reply(finish("Linux"));
    let reporter = lab.call();
    assert_eq!(reporter.report_input()["leaf_results"][0]["result"], "Linux");
    reporter.reply(shell("touch reporter-side-effect"));
    let task = lab.wait(root, "failed");
    assert_eq!(task["turns_used"], 2);
    assert_eq!(task["subtree_receipts"], json!([]));
    assert!(!lab.ws.join("reporter-side-effect").exists());
}

#[test]
fn worker_recovers_from_malformed_action_json_within_budget() {
    let lab = Lab::new();
    let attached = lab.client(&["exec", "Return recovered report"]);
    let first = lab.call();
    let root = first.id.clone();
    // 1st worker turn returns a markdown-fenced JSON block instead of raw JSON.
    let body = json!({
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "```json\n{\"op\":\"finish\",\"text\":\"oops\"}\n```"}}]
    })
    .to_string();
    write!(
        { first }.socket,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();

    // Runtime feeds the protocol error back cleanly on a budget-charged correction turn.
    let retry = lab.next(&root);
    assert!(retry.records().any(|v| v["protocol_error"].as_str().is_some_and(|s| s.contains("code fence"))));
    retry.reply(finish("recovered work"));
    let reporter = lab.call();
    assert_eq!(reporter.report_input()["leaf_results"][0]["result"], "recovered work");
    reporter.reply(finish("Recovered cleanly after format slip"));
    let out = attached.json();
    assert_eq!(out["status"], "done");
    assert_eq!(out["turns_used"], 3);

    // Three consecutive malformed worker turns exhaust MAX_ACTION_REJECTIONS (2 retries -> 3rd fails).
    let fail_receipt = lab.json(&["exec", "--detach", "Exhaust format retries"]);
    let fail_root = field(&fail_receipt, "id");
    for _ in 0..3 {
        let mut turn = lab.next(fail_root);
        let bad = json!({
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "not json"}}]
        })
        .to_string();
        write!(
            turn.socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bad}",
            bad.len()
        )
        .unwrap();
    }
    let reporter = lab.call();
    reporter.reply(finish("Reported failure after 3 malformed turns"));
    let settled = lab.wait(fail_root, "failed");
    assert!(settled["stage_error"].as_str().is_some_and(|e| e.contains("non-JSON text")));
    assert_eq!(settled["turns_used"], 4);
}
