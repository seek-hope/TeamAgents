//! D-446: real CLI receipts for history paging, opening and deletion; no model requests.
use serde_json::{json, Value as Json};
use std::{path::PathBuf, process::Command};
use teamagents_engine::v2::{session_browser, sessions::Registry};

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("ta-session-ui-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        let root = dir.join("base");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(dir.join("config/teamagents")).unwrap();
        std::fs::write(dir.join("config/teamagents/config.toml"),
            "skills_paths = []\n[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_SESSION_UI_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ncontext_window = 128000\n").unwrap();
        Self { dir, root }
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("--state-root")
            .arg(&self.root)
            .args(args)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("TA_SESSION_UI_KEY", "no-model-called")
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Json {
        let output = self.run(args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(registry) = Registry::load(&self.root) {
            for (_, dir) in registry.rows() {
                let _ = Command::new(env!("CARGO_BIN_EXE_teamagents"))
                    .arg("--state-root")
                    .arg(dir)
                    .args(["daemon", "--stop"])
                    .output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn all_history_pages_include_archived_members_and_survive_new_appends() {
    let fixture = Fixture::new();
    let mut registry = Registry::load(&fixture.root).unwrap();
    let entry = registry.new_session("old conversation").unwrap();
    let db = entry.dir(&fixture.root).join("session.sqlite");
    let conn = teamagents_core::v2::store::open(&db, true).unwrap();
    conn.execute("INSERT INTO instances (id,session_id,profile_revision,workspace_ref,lifecycle,phase,profile_json) VALUES ('i-worker','s-history',1,'/workspace/worker','ACTIVE','READY','{}')", []).unwrap();
    for idx in 1..=450 {
        conn.execute("INSERT INTO context_entries (id,instance_id,epoch,idx,kind,message_json,created) VALUES (?1,?2,?3,?4,'user',?5,0)",
            rusqlite::params![format!("entry-{idx}"), if idx%2==0 {"i-leader"} else {"i-worker"}, idx/200, idx,
                json!({"content":format!("record {idx}")}).to_string()]).unwrap();
    }
    drop(conn);
    registry.archive(&entry.id).unwrap();
    let db = registry.find(&entry.id).unwrap().dir(&fixture.root).join("session.sqlite");
    let bytes = std::fs::read(&db).unwrap();
    let first = fixture.json(&["sessions", "history", "--id", &entry.id, "--json"]);
    assert_eq!(first["entries"].as_array().unwrap().len(), 200);
    assert_eq!(first["entries"][0]["idx"], 251);
    assert_eq!(first["entries"][0]["workspace"], "/workspace/worker");
    assert!(first["entries"][1]["workspace"].is_null(), "missing members have no invented workspace");
    assert_eq!(bytes, std::fs::read(&db).unwrap(), "reading does not rewrite the database");
    assert!(!db.parent().unwrap().join("daemon.sock").exists());
    let conn = teamagents_core::v2::store::open(&db, false).unwrap();
    conn.execute("INSERT INTO context_entries (id,instance_id,epoch,idx,kind,message_json,created) VALUES ('new','i-leader',2,451,'user','{}',0)", []).unwrap();
    drop(conn);
    let cursor = first["next_before"].to_string();
    let second = fixture.json(&["sessions", "history", "--id", &entry.id, "--before", &cursor, "--json"]);
    assert_eq!(second["entries"][0]["idx"], 51);
    let cursor = second["next_before"].to_string();
    let last = fixture.json(&["sessions", "history", "--id", &entry.id, "--before", &cursor, "--json"]);
    assert_eq!(last["entries"].as_array().unwrap().len(), 50);
    assert_eq!(last["entries"][0]["idx"], 1);
    assert!(last["next_before"].is_null());
    let records: Vec<_> = last["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["entries"].as_array().unwrap())
        .chain(first["entries"].as_array().unwrap())
        .map(|row| row["idx"].as_i64().unwrap())
        .collect();
    assert_eq!(records, (1..=450).collect::<Vec<_>>());
    for args in [
        vec!["sessions", "history", "--limit", "0"],
        vec!["sessions", "history", "--before", "0"],
        vec!["sessions", "list", "--before", "2"],
        vec!["sessions", "list", "--stop"],
    ] {
        assert!(!fixture.run(&args).status.success());
    }
}

#[test]
fn search_and_export_cli_are_current_read_only_surfaces() {
    let fixture = Fixture::new();
    let db = fixture.root.join("session.sqlite");
    let conn = teamagents_core::v2::store::open(&db, true).unwrap();
    conn.execute(
        "INSERT INTO instances (id,session_id,profile_revision,workspace_ref,lifecycle,phase,profile_json) VALUES ('i-leader','s-main',1,'/workspace','ACTIVE','READY','{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO context_entries (id,instance_id,epoch,idx,kind,message_json,created) VALUES ('entry-search','i-leader',0,1,'user',?1,0)",
        [json!({"content":"Unicode needle λ and <script>alert(1)</script>"}).to_string()],
    )
    .unwrap();
    drop(conn);

    let registry_file = fixture.root.join("sessions.json");
    let registry_before = std::fs::read(&registry_file).ok();
    let search = fixture.json(&["sessions", "search", "--query", "needle", "--limit", "10", "--json"]);
    assert_eq!(search["hits"].as_array().unwrap().len(), 1);
    assert!(search["hits"][0]["snippet"].as_str().unwrap().contains("needle"));
    assert_eq!(registry_before, std::fs::read(&registry_file).ok());
    assert!(!fixture.root.join("sessions.lock").exists(), "read-only search must not acquire a writer lock");

    let json_path = fixture.dir.join("export.json");
    let json_path_string = json_path.to_string_lossy().into_owned();
    let exported = fixture.json(&[
        "sessions",
        "export",
        "--id",
        "default",
        "--output",
        &json_path_string,
        "--format",
        "json",
        "--json",
    ]);
    assert_eq!(exported["complete"], true);
    let body: Json = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(body["snapshot"]["session_id"], "default");
    assert!(body["context_entries"][0]["message"]["content"].as_str().unwrap().contains("needle"));
    assert!(body["context_entries"][0]["message"]["content"].as_str().unwrap().contains("<script>"));
    assert!(
        !fixture.run(&["sessions", "export", "--id", "default", "--output", &json_path_string,]).status.success(),
        "export must refuse to overwrite"
    );

    let markdown_path = fixture.dir.join("export.md");
    let markdown_path_string = markdown_path.to_string_lossy().into_owned();
    let markdown = fixture.json(&[
        "sessions",
        "export",
        "--id",
        "default",
        "--output",
        &markdown_path_string,
        "--format",
        "markdown",
        "--json",
    ]);
    assert_eq!(markdown["complete"], true);
    let markdown_body = std::fs::read_to_string(&markdown_path).unwrap();
    assert!(markdown_body.starts_with("# Session export"));
    assert!(markdown_body.contains("Unicode needle λ and <script>alert(1)</script>"));

    let incompatible = fixture.run(&["sessions", "export", "--output", &markdown_path_string, "--cwd", "."]);
    assert!(!incompatible.status.success());
    assert!(String::from_utf8_lossy(&incompatible.stderr).contains("overrides do not apply"));
}

#[test]
fn export_ctrl_c_reaps_worker_and_removes_unpublished_temporary() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let conn = teamagents_core::v2::store::open(&fixture.root.join("session.sqlite"), true).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    let message = json!({"content":"source line ".repeat(45000)}).to_string();
    for idx in 1..=80 {
        conn.execute("INSERT INTO context_entries (id,instance_id,epoch,idx,kind,message_json,created) VALUES (?1,'i-leader',0,?2,'user',?3,0)",
            rusqlite::params![format!("signal-{idx}"),idx,message]).unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    drop(conn);
    let output = fixture.dir.join("cancelled.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("--state-root")
        .arg(&fixture.root)
        .args(["sessions", "export", "--output"])
        .arg(&output)
        .arg("--json")
        .env("XDG_CONFIG_HOME", fixture.dir.join("config"))
        .env("XDG_STATE_HOME", fixture.dir.join("state"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let temporary = || {
        std::fs::read_dir(&fixture.dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(".teamagents-export-"))
    };
    while !temporary() {
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = child.kill();
            let receipt = child.wait_with_output().unwrap();
            panic!("export ended before cancellation: {}", String::from_utf8_lossy(&receipt.stderr));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    // SAFETY: child.id is still our unreaped child; signal that exact process, not a reused PID.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("export ignored SIGINT");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let receipt = child.wait_with_output().unwrap();
    assert!(!receipt.status.success());
    assert!(String::from_utf8_lossy(&receipt.stderr).contains("cancelled"), "{receipt:?}");
    assert!(!output.exists());
    assert!(!temporary());
    // The same CLI can publish a subsequent complete snapshot and refuses to overwrite it.
    let next = fixture.json(&["sessions", "export", "--output", output.to_str().unwrap(), "--json"]);
    assert_eq!(next["complete"], true);
    let data: Json = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(data["context_entries"].as_array().unwrap().len(), 80);
}

#[test]
fn deleting_a_live_default_requires_confirmation_and_preserves_its_siblings() {
    let fixture = Fixture::new();
    let named = fixture.json(&["sessions", "new", "--name", "kept", "--json"]);
    std::fs::write(fixture.root.join("memory.jsonl"), "shared memory").unwrap();
    std::fs::write(fixture.root.join("keep.txt"), "unrelated").unwrap();
    fixture.json(&["sessions", "open", "--id", "default", "--json"]);
    assert!(fixture.run(&["daemon", "--stop"]).status.success());
    fixture.json(&["sessions", "open", "--id", "default", "--json"]);
    for args in [
        vec!["sessions", "delete", "--id", "default", "--yes"],
        vec!["sessions", "delete", "--id", "default", "--stop"],
    ] {
        assert!(!fixture.run(&args).status.success());
        assert!(fixture.root.join("session.sqlite").exists());
        assert!(teamagents_engine::v2::sessions::is_live(&fixture.root));
    }
    let report = fixture.json(&["sessions", "delete", "--id", "default", "--stop", "--yes", "--json"]);
    assert_eq!(report["deleted"], true);
    assert!(!fixture.root.join("session.sqlite").exists());
    assert!(!fixture.root.join("daemon.sock").exists());
    assert_eq!(std::fs::read_to_string(fixture.root.join("memory.jsonl")).unwrap(), "shared memory");
    assert!(fixture.root.join("keep.txt").exists());
    let id = named["id"].as_str().unwrap();
    assert!(Registry::load(&fixture.root).unwrap().resolve(id).unwrap().exists());
    fixture.json(&["sessions", "open", "--id", id, "--json"]);
    fixture.json(&["sessions", "delete", "--id", id, "--stop", "--yes", "--json"]);
    assert!(Registry::load(&fixture.root).unwrap().find(id).is_none());
    assert!(!fixture.root.join("sessions").join(id).exists());
}

#[test]
fn failed_deletion_keeps_history_and_refuses_path_aliases_and_workspaces() {
    let fixture = Fixture::new();
    let mut registry = Registry::load(&fixture.root).unwrap();
    std::fs::write(fixture.root.join("session.sqlite"), "history").unwrap();
    let lock = teamagents_engine::jobs::state_lock(&fixture.root.join("coordinator.lock")).unwrap();
    assert!(session_browser::delete(&mut registry, "default", true).is_err());
    assert_eq!(std::fs::read_to_string(fixture.root.join("session.sqlite")).unwrap(), "history");
    drop(lock);
    let entry = registry.new_session("aliased").unwrap();
    std::fs::remove_dir(entry.dir(&fixture.root)).unwrap();
    std::os::unix::fs::symlink(&fixture.root, entry.dir(&fixture.root)).unwrap();
    assert!(session_browser::delete(&mut registry, &entry.id, true).unwrap_err().contains("symlink"));
    assert!(fixture.root.join("session.sqlite").exists());
    std::fs::remove_file(entry.dir(&fixture.root)).unwrap();
    std::fs::create_dir_all(entry.dir(&fixture.root).join("instances/i-worker/work")).unwrap();
    assert!(session_browser::delete(&mut registry, &entry.id, true).unwrap_err().contains("workspace"));
    assert!(registry.find(&entry.id).is_some());
    let original = registry.sessions[0].clone();
    registry.sessions[0].id.clear();
    registry.sessions[0].path = "sessions".into();
    assert!(session_browser::delete(&mut registry, "", true).unwrap_err().contains("registered identity"));
    assert!(entry.dir(&fixture.root).exists());
    registry.sessions[0] = original;
    // The last action before shutdown can create a workspace after the initial preflight.
    // The fake daemon makes that ordering deterministic, rather than relying on a timed sleep.
    let socket = fixture.root.join("daemon.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let late = fixture.root.join("instances/i-late/work");
    let late_in_server = late.clone();
    let server = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        let (mut stream, _) = listener.accept().unwrap();
        writeln!(
            stream,
            "{}",
            json!({"server": "teamagents-daemon", "protocol_version": 1,
            "session_id": "s-main", "state_root": socket.parent().unwrap()})
        )
        .unwrap();
        let mut request = String::new();
        std::io::BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
        let request: Json = serde_json::from_str(&request).unwrap();
        assert_eq!(request["method"], "shutdown");
        std::fs::create_dir_all(&late_in_server).unwrap();
        std::fs::write(late_in_server.join("work.txt"), "unfinished work").unwrap();
        writeln!(stream, "{}", json!({"request_id": request["request_id"], "ok": true, "result": {}})).unwrap();
        drop(listener);
        std::fs::remove_file(socket).unwrap();
    });
    let error = session_browser::delete(&mut registry, "default", true).unwrap_err();
    server.join().unwrap();
    assert!(error.contains("workspace"), "{error}");
    assert!(late.join("work.txt").exists());
    assert!(fixture.root.join("session.sqlite").exists());
}
