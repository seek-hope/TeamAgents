use super::{Command, Daemon, Scratch};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teamagents_engine::v2::exec::Client;

#[test]
fn directory_refresh_changes_only_new_requests_and_rollback_restores_metadata() {
    let root = Scratch::new("catalog-refresh");
    let config = root.join("cfg");
    let state = root.join("state");
    let session = root.join("s");
    let workspace = root.join("ws");
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let row = |id: &str, window: u64, images: bool| json!({"id":id,"architecture":{"input_modalities":if images{json!(["text","image"])}else{json!(["text"])},"output_modalities":["text"]},"supported_parameters":["tools"],"context_length":window});
    let initial = json!({"data":[row("a",100000,false),row("b",80000,true),row("override",1200,true)]});
    let changed = json!({"data":[row("a",200000,true),row("override",2000,true)]});
    let directory = Arc::new(Mutex::new(initial));
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let serving = directory.clone();
    let log = calls.clone();
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let live = running.clone();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        while live.load(std::sync::atomic::Ordering::Relaxed) {
            let (mut socket, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("accept: {e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = BufReader::new(socket.try_clone().unwrap());
            let mut first = String::new();
            request.read_line(&mut first).unwrap();
            let mut size = 0;
            let mut credential = false;
            loop {
                let mut header = String::new();
                let n = request.read_line(&mut header).unwrap();
                if n == 0 || header == "\r\n" {
                    break;
                }
                let lower = header.to_ascii_lowercase();
                if let Some(len) = lower.strip_prefix("content-length:") {
                    size = len.trim().parse().unwrap()
                }
                if lower.trim() == "authorization: bearer fixture" {
                    credential = true
                }
            }
            assert!(credential, "expected the seed's test-only credential");
            let (mime, body) = if first.starts_with("GET /v1/models ") {
                ("application/json", serving.lock().unwrap().to_string())
            } else {
                assert!(first.starts_with("POST /v1/chat/completions "), "{first}");
                let mut buf = vec![0; size];
                request.read_exact(&mut buf).unwrap();
                let mut calls = log.lock().unwrap();
                calls.push(serde_json::from_slice(&buf).unwrap());
                let delta = if calls.len() == 1 {
                    json!({"tool_calls":[{"index":0,"id":"f","type":"function","function":{"name":"finish","arguments":json!({"status":"success","summary":"answered"}).to_string()}}]})
                } else {
                    json!({"content":"answer"})
                };
                let event =
                    json!({"choices":[{"delta":delta,"finish_reason":if calls.len()==1{"tool_calls"}else{"stop"}}]});
                ("text/event-stream", format!("data: {event}\n\ndata: [DONE]\n\n"))
            };
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    struct Stop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let stop = Stop(running);
    let text=format!("[catalog_sources]\nlab='seed'\n[models.seed]\nprovider='fixture'\nmodel='seed'\nbase_url='{base}'\napi_key_env='TA_TEST_CATALOG_TOKEN'\n[models.\"lab::override\"]\nprovider='fixture'\nmodel='override'\nbase_url='{base}'\napi_key_env='TA_TEST_CATALOG_TOKEN'\ncontext_window=100000\nimages=false\n");
    let file = config.join("teamagents/config.toml");
    std::fs::write(&file, &text).unwrap();
    let command = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        c.env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("TA_TEST_CATALOG_TOKEN", "fixture")
            .current_dir(&workspace);
        c
    };
    let catalog = |verb: &str, ok: bool| {
        let result = command().args(["catalog", verb, "--name", "lab", "--json"]).output().unwrap();
        assert_eq!(result.status.success(), ok, "{result:?}");
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        report["sources"]["lab"].clone()
    };
    assert_eq!(catalog("list", true)["version"], 0);
    assert_eq!(catalog("refresh", true)["changes"]["a"]["after"]["window"], 100000);
    assert_eq!(catalog("refresh", true)["version"], 1);
    let mut daemon = Daemon(
        command()
            .args(["daemon", "--model", "lab::a", "--state-root"])
            .arg(&session)
            .arg("--cwd")
            .arg(&workspace)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    // The same startup budget as `cli.rs`'s other daemon waits: a loaded runner needs longer than five
    // seconds to open the store and reach the accept loop, and this wait is not asserting a product timeout.
    let deadline = Instant::now() + super::DAEMON_START;
    let mut client = loop {
        if let Ok(c) = Client::connect(&session.join("daemon.sock")) {
            break c;
        }
        assert!(Instant::now() < deadline && daemon.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(10));
    };
    let send = |c: &mut Client, i: usize| {
        let id = format!("q-{i}");
        c.command(
            &id,
            "submit_input",
            json!({"instance_id":"i-leader","envelope_id":id,"text":format!("question {i}")}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if c.events().unwrap().iter().any(|e| e["kind"] == "response_imported") {
                break;
            }
            assert!(Instant::now() < deadline, "no reply for {id}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = calls.lock().unwrap();
        assert_eq!(requests.len(), i);
        requests.last().unwrap().clone()
    };
    let first = send(&mut client, 1);
    assert_eq!(first["model"], "a");
    let has_view = |body: &Value| body["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "image");
    assert!(!has_view(&first));
    *directory.lock().unwrap() = changed;
    let report = catalog("refresh", true);
    assert_eq!(report["version"], 2);
    assert_eq!(report["changes"]["b"]["after"]["missing"], true);
    let second = send(&mut client, 2);
    assert_eq!(second["model"], "a");
    assert_eq!(report["changes"]["a"]["after"]["images"], true);
    assert!(has_view(&second));
    *directory.lock().unwrap() = json!({"data":[]});
    assert!(catalog("refresh", false)["error"].is_string());
    assert_eq!(catalog("list", true)["version"], 2);
    assert_eq!(catalog("rollback", true)["version"], 3);
    let third = send(&mut client, 3);
    assert!(!has_view(&third));
    client
        .command("choose-override", "set_instance_profile", json!({"instance_id":"i-leader","model":"lab::override"}))
        .unwrap();
    let fourth = send(&mut client, 4);
    assert_eq!(fourth["model"], "override");
    assert!(!has_view(&fourth));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    drop(daemon);
    drop(stop);
    server.join().unwrap();
}
