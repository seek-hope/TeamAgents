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
    /// The input never landed: it waited for a boundary and a context reset
    /// sealed it with its epoch before the instance got to it. Nothing of this
    /// run happened, so there is nothing to report as its outcome — exit 1
    /// (D-72).
    Undelivered,
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
            End::Undelivered => "undelivered",
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
            End::Completed | End::Reply | End::Unsettled | End::Undelivered | End::Failed => 1,
            End::ApprovalRequired => 3,
            End::Timeout => 124,
        }
    }
}

/// Output kept per acceptance command, in the report and on stderr.
const CHECK_OUTPUT_CAP: usize = 8000;

/// What this run's **own** input produced, read from one snapshot of the
/// conversation (D-72). A client may only report an outcome that belongs to its
/// input: the session can be busy with an earlier turn when `exec` submits, so a
/// settlement or a reply recorded *before* this input landed belongs to that
/// other turn. `exec` therefore finds its own entry (by the envelope id it
/// generated) and reads the first turn-ending entry after it: that entry is its
/// answer, and everything else is somebody else's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attribution {
    /// The entry that ends the turn which started at this input: the goal settled.
    Settled(String),
    /// The entry that ends that turn: the member's own text.
    Reply(String),
    /// The entry that ends that turn: the runtime's closing word, nothing settled.
    Closed,
    /// No turn has ended at this input yet.
    Pending,
}

impl Attribution {
    /// A turn ends at the member's own text (a message with no pending tool
    /// calls) or at the runtime's closing word. Tool traffic does not end it.
    fn ends_a_turn(entry: &Json) -> bool {
        match entry["kind"].as_str().unwrap_or("") {
            "assistant" => {
                let text = entry["message"]["content"].as_str().unwrap_or("");
                let calls = entry["message"]["tool_calls"].as_array().map(Vec::len).unwrap_or(0);
                !text.trim().is_empty() && calls == 0
            }
            "runtime" => true,
            _ => false,
        }
    }

    /// The runtime's own word for a settlement, told apart from a closed turn by
    /// the envelope the runtime generated for it (`goal-close-*`, `goal-block-*`).
    fn settles_a_goal(entry: &Json) -> bool {
        entry["kind"] == json!("runtime")
            && entry["envelope_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("goal-close-") || id.starts_with("goal-block-"))
    }

    fn read(envelope: &str, goal_status: Option<&str>, history: &[Json]) -> Attribution {
        let Some(mine) = history.iter().position(|entry| entry["envelope_id"].as_str() == Some(envelope)) else {
            // the input has not landed yet (a turn was in flight when it arrived):
            // nothing the session did in the meantime is its outcome
            return Attribution::Pending;
        };
        let Some(end) = history[mine + 1..].iter().find(|entry| Attribution::ends_a_turn(entry)) else {
            return Attribution::Pending;
        };
        if Attribution::settles_a_goal(end) {
            // the settlement's own word was written in the same transaction as the
            // goal's status, so a checkpoint that still says ACTIVE has simply not
            // caught up: decide on the next pass instead of guessing a status
            return match goal_status {
                Some(status) if status != "ACTIVE" => Attribution::Settled(status.to_string()),
                _ => Attribution::Pending,
            };
        }
        match end["kind"].as_str().unwrap_or("") {
            "assistant" => Attribution::Reply(end["message"]["content"].as_str().unwrap_or("").trim().to_string()),
            "runtime" => Attribution::Closed,
            _ => Attribution::Pending,
        }
    }
}

/// The outcome of one headless run: the JSON report the `--json` mode prints,
/// the terminal state that decides the exit code, and the acceptance verdict.
pub struct ExecRun {
    pub report: Json,
    pub end: End,
    pub checks_ok: bool,
}

/// The runtime's own words for a fact that ends *this* run while its turn can never finish: a request it
/// refused before it began (a goal budget ceiling, A18; a passed goal deadline, A35 — D-97), an instance the
/// user retired mid-run (D-98), or a reset that took the turn's epoch away (D-100). `None` for every other
/// kind. Keeping every such fact in one place is deliberate: the client used to learn about them one at a
/// time, and three of them were found only by driving the product (`end=timeout` for work nobody was doing).
fn run_fate(event: &Json, instance: &str) -> Option<String> {
    let payload = &event["payload"];
    match event["kind"].as_str()? {
        "budget_refused" => Some(format!(
            "goal {} budget exceeded: known {} + reserved {} + est {} > max {}",
            payload["goal_id"].as_str().unwrap_or("?"),
            payload["known"].as_i64().unwrap_or(0),
            payload["reserved"].as_i64().unwrap_or(0),
            payload["est"].as_i64().unwrap_or(0),
            payload["max"].as_i64().unwrap_or(0),
        )),
        "goal_deadline_refused" => Some(format!(
            "goal {} deadline passed before request {} could start",
            payload["goal_id"].as_str().unwrap_or("?"),
            payload["request_id"].as_str().unwrap_or("?"),
        )),
        // The user retired the instance: the control plane closes the epoch's execution, so nothing will ever
        // answer this run. A *pause* is deliberately absent — it stops the instance at a safe boundary, the
        // turn may still finish, and a resumed instance lets the run report the turn's own outcome (D-98).
        "instance_lifecycle" if payload["lifecycle"] == json!("TERMINATED") => Some(format!(
            "instance {instance} is terminated; this run cannot finish (termination is final — start a fresh \
             state root for new work)"
        )),
        // A reset moves the instance to a new epoch: the turn this run was waiting on goes with the old one.
        "instance_reset" => Some(format!(
            "instance {instance} was reset while this run was in flight (epoch {} → {}); its turn is gone, so \
             nothing will answer this input",
            payload["old_epoch"].as_i64().unwrap_or(-1),
            payload["new_epoch"].as_i64().unwrap_or(-1),
        )),
        _ => None,
    }
}

/// One headless run with no printing at all: submit the prompt to the leader,
/// follow the run to its terminal state, then run the user's acceptance
/// commands. Errors are (exit code, message).
pub fn execute(options: &ExecOptions) -> Result<ExecRun, (i32, String)> {
    let mut client = Client::connect(&options.socket)
        .map_err(|error| (2, format!("exec: {error}; start teamagents daemon first (or just run teamagents)")))?;
    // Drain the events recorded before this input and keep the watermark: a
    // goal settlement from an earlier run is history, not this run's outcome.
    client.events().map_err(|error| socket_lost("reading the event stream", &error))?;
    let checkpoint =
        client.call("checkpoint", json!({})).map_err(|error| socket_lost("reading the checkpoint", &error))?;
    let (instance, lifecycle) = leader_instance(&checkpoint)
        .ok_or_else(|| (2, format!("exec: the session has no usable leader instance: {checkpoint}")))?;
    // A parked or paused leader will not run the input: saying so beats
    // submitting work that sits in a queue nobody is draining until the caller's
    // own deadline expires (the user resumes it in the TUI, §5.4). A *terminated*
    // leader is different advice: termination is final — the instance is retired and
    // the control plane refuses `set_lifecycle` on it ("instance … is terminated"), so
    // pointing at the resume key would send the user to a refusal.
    if lifecycle != "ACTIVE" {
        let advice = if lifecycle == "TERMINATED" {
            format!(
                "exec: the leader instance {instance} is TERMINATED; termination is final (the instance is \
                 retired), so this session cannot take new input. Start a fresh state root instead \
                 (--state-root <new directory>); nothing was submitted."
            )
        } else {
            format!(
                "exec: the leader instance {instance} is {lifecycle}; new input would not run. \
                 Resume it (`teamagents instances resume --id {instance}`, or `r` in the TUI instances panel) \
                 or use a fresh state root; nothing was submitted."
            )
        };
        return Err((2, advice));
    }
    let envelope = format!("env-{}", uuid::Uuid::new_v4());
    let submitted = client
        .command(
            &format!("input-{envelope}"),
            "submit_input",
            json!({"instance_id": instance, "envelope_id": envelope, "text": options.prompt}),
        )
        .map_err(|error| socket_lost("submitting the input", &error))?;
    // A turn that was already in flight when this run started cannot include the
    // input: it waits for the boundary and enters the conversation after that
    // turn's own reply (D-63). Report it instead of pretending it landed.
    let queued = submitted["queued"] == json!(true);
    let deadline = Instant::now() + Duration::from_secs(options.timeout_s);
    let mut goal_status: Option<String> = None;
    let mut turn_failure: Option<String> = None;
    let mut reply: Option<String> = None;
    let mut pending_approval: Option<String> = None;
    // set when the runtime refused a request before it began (budget ceiling, passed goal deadline)
    let mut refused: Option<String> = None;
    // the lifecycle as the loop last saw it: a run that ends on its deadline has to say *what* the instance
    // was doing, because "still running" is wrong for a paused or retired one (D-98)
    let mut lifecycle_now = String::new();
    // set when the runtime names *this* envelope among the ones it sealed
    let mut undelivered = false;
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
                    // A permanently failed leader request is a terminal outcome
                    // of this run: report it instead of waiting out the deadline.
                    // Failures of *other* instances belong to the leader's turn,
                    // not to this client.
                    if event["kind"] == json!("request_failed") && event["scope"] == json!(instance) {
                        turn_failure =
                            Some(event["payload"]["reason"].as_str().unwrap_or("the model request failed").to_string());
                    }
                    // One place decides whether this event ends the run while its turn can never finish (see
                    // `run_fate`): a refused request, a retired instance, a reset that took the epoch away.
                    // Reporting any of them as `124` would tell the caller "still running" about work nobody is
                    // doing, and burn its deadline waiting for it.
                    if event["scope"] == json!(instance) {
                        if let Some(reason) = run_fate(event, &instance) {
                            // remembered separately from `turn_failure`: a run whose turn never started (or
                            // never finished) has no acceptance to verify either (D-96/D-97)
                            refused.get_or_insert_with(|| reason.clone());
                            turn_failure.get_or_insert(reason);
                        }
                    }
                    // A queued input is sealed, not delivered, when a context reset
                    // moves the epoch before the boundary reaches it (§5.3/A24): the
                    // run has nothing to wait for and must not call its own deadline
                    // a timeout (D-72). The runtime names every envelope it seals.
                    if event["kind"] == json!("envelopes_sealed") && event["scope"] == json!(instance) {
                        let sealed = event["payload"]["envelope_ids"].as_array();
                        if sealed.is_some_and(|ids| ids.iter().any(|id| id == &json!(envelope))) {
                            undelivered = true;
                        }
                    }
                }
            }
            Err(error) => return Err(socket_lost("reading the event stream", &error)),
        }
        let instances = snapshot["snapshot"]["instances"]
            .as_array()
            .or_else(|| snapshot["instances"].as_array())
            .cloned()
            .unwrap_or_default();
        if let Some(lifecycle) =
            instances.iter().find(|row| row["id"] == json!(instance)).and_then(|row| row["lifecycle"].as_str())
        {
            lifecycle_now = lifecycle.to_string();
        }
        let settled = instances.iter().any(|entry| entry["id"] == json!(instance) && entry["phase"] == json!("READY"));
        // What *this* run's input produced (D-72): its own entry, and the first
        // turn-ending entry after it. A settlement or a reply that happened before
        // this input landed belongs to another turn, however the instance looks now.
        let goal_now = snapshot["snapshot"]["goal"]["status"].as_str().or_else(|| snapshot["goal"]["status"].as_str());
        let mut attribution = Attribution::Pending;
        if settled {
            if let Ok(entries) = client.history(&instance) {
                attribution = Attribution::read(&envelope, goal_now, &entries);
            }
        }
        match &attribution {
            Attribution::Settled(status) => goal_status = Some(status.clone()),
            Attribution::Reply(text) => reply = Some(text.clone()),
            _ => {}
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
        // The run's own outcome first: what this input's turn produced. A
        // session-level fact that makes the run impossible (a failed request, an
        // approval no headless caller can give, an input a reset sealed) is
        // reported only while nothing of the run's own has happened.
        // `undelivered` comes first: it is a fact the runtime stated *about this input's envelope* (a reset
        // named it as sealed), which is more precise than "the run failed" — and a reset sets both (D-72).
        let terminal = match &attribution {
            Attribution::Settled(status) => Some(if status == "SUCCEEDED" { End::Completed } else { End::Failed }),
            Attribution::Reply(_) => Some(End::Reply),
            Attribution::Closed => Some(End::Unsettled),
            Attribution::Pending if undelivered => Some(End::Undelivered),
            Attribution::Pending if turn_failure.is_some() => Some(End::Failed),
            Attribution::Pending if pending_approval.is_some() => Some(End::ApprovalRequired),
            Attribution::Pending => None,
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
    // Nothing runs when the turn is not finished — an approval no headless caller can give, the caller's own
    // deadline passing, or a request the runtime *refused before it began* (a budget ceiling, a passed goal
    // deadline: `refused`) — because there is no acceptance to verify (§8), or when the input never landed,
    // where there is no turn to accept at all (D-72). The deadline belongs here for a second reason: a check
    // inherits `timeout_s`, so running one *after* the deadline would stretch the run's wall clock past the
    // bound the caller set (measured 2026-09-26: `--timeout 3` plus a check took 7 s and wrote a ledger of
    // verdicts for a turn that never finished).
    let verification = if options.checks.is_empty()
        || refused.is_some()
        || matches!(end, End::ApprovalRequired | End::Undelivered | End::Timeout)
    {
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
        "instance_lifecycle": if lifecycle_now.is_empty() { "ACTIVE" } else { lifecycle_now.as_str() },
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

/// The daemon's socket died under a run — say what that means for the user.
///
/// The daemon can be stopped on purpose: SIGTERM is the supported stop since D-150, and a client that was
/// waiting on it then sees a bare transport error. Measured 2026-09-27: `exec: the event stream broke: daemon
/// write: Broken pipe (os error 32)` — accurate and useless, because a user cannot tell a crashed daemon from a
/// stopped one from a lost run. The session and its committed state survive a stop, so the message says so and
/// names the way on; a failure that is *not* a lost socket keeps the plain wording, since calling every failure
/// a stopped daemon would hide the real one. The exit code is the documented `2` either way (§1.1).
fn socket_lost(what: &str, error: &str) -> (i32, String) {
    let lost = ["Broken pipe", "Connection reset", "UnexpectedEof", "os error 32", "os error 104", "connection closed"]
        .iter()
        .any(|marker| error.contains(marker));
    if lost {
        (
            2,
            format!(
                "exec: the daemon's socket was lost {what} ({error}) — the session and its committed state \
                     are kept; start the daemon again (`teamagents` or `exec`) to continue"
            ),
        )
    } else {
        (2, format!("exec: {what} failed: {error}"))
    }
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
            "{}",
            timeout_line(
                report["instance_id"].as_str().unwrap_or("the leader"),
                report["instance_lifecycle"].as_str().unwrap_or("ACTIVE"),
                options.timeout_s
            )
        ),
        // the instance stopped on the runtime's own closing word and no goal
        // settled in this run: say so instead of leaving the caller to guess
        // from a missing reply (D-71)
        "unsettled" => println!(
            "the turn closed without settling anything: no goal ended in this run and the last word was the runtime's"
        ),
        "undelivered" => println!(
            "undelivered: the input waited behind a running turn and a context reset sealed it before it landed; nothing was answered"
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

/// The one-line verdict for a run that ended on its deadline. "Still running" is only true for an ACTIVE
/// instance: a paused one is stopped at a boundary (its turn continues when it is resumed), and a retired one
/// cannot continue at all — the caller should be told which of the three it is (D-98).
fn timeout_line(instance: &str, lifecycle: &str, seconds: u64) -> String {
    match lifecycle {
        "ACTIVE" => format!("timed out: {instance} is still running ({seconds}s)"),
        "TERMINATED" => format!(
            "timed out: {instance} is TERMINATED; this run cannot finish (termination is final — start a fresh \
             state root for new work)"
        ),
        other => format!(
            "timed out: {instance} is {other}, so its turn cannot finish ({seconds}s); resume it with \
             `teamagents instances resume --id {instance}`"
        ),
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
    use super::{
        capped, leader_instance, parse_check_result, run_checks, run_fate, socket_lost, timeout_line, Attribution, End,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn a_deadline_says_what_the_instance_was_doing() {
        assert_eq!(timeout_line("i-leader", "ACTIVE", 60), "timed out: i-leader is still running (60s)");
        let paused = timeout_line("i-leader", "PAUSED", 60);
        assert!(paused.contains("is PAUSED") && paused.contains("instances resume --id i-leader"), "{paused}");
        let retired = timeout_line("i-leader", "TERMINATED", 60);
        assert!(retired.contains("TERMINATED") && retired.contains("termination is final"), "{retired}");
    }

    #[test]
    fn the_facts_that_end_a_run_are_named_in_the_runtimes_words() {
        // the budget gate's committed event: the sentence matches what the runtime parks the instance with
        let budget = json!({"kind": "budget_refused", "scope": "i-leader", "payload": {
            "goal_id": "goal-s-main", "request_id": "req-1", "known": 0, "reserved": 0, "est": 2235, "max": 1000}});
        assert_eq!(
            run_fate(&budget, "i-leader").unwrap(),
            "goal goal-s-main budget exceeded: known 0 + reserved 0 + est 2235 > max 1000"
        );
        // the deadline gate's event
        let deadline = json!({"kind": "goal_deadline_refused", "scope": "i-leader",
                              "payload": {"goal_id": "goal-s-main", "request_id": "req-2"}});
        assert_eq!(
            run_fate(&deadline, "i-leader").unwrap(),
            "goal goal-s-main deadline passed before request req-2 could start"
        );
        // a retired instance ends the run; a *paused* one must not (D-98)
        let terminated =
            json!({"kind": "instance_lifecycle", "scope": "i-leader", "payload": {"lifecycle": "TERMINATED"}});
        assert!(run_fate(&terminated, "i-leader").unwrap().contains("termination is final"));
        let paused = json!({"kind": "instance_lifecycle", "scope": "i-leader", "payload": {"lifecycle": "PAUSED"}});
        assert!(run_fate(&paused, "i-leader").is_none());
        // a reset names the epoch move (D-100)
        let reset = json!({"kind": "instance_reset", "scope": "i-leader",
                           "payload": {"old_epoch": 0, "new_epoch": 1, "reason": "probe"}});
        assert!(run_fate(&reset, "i-leader").unwrap().contains("epoch 0 → 1"));
        // every other event is not a fate, and a malformed payload never panics
        assert!(run_fate(&json!({"kind": "request_failed", "payload": {}}), "i-leader").is_none());
        assert!(run_fate(&json!({"kind": "budget_refused"}), "i-leader").is_some());
    }

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

    /// A daemon that stops under a run is a supported thing to do (D-150), so the client says what it means
    /// rather than printing the errno: measured 2026-09-27, `exec: the event stream broke: daemon write: Broken
    /// pipe (os error 32)` was the whole of it. A failure that is *not* a lost socket keeps the plain wording —
    /// calling every failure a stopped daemon would hide the real one — and the exit code is `2` either way.
    #[test]
    fn a_lost_socket_is_reported_as_what_it_means() {
        let (code, message) = socket_lost("reading the event stream", "daemon write: Broken pipe (os error 32)");
        assert_eq!(code, 2);
        assert!(message.contains("the daemon's socket was lost reading the event stream"), "{message}");
        assert!(message.contains("committed state are kept"), "{message}");
        assert!(message.contains("start the daemon again"), "{message}");
        for transport in ["Connection reset by peer", "UnexpectedEof", "os error 104", "connection closed"] {
            assert!(socket_lost("submitting the input", transport).1.contains("was lost"), "{transport}");
        }
        let (code, message) = socket_lost("submitting the input", "no such instance: i-ghost");
        assert_eq!(code, 2);
        assert_eq!(message, "exec: submitting the input failed: no such instance: i-ghost");
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
        assert_eq!(End::Unsettled.exit_code(true), 1, "a closed turn settled nothing");
        assert_eq!(End::Undelivered.exit_code(true), 1, "an input that never landed delivered nothing");
        assert_eq!(End::Failed.exit_code(true), 1);
        assert_eq!(End::Failed.exit_code(false), 1);
        assert_eq!(End::ApprovalRequired.exit_code(true), 3);
        assert_eq!(End::Timeout.exit_code(true), 124);
    }

    /// D-72: the client reports what *its own* input produced. The conversation
    /// is one snapshot, so the rule is positional: find this run's entry, then read
    /// the first turn-ending entry after it — a settlement or a reply that happened
    /// before this input landed belongs to the turn the input was not part of.
    #[test]
    fn an_outcome_before_the_runs_own_input_is_not_its_outcome() {
        let user = |envelope: &str, text: &str| json!({"kind": "user", "envelope_id": envelope, "message": {"role": "user", "content": text}});
        let reply = |text: &str| json!({"kind": "assistant", "message": {"role": "assistant", "content": text}});
        let call = json!({"kind": "assistant", "message": {"role": "assistant", "content": "",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{}"}}]}});
        let result =
            json!({"kind": "tool_result", "message": {"role": "tool", "tool_call_id": "c1", "content": "out"}});
        let close = json!({"kind": "runtime", "envelope_id": "goal-close-g1",
                           "message": {"role": "user", "content": "runtime: goal g1 closed as SUCCEEDED"}});
        let turn_close = json!({"kind": "runtime", "envelope_id": "turn-close-i-leader",
                                "message": {"role": "user", "content": "runtime: turn closed"}});

        // the run's input never landed: nothing in the conversation is its outcome
        let queued = vec![user("env-old", "first question"), reply("first answer")];
        assert_eq!(Attribution::read("env-mine", Some("ACTIVE"), &queued), Attribution::Pending);
        // …even when an earlier turn settled the goal on its way out
        let settled_earlier = vec![reply("first answer"), close.clone(), user("env-mine", "second question")];
        assert_eq!(Attribution::read("env-mine", Some("SUCCEEDED"), &settled_earlier), Attribution::Pending);

        // this run's own turn: tool traffic does not end it, its reply does
        let mine = vec![
            reply("first answer"),
            close.clone(),
            user("env-mine", "second question"),
            call.clone(),
            result,
            reply("the answer to the second question"),
        ];
        assert_eq!(
            Attribution::read("env-mine", Some("SUCCEEDED"), &mine),
            Attribution::Reply("the answer to the second question".to_string())
        );
        // a later turn's entries are not mine either: the first ending after my
        // entry is the one that answers me
        let mut later = mine.clone();
        later.push(user("env-someone-else", "third question"));
        later.push(reply("an answer to somebody else"));
        assert_eq!(
            Attribution::read("env-mine", Some("SUCCEEDED"), &later),
            Attribution::Reply("the answer to the second question".to_string())
        );

        // the settlement this run caused: the note follows its own entry, and the
        // status comes from the checkpoint (a checkpoint that has not caught up yet
        // is not a licence to guess)
        let settled = vec![user("env-old", "first"), reply("first answer"), user("env-mine", "second"), close.clone()];
        assert_eq!(
            Attribution::read("env-mine", Some("SUCCEEDED"), &settled),
            Attribution::Settled("SUCCEEDED".into())
        );
        assert_eq!(Attribution::read("env-mine", Some("ACTIVE"), &settled), Attribution::Pending);
        // a closed turn is the runtime's word too, but it settles nothing
        let closed = vec![user("env-mine", "second"), turn_close];
        assert_eq!(Attribution::read("env-mine", Some("SUCCEEDED"), &closed), Attribution::Closed);
        // a bare tool call is not an answer (D-49)
        let calling = vec![user("env-mine", "second"), call];
        assert_eq!(Attribution::read("env-mine", Some("ACTIVE"), &calling), Attribution::Pending);
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
    /// stop at the first failure. Where the kernel forbids unprivileged user
    /// namespaces (CI, D-113) there is no sandbox to run in, and the claim that
    /// *is* observable there is the fail-closed one: a check that cannot run is
    /// never a pass and never runs on the host.
    #[test]
    fn acceptance_commands_run_in_order_and_stop_at_the_first_failure() {
        let workspace = std::env::temp_dir();
        let workspace = Path::new(&workspace);
        if !crate::tools::sandbox_usable() {
            // no isolation anywhere: every check must be refused, the list must stop at the first one, and no
            // command output may appear (the command never ran)
            let refused = vec!["echo one".to_string(), "true".to_string()];
            let checks = run_checks(&refused, workspace, 60);
            assert_eq!(checks.len(), 1, "the first refusal stops the list: {checks:?}");
            assert_eq!(checks[0]["ok"], json!(false), "a check that cannot run is not a pass: {checks:?}");
            assert_eq!(checks[0]["exit_code"], json!(-1), "{checks:?}");
            assert!(checks[0]["output"].as_str().unwrap_or("").is_empty(), "{checks:?}");
            let error = checks[0]["error"].as_str().unwrap_or("");
            assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation: {checks:?}");
            return;
        }
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
