//! The client's reconnect path against a daemon that really dies and comes back (§9).
//!
//! The unit tests in `daemon_client.rs` cover the scripted round trips; this one covers the transition the
//! TUI lives through: a healthy connection, the daemon going away, and a *new* daemon listening on the same
//! socket. Which is also how D-99 was diagnosed: the client did reconnect (a call succeeded and the flag
//! cleared), but nothing rebuilt the frame, so the status line kept saying "disconnected" until some
//! unrelated event forced a redraw.
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::thread;
use teamagents_tui::daemon_client::DaemonClient;

const GREETING: &str = r#"{"server":"teamagents-daemon","protocol_version":1,"session_id":"s-main","state_root":"/tmp/x","permissions":"full_auto","workspace":"/tmp"}"#;

/// Bind `socket`, accept one client, answer its first request, then die with the listener.
fn serve_once(socket: PathBuf, ready: Sender<()>) {
    // a killed daemon leaves its socket file behind and bind does not unlink: the real daemon removes it
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind");
    ready.send(()).unwrap();
    let Ok((stream, _)) = listener.accept() else { return };
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);
    if writer.write_all(format!("{GREETING}\n").as_bytes()).and_then(|()| writer.flush()).is_err() {
        return;
    }
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) > 0 {
        // echo the request id: the client matches replies by it
        let request: serde_json::Value = serde_json::from_str(&line).unwrap_or(json!({}));
        let answer = json!({"request_id": request["request_id"], "ok": true,
                            "result": {"watermark": 1, "snapshot": {}}});
        let _ = writer.write_all(format!("{answer}\n").as_bytes()).and_then(|()| writer.flush());
    }
    // keep the connection open until the client has read the reply, then drop listener and stream
    let _ = UnixStream::connect(&socket);
}

#[test]
fn the_client_reconnects_after_the_daemon_restarts() {
    let dir = std::env::temp_dir().join(format!("ta-reconnect-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("daemon.sock");
    let (tx, rx) = channel();
    let first = thread::spawn({
        let socket = socket.clone();
        move || serve_once(socket, tx)
    });
    rx.recv().unwrap();
    let mut client = DaemonClient::connect(&socket).expect("connect");
    assert!(client.call("checkpoint", json!({})).is_ok(), "the first daemon answers");
    assert!(!client.disconnected);
    first.join().unwrap();

    // the daemon is gone: the next call fails and marks the client disconnected
    assert!(client.call("checkpoint", json!({})).is_err(), "a dead daemon must fail the call");
    assert!(client.disconnected, "and the client says so");

    // a new daemon on the same socket must be reachable through the same client
    let (tx2, rx2) = channel();
    let second = thread::spawn({
        let socket = socket.clone();
        move || serve_once(socket, tx2)
    });
    rx2.recv().unwrap();
    let again = client.call("checkpoint", json!({}));
    assert!(again.is_ok(), "the client reconnected: {again:?}");
    assert!(!client.disconnected, "and it reports itself connected again");
    second.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
