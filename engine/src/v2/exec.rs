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
    /// The mode the session booted with (D-41); the daemon reports it in the
    /// greeting because it is fixed for the whole session.
    pub permissions: String,
    /// The directory the session works in (D-57): also fixed at boot, and not
    /// necessarily the one this client is running in.
    pub session_workspace: String,
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
            permissions: greeting["permissions"].as_str().unwrap_or("unknown").to_string(),
            session_workspace: greeting["workspace"].as_str().unwrap_or("unknown").to_string(),
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
    /// Acceptance commands the user pre-authorized on the command line
    /// (`--check`). They run in the client's workspace after the turn ends and
    /// gate the exit code; an empty list means no client-side verification.
    pub checks: Vec<String>,
    /// The workspace the acceptance commands run in (the client's `--cwd` or
    /// its current directory).
    pub workspace: PathBuf,
}

/// Terminal state of one headless run. Documented exit codes (D-32/D-49):
/// 0 completed, 1 failed or unfinished, 3 approval required, 124 timeout
/// (2 is reserved for usage and infrastructure errors, raised before this).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum End {
    /// The goal settled as SUCCEEDED.
    Completed,
    /// The leader ended its turn with a plain reply and no goal settlement.
    Reply,
    /// The turn ended on the runtime's own closing word with nothing settled
    /// this run can claim: the model's `finish` had nothing to settle, or the
    /// goal it addressed had already settled earlier. Not a `Reply` (that text
    /// is the runtime's, not the member's) and not a settlement, so the run is
    /// unfinished — exit 1 (D-71).
    Unsettled,
    /// The goal settled as something other than SUCCEEDED.
    Failed,
    /// The turn is parked on a user approval: a non-interactive run has nobody
    /// to answer it, so it reports immediately instead of burning the timeout.
    ApprovalRequired,
    /// The deadline passed with the instance still running.
    Timeout,
}

impl End {
    fn name(self) -> &'static str {
        match self {
            End::Completed => "completed",
            End::Reply => "reply",
            End::Unsettled => "unsettled",
            End::Failed => "failed",
            End::ApprovalRequired => "approval_required",
            End::Timeout => "timeout",
        }
    }

    /// The `--check` verdict can only turn a success into a failure, never the
    /// other way round, and it never masks approval-required or a timeout.
    pub fn exit_code(self, checks_ok: bool) -> i32 {
        match self {
            End::Completed | End::Reply if checks_ok => 0,
            End::Completed | End::Reply | End::Unsettled | End::Failed => 1,
            End::ApprovalRequired => 3,
            End::Timeout => 124,
        }
    }
}

/// Output kept per acceptance command, in the report and on stderr.
const CHECK_OUTPUT_CAP: usize = 8000;

/// The outcome of one headless run: the JSON report the `--json` mode prints,
/// the terminal state that decides the exit code, and the acceptance verdict.
pub struct ExecRun {
    pub report: Json,
    pub end: End,
    pub checks_ok: bool,
}

/// One headless run with no printing at all: submit the prompt to the leader,
/// follow the run to its terminal state, then run the user's acceptance
/// commands. Errors are (exit code, message).
pub fn execute(options: &ExecOptions) -> Result<ExecRun, (i32, String)> {
    let mut client = Client::connect(&options.socket)
        .map_err(|error| (2, format!("exec: {error}; start teamagents daemon first (or just run teamagents)")))?;
    // Drain the events recorded before this input and keep the watermark: a
    // goal settlement from an earlier run is history, not this run's outcome.
    client.events().map_err(|error| (2, format!("exec: the event stream broke: {error}")))?;
    let checkpoint =
        client.call("checkpoint", json!({})).map_err(|error| (2, format!("exec: checkpoint failed: {error}")))?;
    let (instance, lifecycle) = leader_instance(&checkpoint)
        .ok_or_else(|| (2, format!("exec: the session has no usable leader instance: {checkpoint}")))?;
    // A parked or paused leader will not run the input: saying so beats
    // submitting work that sits in a queue nobody is draining until the caller's
    // own deadline expires (the user resumes it in the TUI, §5.4).
    if lifecycle != "ACTIVE" {
        return Err((
            2,
            format!(
                "exec: the leader instance {instance} is {lifecycle}; new input would not run. \
                 Resume it in the TUI instances panel (r) or use a fresh state root; nothing was submitted."
            ),
        ));
    }
    let envelope = format!("env-{}", uuid::Uuid::new_v4());
    let submitted = client
        .command(
            &format!("input-{envelope}"),
            "submit_input",
            json!({"instance_id": instance, "envelope_id": envelope, "text": options.prompt}),
        )
        .map_err(|error| (2, format!("exec: submitting the input failed: {error}")))?;
    // A turn that was already in flight when this run started cannot include the
    // input: it waits for the boundary and enters the conversation after that
    // turn's own reply (D-63). Report it instead of pretending it landed.
    let queued = submitted["queued"] == json!(true);
    let deadline = Instant::now() + Duration::from_secs(options.timeout_s);
    let mut goal_status: Option<String> = None;
    let mut turn_failure: Option<String> = None;
    let mut reply: Option<String> = None;
    let mut pending_approval: Option<String> = None;
    // the deadline is the default outcome: a loop that breaks on a terminal
    // state always overwrites it
    let mut end = End::Timeout;
    loop {
        // The checkpoint is read *before* the events on purpose. A settlement
        // commits its event and the instance's phases in one transaction, so a
        // phase read that missed the settlement is always followed by an event
        // read that missed it too — and the loop takes another pass. The other
        // order could see an idle instance whose settlement event was still
        // unread and report a finished run as `unsettled` (D-71).
        let snapshot = client.call("checkpoint", json!({})).unwrap_or(Json::Null);
        match client.events() {
            Ok(events) => {
                for event in &events {
                    if event["kind"] == json!("goal_completed") {
                        goal_status = Some(event["payload"]["status"].as_str().unwrap_or("unknown").to_string());
                    }
                    // A permanently failed leader request is a terminal outcome
                    // of this run: report it instead of waiting out the deadline.
                    // Failures of *other* instances belong to the leader's turn,
                    // not to this client.
                    if event["kind"] == json!("request_failed") && event["scope"] == json!(instance) {
                        turn_failure =
                            Some(event["payload"]["reason"].as_str().unwrap_or("the model request failed").to_string());
                    }
                }
            }
            Err(error) => return Err((2, format!("exec: the event stream broke: {error}"))),
        }
        let instances = snapshot["snapshot"]["instances"]
            .as_array()
            .or_else(|| snapshot["instances"].as_array())
            .cloned()
            .unwrap_or_default();
        let settled = instances.iter().any(|entry| entry["id"] == json!(instance) && entry["phase"] == json!("READY"));
        // The runtime's own closing word, if it is what the instance stopped on
        // (§8: a settled goal, an accepted `finish` with nothing left to settle).
        // It is never the member's reply, which is exactly the confusion that made
        // a runtime-blocked goal exit 0 (D-71).
        let mut runtime_closed = false;
        if settled {
            if let Ok(entries) = client.history(&instance) {
                if let Some(last) = entries.last() {
                    if last["kind"] == json!("assistant") {
                        // an empty assistant tail (a bare tool call) is not a
                        // reply the user can read
                        reply = last["message"]["content"]
                            .as_str()
                            .filter(|text| !text.trim().is_empty())
                            .map(str::to_string);
                    } else if last["kind"] == json!("runtime") {
                        runtime_closed = true;
                    }
                }
            }
        }
        // A pending approval blocks the turn on the user, and a headless run
        // has nobody to answer it: report it immediately instead of waiting for
        // the deadline (the TUI is the approval surface, §9).
        if let Ok(approvals) = client.call("approvals", json!({})) {
            for approval in approvals["approvals"].as_array().into_iter().flatten() {
                let preview = approval["preview"].as_str().unwrap_or("");
                if pending_approval.as_deref() != Some(preview) {
                    pending_approval = Some(preview.to_string());
                }
            }
        }
        // A settlement recorded after this input was submitted is this run's
        // outcome (history was drained before submitting); a plain reply only
        // counts once the leader is back to READY.
        let terminal = match &goal_status {
            Some(status) => Some(if status == "SUCCEEDED" { End::Completed } else { End::Failed }),
            None if turn_failure.is_some() => Some(End::Failed),
            None if pending_approval.is_some() => Some(End::ApprovalRequired),
            None if settled && reply.is_some() => Some(End::Reply),
            // The turn is over and this client has nothing to report as its
            // outcome: waiting for the deadline would mislabel a closed turn as
            // a timeout (D-71).
            None if runtime_closed => Some(End::Unsettled),
            None => None,
        };
        if let Some(terminal) = terminal {
            end = terminal;
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // The user's own acceptance commands are the last word on a finished turn:
    // they run in this workspace through the same isolated shell the tools use.
    // Nothing runs when the run stopped for an approval — that turn is not
    // finished, so there is no acceptance to verify (§8).
    let verification = if options.checks.is_empty() || end == End::ApprovalRequired {
        Vec::new()
    } else {
        run_checks(&options.checks, &options.workspace, options.timeout_s)
    };
    let verification_path = write_verification(&client.state_root, &verification);
    let checks_ok = verification.iter().all(|entry| entry["ok"] == json!(true));
    let report = json!({
        "session_id": client.session_id,
        "state_root": client.state_root,
        "permissions": client.permissions,
        // the checks below run where *this client* works; the session itself may
        // work somewhere else (its own --cwd), so both are reported
        "session_workspace": client.session_workspace,
        "instance_id": instance,
        "end": end.name(),
        "goal_status": goal_status,
        "reply": reply.as_ref().map(|text| text.chars().take(2000).collect::<String>()),
        "failure": turn_failure,
        "approval": pending_approval,
        "input_queued": queued,
        "workspace": options.workspace.to_string_lossy(),
        "verification": verification,
        "verification_path": verification_path,
        "watermark": client.watermark,
    });
    Ok(ExecRun { report, end, checks_ok })
}

/// One headless run: the outcome on stdout (JSON when asked for it) and the
/// documented exit code — 0 settled, 1 failed or unfinished, 3 approval
/// required, 124 timeout, 2 usage/infrastructure.
pub fn run(options: ExecOptions) -> i32 {
    let run = match execute(&options) {
        Ok(run) => run,
        Err((code, message)) => {
            eprintln!("{message}");
            return code;
        }
    };
    let report = &run.report;
    if options.json_out {
        println!("{}", serde_json::to_string(&report).unwrap_or_else(|_| "{}".into()));
    } else {
        print_human(report, &options);
    }
    run.end.exit_code(run.checks_ok)
}

fn print_human(report: &Json, options: &ExecOptions) {
    match report["end"].as_str().unwrap_or("") {
        "approval_required" => {
            println!("approval required: {}", report["approval"].as_str().unwrap_or("see the TUI approvals panel"));
            eprintln!("[exec] approve or deny it in the TUI; or start the daemon with --full-auto to skip the gate");
        }
        "timeout" => println!(
            "timed out: {} is still running ({}s)",
            report["instance_id"].as_str().unwrap_or("the leader"),
            options.timeout_s
        ),
        // the instance stopped on the runtime's own closing word and no goal
        // settled in this run: say so instead of leaving the caller to guess
        // from a missing reply (D-71)
        "unsettled" => println!(
            "the turn closed without settling anything: no goal ended in this run and the last word was the runtime's"
        ),
        _ => {
            if report["input_queued"] == json!(true) {
                println!("queued: a turn was already running, so this input enters after it ends");
            }
            if let Some(reason) = report["failure"].as_str() {
                println!("turn failed: {reason}");
            } else if let Some(status) = report["goal_status"].as_str() {
                println!("goal ended: {status}");
            } else if let Some(text) = report["reply"].as_str() {
                println!("{text}");
            } else {
                println!("the run ended without a reply");
            }
        }
    }
    print_checks(report);
}

/// Human-readable check verdicts on stdout (the verdict is product output); a
/// failing command's output is diagnostic and goes to stderr.
fn print_checks(report: &Json) {
    let Some(checks) = report["verification"].as_array().filter(|checks| !checks.is_empty()) else {
        return;
    };
    for (index, entry) in checks.iter().enumerate() {
        let ok = entry["ok"] == json!(true);
        println!(
            "check {}: {} (exit {})  {}",
            index + 1,
            if ok { "ok" } else { "FAILED" },
            entry["exit_code"].as_i64().unwrap_or(-1),
            entry["command"].as_str().unwrap_or("")
        );
        if !ok {
            let output = entry["output"].as_str().unwrap_or("");
            if !output.is_empty() {
                eprintln!("{output}");
            }
        }
    }
    if let Some(path) = report["verification_path"].as_str() {
        println!("verification: {path}");
    }
}

/// Run the user's acceptance commands in order (v1 contract): the first failure
/// stops the list, since later commands may depend on earlier ones.
pub fn run_checks(commands: &[String], workspace: &Path, timeout_s: u64) -> Vec<Json> {
    let mut results = Vec::new();
    for command in commands {
        let marker = format!("__TEAMAGENTS_CHECK_RC_{}__", uuid::Uuid::new_v4().simple());
        // the command runs in a subshell so that its own `exit` cannot skip the
        // marker: the marker, not the rendered transcript, is the verdict
        let wrapped = format!("( {command} ); rc=$?; printf '\\n%s%s\\n' '{marker}' \"$rc\"; exit $rc");
        let (output, exit_code, note) = match crate::tools::shell_run(&wrapped, workspace, timeout_s, false, None) {
            Ok(text) => {
                let (output, code) = parse_check_result(&text, &marker);
                (output, code, None)
            }
            // isolation/setup/timeout: the command never produced a status
            Err(reason) => (String::new(), -1, Some(reason)),
        };
        let ok = exit_code == 0;
        results.push(json!({
            "command": command,
            "ok": ok,
            "exit_code": exit_code,
            "output": capped(&output, CHECK_OUTPUT_CAP),
            "error": note,
        }));
        if !ok {
            break;
        }
    }
    results
}

/// Split a check transcript into (output, exit code). The exit code is the one
/// the *command* reported through the marker, so a command that prints
/// "(exit 0)" cannot fake success; a missing marker means "no status".
fn parse_check_result(text: &str, marker: &str) -> (String, i32) {
    let Some((before, after)) = text.rsplit_once(marker) else {
        return (text.trim_end().to_string(), -1);
    };
    let code = after.lines().next().and_then(|line| line.trim().parse().ok()).unwrap_or(-1);
    (before.trim_end().to_string(), code)
}

fn capped(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push_str("… (truncated)");
    out
}

/// The row a user named by id: the full id, or an unambiguous prefix of one (the
/// TUI and `--json` print the full id; a human types a short form). Shared by the
/// client surfaces that take an id, so they refuse an unknown or ambiguous name
/// with the same words.
pub(crate) fn resolve_prefix(
    rows: &[Json],
    key: &str,
    wanted: &str,
    what: &str,
    list_with: &str,
) -> Result<Json, (i32, String)> {
    if wanted.trim().is_empty() {
        return Err((2, format!("{what} needs an id; list them with `{list_with}`")));
    }
    let matching: Vec<&Json> =
        rows.iter().filter(|row| row[key].as_str().is_some_and(|id| id.starts_with(wanted))).collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err((2, format!("no {what} id starts with {wanted:?}; `{list_with}` shows the ids"))),
        many => Err((
            2,
            format!(
                "{wanted:?} matches {} {what} ids ({}); give more characters",
                many.len(),
                many.iter().filter_map(|row| row[key].as_str()).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

/// Persist the verification ledger next to the session database (the evidence
/// a CI job can archive) and report its path.
fn write_verification(state_root: &str, verification: &[Json]) -> Option<String> {
    if verification.is_empty() {
        return None;
    }
    let root = Path::new(state_root);
    if std::fs::create_dir_all(root).is_err() {
        return None;
    }
    let path = root.join("verification.json");
    let body = serde_json::to_vec_pretty(&json!({"verification": verification})).ok()?;
    match std::fs::write(&path, body) {
        Ok(()) => Some(path.to_string_lossy().into_owned()),
        Err(error) => {
            eprintln!("[exec] cannot write {}: {error}", path.display());
            None
        }
    }
}

/// The session's leader instance and its lifecycle: the conventional id first,
/// then any ACTIVE instance (a fresh session boots exactly one).
fn leader_instance(checkpoint: &Json) -> Option<(String, String)> {
    // the daemon's checkpoint wraps the read snapshot: {"snapshot": {…}}
    let instances = checkpoint["snapshot"]["instances"].as_array().or_else(|| checkpoint["instances"].as_array())?;
    let entry = instances
        .iter()
        .find(|entry| entry["id"] == json!("i-leader"))
        .or_else(|| instances.iter().find(|entry| entry["lifecycle"] == json!("ACTIVE")))?;
    Some((entry["id"].as_str()?.to_string(), entry["lifecycle"].as_str().unwrap_or("UNKNOWN").to_string()))
}

#[cfg(test)]
mod tests {
    use super::{capped, leader_instance, parse_check_result, run_checks, End};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn the_leader_instance_prefers_the_conventional_id() {
        let checkpoint = json!({"instances": [
            {"id": "i-worker", "lifecycle": "ACTIVE"},
            {"id": "i-leader", "lifecycle": "PARKED"},
        ]});
        // the conventional id wins, and its lifecycle travels with it so the
        // caller can see that this leader would not run anything
        assert_eq!(leader_instance(&checkpoint), Some(("i-leader".to_string(), "PARKED".to_string())));
        let only = json!({"instances": [{"id": "i-other", "lifecycle": "ACTIVE"}]});
        assert_eq!(leader_instance(&only), Some(("i-other".to_string(), "ACTIVE".to_string())));
        let none = json!({"instances": [{"id": "i-x", "lifecycle": "TERMINATED"}]});
        assert_eq!(leader_instance(&none), None);
    }

    /// The documented headless contract (D-32/D-49): a plain success is 0, a
    /// failed goal or a failed acceptance check is 1, nobody can answer an
    /// approval in a headless run so it is 3, and a deadline is 124.
    #[test]
    fn exit_codes_follow_the_documented_contract() {
        assert_eq!(End::Completed.exit_code(true), 0);
        assert_eq!(End::Completed.exit_code(false), 1);
        assert_eq!(End::Reply.exit_code(true), 0);
        assert_eq!(End::Reply.exit_code(false), 1);
        assert_eq!(End::Failed.exit_code(true), 1);
        assert_eq!(End::Failed.exit_code(false), 1);
        assert_eq!(End::ApprovalRequired.exit_code(true), 3);
        assert_eq!(End::Timeout.exit_code(true), 124);
    }

    /// The exit code comes from the marker the wrapper prints, not from
    /// anything the command itself wrote: a check cannot fake success by
    /// printing an exit-code line.
    #[test]
    fn the_check_verdict_reads_the_wrapper_marker() {
        let marker = "__TEAMAGENTS_CHECK_RC_fixed__";
        assert_eq!(parse_check_result("all good\n__TEAMAGENTS_CHECK_RC_fixed__0\n", marker), ("all good".into(), 0));
        assert_eq!(parse_check_result("boom\n__TEAMAGENTS_CHECK_RC_fixed__2\n", marker), ("boom".into(), 2));
        // a command that prints its own "(exit 0)" and no marker has no status
        assert_eq!(parse_check_result("faked (exit 0)\n", marker), ("faked (exit 0)".into(), -1));
    }

    #[test]
    fn long_check_output_is_capped() {
        assert_eq!(capped("short", 8), "short");
        assert!(capped(&"x".repeat(20), 8).starts_with("xxxxxxxx"));
        assert!(capped(&"x".repeat(20), 8).ends_with("(truncated)"));
    }

    /// The acceptance commands really run, in order, in the isolated shell and
    /// stop at the first failure. Shell isolation needs bubblewrap, so this is
    /// skipped where the kernel forbids unprivileged user namespaces (CI).
    #[test]
    fn acceptance_commands_run_in_order_and_stop_at_the_first_failure() {
        if !crate::tools::bwrap_available() {
            eprintln!("skipped: bwrap is unavailable");
            return;
        }
        let workspace = std::env::temp_dir();
        let workspace = Path::new(&workspace);
        let commands = vec!["echo first; exit 3".to_string(), "echo second".to_string()];
        let checks = run_checks(&commands, workspace, 60);
        assert_eq!(checks.len(), 1, "the first failure stops the list: {checks:?}");
        assert_eq!(checks[0]["ok"], json!(false));
        assert_eq!(checks[0]["exit_code"], json!(3));
        assert!(checks[0]["output"].as_str().unwrap().contains("first"), "{checks:?}");

        let passing = vec!["echo one".to_string(), "true".to_string()];
        let checks = run_checks(&passing, workspace, 60);
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|check| check["ok"] == json!(true)), "{checks:?}");
        assert_eq!(checks[0]["exit_code"], json!(0));
    }
}
