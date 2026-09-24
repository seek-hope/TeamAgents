//! The headless client of the session daemon (§9). `teamagents
//! exec` talks to the same backend as the TUI: it submits one input and reports
//! the outcome — it never runs a second engine.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Must match engine v2::daemon::PROTOCOL_VERSION.
const PROTOCOL_VERSION: u64 = 1;

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

pub struct Client {
    socket: PathBuf,
    conn: Conn,
    pub session_id: String,
    pub state_root: String,
    next_request: u64,
    watermark: i64,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Client, String> {
        let (conn, greeting) = handshake(socket)?;
        Ok(Client {
            socket: socket.to_path_buf(),
            conn,
            session_id: greeting["session_id"].as_str().unwrap_or("").to_string(),
            state_root: greeting["state_root"].as_str().unwrap_or("").to_string(),
            next_request: 0,
            watermark: 0,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn roundtrip(&mut self, method: &str, params: Json, command_id: Option<&str>) -> Result<Json, String> {
        self.next_request += 1;
        let request_id = format!("exec-{}", self.next_request);
        let mut frame = json!({"protocol_version": PROTOCOL_VERSION, "request_id": request_id,
                               "method": method, "params": params});
        if let Some(command_id) = command_id {
            frame["command_id"] = json!(command_id);
        }
        let mut line = serde_json::to_string(&frame).map_err(|e| e.to_string())?;
        line.push('\n');
        self.conn.writer.write_all(line.as_bytes()).map_err(|e| format!("daemon write: {e}"))?;
        let mut reply = String::new();
        self.conn.reader.read_line(&mut reply).map_err(|e| format!("daemon read: {e}"))?;
        let reply: Json = serde_json::from_str(&reply).map_err(|e| format!("daemon reply: {e}"))?;
        if reply["ok"] != json!(true) {
            return Err(format!("daemon refused {method}: {}", reply["error"]));
        }
        Ok(reply["result"].clone())
    }

    pub fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        self.roundtrip(method, params, None)
    }

    pub fn command(&mut self, command_id: &str, method: &str, params: Json) -> Result<Json, String> {
        self.roundtrip(method, params, Some(command_id))
    }

    /// Events after the last delivered watermark (§9 reconnect contract).
    pub fn events(&mut self) -> Result<Vec<Json>, String> {
        let result = self.call("events", json!({"since": self.watermark}))?;
        let events = result["events"].as_array().cloned().unwrap_or_default();
        for event in &events {
            self.watermark = self.watermark.max(event["sequence"].as_i64().unwrap_or(self.watermark));
        }
        Ok(events)
    }

    pub fn history(&mut self, instance: &str) -> Result<Vec<Json>, String> {
        let result = self.call("history", json!({"instance_id": instance, "limit": 400}))?;
        Ok(result["entries"].as_array().cloned().unwrap_or_default())
    }
}

fn handshake(socket: &Path) -> Result<(Conn, Json), String> {
    let stream = UnixStream::connect(socket).map_err(|e| format!("connect {}: {e}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader.read_line(&mut greeting).map_err(|e| format!("daemon greeting: {e}"))?;
    let greeting: Json = serde_json::from_str(&greeting).map_err(|e| format!("daemon greeting: {e}"))?;
    if greeting["server"] != json!("teamagents-daemon") || greeting["protocol_version"] != json!(PROTOCOL_VERSION) {
        return Err(format!("socket {} is not a v2 daemon ({greeting})", socket.display()));
    }
    Ok((Conn { reader, writer }, greeting))
}

pub struct ExecOptions {
    pub socket: PathBuf,
    pub prompt: String,
    pub timeout_s: u64,
    pub json_out: bool,
}

/// One headless run: submit the prompt to the leader and report the outcome
/// (goal settlement, a plain reply, or the deadline). Exit code: 0 settled,
/// 1 failed/timeout, 2 usage/infrastructure.
pub fn run(options: ExecOptions) -> i32 {
    let mut client = match Client::connect(&options.socket) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("exec: {error}; start teamagents daemon first (or just run teamagents)");
            return 2;
        }
    };
    let checkpoint = match client.call("checkpoint", json!({})) {
        Ok(checkpoint) => checkpoint,
        Err(error) => {
            eprintln!("exec: checkpoint failed: {error}");
            return 2;
        }
    };
    let instance = leader_instance(&checkpoint);
    let Some(instance) = instance else {
        eprintln!("exec: the session has no usable leader instance: {checkpoint}");
        return 2;
    };
    let envelope = format!("env-{}", uuid::Uuid::new_v4());
    if let Err(error) = client.command(
        &format!("input-{envelope}"),
        "submit_input",
        json!({"instance_id": instance, "envelope_id": envelope, "text": options.prompt}),
    ) {
        eprintln!("exec: submitting the input failed: {error}");
        return 2;
    }
    let deadline = Instant::now() + Duration::from_secs(options.timeout_s);
    let mut goal_status: Option<String> = None;
    let mut reply: Option<String> = None;
    let mut announced_approval = String::new();
    loop {
        match client.events() {
            Ok(events) => {
                for event in &events {
                    if event["kind"] == json!("goal_completed") {
                        goal_status = Some(event["payload"]["status"].as_str().unwrap_or("unknown").to_string());
                    }
                }
            }
            Err(error) => {
                eprintln!("exec: the event stream broke: {error}");
                return 2;
            }
        }
        let snapshot = client.call("checkpoint", json!({})).unwrap_or(Json::Null);
        let instances = snapshot["snapshot"]["instances"]
            .as_array()
            .or_else(|| snapshot["instances"].as_array())
            .cloned()
            .unwrap_or_default();
        let settled = instances.iter().any(|entry| entry["id"] == json!(instance) && entry["phase"] == json!("READY"));
        if settled {
            if let Ok(entries) = client.history(&instance) {
                if let Some(last) = entries.last() {
                    if last["kind"] == json!("assistant") {
                        reply = last["message"]["content"].as_str().map(str::to_string);
                    }
                }
            }
        }
        // a pending approval blocks the turn on the user: say so instead of
        // looking stuck (the TUI is the approval surface, §9)
        if let Ok(approvals) = client.call("approvals", json!({})) {
            for approval in approvals["approvals"].as_array().into_iter().flatten() {
                let id = approval["id"].as_str().unwrap_or("");
                if id != announced_approval.as_str() {
                    announced_approval = id.to_string();
                    eprintln!(
                        "[exec] waiting for user approval: {} (approve or deny in the TUI; or start the daemon with --full-auto to skip the gate)",
                        approval["preview"].as_str().unwrap_or("")
                    );
                }
            }
        }
        if goal_status.is_some() || reply.is_some() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let end = if goal_status.is_some() {
        "completed"
    } else if reply.is_some() {
        "reply"
    } else {
        "timeout"
    };
    let report = json!({
        "session_id": client.session_id,
        "state_root": client.state_root,
        "instance_id": instance,
        "end": end,
        "goal_status": goal_status,
        "reply": reply.as_ref().map(|text| text.chars().take(2000).collect::<String>()),
        "watermark": client.watermark,
    });
    if options.json_out {
        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
    } else {
        match (&goal_status, &reply) {
            (Some(status), _) => println!("goal ended: {status}"),
            (_, Some(text)) => println!("{text}"),
            _ => println!("timed out: {instance} is still running ({}s)", options.timeout_s),
        }
    }
    match end {
        "completed" => 0,
        "reply" => 0,
        _ => 1,
    }
}

/// The session's leader instance: the conventional id first, then any ACTIVE
/// instance (a fresh session boots exactly one).
fn leader_instance(checkpoint: &Json) -> Option<String> {
    // the daemon's checkpoint wraps the read snapshot: {"snapshot": {…}}
    let instances = checkpoint["snapshot"]["instances"].as_array().or_else(|| checkpoint["instances"].as_array())?;
    if let Some(leader) = instances.iter().find(|entry| entry["id"] == json!("i-leader")) {
        return leader["id"].as_str().map(str::to_string);
    }
    instances
        .iter()
        .find(|entry| entry["lifecycle"] == json!("ACTIVE"))
        .and_then(|entry| entry["id"].as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::leader_instance;
    use serde_json::json;

    #[test]
    fn the_leader_instance_prefers_the_conventional_id() {
        let checkpoint = json!({"instances": [
            {"id": "i-worker", "lifecycle": "ACTIVE"},
            {"id": "i-leader", "lifecycle": "ACTIVE"},
        ]});
        assert_eq!(leader_instance(&checkpoint).as_deref(), Some("i-leader"));
        let only = json!({"instances": [{"id": "i-other", "lifecycle": "ACTIVE"}]});
        assert_eq!(leader_instance(&only).as_deref(), Some("i-other"));
        let none = json!({"instances": [{"id": "i-x", "lifecycle": "TERMINATED"}]});
        assert_eq!(leader_instance(&none), None);
    }
}
