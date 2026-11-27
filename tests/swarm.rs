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
    fn read_http(socket: TcpStream, expected_path: &str) -> (String, Value, TcpStream) {
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = BufReader::new(&socket);
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            assert!(reader.read_line(&mut headers).unwrap() > 0 && headers.len() < 8192);
        }
        assert!(headers.starts_with(&format!("POST {expected_path} HTTP/1.1\r\n")), "unexpected request: {headers}");
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
        drop(reader);
        (headers, body, socket)
    }

    fn read(socket: TcpStream, model: &str, effort: Option<&str>) -> Self {
        let (headers, body, socket) = Self::read_http(socket, "/v1/chat/completions");
        assert!(!headers.to_ascii_lowercase().contains("\r\nauthorization:"));
        assert_eq!(body["model"], model);
        assert_eq!(body.get("reasoning_effort"), effort.map(Value::from).as_ref(), "model {model}");
        let messages = body["messages"].as_array().unwrap().clone();
        let (agent, task) =
            field(&messages[1], "content").strip_prefix("Agent: ").unwrap().split_once("\nTask: ").unwrap();
        Self { socket, agent: agent.into(), id: task.into(), messages }
    }
    fn users(&self) -> impl Iterator<Item = &str> {
        self.messages
            .iter()
            .filter(|m| matches!(m["role"].as_str(), Some("user" | "tool")))
            .map(|m| field(m, "content"))
    }
    fn contains(&self, text: &str) -> bool {
        self.users().any(|s| s.contains(text))
    }
    fn any_message(&self, text: &str) -> bool {
        self.messages.iter().any(|m| m["content"].as_str().is_some_and(|s| s.contains(text)))
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
    extra_env: Vec<(String, String)>,
    set_openai_base: bool,
}
impl Lab {
    fn new() -> Self {
        Self::new_with(&[], true)
    }
    fn new_with(extra_env: &[(&str, &str)], set_openai_base: bool) -> Self {
        let tmp = Temp(std::env::temp_dir().join(format!("tree-{}", Uuid::new_v4())));
        fs::create_dir(&tmp.0).unwrap();
        let ws = tmp.0.join("repo");
        let session = tmp.0.join("session");
        fs::create_dir(&ws).unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let extra_env = extra_env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        let mut lab = Self { daemon: None, socket, tmp, ws, session, extra_env, set_openai_base };
        lab.start();
        lab
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(&self.tmp.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.tmp.0)
            .env("TEAMAGENTS_MODEL", MODEL);
        if self.set_openai_base {
            c.env("OPENAI_BASE_URL", format!("http://{}/v1", self.socket.local_addr().unwrap()));
        }
        for (k, v) in &self.extra_env {
            let val = v.replace("{URL}", &format!("http://{}", self.socket.local_addr().unwrap()));
            c.env(k, val);
        }
        c.arg("--session").arg(&self.session).arg("--workspace").arg(&self.ws).stdin(Stdio::null());
        if let Some(helper) = std::env::var_os("TEAMAGENTS_SLIRP4NETNS") {
            c.env("TEAMAGENTS_SLIRP4NETNS", helper);
        }
        c
    }
    fn start(&mut self) {
        assert!(self.daemon.is_none());
        let mut c = self.cmd();
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
        self.start();
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
    drop(api); // Exhaust all 5 transient HTTP retry attempts while the other leaf stays in flight.
    for _ in 0..4 {
        drop(lab.next(&api_id));
    }
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
    assert_eq!(exec.json()["result"], "Work complete");
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
    resumed.reply(handoff("Review version 2", &[("reader1", "Read part 1"), ("reader2", "Read part 2")]));
    let c1 = lab.call_with_model("reviewer", Some("high"));
    let c2 = lab.call_with_model("reviewer", Some("high"));
    assert_eq!(lab.status(&c1.id)["parent"], root);
    assert_eq!(lab.status(&c2.id)["parent"], root);
    c1.reply(finish("Notes 1 reviewed"));
    c2.reply(finish("Notes 2 reviewed"));
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
    let result = lab.wait(root, "done");
    assert_eq!(result["result"], "unverified leaf answer");
    assert_eq!(result["continuation_node"], root);
    assert_eq!(result["policy"], "auto");
    assert_eq!(result["verification"], "not_requested");
    assert_eq!(result["evidence"], json!([]));
    assert_eq!(result["turns_used"], 2);
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
    let other_result = second.json();
    assert_eq!(other_result["id"], other_id);
    assert_eq!(other_result["result"], "independent answer");
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
    let result = lab.wait(&root, "done");
    assert_eq!(fs::read(lab.ws.join("accepted")).unwrap(), b"checked");
    assert_eq!(result["result"], token);
    assert_eq!(result["continuation_node"], leaf);
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
    assert_eq!(task["continuation_kind"], "summarizer");
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
            let settled = lab.wait(root, "done");
            assert_eq!(settled["result"], "permission denied");
        } else {
            lab.wait(root, "cancelled");
        }
        assert!(!lab.client(&["approve", &approval]).output().status.success());
        assert!(!lab.ws.join("refused").exists());
    }
}

#[test]
fn attempt_cap_limits_checked_repairs() {
    let check = "printf x >> attempts; printf 'still missing' >&2; test -f missing.txt";
    let lab = Lab::new();
    let receipt = lab.json(&["exec", "--detach", "--check", check, "Produce the missing file"]);
    let root = field(&receipt, "id");
    lab.next(root).reply(finish("first claim"));
    let mut stages = vec![root.to_string()];
    for _ in 1..10 {
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
    assert_eq!(task["turns_used"], 11);
    verify_receipts(&task, check, &[false; 10]);
    assert_eq!(fs::read(lab.ws.join("attempts")).unwrap(), b"xxxxxxxxxx");
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
        let root = attached.json();
        assert_eq!(root["result"], "installer fixed");
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
    lab.next(root).reply(handoff("split lookup", &[("w1", "Check arch"), ("w2", "Check os")]));
    let w1 = lab.call();
    let w2 = lab.call();
    w1.reply(finish("x86_64"));
    w2.reply(finish("Linux"));
    let reporter = lab.call();
    assert_eq!(reporter.report_input()["leaf_results"].as_array().unwrap().len(), 2);
    reporter.reply(shell("touch reporter-side-effect"));
    let task = lab.wait(root, "failed");
    assert_eq!(task["turns_used"], 4);
    assert_eq!(task["subtree_receipts"], json!([]));
    assert!(!lab.ws.join("reporter-side-effect").exists());
}

#[test]
fn worker_recovers_from_malformed_action_json_within_budget() {
    let lab = Lab::new();
    // Clean single markdown JSON fence unwraps leniently in 1 turn.
    let fenced = lab.client(&["exec", "Return fenced JSON report"]);
    let fenced_turn = lab.call();
    let fenced_body = json!({
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "```json\n{\"op\":\"finish\",\"text\":\"fenced ok\"}\n```"}}]
    })
    .to_string();
    write!(
        { fenced_turn }.socket,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{fenced_body}",
        fenced_body.len()
    )
    .unwrap();
    let fenced_out = fenced.json();
    assert_eq!(fenced_out["status"], "done");
    assert_eq!(fenced_out["result"], "fenced ok");
    assert_eq!(fenced_out["turns_used"], 1);

    // Prose-wrapped JSON still rejects with protocol_error and recovers on retry.
    let attached = lab.client(&["exec", "Return recovered report"]);
    let first = lab.call();
    let root = first.id.clone();
    let body = json!({
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "Here is the JSON: {\"op\":\"finish\",\"text\":\"oops\"}"}}]
    })
    .to_string();
    write!(
        { first }.socket,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();

    let retry = lab.next(&root);
    assert!(retry.records().any(|v| v["protocol_error"].as_str().is_some_and(|s| s.contains("non-JSON text"))));
    retry.reply(finish("recovered work"));
    let out = attached.json();
    assert_eq!(out["status"], "done");
    assert_eq!(out["result"], "recovered work");
    assert_eq!(out["turns_used"], 2);

    // Six consecutive malformed worker turns exhaust MAX_ACTION_REJECTIONS (5 retries -> 6th fails).
    let fail_receipt = lab.json(&["exec", "--detach", "Exhaust format retries"]);
    let fail_root = field(&fail_receipt, "id");
    for _ in 0..6 {
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
    let settled = lab.wait(fail_root, "failed");
    assert!(settled["stage_error"].as_str().is_some_and(|e| e.contains("non-JSON text")));
    assert_eq!(settled["turns_used"], 6);
}

#[test]
fn single_leaf_and_summarizer_continuation_end_to_end() {
    let lab = Lab::new();

    // Two concurrent exec runs on @default arrive concurrently (no same-agent serialization) and stay isolated.
    let first = lab.client(&["exec", "SECRET-SESSION-ONE"]);
    let second_par = lab.client(&["exec", "CONCURRENT-DEFAULT-TWO"]);
    let c1 = lab.call();
    let c2 = lab.call();
    let (f_turn, p_turn) = if c1.contains("SECRET-SESSION-ONE") { (c1, c2) } else { (c2, c1) };
    assert!(f_turn.contains("SECRET-SESSION-ONE") && p_turn.contains("CONCURRENT-DEFAULT-TWO"));
    assert!(!p_turn.contains("SECRET-SESSION-ONE"));
    f_turn.reply(finish("done first"));
    p_turn.reply(finish("done parallel"));
    assert_eq!(first.json()["result"], "done first");
    assert_eq!(second_par.json()["result"], "done parallel");

    let second = lab.client(&["exec", "Fresh unrelated session"]);
    let s_turn = lab.call();
    assert!(!s_turn.contains("SECRET-SESSION-ONE") && !s_turn.contains("done first"));
    let s_root = s_turn.id.clone();
    s_turn.reply(handoff("assign worker", &[("worker", "Step 1")]));
    let w_leaf = lab.call();
    let leaf_id = w_leaf.id.clone();
    w_leaf.reply(finish("Leaf 1 done"));
    let s2 = second.json();
    assert_eq!(s2["status"], "done");
    assert_eq!(s2["continuation_node"], leaf_id);
    assert_eq!(s2["continuation_kind"], "leaf");

    // Continue from the sole leaf node: promotes `leaf_id` to subtree root with 2-leaf summarizer!
    let cont1 = lab.client(&["continue", &leaf_id, "Expand via 2 workers"]);
    let leaf_r2 = lab.next(&leaf_id);
    assert!(leaf_r2.any_message("Leaf 1 done") && leaf_r2.contains("Expand via 2 workers"));
    leaf_r2.reply(handoff("fan out", &[("default", "Reuse default"), ("helper", "Second child")]));
    let a = lab.call();
    let b = lab.call();
    a.reply(finish("Default R2"));
    b.reply(finish("Helper R2"));
    let sum = lab.call();
    let sum_id = sum.id.clone();
    assert_eq!(sum.report_input()["leaf_results"].as_array().unwrap().len(), 2);
    sum.reply(finish("Summarized R2"));
    let done_r2 = cont1.json();
    assert_eq!(done_r2["id"], s_root);
    assert_eq!(done_r2["status"], "done");
    assert_eq!(done_r2["round"], 2);
    assert_eq!(done_r2["continuation_node"], sum_id);
    assert_eq!(done_r2["continuation_kind"], "summarizer");
    assert_eq!(done_r2["rounds"].as_array().unwrap().len(), 2);
    assert!(done_r2["tree_nodes"].as_array().unwrap().iter().any(|n| n["task"] == sum_id && n["kind"] == "summarizer"));

    // Continue directly from the in-tree summarizer node for Round 3!
    let cont2 = lab.client(&["continue", &s_root, "Follow up from summarizer"]);
    let sum_r3 = lab.next(&sum_id);
    assert!(sum_r3.any_message("Summarized R2") && sum_r3.contains("Follow up from summarizer"));
    sum_r3.reply(finish("Summarizer direct R3"));
    let done_r3 = cont2.json();
    assert_eq!(done_r3["round"], 3);
    assert_eq!(done_r3["result"], "Summarizer direct R3");
    assert_eq!(done_r3["continuation_node"], sum_id);
    assert_eq!(done_r3["rounds"].as_array().unwrap().len(), 3);
}

#[test]
fn collaborative_whiteboard_coordinates_sibling_leaves_summarizer_and_cli() {
    let lab = Lab::new();
    let attached = lab.client(&["exec", "Coordinate reader and writer via whiteboard"]);
    let root = lab.call();
    let root_id = root.id.clone();
    root.reply(handoff("split work", &[("writer", "publish api contract"), ("reader", "wait for api contract")]));

    let c1 = lab.call();
    let c2 = lab.call();
    let (writer, reader) = if c1.agent == "writer" { (c1, c2) } else { (c2, c1) };
    let writer_id = writer.id.clone();
    let reader_id = reader.id.clone();

    // Ensure reader commits `wb_wait` before writer writes `contract:api` so wake is deterministically "updated".
    reader.reply(json!({"op":"wb_wait","key":"contract:"}));
    let until = Instant::now() + LIMIT;
    while lab.status(&reader_id)["status"] != "waiting" {
        assert!(Instant::now() < until, "reader did not enter waiting state");
        thread::sleep(POLL);
    }

    writer.reply(json!({
        "op":"wb_write",
        "key":"contract:api",
        "mode":"set",
        "kind":"contract",
        "content":"endpoint=/v2/items"
    }));

    let n1 = lab.call();
    let n2 = lab.call();
    let (writer2, reader2) = if n1.id == writer_id { (n1, n2) } else { (n2, n1) };
    assert!(writer2.contains("contract:api"));
    assert!(reader2.contains("updated") && reader2.contains("endpoint=/v2/items"));
    writer2.reply(finish("published contract:api"));
    reader2.reply(json!({"op":"wb_read","prefix":"contract:"}));

    let reader3 = lab.next(&reader_id);
    assert!(reader3.contains("endpoint=/v2/items"));
    reader3.reply(finish("read endpoint=/v2/items"));

    let sum = lab.call();
    let pkt = sum.report_input();
    let wb = pkt["whiteboard"].as_array().unwrap();
    assert_eq!(wb.len(), 1);
    assert_eq!(wb[0]["key"], "contract:api");
    sum.reply(finish("coordinated summary"));

    let done = attached.json();
    assert_eq!(done["status"], "done");
    assert_eq!(done["result"], "coordinated summary");
    assert_eq!(done["whiteboard"].as_array().unwrap().len(), 1);

    let cli_read = lab.json(&["whiteboard", &root_id]);
    assert_eq!(cli_read["entries"][0]["key"], "contract:api");
    let cli_write = lab.json(&["whiteboard", &root_id, "set", "user:signoff", "ok"]);
    assert_eq!(cli_write["ok"], true);
    let cli_alias = lab.json(&["wb", &root_id]);
    assert_eq!(cli_alias["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn staged_flag_and_auto_compaction_recover_from_overflow() {
    let lab = Lab::new();

    // Default mode is direct workspace-write.
    let direct = lab.client(&["exec", "direct write check"]);
    let dt1 = lab.call();
    let dt_id = dt1.id.clone();
    dt1.reply(shell("echo DIRECT > direct.txt"));
    let dt2 = lab.next(&dt_id);
    dt2.observed("shell", "");
    dt2.reply(finish("direct ok"));
    let dj = direct.json();
    assert_eq!(dj["sandbox"], "direct");
    assert_eq!(fs::read_to_string(lab.ws.join("direct.txt")).unwrap().trim(), "DIRECT");

    // `--staged` opts into staged copy-on-exit publication.
    let staged = lab.client(&["exec", "--staged", "staged write check"]);
    let st1 = lab.call();
    let st_id = st1.id.clone();
    st1.reply(shell("echo STAGED > staged.txt"));
    let st2 = lab.next(&st_id);
    st2.observed("shell", "");
    st2.reply(finish("staged ok"));
    let sj = staged.json();
    assert_eq!(sj["sandbox"], "staged");
    assert_eq!(fs::read_to_string(lab.ws.join("staged.txt")).unwrap().trim(), "STAGED");

    // Build >= 6 history messages, trigger HTTP 413 overflow -> automatic compaction -> recovery.
    let comp = lab.client(&["exec", "Run steps then recover from overflow"]);
    let mut turn = lab.call();
    let tid = turn.id.clone();
    for step in 1..=3 {
        turn.reply(shell(&format!("echo step-{step}")));
        turn = lab.next(&tid);
        turn.observed("shell", &format!("step-{step}"));
    }
    let overflow = json!({
        "error": {"code": "context_length_exceeded", "message": "maximum context length exceeded"}
    })
    .to_string();
    write!(
        turn.socket,
        "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{overflow}",
        overflow.len()
    )
    .unwrap();

    let compact_turn = lab.call();
    assert!(compact_turn.contains("Summarize these"));
    compact_turn.reply(finish("Compacted summary of steps 1..3"));

    let resumed = lab.next(&tid);
    assert!(resumed.contains("compacted_history") && resumed.contains("Compacted summary of steps 1..3"));
    resumed.reply(finish("recovered after compaction"));

    let out = comp.json();
    assert_eq!(out["status"], "done");
    assert_eq!(out["result"], "recovered after compaction");
}

fn accept_raw(socket: &TcpListener) -> TcpStream {
    let until = Instant::now() + LIMIT;
    loop {
        match socket.accept() {
            Ok((s, _)) => return s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => thread::sleep(POLL),
            Err(e) => panic!("expected provider socket accept ({e})"),
        }
    }
}

fn sse_frames(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\r\ndata: {e}\r\n\r\n", e["type"].as_str().unwrap())).collect()
}

#[test]
fn responses_provider_handles_commentary_and_tools_end_to_end() {
    let lab = Lab::new_with(&[("TEAMAGENTS_PROVIDER", "responses")], true);
    let exec = lab.client(&["exec", "Create hello.txt via Responses API"]);

    // Turn 1: SSE with commentary + native function_call(shell).
    let s1 = accept_raw(&lab.socket);
    let (_, req1, mut s1) = Call::read_http(s1, "/v1/responses");
    assert_eq!(req1["stream"], true);
    assert_eq!(req1["tool_choice"], "required");
    assert_eq!(req1["parallel_tool_calls"], false);
    assert!(req1["tools"].as_array().unwrap().iter().any(|t| t["name"] == "edit_file"));
    let fc_args = json!({"command":"echo RESPONSES_OK > hello.txt"}).to_string();
    let sse1 = sse_frames(&[
        json!({"type":"response.created","response":{"id":"resp1","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"c1","role":"assistant","phase":"commentary","status":"in_progress","content":[]}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"c1","delta":"Planning command"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"c1","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"Planning command"}]}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call_1","name":"shell","arguments":"","status":"in_progress"}}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc1","delta":&fc_args[..12]}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc1","delta":&fc_args[12..]}),
        json!({"type":"response.function_call_arguments.done","output_index":1,"item_id":"fc1","arguments":fc_args}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call_1","name":"shell","arguments":fc_args,"status":"completed"}}),
        json!({"type":"response.completed","response":{"id":"resp1","status":"completed","output":[
            {"type":"message","id":"c1","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"Planning command"}]},
            {"type":"function_call","id":"fc1","call_id":"call_1","name":"shell","arguments":fc_args,"status":"completed"}
        ]}}),
    ]);
    write!(
        s1,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse1}",
        sse1.len()
    )
    .unwrap();

    // Turn 2: commentary-only triggers InvalidAction protocol retry without crashing, and history replays natively.
    let s2 = accept_raw(&lab.socket);
    let (_, req2, mut s2) = Call::read_http(s2, "/v1/responses");
    let input2 = req2["input"].as_array().unwrap();
    assert!(
        input2.iter().any(|m| m["type"] == "function_call" && m["name"] == "shell" && m["call_id"] == "call_1")
            && input2.iter().any(|m| {
                m["type"] == "function_call_output"
                    && m["call_id"] == "call_1"
                    && m["output"].as_str().is_some_and(|c| c.contains("\"tool\":\"shell\""))
            })
            && !input2.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("Original request:")))
    );
    let sse2 = sse_frames(&[
        json!({"type":"response.completed","response":{"id":"resp2","status":"completed","output":[
            {"type":"message","id":"c2","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"Oops forgot tool call"}]}
        ]}}),
    ]);
    write!(
        s2,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse2}",
        sse2.len()
    )
    .unwrap();

    let s3 = accept_raw(&lab.socket);
    let (_, req3, mut s3) = Call::read_http(s3, "/v1/responses");
    assert!(req3["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["content"].as_str().is_some_and(|c| c.contains("protocol_error"))));
    let finish_json = json!({
        "id": "resp3",
        "status": "completed",
        "output": [{"type":"function_call","id":"fc3","call_id":"call_3","name":"finish","arguments":r#"{"text":"responses done"}"#,"status":"completed"}]
    })
    .to_string();
    write!(s3, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{finish_json}", finish_json.len()).unwrap();

    let out = exec.json();
    assert_eq!(out["status"], "done");
    assert_eq!(out["result"], "responses done");
    assert_eq!(fs::read_to_string(lab.ws.join("hello.txt")).unwrap().trim(), "RESPONSES_OK");
}

#[test]
fn anthropic_provider_handles_thinking_stream_and_tools_end_to_end() {
    let lab = Lab::new_with(&[("ANTHROPIC_BASE_URL", "{URL}"), ("ANTHROPIC_API_KEY", "ant-secret")], false);
    assert!(lab.json(&["exec", "--detach", "Check provider"])["id"].as_str().is_some());
    let s0 = accept_raw(&lab.socket);
    let (h0, body0, mut s0) = Call::read_http(s0, "/v1/messages");
    assert!(h0.to_ascii_lowercase().contains("x-api-key: ant-secret"));
    assert!(h0.to_ascii_lowercase().contains("anthropic-version: 2023-06-01"));
    assert!(body0["system"].as_str().unwrap().contains("root worker"));
    assert_eq!(body0["tool_choice"], json!({"type":"any","disable_parallel_tool_use":true}));
    assert!(body0["tools"].as_array().unwrap().iter().any(|t| t["name"] == "edit_file"));
    let sse0 = sse_frames(&[
        json!({"type":"message_start","message":{"id":"a1","type":"message","role":"assistant","stop_reason":null}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"pondering"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"shell","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":r#"{"command":"echo ANT_OK > ant.txt"}"#}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    write!(
        s0,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse0}",
        sse0.len()
    )
    .unwrap();

    let s1 = accept_raw(&lab.socket);
    let (_, body1, mut s1) = Call::read_http(s1, "/v1/messages");
    // Coalesced messages keep strict alternating roles and replay native tool_use + tool_result pairs!
    let msgs1 = body1["messages"].as_array().unwrap();
    for pair in msgs1.windows(2) {
        assert_ne!(pair[0]["role"], pair[1]["role"]);
    }
    assert!(
        msgs1.iter().any(|m| {
            m["role"] == "assistant"
                && m["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b["type"] == "tool_use" && b["name"] == "shell" && b["id"] == "call_1")
        }) && msgs1.iter().any(|m| {
            m["role"] == "user"
                && m["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_1")
        })
    );
    let json1 = json!({
        "type": "message",
        "role": "assistant",
        "stop_reason": "tool_use",
        "content": [{"type":"tool_use","id":"tu2","name":"finish","input":{"text":"anthropic done"}}]
    })
    .to_string();
    write!(
        s1,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json1}",
        json1.len()
    )
    .unwrap();

    // Verify workspace file written and session completed cleanly.
    let until = Instant::now() + LIMIT;
    while lab.json(&["tasks"])[0]["status"] != "done" {
        assert!(Instant::now() < until);
        thread::sleep(POLL);
    }
    assert_eq!(lab.json(&["tasks"])[0]["result"], "anthropic done");
    assert_eq!(fs::read_to_string(lab.ws.join("ant.txt")).unwrap().trim(), "ANT_OK");
}

#[test]
fn api_provider_handles_native_tool_calls_end_to_end() {
    let lab = Lab::new();
    let exec = lab.client(&["exec", "Create via native tool_calls"]);

    let s1 = accept_raw(&lab.socket);
    let (_, body1, mut s1) = Call::read_http(s1, "/v1/chat/completions");
    assert_eq!(body1["tool_choice"], "required");
    assert_eq!(body1["parallel_tool_calls"], false);
    assert!(body1["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "edit_file"));
    let r1 = json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "tc1",
                    "type": "function",
                    "function": {"name": "edit_file", "arguments": r#"{"path":"note.txt","old":"","new":"CHAT_TOOL_OK"}"#}
                }]
            }
        }]
    })
    .to_string();
    write!(
        s1,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{r1}",
        r1.len()
    )
    .unwrap();

    let s2 = accept_raw(&lab.socket);
    let (_, body2, mut s2) = Call::read_http(s2, "/v1/chat/completions");
    let msgs2 = body2["messages"].as_array().unwrap();
    assert!(
        msgs2.windows(2).any(|w| {
            w[0]["role"] == "assistant"
                && w[0]["tool_calls"][0]["id"] == "call_1"
                && w[0]["tool_calls"][0]["function"]["name"] == "edit_file"
                && w[1]["role"] == "tool"
                && w[1]["tool_call_id"] == "call_1"
        }),
        "expected native assistant tool_calls + tool pair, got: {msgs2:?}"
    );
    let r2 = json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "tc2",
                    "type": "function",
                    "function": {"name": "finish", "arguments": r#"{"text":"chat tool ok"}"#}
                }]
            }
        }]
    })
    .to_string();
    write!(
        s2,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{r2}",
        r2.len()
    )
    .unwrap();

    let out = exec.json();
    assert_eq!(out["status"], "done");
    assert_eq!(out["result"], "chat tool ok");
    assert_eq!(fs::read_to_string(lab.ws.join("note.txt")).unwrap(), "CHAT_TOOL_OK");
}

#[test]
fn quota_exceeded_pauses_remaining_turns_and_allows_continue() {
    let lab = Lab::new();
    let exec = lab.client(&["exec", "Split into two branches"]);
    let root = lab.call();
    let root_id = root.id.clone();
    root.reply(handoff("split", &[("a", "Branch A"), ("b", "Branch B")]));

    let c1 = lab.call();
    let c2 = lab.call();
    let (a, b) = if c1.agent == "a" { (c1, c2) } else { (c2, c1) };
    // Branch A fails with 429 insufficient_quota (activating 30s daemon quota pause); Branch B finishes normally.
    let quota_body = json!({"error":{"code":"insufficient_quota","message":"billing exhausted"}}).to_string();
    write!(
        { a }.socket,
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{quota_body}",
        quota_body.len()
    )
    .unwrap();
    b.reply(finish("Branch B succeeded"));

    // Reporter turn MUST be short-circuited by daemon quota_pause without hitting lab.socket!
    let settled = exec.output();
    assert!(!settled.status.success());
    let status = lab.status(&root_id);
    assert_eq!(status["status"], "failed");
    assert!(
        status["result"].as_str().unwrap().contains("quota limit reached"),
        "expected quota pause fallback, got: {status}"
    );
    assert!(matches!(lab.socket.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));

    // Continuing after top-up clears quota_pause and completes cleanly.
    let cont = lab.client(&["continue", &root_id, "Resume after top-up"]);
    let sum_resume = lab.call();
    sum_resume.reply(finish("Resumed after top-up"));
    let resumed = cont.json();
    assert_eq!(resumed["status"], "done");
    assert_eq!(resumed["result"], "Resumed after top-up");
}
