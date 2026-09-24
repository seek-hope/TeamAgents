//! Synchronous client of the v2 session daemon (plan §9): one Unix-socket
//! JSON-lines connection, greeting-verified on connect, request/reply
//! matched by request id, and a reconnect path that resumes events from the
//! last watermark — the TUI never executes anything itself.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Must match engine v2::daemon::PROTOCOL_VERSION.
pub const PROTOCOL_VERSION: u64 = 1;

pub struct DaemonClient {
    socket: PathBuf,
    conn: Option<Conn>,
    /// Session identity learned from the greeting (reconnect re-verifies).
    pub session_id: String,
    pub state_root: String,
    /// Highest event sequence delivered to the UI; reconnect resumes after it.
    watermark: i64,
    /// Set when the last I/O failed; the next call reconnects first (§9).
    pub disconnected: bool,
    next_request: u64,
}

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl DaemonClient {
    /// Connect and verify the greeting: server identity, protocol version,
    /// and — when the caller already holds a session — the same state root
    /// (§9 handshake: old servers and different roots are refused).
    pub fn connect(socket: &Path) -> Result<DaemonClient, String> {
        let (conn, greeting) = handshake(socket)?;
        Ok(DaemonClient {
            socket: socket.to_path_buf(),
            conn: Some(conn),
            session_id: greeting["session_id"].as_str().unwrap_or("").to_string(),
            state_root: greeting["state_root"].as_str().unwrap_or("").to_string(),
            watermark: 0,
            disconnected: false,
            next_request: 0,
        })
    }

    fn reconnect(&mut self) -> Result<(), String> {
        let (conn, greeting) = handshake(&self.socket)?;
        if greeting["session_id"].as_str().unwrap_or("") != self.session_id
            || greeting["state_root"].as_str().unwrap_or("") != self.state_root
        {
            return Err("daemon restarted with a different session; resync from scratch".into());
        }
        self.conn = Some(conn);
        self.disconnected = false;
        Ok(())
    }

    /// One request/reply round trip. A broken connection flips the client
    /// into the disconnected state; the caller retries after `reconnect`.
    pub fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        if self.conn.is_none() {
            self.reconnect()?;
        }
        self.next_request += 1;
        let request_id = format!("tui-{}", self.next_request);
        let frame = json!({"protocol_version": PROTOCOL_VERSION, "request_id": request_id,
                           "method": method, "params": params});
        match self.roundtrip(&frame, &request_id) {
            Ok(result) => result,
            Err(error) => {
                self.conn = None;
                self.disconnected = true;
                Err(error)
            }
        }
    }

    /// A business command with a caller-chosen id, stable across reconnects
    /// (§9): the control plane dedups, so a retry after a lost reply is safe.
    pub fn command(&mut self, command_id: &str, method: &str, params: Json) -> Result<Json, String> {
        if self.conn.is_none() {
            self.reconnect()?;
        }
        self.next_request += 1;
        let request_id = format!("tui-{}", self.next_request);
        let frame = json!({"protocol_version": PROTOCOL_VERSION, "request_id": request_id,
                           "method": method, "command_id": command_id, "params": params});
        match self.roundtrip(&frame, &request_id) {
            Ok(result) => result,
            Err(error) => {
                self.conn = None;
                self.disconnected = true;
                Err(error)
            }
        }
    }

    fn roundtrip(&mut self, frame: &Json, request_id: &str) -> Result<Result<Json, String>, String> {
        let conn = self.conn.as_mut().ok_or("not connected")?;
        conn.writer
            .write_all(format!("{frame}\n").as_bytes())
            .and_then(|_| conn.writer.flush())
            .map_err(|e| format!("daemon write: {e}"))?;
        let mut line = String::new();
        if conn.reader.read_line(&mut line).map_err(|e| format!("daemon read: {e}"))? == 0 {
            return Err("daemon closed the connection".into());
        }
        let reply: Json = serde_json::from_str(&line).map_err(|e| format!("daemon reply JSON: {e}"))?;
        if reply["request_id"].as_str() != Some(request_id) {
            return Err(format!("daemon reply out of order (want {request_id})"));
        }
        if reply["ok"].as_bool().unwrap_or(false) {
            Ok(Ok(reply.get("result").cloned().unwrap_or(Json::Null)))
        } else {
            Ok(Err(reply["error"].as_str().unwrap_or("daemon error").to_string()))
        }
    }

    /// Snapshot + watermark from one consistent read (§9 reconnect entry).
    pub fn checkpoint(&mut self) -> Result<(Json, i64), String> {
        let result = self.call("checkpoint", json!({}))?;
        let watermark = result["watermark"].as_i64().ok_or("checkpoint.watermark missing")?;
        self.watermark = self.watermark.max(watermark);
        Ok((result["snapshot"].clone(), watermark))
    }

    /// Events after the delivered watermark; updates the delivered position.
    pub fn poll_events(&mut self) -> Result<Vec<Json>, String> {
        let result = self.call("events", json!({"since": self.watermark}))?;
        if result["resync_required"].as_bool().unwrap_or(false) {
            return Err("event watermark was reclaimed; take a fresh checkpoint".into());
        }
        let events = result["events"].as_array().cloned().unwrap_or_default();
        if let Some(watermark) = result["watermark"].as_i64() {
            self.watermark = self.watermark.max(watermark);
        }
        Ok(events)
    }

    /// Current watermark (for tests and status views).
    pub fn watermark(&self) -> i64 {
        self.watermark
    }
}

/// A wedged daemon must mark the client disconnected, never freeze the UI:
/// local-socket round trips are sub-millisecond, so a multi-second silence
/// means the server is stuck (§9 reconnect).
const IO_TIMEOUT: Duration = Duration::from_secs(2);

fn handshake(socket: &Path) -> Result<(Conn, Json), String> {
    let writer = UnixStream::connect(socket).map_err(|e| format!("connect {}: {e}", socket.display()))?;
    writer.set_read_timeout(Some(IO_TIMEOUT)).map_err(|e| e.to_string())?;
    writer.set_write_timeout(Some(IO_TIMEOUT)).map_err(|e| e.to_string())?;
    let reader = BufReader::new(writer.try_clone().map_err(|e| e.to_string())?);
    let mut conn = Conn { reader, writer };
    let mut line = String::new();
    if conn.reader.read_line(&mut line).map_err(|e| format!("greeting: {e}"))? == 0 {
        return Err("daemon closed before the greeting".into());
    }
    let greeting: Json = serde_json::from_str(&line).map_err(|e| format!("greeting JSON: {e}"))?;
    if greeting["server"].as_str() != Some("teamagents-daemon") {
        return Err("not a teamagents daemon (old server?)".into());
    }
    if greeting["protocol_version"].as_u64() != Some(PROTOCOL_VERSION) {
        return Err(format!(
            "daemon protocol {} != client {PROTOCOL_VERSION} (incompatible build)",
            greeting["protocol_version"]
        ));
    }
    Ok((conn, greeting))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc::{channel, Sender};
    use std::thread;

    /// Scripted fake daemon: serves the greeting, then answers scripted
    /// replies; reports every received frame back over the channel.
    struct FakeDaemon {
        socket: PathBuf,
        frames: std::sync::mpsc::Receiver<Json>,
        /// Keeps the server thread's channel endpoint alive for its lifetime.
        _stop: Sender<()>,
    }

    fn fake_daemon(tag: &str, protocol_version: u64, replies: Vec<Json>) -> FakeDaemon {
        let dir = std::env::temp_dir().join(format!("ta-tui-daemon-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (frames_tx, frames_rx) = channel();
        let (stop_tx, stop_rx) = channel::<()>();
        thread::spawn(move || {
            let _ = stop_rx;
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            // the daemon greets first (§9 handshake), then serves replies
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"server": "teamagents-daemon", "protocol_version": protocol_version,
                               "session_id": "s-test", "state_root": "/tmp/fake-root"})
                    )
                    .as_bytes(),
                )
                .unwrap();
            let mut replies = replies.into_iter();
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return; // client gone
                }
                let frame: Json = serde_json::from_str(&line).unwrap();
                frames_tx.send(frame.clone()).ok();
                if let Some(mut reply) = replies.next() {
                    reply["request_id"] = frame["request_id"].clone();
                    writer.write_all(format!("{reply}\n").as_bytes()).unwrap();
                } else {
                    return; // out of script: drop the connection
                }
            }
        });
        FakeDaemon { socket, frames: frames_rx, _stop: stop_tx }
    }

    #[test]
    fn greeting_mismatch_is_refused() {
        let daemon = fake_daemon("version", PROTOCOL_VERSION + 9, vec![]);
        let error = DaemonClient::connect(&daemon.socket).err().expect("must refuse");
        assert!(error.contains("incompatible"), "{error}");
    }

    #[test]
    fn checkpoint_and_commands_round_trip() {
        let daemon = fake_daemon(
            "roundtrip",
            PROTOCOL_VERSION,
            vec![
                json!({"ok": true, "result": {"snapshot": {"instances": []}, "watermark": 7}}),
                json!({"ok": true, "result": {"accepted": true}}),
                json!({"ok": false, "error": "not dispatchable"}),
            ],
        );
        let mut client = DaemonClient::connect(&daemon.socket).unwrap();
        assert_eq!(client.session_id, "s-test");
        let (_snapshot, watermark) = client.checkpoint().unwrap();
        assert_eq!(watermark, 7);
        assert_eq!(client.watermark(), 7);
        let accepted = client.command("c-1", "submit_input", json!({"text": "hi"})).unwrap();
        assert_eq!(accepted, json!({"accepted": true}));
        // the command id rode at the request level (§9)
        let frame = daemon.frames.recv().unwrap();
        let _ = frame;
        let frames: Vec<Json> = std::iter::from_fn(|| daemon.frames.try_recv().ok()).collect();
        assert_eq!(frames[0]["command_id"], json!("c-1"));
        // daemon-side refusal surfaces as an error, not a disconnect
        let error = client.command("c-2", "submit_input", json!({})).unwrap_err();
        assert!(error.contains("not dispatchable"), "{error}");
        assert!(!client.disconnected);
    }

    #[test]
    fn lost_connection_marks_disconnected_and_reconnect_resumes() {
        let daemon = fake_daemon(
            "reconnect",
            PROTOCOL_VERSION,
            vec![json!({"ok": true, "result": {"snapshot": {"instances": []}, "watermark": 3}})],
        );
        let mut client = DaemonClient::connect(&daemon.socket).unwrap();
        client.checkpoint().unwrap();
        assert_eq!(client.watermark(), 3);
        // the fake drops the connection after its one scripted reply
        let error = client.poll_events().unwrap_err();
        assert!(!error.is_empty());
        assert!(client.disconnected);
        // a new fake daemon for the same session answers the resumed poll
        let daemon2 = fake_daemon(
            "reconnect2",
            PROTOCOL_VERSION,
            vec![
                json!({"ok": true, "result": {"events": [{"sequence": 4, "kind": "note"}], "watermark": 4, "resync_required": false}}),
            ],
        );
        client.socket = daemon2.socket.clone();
        let events = client.poll_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(client.watermark(), 4);
        // the resumed poll asked for events after the *old* watermark
        let frame = daemon2.frames.recv().unwrap();
        assert_eq!(frame["params"]["since"], json!(3));
    }
}
