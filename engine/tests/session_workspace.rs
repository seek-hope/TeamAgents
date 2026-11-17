//! Cross-directory session selection and restart receipts using an unavailable local model endpoint.
#[path = "../../test_scratch_owner.rs"]
mod scratch_owner;
use serde_json::Value as Json;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
use teamagents_engine::v2::{session_workspace, sessions::Registry};

struct Fixture {
    dir: PathBuf,
    base: PathBuf,
    a: PathBuf,
    b: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ta-ws-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        let base = dir.join("state/teamagents/v2");
        let a = dir.join("a");
        let b = dir.join("b");
        for path in [&a, &b, &dir.join("config/teamagents")] {
            std::fs::create_dir_all(path).unwrap();
        }
        scratch_owner::record_ownership(&dir);
        std::fs::write(dir.join("config/teamagents/config.toml"),
            "skills_paths = []\n[permissions]\ntrust_project = true\n[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_WORKSPACE_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ncontext_window = 128000\n").unwrap();
        let tui = dir.join("tui");
        std::fs::write(&tui, "#!/usr/bin/env python3\nimport socket,sys\nwith socket.socket(socket.AF_UNIX) as s:\n s.connect(sys.argv[sys.argv.index('--daemon')+1])\n print(s.makefile().readline(),end='')\n").unwrap();
        std::fs::set_permissions(tui, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir, base, a, b }
    }
    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        command
            .current_dir(cwd)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("TA_WORKSPACE_KEY", "no-model-requests")
            .env("TEAMAGENTS_TUI", self.dir.join("tui"));
        command
    }
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().unwrap()
    }
    fn json(&self, cwd: &Path, args: &[&str]) -> Json {
        let result = self.run(cwd, args);
        assert!(result.status.success(), "{args:?}: {}", String::from_utf8_lossy(&result.stderr));
        serde_json::from_slice(&result.stdout).unwrap()
    }
    fn stop(&self, root: &Path) {
        assert!(self.run(&self.b, &["--state-root", root.to_str().unwrap(), "daemon", "--stop"]).status.success());
    }
    fn stored(&self, root: &Path) -> String {
        let db = teamagents_core::v2::store::open_read_only(&root.join("session.sqlite")).unwrap();
        db.query_row("SELECT workspace_ref FROM instances WHERE id='i-leader'", [], |row| row.get(0)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(registry) = Registry::load(&self.base) {
            for (_, dir) in registry.rows() {
                let _ = self.command(&self.a).arg("--state-root").arg(dir).args(["daemon", "--stop"]).output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn ordinary_startup_restores_the_latest_open_session_in_each_workspace() {
    let f = Fixture::new();
    let first = f.json(&f.a, &[]);
    let other = f.json(&f.b, &[]);
    assert_eq!(first["workspace"], f.a.to_str().unwrap());
    assert_eq!(other["workspace"], f.b.to_str().unwrap());
    assert_ne!(first["state_root"], other["state_root"]);
    assert_eq!(f.json(&f.a, &[])["state_root"], first["state_root"]);
    let alias = f.dir.join("alias");
    std::os::unix::fs::symlink(&f.a, &alias).unwrap();
    assert_eq!(f.json(&alias, &[])["state_root"], first["state_root"]);

    let second = f.json(&f.a, &["sessions", "new", "--name", "Another", "--json"]);
    let id = second["id"].as_str().unwrap();
    let dir = Path::new(second["path"].as_str().unwrap());
    assert_eq!(session_workspace::metadata(dir).unwrap().unwrap().workspace, f.a);
    assert!(!dir.join("session.sqlite").exists(), "creation binds cwd without booting a model");
    let opened = f.json(&f.b, &["--session", id]);
    assert_eq!(opened["workspace"], f.a.to_str().unwrap());
    assert_eq!(f.json(&f.a, &[])["state_root"], opened["state_root"]);
    let used = session_workspace::metadata(dir).unwrap().unwrap().last_used_ms;
    f.json(&f.b, &["sessions", "history", "--id", id, "--json"]);
    assert_eq!(session_workspace::metadata(dir).unwrap().unwrap().last_used_ms, used);
    f.stop(dir);
    assert!(!f.run(&f.b, &["--session", id, "--model", "missing-profile"]).status.success());
    assert_eq!(session_workspace::metadata(dir).unwrap().unwrap().last_used_ms, used, "failed open is not a use");
    assert!(f.run(&f.b, &["sessions", "archive", "--id", id]).status.success());
    assert_eq!(f.json(&f.a, &[])["state_root"], first["state_root"]);
    f.json(&f.b, &["sessions", "open", "--id", id, "--json"]);
    assert_eq!(f.json(&f.a, &[])["state_root"], opened["state_root"]);
    f.json(&f.b, &["sessions", "delete", "--id", id, "--stop", "--yes", "--json"]);
    assert_eq!(f.json(&f.a, &[])["state_root"], first["state_root"]);
    assert_eq!(f.json(&f.b, &[])["state_root"], other["state_root"]);
    // Headless entry and read-only CLI inspection choose the same directory-bound session.
    let root = Path::new(other["state_root"].as_str().unwrap());
    let used = session_workspace::metadata(root).unwrap().unwrap().last_used_ms;
    f.json(&f.b, &["status", "--json"]);
    assert_eq!(session_workspace::metadata(root).unwrap().unwrap().last_used_ms, used);
    let output = f.run(&f.b, &["exec", "--json", "--timeout", "1", "fixture input"]);
    let report: Json = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["session_workspace"], f.b.to_str().unwrap(), "{report}");
    assert!(session_workspace::metadata(root).unwrap().unwrap().last_used_ms > used);
}

#[test]
fn existing_default_and_relative_paths_keep_one_absolute_workspace_on_restart() {
    let f = Fixture::new();
    // An explicitly selected default remains eligible for workspace-based selection.
    let first = f.json(&f.a, &["--state-root", "../state/teamagents/v2", "--cwd", "."]);
    assert_eq!(first["state_root"], f.base.to_str().unwrap());
    assert_eq!(first["workspace"], f.a.to_str().unwrap());
    assert_eq!(f.stored(&f.base), f.a.to_str().unwrap());
    // A pre-index database is discovered read-only, without using client cwd as a replacement.
    std::fs::remove_file(f.base.join(session_workspace::FILE)).unwrap();
    assert_eq!(f.json(&f.a, &[])["state_root"], first["state_root"]);
    f.stop(&f.base);
    // Loading B's project config would fail. Reopening A must load A before choosing the profile.
    std::fs::create_dir(f.b.join(".teamagents")).unwrap();
    std::fs::write(f.b.join(".teamagents/config.toml"), "not valid [toml").unwrap();
    std::fs::write(f.a.join("only-a.txt"), "A").unwrap();
    std::fs::write(f.b.join("only-b.txt"), "B").unwrap();
    for args in [vec!["--session", "default"], vec!["--state-root", f.base.to_str().unwrap(), "--cwd", "."]] {
        let reopened = f.json(&f.b, &args);
        assert_eq!(reopened["workspace"], first["workspace"]);
        assert_eq!(reopened["state_root"], first["state_root"]);
        assert_eq!(f.stored(&f.base), f.a.to_str().unwrap());
        let paths = f.json(&f.b, &["--session", "default", "inputs", "paths", "--query", "only-", "--json"]);
        assert!(paths.to_string().contains("only-a.txt"), "{paths}");
        assert!(!paths.to_string().contains("only-b.txt"), "{paths}");
        f.stop(&f.base);
    }
    f.json(&f.b, &["sessions", "open", "--id", "default", "--json"]);
    let client = teamagents_engine::v2::exec::Client::connect(&f.base.join("daemon.sock")).unwrap();
    assert_eq!(client.session_workspace, f.a.to_str().unwrap());
}

#[test]
fn creation_binds_cwd_and_legacy_relative_workspaces_are_not_reinterpreted() {
    let f = Fixture::new();
    let new = f.json(&f.a, &["sessions", "new", "--cwd", ".", "--json"]);
    let id = new["id"].as_str().unwrap();
    let dir = Path::new(new["path"].as_str().unwrap());
    assert_eq!(f.json(&f.b, &["--session", id])["workspace"], f.a.to_str().unwrap());
    f.stop(dir);
    let db = rusqlite::Connection::open(dir.join("session.sqlite")).unwrap();
    db.execute("UPDATE instances SET workspace_ref='.' WHERE id='i-leader'", []).unwrap();
    drop(db);
    let before = std::fs::read(dir.join("session.sqlite")).unwrap();
    let output = f.run(&f.b, &["sessions", "open", "--id", id, "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("relative saved workspace"));
    assert_eq!(before, std::fs::read(dir.join("session.sqlite")).unwrap());
    assert!(!dir.join("daemon.sock").exists());
    let healthy = f.json(&f.b, &[]);
    assert_eq!(healthy["workspace"], f.b.to_str().unwrap(), "unrelated workspaces remain usable");
    assert_eq!(before, std::fs::read(dir.join("session.sqlite")).unwrap());
    assert!(f.run(&f.b, &["sessions", "archive", "--id", id]).status.success());
    let archived = Registry::load(&f.base).unwrap().find(id).unwrap().dir(&f.base);
    std::fs::write(archived.join(session_workspace::FILE), "invalid JSON").unwrap();
    assert_eq!(f.json(&f.b, &[])["state_root"], healthy["state_root"]);
    let healthy_root = Path::new(healthy["state_root"].as_str().unwrap());
    let registry = Registry::load(&f.base).unwrap();
    let before_hint = std::fs::read(healthy_root.join(session_workspace::FILE)).unwrap();
    for reported in [Path::new("."), f.a.as_path()] {
        assert!(
            session_workspace::touch(&registry, healthy_root, reported).is_err(),
            "relative or contradictory live daemon facts cannot change workspace identity"
        );
    }
    assert_eq!(before_hint, std::fs::read(healthy_root.join(session_workspace::FILE)).unwrap());
}

#[test]
fn simultaneous_workspace_selection_creates_one_session_without_starting_it() {
    let f = Fixture::new();
    assert!(!f.run(&f.a, &["status", "--json"]).status.success());
    assert!(!f.base.exists(), "read-only lookup does not create state");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let (base, workspace, barrier) = (f.base.clone(), f.a.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                session_workspace::select(&base, &workspace, true).unwrap().unwrap()
            })
        })
        .collect();
    let roots: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert_eq!(roots[0], roots[1]);
    assert_eq!(Registry::load(&f.base).unwrap().sessions.len(), 1);
    assert!(!roots[0].join("session.sqlite").exists());
    assert!(!roots[0].join("daemon.sock").exists());
}
