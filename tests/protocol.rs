use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;
use uuid::Uuid;

struct Lab {
    dir: PathBuf,
    state: PathBuf,
    workspace: PathBuf,
}
impl Lab {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ta-wire-{}", Uuid::new_v4()));
        let state = dir.join("state");
        let workspace = dir.join("work");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir(&workspace).unwrap();
        Self { dir, state, workspace }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.dir)
            .args(["--session"])
            .arg(&self.state)
            .arg("--workspace")
            .arg(&self.workspace)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self.run(&["stop"]);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn new_client_sends_only_hello_to_an_old_daemon() {
    let lab = Lab::new();
    let listener = UnixListener::bind(lab.state.join("swarm.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut buf = String::new();
        reader.read_line(&mut buf).unwrap();
        let hello: Value = serde_json::from_str(&buf).unwrap();
        assert_eq!(hello["body"], json!({"op":"hello"}));
        assert_eq!(hello["protocol"], 3);
        // A v2 daemon knows the greeting but speaks the old delegation protocol.
        writeln!(socket, "{}", json!({"value":{"protocol":2}})).unwrap();
        buf.clear();
        assert_eq!(reader.read_line(&mut buf).unwrap(), 0);
    });
    let out = lab.run(&["exec", "--detach", "do not deliver this"]);
    server.join().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("incompatible session protocol"));
}

#[test]
fn new_daemon_refuses_a_legacy_submit_before_persisting_it() {
    let lab = Lab::new();
    assert!(lab.run(&["members"]).status.success());
    let mut stream = UnixStream::connect(lab.state.join("swarm.sock")).unwrap();
    let body = json!({"workspace":lab.workspace.as_os_str().as_encoded_bytes(),"body":{"op":"submit","agent":"default","text":"legacy","review":false}});
    writeln!(stream, "{body}").unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    assert!(serde_json::from_str::<Value>(&response).unwrap()["error"].as_str().unwrap().contains("incompatible"));
    let mut old_client = UnixStream::connect(lab.state.join("swarm.sock")).unwrap();
    let hello = json!({"protocol":2,"workspace":lab.workspace.as_os_str().as_encoded_bytes(),"body":{"op":"hello"}});
    writeln!(old_client, "{hello}").unwrap();
    let mut reply = String::new();
    BufReader::new(old_client).read_line(&mut reply).unwrap();
    assert!(serde_json::from_str::<Value>(&reply).unwrap()["error"].as_str().unwrap().contains("incompatible"));
    let out = lab.run(&["tasks", "--json"]);
    assert!(out.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), json!([]));
}
