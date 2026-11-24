//! Exercise the installed interface without credentials or a model server.
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use uuid::Uuid;

struct Session {
    dir: PathBuf,
    state: PathBuf,
    work: PathBuf,
}

impl Session {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ta-control-{}", Uuid::new_v4()));
        let state = dir.join("long-session-component".repeat(7));
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        Self { dir, state, work }
    }

    fn run(&self, workspace: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.dir)
            .env("TEAMAGENTS_MAX_AGENTS", "3")
            .env("TEAMAGENTS_CONCURRENCY", "2")
            .env("TEAMAGENTS_TURN_BUDGET", "12")
            .args(["--session"])
            .arg(&self.state)
            .arg("--workspace")
            .arg(workspace)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.run(&self.work, &["stop"]);
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn json(output: Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn concurrent_start_workspace_binding_and_clean_restart() {
    let session = Session::new();
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..4).map(|_| s.spawn(|| session.run(&session.work, &["members", "--json"]))).collect();
        for run in runs {
            assert_eq!(json(run.join().unwrap())[0]["name"], "default");
        }
    });
    let other = session.dir.join("another-project");
    fs::create_dir(&other).unwrap();
    let mismatch = session.run(&other, &["members"]);
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("another workspace"));
    let duplicate = session.run(&session.work, &["daemon"]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already served"));
    for file in ["swarm.sqlite3", "swarm.sock"] {
        assert_eq!(session.state.join(file).metadata().unwrap().mode() & 0o077, 0);
    }
    assert!(session.run(&session.work, &["stop"]).status.success());
    let mismatch = session.run(&other, &["members"]);
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("another workspace"));
    assert_eq!(json(session.run(&session.work, &["members", "--json"]))[0]["name"], "default");
}

#[test]
fn refuse_overlap_foreign_data_and_unsafe_state_directory() {
    let mut session = Session::new();
    session.state = session.work.join("nested-state");
    assert!(!session.run(&session.work, &["members"]).status.success());
    assert!(!session.state.join("swarm.sqlite3").exists());
    session.state = session.dir.join("foreign");
    fs::create_dir(&session.state).unwrap();
    fs::set_permissions(&session.state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!session.run(&session.work, &["members"]).status.success());
    assert!(!session.state.join("swarm.sqlite3").exists());
    fs::set_permissions(&session.state, fs::Permissions::from_mode(0o700)).unwrap();
    let db_path = session.state.join("swarm.sqlite3");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch("CREATE TABLE foreign_app(id); INSERT INTO foreign_app VALUES(123);").unwrap();
    drop(db);
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(&db_path).unwrap();
    assert!(!session.run(&session.work, &["members"]).status.success());
    assert_eq!(fs::read(&db_path).unwrap(), before);
}
