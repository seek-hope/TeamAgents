use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn request(conn: &mut UnixStream) -> Value {
    let mut reader = BufReader::new(&*conn);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["protocol"], 3);
    assert_eq!(hello["body"], json!({"op":"hello"}));
    writeln!(reader.get_mut(), "{}", json!({"value":{"protocol":3}})).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["protocol"], 3);
    assert_eq!(frame["workspace"], hello["workspace"]);
    frame["body"].clone()
}

struct Lab(PathBuf);

impl Lab {
    fn new() -> (Self, UnixListener) {
        let root = std::env::temp_dir().join(format!("chat-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir(root.join("work")).unwrap();
        let socket = UnixListener::bind(root.join("state/swarm.sock")).unwrap();
        (Self(root), socket)
    }

    fn client(&self, cmd: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        command
            .env_clear()
            .args([cmd, "--session"])
            .arg(self.0.join("state"))
            .arg("--workspace")
            .arg(self.0.join("work"));
        command
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unattended_chat_shows_only_final_feedback() {
    exercise_chat(false);
}

#[test]
fn review_chat_shows_child_approval_and_final_feedback() {
    exercise_chat(true);
}

fn exercise_chat(review: bool) {
    let (lab, socket) = Lab::new();
    let mut command = lab.client("chat");
    if review {
        command.arg("--review");
    }
    let mut chat = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = chat.stdin.take().unwrap();
    let stdout = chat.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            tx.send(line.unwrap()).unwrap();
        }
    });
    let until = Instant::now() + Duration::from_secs(10);
    let expect_line = |text: &str| loop {
        let line = lines
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|e| panic!("waiting for {text:?}: {e}"));
        assert!(
            !line.contains("[waiting]") && !line.contains("intermediate action") && !line.contains("private contents"),
            "{line}"
        );
        assert!(review || !line.contains("Submitted"), "{line}");
        if line.contains(text) {
            break;
        }
    };
    let peer = thread::spawn(move || {
        let mut rows = json!([]);
        let final_row =
            json!({"id":"root","agent":"worker","status":"done","stage_status":"done","result":"fixed parser"});
        loop {
            let (mut conn, _) = socket.accept().unwrap();
            let body = request(&mut conn);
            let op = body["op"].as_str().unwrap();
            let reply = match op {
                "tasks" => {
                    let snapshot = rows.clone();
                    if !review && rows[0]["status"] == "waiting" {
                        rows = json!([final_row]);
                    }
                    snapshot
                }
                "submit" => {
                    assert_eq!(body, json!({"op":"submit","agent":"worker","text":"run checks","review":review}));
                    rows = if review {
                        json!([{"id":"child","parent":"root","agent":"helper","status":"approval","approval_id":"approval-7","action":{"op":"shell","command":"cargo test"}}])
                    } else {
                        json!([{"id":"root","agent":"worker","status":"waiting","action":{"op":"shell","command":"intermediate action"}}, {"id":"child","parent":"root","status":"done","result":"private contents"}])
                    };
                    json!({"id":"root"})
                }
                "approve" => {
                    assert!(review);
                    assert_eq!(body["id"], "approval-7");
                    rows = json!([final_row]);
                    json!({"ok":true})
                }
                "status" => {
                    assert_eq!(body["id"], "root");
                    let mut row = final_row.clone();
                    row["evidence"] = json!([{ "action":{"op":"read_file","path":"file"}, "ok":true, "result":"private contents"}, {"action":{"op":"shell","command":"cargo test"}, "ok":true, "result":"12 tests passed"}]);
                    row["leaf_results"] =
                        json!([{"task":"child","agent":"helper","status":"done","result":"tokenized input"}]);
                    row["subtree_receipts"] = json!([{"task":"child","agent":"helper","op":"shell","ok":true}]);
                    row
                }
                _ => panic!("unexpected request: {op}"),
            };
            writeln!(conn, "{}", json!({"value":reply})).unwrap();
            if op == "status" {
                break;
            }
        }
    });
    writeln!(input, "/to worker\nrun checks").unwrap();
    if review {
        expect_line("child [approval]");
        expect_line("cargo test");
        expect_line("approval_id: approval-7");
        writeln!(input, "/approve approval-7").unwrap();
    }
    expect_line("root aggregate: done | worker stage: done");
    expect_line("Model report: fixed parser");
    expect_line("child helper [done] tokenized input");
    expect_line("Model reports alone");
    expect_line("Observed shell [ok] cargo test — 12 tests passed");
    expect_line("child helper: shell [ok]");
    writeln!(input, "/quit").unwrap();
    loop {
        if let Some(status) = chat.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= until {
            let _ = chat.kill();
            let _ = chat.wait();
            panic!("/quit did not exit");
        }
        thread::sleep(Duration::from_millis(10));
    }
    peer.join().unwrap();
    reader.join().unwrap();
}

#[test]
fn reconnect_announces_questions_and_routes_only_unambiguous_replies() {
    let (lab, socket) = Lab::new();
    let peer = thread::spawn(move || {
        let mut tasks = vec![
            json!({"id":"first","agent":"default","status":"input","question":"Which target?"}),
            json!({"id":"second","agent":"default","status":"input","question":"Which format?\u{1b}[2J"}),
        ];
        let mut answers = 0;
        loop {
            let (mut conn, _) = socket.accept().unwrap();
            let body = request(&mut conn);
            let reply = match body["op"].as_str().unwrap() {
                "tasks" => json!(tasks),
                "answer" => {
                    let (id, text) = [("second", "plain  text"), ("first", "Linux")][answers];
                    assert_eq!(body, json!({"op":"answer","id":id,"text":text}));
                    tasks.retain(|t| t["id"] != id);
                    answers += 1;
                    json!({"ok":true})
                }
                "submit" => {
                    assert_eq!(answers, 2);
                    assert_eq!(body["text"], "separate task");
                    writeln!(conn, "{}", json!({"value":{"id":"third"}})).unwrap();
                    break;
                }
                op => panic!("unexpected {op}"),
            };
            writeln!(conn, "{}", json!({"value":reply})).unwrap();
        }
    });
    let mut chat =
        lab.client("chat").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    // The initial snapshot and its announcements precede any stdin processing.
    writeln!(chat.stdin.take().unwrap(), "ambiguous\n/answer second plain  text\nLinux\n/new separate task\n/quit")
        .unwrap();
    let output = chat.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for msg in ["Question: Which target?", r"Question: Which format?\u001b[2J", "/answer second TEXT"] {
        assert!(stdout.contains(msg), "{stdout}");
    }
    assert!(!stdout.contains('\u{1b}'));
    assert!(String::from_utf8(output.stderr).unwrap().contains("Specify /answer ROOT_ID TEXT"));
    peer.join().unwrap();
}

#[test]
fn attached_exec_returns_waiting_question_and_cli_answer_resumes_it() {
    let (lab, socket) = Lab::new();
    let peer = thread::spawn(move || {
        for op in ["submit", "status", "answer"] {
            let (mut conn, _) = socket.accept().unwrap();
            let body = request(&mut conn);
            assert_eq!(body["op"], op);
            let reply = match op {
                "submit" => json!({"id":"root"}),
                "status" => json!({"id":"root","status":"input","question":"Which output format?"}),
                "answer" => {
                    assert_eq!(body, json!({"op":"answer","id":"root","text":"plain text"}));
                    json!({"ok":true})
                }
                _ => unreachable!(),
            };
            writeln!(conn, "{}", json!({"value":reply})).unwrap();
        }
    });
    let exec = lab.client("exec").args(["--json", "--timeout", "5", "Export this"]).output().unwrap();
    assert!(!exec.status.success());
    let status: Value = serde_json::from_slice(&exec.stdout).unwrap();
    assert_eq!(status["status"], "input");
    assert_eq!(status["question"], "Which output format?");
    assert!(String::from_utf8(exec.stderr).unwrap().contains("teamagents answer root TEXT"));
    let reply = lab.client("answer").args(["--json", "root", "plain", "text"]).output().unwrap();
    assert!(reply.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&reply.stdout).unwrap(), json!({"ok":true}));
    peer.join().unwrap();
}
