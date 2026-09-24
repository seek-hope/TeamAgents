//! Executable correspondence between the TLA+ specs and the code (the control plane).
//!
//! `verification/tla/*.tla` model-checks an abstract state machine; this test recomputes the
//! same invariants against the real `core::v2::Control`: it enumerates every command sequence
//! up to length 2, then runs a fixed-seed random walk, checking the facts in SQLite after
//! every step. Run it with:
//!
//! ```text
//! cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants
//! ```
//!
//! Coverage (the parenthesised names are the properties in the TLA+ specs):
//! `TypeOK`, settled tasks are final (`SettledIsFinal`), the narrow return path is revoked when a task settles
//! (`ReturnPathOnlyWhileOpen`), dependencies point at earlier tasks (`DependenciesPointBackwards`),
//! a dead assignee holds no open task (`NoOpenTaskOnDeadAssignee`), no instance points at a settled goal
//! (`NoStaleActiveGoal`, V-G1), closing a request releases its reservation (`ReservationReleased`),
//! one active request per instance (`OneActiveRequest`), a selected attempt is complete (`SelectionIsComplete`),
//! a resolved wait answers its tool_call (`ResolvedWaitIsAnswered`, V-W1), no effect before approval
//! (`NoEffectBeforeApproval`), LIVE artifacts have bytes (`LiveIsPersisted`).

use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use teamagents_core::v2::control::{Command, Control, Identity};

const LIFECYCLES: &[&str] = &["ACTIVE", "PAUSED", "PARKED", "TERMINATED"];
const PHASES: &[&str] = &["READY", "MODEL_PENDING", "TOOLS_PENDING", "WAITING", "COMPLETION_PENDING"];
const GOAL_STATUSES: &[&str] = &["ACTIVE", "SUCCEEDED", "FAILED", "BLOCKED"];
const TERMINAL_GOAL: &[&str] = &["SUCCEEDED", "FAILED", "BLOCKED"];
const TASK_STATUSES: &[&str] = &["PENDING", "RUNNING", "BLOCKED", "SUCCEEDED", "FAILED", "CANCELLED"];
const OPEN_TASKS: &[&str] = &["PENDING", "RUNNING", "BLOCKED"];
const SETTLED_TASKS: &[&str] = &["SUCCEEDED", "FAILED", "CANCELLED"];
const WAIT_STATUSES: &[&str] = &["PENDING", "SATISFIED", "CANCELLED"];
const REQUEST_STATUSES: &[&str] = &["PENDING", "COMPLETE", "FAILED", "CANCELLED"];
const OP_STATUSES: &[&str] = &[
    "PREPARED",
    "DISPATCH_COMMITTED",
    "RUNNING",
    "SUCCEEDED",
    "FAILED",
    "CANCELLED",
    "CANCELLED_BEFORE_START",
    "OUTCOME_UNKNOWN",
];
const TERMINAL_OPS: &[&str] = &["SUCCEEDED", "FAILED", "CANCELLED", "CANCELLED_BEFORE_START", "OUTCOME_UNKNOWN"];
const ARTIFACT_STATES: &[&str] = &["STAGING", "LIVE", "DELETING", "ABANDONED"];
const APPROVAL_STATUSES: &[&str] = &["PENDING", "APPROVED", "DENIED", "EXPIRED"];
const ATTEMPT_STATUSES: &[&str] = &["COMPLETE", "FAILED", "CANCELLED"];

fn contains(set: &[&str], value: &str) -> bool {
    set.contains(&value)
}

/// Key states the walk reached (proof that the exploration is not vacuous)
#[derive(Default, Clone, Copy)]
struct Coverage {
    resolved_wait: bool,
    settled_goal: bool,
    settled_task: bool,
    settled_operation: bool,
    reset_epoch: bool,
    terminated: bool,
    artifact_live: bool,
    compressed: bool,
    approval_decided: bool,
    replay_checked: bool,
    revocation_checked: bool,
}

impl Coverage {
    fn merge(&mut self, other: Coverage) {
        self.resolved_wait |= other.resolved_wait;
        self.settled_goal |= other.settled_goal;
        self.settled_task |= other.settled_task;
        self.settled_operation |= other.settled_operation;
        self.reset_epoch |= other.reset_epoch;
        self.terminated |= other.terminated;
        self.artifact_live |= other.artifact_live;
        self.compressed |= other.compressed;
        self.approval_decided |= other.approval_decided;
        self.replay_checked |= other.replay_checked;
        self.revocation_checked |= other.revocation_checked;
    }
}

/// One command step: a name (for the failure trace) and the actual submission.
struct Step {
    label: String,
    command: Command,
    identity: Identity,
}

struct Harness {
    ctl: Control,
    path: PathBuf,
    rng: u64,
    counter: u64,
    trace: Vec<String>,
    /// task id -> settled status seen (`SettledIsFinal` memory across steps)
    settled: HashMap<String, String>,
    /// context entries seen (`NoEntryIsEverLost` memory across steps)
    entries_seen: HashSet<String>,
    /// covered entries seen (`CoverageNeverLifted` memory across steps)
    covered_seen: HashSet<String>,
    /// approval id -> decision seen (`ApprovalDecisionIsFinal` memory across steps)
    approvals_seen: HashMap<String, String>,
    /// command id -> stored receipt (`ReceiptsAreStable` memory across steps)
    receipts_seen: HashMap<String, String>,
    /// the last successfully submitted command and its identity (for replay steps)
    last_command: Option<(Command, Identity)>,
    /// the sizes of three tables (commands / events / context entries) after the last step; a replay must not move them
    sizes: (usize, usize, usize),
    /// the previous step's label (to tell whether it was a replay)
    last_label: String,
    /// Replay coverage: the same id with the same payload succeeded, and the same id with a different payload was refused
    replayed: bool,
    divergent_refused: bool,
    /// Revocation coverage: an effective grant was revoked and the following dispatch was refused
    revoked: bool,
    dispatch_refused_after_revoke: bool,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}-wal", self.path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

impl Harness {
    fn new(tag: &str) -> Harness {
        let path = std::env::temp_dir().join(format!("teamagents-v2-invariants-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let ctl = Control::open(&path, "s1", true).expect("open control");
        Harness {
            ctl,
            path,
            rng: 0x9E3779B97F4A7C15,
            counter: 0,
            trace: Vec::new(),
            settled: HashMap::new(),
            entries_seen: HashSet::new(),
            covered_seen: HashSet::new(),
            approvals_seen: HashMap::new(),
            receipts_seen: HashMap::new(),
            last_command: None,
            sizes: (0, 0, 0),
            last_label: String::new(),
            replayed: false,
            divergent_refused: false,
            revoked: false,
            dispatch_refused_after_revoke: false,
        }
    }

    fn next_rand(&mut self) -> u64 {
        // splitmix64: a fixed seed keeps the walk reproducible
        self.rng = self.rng.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, items: &'a [String]) -> Option<&'a String> {
        if items.is_empty() {
            return None;
        }
        let index = (self.next_rand() % items.len() as u64) as usize;
        items.get(index)
    }

    /// A stable number for a "planned" resource (request or task ids and the like)
    fn plan_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// Pick one row from a table (multi-column objects such as tasks and requests)
    fn pick_rows(&mut self, rows: &[Vec<String>]) -> Option<Vec<String>> {
        if rows.is_empty() {
            return None;
        }
        let index = (self.next_rand() % rows.len() as u64) as usize;
        rows.get(index).cloned()
    }

    fn id(&mut self) -> String {
        self.counter += 1;
        format!("c{}", self.counter)
    }

    /// Run one step and record the outcome in the trace; a label starting with `!` means the spec requires the step to be refused.
    fn run_step(&mut self, step: Step) -> Result<Json, String> {
        let Step { label, command, identity } = step;
        // a replay step carries its own command id (a client re-sends the same id after reconnecting); other steps get a fresh id
        let command =
            if command.command_id.is_empty() { Command { command_id: self.id(), ..command } } else { command };
        self.last_label = label.split('(').next().unwrap_or("").to_string();
        let result = self.ctl.submit(command.clone(), identity.clone());
        let outcome = match &result {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("err({})", error.chars().take(48).collect::<String>()),
        };
        self.trace.push(format!("{label} -> {outcome}"));
        if self.trace.len() > 40 {
            self.trace.remove(0);
        }
        if label.starts_with('!') {
            assert!(result.is_err(), "the spec requires a refusal but the code accepted: {label}");
        }
        if self.last_label == "replay_same" && result.is_ok() {
            self.replayed = true;
        }
        if self.last_label == "!replay_divergent" && result.is_err() {
            self.divergent_refused = true;
        }
        if self.last_label == "revoke_grant" && result.is_ok() {
            self.revoked = true;
        }
        if self.last_label == "!dispatch_without_grant" && result.is_err() {
            self.dispatch_refused_after_revoke = true;
        }
        if result.is_ok() {
            // remember replays too (the same id can be replayed again)
            self.last_command = Some((command, identity));
        }
        result
    }

    // ---------------------------------------------------------------- state --
    fn rows(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<Vec<String>> {
        let mut stmt = self.ctl.connection().prepare(sql).expect("prepare");
        let columns = stmt.column_count();
        let mut rows = stmt.query(params).expect("query");
        let mut out = Vec::new();
        while let Some(row) = rows.next().expect("row") {
            let mut values = Vec::with_capacity(columns);
            for index in 0..columns {
                // a column may hold an integer (counts, revisions), a float (timestamps) or text: read it as a string
                let value: rusqlite::types::Value = row.get(index).unwrap_or(rusqlite::types::Value::Null);
                values.push(match value {
                    rusqlite::types::Value::Null => String::new(),
                    rusqlite::types::Value::Integer(number) => number.to_string(),
                    rusqlite::types::Value::Real(number) => format!("{number}"),
                    rusqlite::types::Value::Text(text) => text,
                    rusqlite::types::Value::Blob(_) => "<blob>".into(),
                });
            }
            out.push(values);
        }
        out
    }

    fn instances(&self) -> Vec<Vec<String>> {
        self.rows("SELECT id, lifecycle, phase, COALESCE(active_goal_id,''), COALESCE(active_request_id,''), context_epoch FROM instances ORDER BY id", &[])
    }
    fn goals(&self) -> Vec<Vec<String>> {
        self.rows("SELECT id, status, reservations_json FROM goals ORDER BY id", &[])
    }
    fn tasks(&self) -> Vec<Vec<String>> {
        self.rows("SELECT id, goal_id, assignee, requester, status, dependencies_json FROM tasks ORDER BY rowid", &[])
    }
    fn waits(&self) -> Vec<Vec<String>> {
        self.rows("SELECT id, instance_id, status, epoch FROM waits ORDER BY id", &[])
    }
    fn requests(&self) -> Vec<Vec<String>> {
        self.rows("SELECT request_id, instance_id, status, COALESCE(selected_attempt_id,''), kind FROM model_requests ORDER BY rowid", &[])
    }
    fn operations(&self) -> Vec<Vec<String>> {
        self.rows(
            "SELECT operation_id, status, CASE WHEN receipt_json IS NULL THEN '' ELSE 'receipt' END, decision_id,
                    CASE WHEN receipt_json IS NOT NULL
                              AND json_extract(receipt_json, '$.started') = 1 THEN 'started' ELSE '' END
             FROM operations ORDER BY operation_id",
            &[],
        )
    }
    // ------------------------------------------------------------ invariants --
    /// Whether this walk reached states worth checking (proof that it is not vacuous)
    fn coverage(&self) -> Coverage {
        let exists = |sql: &str| !self.rows(sql, &[]).is_empty();
        Coverage {
            resolved_wait: exists("SELECT id FROM waits WHERE status IN ('SATISFIED', 'CANCELLED')"),
            settled_goal: exists("SELECT id FROM goals WHERE status IN ('SUCCEEDED', 'FAILED', 'BLOCKED')"),
            settled_task: exists("SELECT id FROM tasks WHERE status IN ('SUCCEEDED', 'FAILED', 'CANCELLED')"),
            settled_operation: exists(
                "SELECT operation_id FROM operations
                 WHERE status IN ('SUCCEEDED', 'FAILED', 'CANCELLED', 'CANCELLED_BEFORE_START', 'OUTCOME_UNKNOWN')",
            ),
            reset_epoch: exists("SELECT id FROM instances WHERE context_epoch > 0"),
            terminated: exists("SELECT id FROM instances WHERE lifecycle = 'TERMINATED'"),
            artifact_live: exists("SELECT id FROM artifacts WHERE completeness = 'LIVE'"),
            compressed: exists("SELECT id FROM context_entries WHERE compressed_by IS NOT NULL"),
            approval_decided: exists("SELECT id FROM approvals WHERE status IN ('APPROVED', 'DENIED', 'EXPIRED')"),
            replay_checked: self.replayed && self.divergent_refused,
            revocation_checked: self.revoked && self.dispatch_refused_after_revoke,
        }
    }

    /// Invariants the current state violates (empty = all hold). The checker reads only the facts in the database.
    fn violations(&mut self) -> Vec<String> {
        let mut reports: Vec<String> = Vec::new();
        let instances = self.instances();
        let goals = self.goals();
        let tasks = self.tasks();
        let waits = self.waits();
        let requests = self.requests();
        let operations = self.operations();

        // TypeOK
        for row in &instances {
            if !contains(LIFECYCLES, &row[1]) {
                reports.push(format!("TypeOK: instance {} lifecycle {}", row[0], row[1]));
            }
            if !contains(PHASES, &row[2]) {
                reports.push(format!("TypeOK: instance {} phase {}", row[0], row[2]));
            }
        }
        for row in &goals {
            if !contains(GOAL_STATUSES, &row[1]) {
                reports.push(format!("TypeOK: goal {} status {}", row[0], row[1]));
            }
        }
        for row in &tasks {
            if !contains(TASK_STATUSES, &row[4]) {
                reports.push(format!("TypeOK: task {} status {}", row[0], row[4]));
            }
        }
        for row in &waits {
            if !contains(WAIT_STATUSES, &row[2]) {
                reports.push(format!("TypeOK: wait {} status {}", row[0], row[2]));
            }
        }
        for row in &requests {
            if !contains(REQUEST_STATUSES, &row[2]) {
                reports.push(format!("TypeOK: request {} status {}", row[0], row[2]));
            }
        }
        for row in &operations {
            if !contains(OP_STATUSES, &row[1]) {
                reports.push(format!("TypeOK: operation {} status {}", row[0], row[1]));
            }
        }
        for row in self.rows("SELECT attempt_id, status FROM attempts ORDER BY attempt_id", &[]) {
            if !contains(ATTEMPT_STATUSES, &row[1]) {
                reports.push(format!("TypeOK: attempt {} status {}", row[0], row[1]));
            }
        }
        for row in self.rows("SELECT id, completeness FROM artifacts ORDER BY id", &[]) {
            if !contains(ARTIFACT_STATES, &row[1]) {
                reports.push(format!("TypeOK: artifact {} completeness {}", row[0], row[1]));
            }
        }
        for row in self.rows("SELECT id, status FROM approvals ORDER BY id", &[]) {
            if !contains(APPROVAL_STATUSES, &row[1]) {
                reports.push(format!("TypeOK: approval {} status {}", row[0], row[1]));
            }
        }

        // SettledIsFinal: a settled task is never rewritten
        for row in &tasks {
            let (id, status) = (row[0].clone(), row[4].clone());
            if let Some(previous) = self.settled.get(&id) {
                if previous != &status {
                    reports.push(format!("SettledIsFinal: task {id} went {previous} -> {status}"));
                }
            } else if contains(SETTLED_TASKS, &status) {
                self.settled.insert(id, status);
            }
        }

        // NoStaleActiveGoal（V-G1）
        let goal_status: HashMap<&str, &str> = goals.iter().map(|row| (row[0].as_str(), row[1].as_str())).collect();
        for row in &instances {
            let pointer = &row[3];
            if pointer.is_empty() {
                continue;
            }
            match goal_status.get(pointer.as_str()) {
                Some(status) if *status == "ACTIVE" => {}
                Some(status) => {
                    reports.push(format!("NoStaleActiveGoal: instance {} points at goal {pointer} ({status})", row[0]))
                }
                None => {
                    reports.push(format!("NoStaleActiveGoal: instance {} points at unknown goal {pointer}", row[0]))
                }
            }
        }

        // ReservationReleased: closing a request releases its reservation
        let open_requests: HashSet<&str> =
            requests.iter().filter(|row| row[2] == "PENDING").map(|row| row[0].as_str()).collect();
        for row in &goals {
            let reservations: Json = serde_json::from_str(&row[2]).unwrap_or(json!({}));
            if let Some(map) = reservations.as_object() {
                for key in map.keys() {
                    if !open_requests.contains(key.as_str()) {
                        reports
                            .push(format!("ReservationReleased: goal {} still reserves closed request {key}", row[0]));
                    }
                }
            }
        }

        // OneActiveRequest: the phase is driven by turn requests only (compression requests
        // run alongside it and do not move the phase, see begin_compression), so each instance
        // has at most one open turn request, consistent with its phase
        let mut live_per_instance: HashMap<&str, usize> = HashMap::new();
        for row in &requests {
            if row[2] == "PENDING" && row[4] == "turn" {
                *live_per_instance.entry(row[1].as_str()).or_default() += 1;
            }
        }
        for (instance, count) in &live_per_instance {
            if *count > 1 {
                reports.push(format!("OneActiveRequest: instance {instance} has {count} pending turn requests"));
            }
        }
        for row in &instances {
            let phase = row[2].as_str();
            let live = live_per_instance.get(row[0].as_str()).copied().unwrap_or(0);
            if phase == "MODEL_PENDING" && live != 1 {
                reports.push(format!(
                    "OneActiveRequest: instance {} is MODEL_PENDING with {live} pending turn requests",
                    row[0]
                ));
            }
            if phase != "MODEL_PENDING" && live > 0 {
                reports.push(format!(
                    "OneActiveRequest: instance {} is {phase} while a turn request is still pending",
                    row[0]
                ));
            }
        }

        // SelectionIsComplete: the selected attempt must be complete
        for row in &requests {
            let selected = &row[3];
            if selected.is_empty() {
                continue;
            }
            let found = self.rows("SELECT status FROM attempts WHERE attempt_id = ?1", &[&selected]);
            match found.first() {
                Some(attempt) if attempt[0] == "COMPLETE" => {}
                Some(attempt) => reports.push(format!(
                    "SelectionIsComplete: request {} selected {} which is {}",
                    row[0], selected, attempt[0]
                )),
                None => {
                    reports.push(format!("SelectionIsComplete: request {} selected unknown attempt {selected}", row[0]))
                }
            }
        }

        // ReturnPathOnlyWhileOpen: a live return path belongs to an unsettled task
        for row in self
            .rows("SELECT subject, resource_scope FROM grants WHERE action = 'task_result' AND revoked_at IS NULL", &[])
        {
            let task_id = row[1].strip_prefix("task:").unwrap_or("").to_string();
            match tasks.iter().find(|task| task[0] == task_id) {
                Some(task) if contains(OPEN_TASKS, &task[4]) => {}
                Some(task) => reports.push(format!(
                    "ReturnPathOnlyWhileOpen: task {} is {} but its return grant is live",
                    task_id, task[4]
                )),
                None => reports.push(format!("ReturnPathOnlyWhileOpen: live return grant for unknown task {task_id}")),
            }
        }

        // DependenciesPointBackwards: a dependency is created earlier (row order is creation
        // order), so the dependency graph is acyclic
        let order: HashMap<&str, usize> =
            tasks.iter().enumerate().map(|(index, row)| (row[0].as_str(), index)).collect();
        for row in &tasks {
            let deps: Json = serde_json::from_str(&row[5]).unwrap_or(json!([]));
            for dep in deps.as_array().cloned().unwrap_or_default() {
                let Some(dep) = dep.as_str() else { continue };
                match order.get(dep) {
                    Some(index) if *index < order[row[0].as_str()] => {}
                    Some(_) => {
                        reports.push(format!("DependenciesPointBackwards: task {} depends on later {dep}", row[0]))
                    }
                    None => {
                        reports.push(format!("DependenciesPointBackwards: task {} depends on unknown {dep}", row[0]))
                    }
                }
            }
        }

        // NoOpenTaskOnDeadAssignee
        let lifecycle: HashMap<&str, &str> = instances.iter().map(|row| (row[0].as_str(), row[1].as_str())).collect();
        for row in &tasks {
            if !contains(OPEN_TASKS, &row[4]) {
                continue;
            }
            if lifecycle.get(row[2].as_str()) == Some(&"TERMINATED") {
                reports
                    .push(format!("NoOpenTaskOnDeadAssignee: task {} is {} for terminated {}", row[0], row[4], row[2]));
            }
        }

        // ResolvedWaitIsAnswered (V-W1): once a wait is resolved there is an entry answering its own tool_call
        for row in &waits {
            let answered = self.rows(
                "SELECT id FROM context_entries WHERE instance_id = ?1 AND envelope_id = ?2",
                &[&row[1], &row[0]],
            );
            match row[2].as_str() {
                "PENDING" if !answered.is_empty() => reports.push(format!(
                    "ResolvedWaitIsAnswered: pending wait {} already has {} answer(s)",
                    row[0],
                    answered.len()
                )),
                "SATISFIED" | "CANCELLED" if answered.len() != 1 => reports.push(format!(
                    "ResolvedWaitIsAnswered: {} wait {} has {} answer(s)",
                    row[2],
                    row[0],
                    answered.len()
                )),
                _ => {}
            }
        }

        // NoEffectBeforeApproval / a terminal operation always has a receipt
        for row in &operations {
            if !TERMINAL_OPS.contains(&row[1].as_str()) || row[1] == "PREPARED" {
                continue;
            }
            if let Some(approval) =
                self.rows("SELECT status FROM approvals WHERE operation_id = ?1", &[&row[0]]).first()
            {
                if approval[0] == "PENDING" && row[2] == "receipt" {
                    reports.push(format!(
                        "NoEffectBeforeApproval: operation {} has a receipt while its approval is PENDING",
                        row[0]
                    ));
                }
            }
        }
        for row in &operations {
            if row[1] == "SUCCEEDED" && row[2].is_empty() {
                reports.push(format!("NoEffectBeforeApproval: operation {} succeeded without a receipt", row[0]));
            }
        }

        // LiveIsPersisted: a LIVE artifact has bytes
        for row in self.rows("SELECT id, completeness, storage_ref FROM artifacts ORDER BY id", &[]) {
            if row[1] == "LIVE" && !std::path::Path::new(&row[2]).exists() {
                reports.push(format!("LiveIsPersisted: artifact {} is LIVE without bytes at {}", row[0], row[2]));
            }
        }

        // A28: command receipts are stable (a stored receipt for a command id is never
        // rewritten), and a replay step must not move state at all (the three table sizes stay put)
        let commands = self.rows("SELECT command_id, result_json FROM commands ORDER BY command_id", &[]);
        for row in &commands {
            match self.receipts_seen.get(&row[0]) {
                Some(previous) if previous != &row[1] => {
                    reports.push(format!("ReceiptsAreStable: command {} returned a different receipt", row[0]))
                }
                Some(_) => {}
                None => {
                    self.receipts_seen.insert(row[0].clone(), row[1].clone());
                }
            }
        }
        if self.receipts_seen.len() > commands.len() {
            reports.push("LogMonotone: the command receipt log shrank".to_string());
        }
        let count =
            |sql: &str| -> usize { self.rows(sql, &[]).first().and_then(|row| row[0].parse().ok()).unwrap_or(0) };
        let sizes =
            (commands.len(), count("SELECT COUNT(*) FROM events"), count("SELECT COUNT(*) FROM context_entries"));
        let previous = self.sizes;
        if self.last_label.starts_with("replay") && sizes != previous {
            reports.push(format!("ReplayedCommandIsInert: a replay moved the state {previous:?} -> {sizes:?}"));
        }
        if sizes.1 < previous.1 {
            reports.push("LogMonotone: the event log shrank".to_string());
        }
        self.sizes = sizes;

        // A25/RT-06: a pending approval belongs to an operation that has not started (once the
        // operation is terminal its approval must be expired); a denied or expired approval has no
        // effect; a decided approval is never rewritten
        let approvals = self.rows("SELECT id, operation_id, status FROM approvals ORDER BY id", &[]);
        for row in &approvals {
            let (approval_id, operation_id, status) = (row[0].clone(), row[1].clone(), row[2].clone());
            // PENDING -> any decision is legal (RT-06 expiry is one of them); a decided approval never changes
            let decided = |value: &str| matches!(value, "APPROVED" | "DENIED" | "EXPIRED");
            match self.approvals_seen.get(&approval_id) {
                Some(previous) if decided(previous) && previous != &status => {
                    reports.push(format!("ApprovalDecisionIsFinal: approval {approval_id} went {previous} -> {status}"))
                }
                Some(_) => {}
                None => {
                    self.approvals_seen.insert(approval_id.clone(), status.clone());
                }
            }
            if status != "PENDING" {
                continue;
            }
            if let Some(operation) = operations.iter().find(|op| op[0] == operation_id) {
                if operation[1] != "PREPARED" {
                    reports.push(format!(
                        "PendingApprovalOnlyForPreparedOperation: operation {operation_id} is {} with a pending approval",
                        operation[1]
                    ));
                }
            }
        }
        let denial: HashMap<&str, &str> = approvals.iter().map(|row| (row[1].as_str(), row[2].as_str())).collect();
        for row in &operations {
            let started = row[4] == "started" || matches!(row[1].as_str(), "DISPATCH_COMMITTED" | "RUNNING");
            if !started {
                continue;
            }
            let decision = denial.get(row[0].as_str()).copied().unwrap_or("");
            if matches!(decision, "DENIED" | "EXPIRED") {
                reports.push(format!(
                    "NoEffectAfterDenial: operation {} started while its approval is {decision}",
                    row[0]
                ));
            }
        }

        // Context entries: append-only, contiguous slots (TailAppend), coverage never lifted, and
        // coverage points at a later summary
        let entries = self.rows(
            "SELECT instance_id, epoch, id, idx, kind, COALESCE(compressed_by, '') FROM context_entries
             ORDER BY instance_id, epoch, idx",
            &[],
        );
        for row in &entries {
            let key = format!("{}|{}|{}", row[0], row[1], row[2]);
            self.entries_seen.insert(key.clone());
            if row[5].is_empty() {
                if self.covered_seen.contains(&key) {
                    reports.push(format!("CoverageNeverLifted: entry {key} is visible again"));
                }
            } else {
                self.covered_seen.insert(key.clone());
                let summary =
                    entries.iter().find(|other| other[0] == row[0] && other[1] == row[1] && other[2] == row[5]);
                match summary {
                    Some(summary) if summary[4] == "summary" && summary[3] > row[3] => {}
                    Some(summary) => reports.push(format!(
                        "CoveragePointsForward: {key} covered by idx {} kind {}",
                        summary[3], summary[4]
                    )),
                    None => reports.push(format!("CoveragePointsForward: {key} covered by unknown {}", row[5])),
                }
            }
        }
        for key in &self.entries_seen {
            let mut parts = key.split('|');
            let (instance, epoch) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let id = parts.next().unwrap_or("");
            if !entries.iter().any(|row| row[0] == instance && row[1] == epoch && row[2] == id) {
                reports.push(format!("NoEntryIsEverLost: entry {key} disappeared"));
            }
        }
        let mut epochs: Vec<(String, String)> = entries.iter().map(|row| (row[0].clone(), row[1].clone())).collect();
        epochs.sort();
        epochs.dedup();
        for (instance, epoch) in epochs {
            let shape = self.rows(
                "SELECT COUNT(*), COALESCE(MAX(idx), -1) FROM context_entries WHERE instance_id = ?1 AND epoch = ?2",
                &[&instance, &epoch],
            );
            if let Some(row) = shape.first() {
                let count: i64 = row[0].parse().unwrap_or(-1);
                let max: i64 = row[1].parse().unwrap_or(-2);
                // append_entry starts at idx = 1 and never trims, so the entry count equals the max idx
                if count != max {
                    reports.push(format!("TailAppend: {instance} epoch {epoch} has {count} entries but max idx {max}"));
                }
            }
            // the newest summary must be visible (older summaries may be covered by later ones)
            let newest = self.rows(
                "SELECT id, COALESCE(compressed_by, '') FROM context_entries
                 WHERE instance_id = ?1 AND epoch = ?2 AND kind = 'summary' ORDER BY idx DESC LIMIT 1",
                &[&instance, &epoch],
            );
            if let Some(row) = newest.first() {
                if !row[1].is_empty() {
                    reports.push(format!("NewestSummaryIsVisible: newest summary {} is covered", row[0]));
                }
            }
        }

        // context_epoch: entries cannot exceed the instance's current epoch
        for row in &instances {
            let epoch: i64 = row[5].parse().unwrap_or(0);
            for entry in self
                .rows("SELECT COUNT(*) FROM context_entries WHERE instance_id = ?1 AND epoch > ?2", &[&row[0], &epoch])
            {
                if entry[0] != "0" {
                    reports.push(format!("TypeOK: instance {} has {} entries beyond epoch {epoch}", row[0], entry[0]));
                }
            }
        }

        reports
    }

    fn check(&mut self, label: &str) {
        let reports = self.violations();
        if !reports.is_empty() {
            let trace = self.trace.join("\n  ");
            panic!("invariant violated at step {label}:\n  {}\ntrace:\n  {trace}", reports.join("\n  "));
        }
    }
}

// ------------------------------------------------------------- step kinds --
const STEPS: usize = 38;

/// Build command kind `kind` for the current state; returns None when its preconditions do not hold (the step is skipped).
fn make_step(harness: &mut Harness, kind: usize) -> Option<Step> {
    let instances = harness.instances();
    let goals = harness.goals();
    let tasks = harness.tasks();
    let waits = harness.waits();
    let requests = harness.requests();
    let operations = harness.operations();
    let approvals = harness.rows("SELECT id, operation_id, status FROM approvals ORDER BY id", &[]);
    let artifacts = harness.rows("SELECT id, completeness, storage_ref FROM artifacts ORDER BY id", &[]);
    let ids: Vec<String> = instances.iter().map(|row| row[0].clone()).collect();
    let goal_ids: Vec<String> = goals.iter().map(|row| row[0].clone()).collect();
    let active_goals: Vec<String> = goals.iter().filter(|row| row[1] == "ACTIVE").map(|row| row[0].clone()).collect();
    let settled_goals: Vec<String> =
        goals.iter().filter(|row| TERMINAL_GOAL.contains(&row[1].as_str())).map(|row| row[0].clone()).collect();
    let open_tasks: Vec<Vec<String>> =
        tasks.iter().filter(|row| OPEN_TASKS.contains(&row[4].as_str())).cloned().collect();
    let pending_tasks: Vec<Vec<String>> = tasks.iter().filter(|row| row[4] == "PENDING").cloned().collect();
    // turn requests: import_response accepts only these (compression is submitted by compress_context)
    let pending_requests: Vec<Vec<String>> =
        requests.iter().filter(|row| row[2] == "PENDING" && row[4] == "turn").cloned().collect();
    // any open request: record_attempt applies to both kinds
    let pending_any: Vec<Vec<String>> = requests.iter().filter(|row| row[2] == "PENDING").cloned().collect();
    // an importable turn request: one complete attempt is selected (the driver imports right after selection)
    let ready_requests: Vec<Vec<String>> = pending_requests.iter().filter(|row| !row[3].is_empty()).cloned().collect();
    let prepared_ops: Vec<String> = harness
        .rows(
            "SELECT o.operation_id FROM operations o
             JOIN decisions d ON o.decision_id = d.decision_id
             JOIN model_requests r ON d.request_id = r.request_id
             JOIN instances i ON r.instance_id = i.id
             WHERE o.status = 'PREPARED' AND i.lifecycle = 'ACTIVE'",
            &[],
        )
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    let live_ops: Vec<String> = operations
        .iter()
        .filter(|row| matches!(row[1].as_str(), "DISPATCH_COMMITTED" | "RUNNING"))
        .map(|row| row[0].clone())
        .collect();
    let staged: Vec<String> = artifacts.iter().filter(|row| row[1] == "STAGING").map(|row| row[0].clone()).collect();
    let live_artifacts: Vec<String> =
        artifacts.iter().filter(|row| row[1] == "LIVE").map(|row| row[0].clone()).collect();
    let pending_approvals: Vec<String> =
        approvals.iter().filter(|row| row[2] == "PENDING").map(|row| row[0].clone()).collect();
    let compression: Vec<Vec<String>> = harness.rows(
        "SELECT request_id, instance_id FROM model_requests WHERE kind = 'compression' AND status = 'PENDING'",
        &[],
    );

    match kind {
        // 0: create an instance (at most 3)
        0 => {
            if ids.len() >= 3 {
                return None;
            }
            let id = format!("i{}", ids.len() + 1);
            Some(Step {
                label: format!("create_instance({id})"),
                command: Command {
                    command_id: String::new(),
                    method: "create_instance".into(),
                    params: json!({"id": id, "workspace_ref": "/tmp/ws"}),
                },
                identity: Identity::User,
            })
        }
        // 1: create a goal and attach it to an instance
        1 => {
            if goal_ids.len() >= 2 || ids.is_empty() {
                return None;
            }
            let instance = harness.pick(&ids)?.clone();
            let id = format!("g{}", goal_ids.len() + 1);
            Some(Step {
                label: format!("create_goal({id} -> {instance})"),
                command: Command {
                    command_id: String::new(),
                    method: "create_goal".into(),
                    params: json!({"id": id, "instance_id": instance, "limits": {"max_total_tokens": 100_000}}),
                },
                identity: Identity::User,
            })
        }
        // 2: open a request (begin_request)
        2 => {
            let row = harness
                .instances()
                .into_iter()
                .find(|row| row[1] == "ACTIVE" && row[2] == "READY" && row[4].is_empty())?;
            let revision: i64 = harness
                .rows("SELECT revision FROM instances WHERE id = ?1", &[&row[0]])
                .first()?
                .first()?
                .parse()
                .ok()?;
            let request_id = format!("r-{}", harness.counter + 1);
            Some(Step {
                label: format!("begin_request({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "begin_request".into(),
                    params: json!({"instance_id": row[0], "request_id": request_id, "revision": revision}),
                },
                identity: Identity::Instance(row[0].clone()),
            })
        }
        // 3: record a complete attempt (both turn and compression requests have attempts)
        3 => {
            let row = harness.pick_rows(&pending_any)?;
            Some(Step {
                label: format!("record_attempt({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "record_attempt".into(),
                    params: json!({"attempt_id": format!("at-{}", row[0]), "request_id": row[0],
                                   "status": "COMPLETE",
                                   "usage": {"prompt_tokens": 12, "completion_tokens": 6, "total_tokens": 18}}),
                },
                identity: Identity::System,
            })
        }
        // 4: import an ordinary reply
        4 => {
            let row = harness.pick_rows(&ready_requests)?;
            Some(Step {
                label: format!("import_reply({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "import_response".into(),
                    params: json!({"request_id": row[0], "decision_id": format!("d-{}", row[0]),
                                   "entry": {"role": "assistant", "content": "ok"}, "intents": []}),
                },
                identity: Identity::System,
            })
        }
        // 5: import a wait (unsatisfied -> parked). Wait on an open task when one exists (it can
        // be woken by settling), otherwise on a user message that never arrives (a permanent park is
        // a state worth checking too)
        5 => {
            let row = harness.pick_rows(&ready_requests)?;
            let condition = match harness.pick_rows(&open_tasks) {
                Some(task) => json!({"kind": "task", "task_id": task[0]}),
                None => json!({"kind": "message", "from": "user"}),
            };
            Some(Step {
                label: format!("import_wait({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "import_response".into(),
                    params: json!({"request_id": row[0], "decision_id": format!("d-{}", row[0]),
                                   "entry": {"role": "assistant", "content": "",
                                             "tool_calls": [{"id": format!("wait-{}", row[0]), "type": "function",
                                                             "function": {"name": "wait", "arguments": "{}"}}]},
                                   "intents": [],
                                   "wait": {"mode": "ANY", "conditions": [condition]}}),
                },
                identity: Identity::System,
            })
        }
        // 6: import a wait plus a timer (closed by fire_timer)
        6 => {
            let row = harness.pick_rows(&ready_requests)?;
            Some(Step {
                label: format!("import_wait_timer({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "import_response".into(),
                    params: json!({"request_id": row[0], "decision_id": format!("d-{}", row[0]),
                                   "entry": {"role": "assistant", "content": "",
                                             "tool_calls": [{"id": format!("wt-{}", row[0]), "type": "function",
                                                             "function": {"name": "wait", "arguments": "{}"}}]},
                                   "intents": [],
                                   "wait": {"mode": "ALL", "conditions": [{"kind": "message", "from": "peer"}],
                                            "timer_seconds": 1}}),
                },
                identity: Identity::System,
            })
        }
        // 7: import a completion candidate (enters COMPLETION_PENDING)
        7 => {
            let row = harness.pick_rows(&ready_requests)?;
            Some(Step {
                label: format!("import_completion({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "import_response".into(),
                    params: json!({"request_id": row[0], "decision_id": format!("d-{}", row[0]),
                                   "entry": {"role": "assistant", "content": "",
                                             "tool_calls": [{"id": format!("fin-{}", row[0]), "type": "function",
                                                             "function": {"name": "finish", "arguments": "{}"}}]},
                                   "completion": {"outcome": "success", "summary": "done", "evidence": ["e"]}}),
                },
                identity: Identity::System,
            })
        }
        // 8: settle the goal
        8 => {
            let goal = harness.pick(&active_goals)?.clone();
            let ready = harness.instances().into_iter().find(|row| row[2] == "COMPLETION_PENDING")?;
            Some(Step {
                label: format!("complete_goal({goal})"),
                command: Command {
                    command_id: String::new(),
                    method: "complete_goal".into(),
                    params: json!({"goal_id": goal, "instance_id": row_or(&ready)}),
                },
                identity: Identity::System,
            })
        }
        // 9: the system blocks the goal
        9 => {
            let goal = harness.pick(&active_goals)?.clone();
            let instance = harness.pick(&ids)?.clone();
            Some(Step {
                label: format!("block_goal({goal})"),
                command: Command {
                    command_id: String::new(),
                    method: "block_goal".into(),
                    params: json!({"goal_id": goal, "instance_id": instance, "reason": "checks exhausted"}),
                },
                identity: Identity::System,
            })
        }
        // 10: delegate a task (with a legal goal)
        10 => {
            if ids.is_empty() || tasks.len() >= 3 {
                return None;
            }
            let instance = harness.pick(&ids)?.clone();
            let goal = if active_goals.is_empty() {
                harness.pick(&goal_ids)?.clone()
            } else {
                harness.pick(&active_goals)?.clone()
            };
            let task_id = format!("t{}", tasks.len() + 1);
            Some(Step {
                label: format!("delegate_task({task_id} -> {instance}, goal {goal})"),
                command: Command {
                    command_id: String::new(),
                    method: "delegate_task".into(),
                    params: json!({"task_id": task_id, "assignee": instance, "goal_id": goal, "description": "work"}),
                },
                identity: Identity::User,
            })
        }
        // 11: the assignee starts the task
        11 => {
            let task = harness.pick_rows(&pending_tasks)?;
            Some(Step {
                label: format!("start_task({})", task[0]),
                command: Command {
                    command_id: String::new(),
                    method: "start_task".into(),
                    params: json!({"task_id": task[0]}),
                },
                identity: Identity::Instance(task[2].clone()),
            })
        }
        // 12: the assignee settles the task
        12 => {
            let task = harness.pick_rows(&open_tasks)?;
            let status = ["SUCCEEDED", "FAILED", "BLOCKED"][(harness.next_rand() % 3) as usize];
            Some(Step {
                label: format!("complete_task({}, {status})", task[0]),
                command: Command {
                    command_id: String::new(),
                    method: "complete_task".into(),
                    params: json!({"task_id": task[0], "status": status, "summary": "s"}),
                },
                identity: Identity::Instance(task[2].clone()),
            })
        }
        // 13: the delegator cancels the task
        13 => {
            let task = harness.pick_rows(&open_tasks)?;
            Some(Step {
                label: format!("cancel_task({})", task[0]),
                command: Command {
                    command_id: String::new(),
                    method: "cancel_task".into(),
                    params: json!({"task_id": task[0], "reason": "no longer needed"}),
                },
                identity: Identity::User,
            })
        }
        // 14: user input (may supersede a wait)
        14 => {
            let instance = harness.pick(&ids)?.clone();
            let envelope = format!("e-{}", harness.counter + 1);
            Some(Step {
                label: format!("submit_input({instance})"),
                command: Command {
                    command_id: String::new(),
                    method: "submit_input".into(),
                    params: json!({"instance_id": instance, "envelope_id": envelope, "text": "ping"}),
                },
                identity: Identity::User,
            })
        }
        // 15: a parked drain (the wake path for waits)
        15 => {
            let instance = harness.pick(&ids)?.clone();
            Some(Step {
                label: format!("drain_inbox({instance})"),
                command: Command {
                    command_id: String::new(),
                    method: "drain_inbox".into(),
                    params: json!({"instance_id": instance}),
                },
                identity: Identity::Instance(instance),
            })
        }
        // 16: a timer
        16 => Some(Step {
            label: "fire_timer".into(),
            command: Command { command_id: String::new(), method: "fire_timer".into(), params: json!({"now": 4e9}) },
            identity: Identity::System,
        }),
        // 17: issue a grant (message / delegate)
        17 => {
            if ids.is_empty() {
                return None;
            }
            let subject = harness.pick(&ids)?.clone();
            let target = harness.pick(&ids)?.clone();
            let action = ["message", "delegate", "shell"][(harness.next_rand() % 3) as usize];
            // the shell grant uses the same scope as create_instance, so it can be re-issued after a revocation
            let scope = if action == "shell" { "workspace".to_string() } else { format!("instance:{target}") };
            Some(Step {
                label: format!("issue_grant({subject} {action} {scope})"),
                command: Command {
                    command_id: String::new(),
                    method: "issue_grant".into(),
                    params: json!({"subject": subject, "action": action, "resource_scope": scope}),
                },
                identity: Identity::User,
            })
        }
        // 18: send a message (may need a grant; a refusal is a legal outcome)
        18 => {
            if ids.len() < 2 {
                return None;
            }
            let sender = harness.pick(&ids)?.clone();
            let mut recipient = harness.pick(&ids)?.clone();
            if recipient == sender {
                recipient = ids.iter().find(|id| **id != sender)?.clone();
            }
            Some(Step {
                label: format!("send_message({sender} -> {recipient})"),
                command: Command {
                    command_id: String::new(),
                    method: "send_message".into(),
                    params: json!({"recipient": recipient, "text": "hi"}),
                },
                identity: Identity::Instance(sender),
            })
        }
        // 19: dispatch an operation. Approval is always required so the walk reliably exercises the
        // approval path (A25); the dispatch-without-approval path is covered by the core unit tests
        19 => {
            let operation = harness.pick(&prepared_ops)?.clone();
            let approval = true;
            Some(Step {
                label: format!("dispatch_operation({operation}, approval={approval})"),
                command: Command {
                    command_id: String::new(),
                    method: "dispatch_operation".into(),
                    params: json!({"operation_id": operation, "approval_required": approval,
                                   "permission_revision": 0}),
                },
                identity: Identity::System,
            })
        }
        // 20: settle the operation
        20 => {
            let operation = harness.pick(&live_ops)?.clone();
            Some(Step {
                label: format!("complete_operation({operation})"),
                command: Command {
                    command_id: String::new(),
                    method: "complete_operation".into(),
                    params: json!({"operation_id": operation, "status": "SUCCEEDED",
                                   "receipt": {"operation_id": operation, "ok": true, "content": "{\"output\":\"ok\"}"}}),
                },
                identity: Identity::System,
            })
        }
        // 21: approve / deny
        21 => {
            let operation = harness.pick(&pending_approvals)?.clone();
            let method = if harness.next_rand().is_multiple_of(2) { "approve" } else { "deny" };
            Some(Step {
                label: format!("{method}({operation})"),
                command: Command {
                    command_id: String::new(),
                    method: method.into(),
                    params: json!({"approval_id": operation}),
                },
                identity: Identity::User,
            })
        }
        // 22: reset the epoch (a safe boundary: only when the instance has no in-flight turn, like the driver)
        22 => {
            let ready: Vec<String> = instances
                .iter()
                .filter(|row| row[2] == "READY" && row[4].is_empty())
                .map(|row| row[0].clone())
                .collect();
            let instance = harness.pick(&ready)?.clone();
            Some(Step {
                label: format!("reset_instance({instance})"),
                command: Command {
                    command_id: String::new(),
                    method: "reset_instance".into(),
                    params: json!({"instance_id": instance, "reason": "invariants harness"}),
                },
                identity: Identity::User,
            })
        }
        // 23: lifecycle (pause / resume / terminate). Termination lands on a safe boundary too: the
        // instance is READY
        23 => {
            let lifecycle = ["PAUSED", "ACTIVE", "TERMINATED"][(harness.next_rand() % 3) as usize];
            let pool: Vec<String> = instances
                .iter()
                .filter(|row| {
                    row[1] != "TERMINATED" && (lifecycle != "TERMINATED" || (row[2] == "READY" && row[4].is_empty()))
                })
                .map(|row| row[0].clone())
                .collect();
            let instance = harness.pick(&pool)?.clone();
            Some(Step {
                label: format!("set_lifecycle({instance}, {lifecycle})"),
                command: Command {
                    command_id: String::new(),
                    method: "set_lifecycle".into(),
                    params: json!({"instance_id": instance, "lifecycle": lifecycle, "reason": "harness"}),
                },
                identity: Identity::User,
            })
        }
        // 24: artifact stage / publish
        24 => {
            if harness.next_rand().is_multiple_of(2) {
                if artifacts.len() >= 2 {
                    return None;
                }
                let id = format!("a{}", artifacts.len() + 1);
                let storage_ref = std::env::temp_dir()
                    .join(format!("teamagents-artifact-{id}-{}.bin", std::process::id()))
                    .to_string_lossy()
                    .to_string();
                std::fs::write(&storage_ref, b"payload").ok()?;
                Some(Step {
                    label: format!("artifact_stage({id})"),
                    command: Command {
                        command_id: String::new(),
                        method: "artifact_stage".into(),
                        params: json!({"id": id, "digest": "d", "size": 7, "storage_ref": storage_ref,
                                       "kind": "response"}),
                    },
                    identity: Identity::System,
                })
            } else {
                let id = harness.pick(&staged)?.clone();
                Some(Step {
                    label: format!("artifact_publish({id})"),
                    command: Command {
                        command_id: String::new(),
                        method: "artifact_publish".into(),
                        params: json!({"id": id}),
                    },
                    identity: Identity::System,
                })
            }
        }
        // 28: import a tool intent (creates a PREPARED operation for the dispatch/approval/receipt paths)
        28 => {
            if operations.len() >= 3 {
                return None;
            }
            let row = harness.pick_rows(&ready_requests)?;
            Some(Step {
                label: format!("import_tools({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "import_response".into(),
                    params: json!({"request_id": row[0], "decision_id": format!("d-{}", row[0]),
                                   "entry": {"role": "assistant", "content": "run tools",
                                             "tool_calls": [{"id": format!("call-{}", row[0]), "type": "function",
                                                             "function": {"name": "shell", "arguments": "{}"}}]},
                                   "intents": [{"index": 0, "call_id": format!("call-{}", row[0]), "name": "shell",
                                                "args": {"command": "echo harness"}}],
                                   "grant_revision": 0}),
                },
                identity: Identity::System,
            })
        }
        // 29: re-authorize (stamp PREPARED operations with the current permission revision after a
        // revocation or bump)
        29 => {
            let operation = harness.pick(&prepared_ops)?.clone();
            Some(Step {
                label: format!("reauthorize_operation({operation})"),
                command: Command {
                    command_id: String::new(),
                    method: "reauthorize_operation".into(),
                    params: json!({"operation_id": operation}),
                },
                identity: Identity::System,
            })
        }
        // 30: open a compression (shares the admission gate with turn requests)
        30 => {
            let instance = harness.pick(&ids)?.clone();
            let request = format!("cr-{}", harness.plan_id());
            Some(Step {
                label: format!("begin_compression({instance})"),
                command: Command {
                    command_id: String::new(),
                    method: "begin_compression".into(),
                    params: json!({"instance_id": instance, "request_id": request, "est_prompt_tokens": 50}),
                },
                identity: Identity::Instance(instance),
            })
        }
        // 31: submit the compression (append the summary, cover old entries, close the request,
        // release the reservation)
        31 => {
            let row = harness.pick_rows(&compression)?;
            let instance = row[1].clone();
            let epoch = harness
                .rows("SELECT context_epoch FROM instances WHERE id = ?1", &[&instance])
                .first()?
                .first()?
                .clone();
            let visible = harness.rows(
                "SELECT id FROM context_entries
                 WHERE instance_id = ?1 AND epoch = ?2 AND compressed_by IS NULL ORDER BY idx",
                &[&instance, &epoch],
            );
            // half the time the earliest visible entry is kept (the rest are covered), otherwise all are
            let keep = if harness.next_rand().is_multiple_of(2) {
                visible.first().map(|entry| json!([entry[0]])).unwrap_or(json!([]))
            } else {
                json!([])
            };
            Some(Step {
                label: format!("compress_context({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "compress_context".into(),
                    params: json!({"instance_id": instance, "request_id": row[0], "attempt_id": "",
                                   "summary": "compressed view", "keep_ids": keep}),
                },
                identity: Identity::System,
            })
        }
        // 32: the compression fails (the context stays put; only the request closes and the
        // reservation is released)
        32 => {
            let row = harness.pick_rows(&compression)?;
            Some(Step {
                label: format!("fail_compression({})", row[0]),
                command: Command {
                    command_id: String::new(),
                    method: "fail_compression".into(),
                    params: json!({"request_id": row[0], "reason": "summary unavailable"}),
                },
                identity: Identity::System,
            })
        }
        // 33: cancel a PREPARED operation (its pending approval expires: the RT-06 path)
        33 => {
            let operation = harness.pick(&prepared_ops)?.clone();
            Some(Step {
                label: format!("cancel_operation({operation})"),
                command: Command {
                    command_id: String::new(),
                    method: "cancel_operation".into(),
                    params: json!({"operation_id": operation}),
                },
                identity: Identity::System,
            })
        }
        // 34: replay the last command (same id, same payload): returns the stored receipt, state unchanged
        34 => {
            let (last, identity) = harness.last_command.clone()?;
            Some(Step {
                label: format!("replay_same({})", last.command_id),
                command: Command { command_id: last.command_id.clone(), ..last },
                identity,
            })
        }
        // 35: counterexample probe - the same command id with a different payload (the spec requires a refusal)
        35 => {
            let (last, identity) = harness.last_command.clone()?;
            let mut params = last.params.clone();
            if let Some(map) = params.as_object_mut() {
                map.insert("__divergent".into(), json!(true));
            }
            Some(Step {
                label: format!("!replay_divergent({})", last.command_id),
                command: Command { command_id: last.command_id.clone(), method: last.method.clone(), params },
                identity,
            })
        }
        // 36: revoke a still-effective shell grant (the revocation surface of A03/A04)
        36 => {
            let live: Vec<String> = harness
                .rows("SELECT id FROM grants WHERE action = 'shell' AND revoked_at IS NULL", &[])
                .into_iter()
                .map(|row| row[0].clone())
                .collect();
            let grant = harness.pick(&live)?.clone();
            Some(Step {
                label: format!("revoke_grant({grant})"),
                command: Command {
                    command_id: String::new(),
                    method: "revoke_grant".into(),
                    params: json!({"grant_id": grant}),
                },
                identity: Identity::User,
            })
        }
        // 37: counterexample probe - an instance whose grant was revoked must not dispatch (the spec requires a refusal)
        37 => {
            let instance = prepared_ops.first().and_then(|operation| {
                harness
                    .rows(
                        "SELECT i.id FROM operations o
                         JOIN decisions d ON o.decision_id = d.decision_id
                         JOIN model_requests r ON d.request_id = r.request_id
                         JOIN instances i ON r.instance_id = i.id
                         WHERE o.operation_id = ?1",
                        &[operation],
                    )
                    .first()
                    .map(|row| row[0].clone())
            })?;
            let live_shell: i64 = harness
                .rows(
                    "SELECT COUNT(*) FROM grants WHERE subject = ?1 AND action = 'shell' AND revoked_at IS NULL",
                    &[&instance],
                )
                .first()
                .and_then(|row| row[0].parse().ok())
                .unwrap_or(1);
            if live_shell > 0 {
                return None; // the probe does not apply while an effective grant exists
            }
            let operation = prepared_ops.first()?.clone();
            Some(Step {
                label: format!("!dispatch_without_grant({operation})"),
                command: Command {
                    command_id: String::new(),
                    method: "dispatch_operation".into(),
                    params: json!({"operation_id": operation, "approval_required": false, "permission_revision": 0}),
                },
                identity: Identity::System,
            })
        }
        // 26: counterexample probe - delegating into a settled goal (the spec requires a refusal)
        26 => {
            let goal = settled_goals.first()?.clone();
            let instance = ids.first()?.clone();
            Some(Step {
                label: format!("!delegate_to_settled_goal({goal})"),
                command: Command {
                    command_id: String::new(),
                    method: "delegate_task".into(),
                    params: json!({"task_id": "tsettled", "assignee": instance, "goal_id": goal,
                                   "description": "should be refused"}),
                },
                identity: Identity::User,
            })
        }
        // 27: counterexample probe - settling a task as someone other than the assignee (the spec requires a refusal)
        27 => {
            let task = harness.pick_rows(&open_tasks)?;
            let foreign = instances.iter().map(|row| row[0].clone()).find(|id| *id != task[2])?;
            Some(Step {
                label: format!("!settle_task_by_foreigner({})", task[0]),
                command: Command {
                    command_id: String::new(),
                    method: "complete_task".into(),
                    params: json!({"task_id": task[0], "status": "SUCCEEDED", "summary": "hijack"}),
                },
                identity: Identity::Instance(foreign),
            })
        }
        // 25: GC claim
        25 => {
            if live_artifacts.is_empty() && waits.is_empty() {
                return None;
            }
            Some(Step {
                label: "artifact_gc_claim".into(),
                command: Command {
                    command_id: String::new(),
                    method: "artifact_gc_claim".into(),
                    params: json!({"limit": 1}),
                },
                identity: Identity::System,
            })
        }
        _ => None,
    }
}

fn row_or(row: &[String]) -> String {
    row[0].clone()
}

fn run_sequence(tag: &str, kinds: &[usize]) {
    let mut harness = Harness::new(tag);
    harness.check("init");
    for (index, kind) in kinds.iter().enumerate() {
        let _ = index;
        if let Some(step) = make_step(&mut harness, *kind) {
            let label = step.label.clone();
            let _ = harness.run_step(step);
            harness.check(&label);
        }
    }
}

#[test]
fn spec_invariants_hold_over_short_command_sequences() {
    // exhaustive enumeration up to length 2: every sequence starts from a fresh database (all
    // command pairs are covered, including refused combinations)
    for first in 0..STEPS {
        run_sequence("exh1", &[first]);
        for second in 0..STEPS {
            run_sequence("exh2", &[first, second]);
        }
    }
}

#[test]
fn spec_invariants_hold_over_random_walks() {
    // fixed-seed random walks: a deeper state space
    let mut reached = Coverage::default();
    for walk in 0..60u64 {
        let mut harness = Harness::new("walk");
        harness.rng = 0x9E3779B97F4A7C15 ^ (walk.wrapping_mul(0xD1B54A32D192ED03) | 1);
        harness.check("init");
        let mut taken: HashMap<String, usize> = HashMap::new();
        for _ in 0..24 {
            // pick only among commands usable right now, preferring the kind used least in this walk
            // (coverage driven): otherwise the walk repeats one safe action and never reaches deep paths
            let mut options: Vec<Step> = (0..STEPS).filter_map(|kind| make_step(&mut harness, kind)).collect();
            if options.is_empty() {
                break;
            }
            let key = |step: &Step| step.label.split('(').next().unwrap_or("").to_string();
            let least = options.iter().map(|step| *taken.get(&key(step)).unwrap_or(&0)).min().unwrap_or(0);
            options.retain(|step| *taken.get(&key(step)).unwrap_or(&0) == least);
            let index = (harness.next_rand() % options.len() as u64) as usize;
            let step = options.swap_remove(index);
            *taken.entry(key(&step)).or_insert(0) += 1;
            let label = step.label.clone();
            let _ = harness.run_step(step);
            harness.check(&label);
        }
        reached.merge(harness.coverage());
    }
    // the walk must really reach these states, otherwise "everything passed" above is vacuous
    for (name, seen) in [
        ("a wait resolved (SATISFIED/CANCELLED)", reached.resolved_wait),
        ("a settled goal", reached.settled_goal),
        ("a settled task", reached.settled_task),
        ("a terminal operation", reached.settled_operation),
        ("an epoch reset", reached.reset_epoch),
        ("a terminated instance", reached.terminated),
        ("a LIVE artifact", reached.artifact_live),
        ("a submitted compression (with covered entries)", reached.compressed),
        ("a decided approval (approved/denied/expired)", reached.approval_decided),
        (
            "a command replay (same payload returns the stored receipt, a different one is refused)",
            reached.replay_checked,
        ),
        ("a dispatch refused after revocation (A03/A04)", reached.revocation_checked),
    ] {
        assert!(seen, "the random walk never covered: {name}");
    }
}

/// Negative control: when the state is broken on purpose the checker must report it, otherwise the
/// "passed" results above mean nothing.
#[test]
fn the_invariant_checker_detects_broken_states() {
    // 1) an unknown task status
    let mut harness = Harness::new("broken-status");
    let step = make_step(&mut harness, 0).expect("create instance");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 1).expect("create goal");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 10).expect("delegate task");
    let _ = harness.run_step(step);
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "a healthy starting point must report no violation: {healthy:?}");
    harness.ctl.connection().execute("UPDATE tasks SET status = 'WAT'", []).unwrap();
    let reported = harness.violations();
    assert!(
        reported.iter().any(|line| line.contains("TypeOK: task")),
        "the unknown status must be reported: {reported:?}"
    );

    // 2) a settled task rewritten (memory across steps): settle it through the control plane, then edit it by hand
    let mut harness = Harness::new("broken-terminal");
    let step = make_step(&mut harness, 0).expect("create instance");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 1).expect("create goal");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 10).expect("delegate task");
    let _ = harness.run_step(step);
    let settled = harness.ctl.submit(
        Command {
            command_id: "settle".into(),
            method: "complete_task".into(),
            params: json!({"task_id": "t1", "status": "SUCCEEDED", "summary": "s"}),
        },
        Identity::Instance("i1".into()),
    );
    assert!(settled.is_ok(), "a normal settle must succeed: {settled:?}");
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "a normal settle must report no violation: {healthy:?}");
    harness.ctl.connection().execute("UPDATE tasks SET status = 'RUNNING'", []).unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("SettledIsFinal")), "the rewrite must be reported: {reported:?}");

    // 4) coverage lifted again (`CoverageNeverLifted`) and coverage pointing at an earlier entry
    // (`CoveragePointsForward`)
    let mut harness = Harness::new("broken-compress");
    for kind in [0, 14, 30, 31] {
        let step = make_step(&mut harness, kind).expect("compression setup");
        let _ = harness.run_step(step);
    }
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "a normal compression must report no violation: {healthy:?}");
    let covered = harness.rows("SELECT id FROM context_entries WHERE compressed_by IS NOT NULL", &[]);
    assert!(!covered.is_empty(), "a submitted compression must cover entries");
    harness
        .ctl
        .connection()
        .execute("UPDATE context_entries SET compressed_by = NULL WHERE compressed_by IS NOT NULL", [])
        .unwrap();
    let reported = harness.violations();
    assert!(
        reported.iter().any(|line| line.contains("CoverageNeverLifted")),
        "lifted coverage must be reported: {reported:?}"
    );

    let mut harness = Harness::new("broken-compress-forward");
    for kind in [0, 14, 30, 31] {
        let step = make_step(&mut harness, kind).expect("compression setup");
        let _ = harness.run_step(step);
    }
    harness
        .ctl
        .connection()
        .execute(
            "UPDATE context_entries SET compressed_by =
                 (SELECT id FROM context_entries WHERE kind = 'summary' LIMIT 1)
             WHERE compressed_by IS NOT NULL",
            [],
        )
        .unwrap();
    // point the summary at itself: its index is no longer strictly greater than the covered entry
    harness.ctl.connection().execute("UPDATE context_entries SET kind = 'note' WHERE kind = 'summary'", []).unwrap();
    let reported = harness.violations();
    assert!(
        reported.iter().any(|line| line.contains("CoveragePointsForward")),
        "coverage pointing at a non-summary must be reported: {reported:?}"
    );

    // 3) an instance pointing at a settled goal (V-G1)
    let mut harness = Harness::new("broken-goal");
    let step = make_step(&mut harness, 0).expect("create instance");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 1).expect("create goal");
    let _ = harness.run_step(step);
    harness.ctl.connection().execute("UPDATE goals SET status = 'SUCCEEDED'", []).unwrap();
    let reported = harness.violations();
    assert!(
        reported.iter().any(|line| line.contains("NoStaleActiveGoal")),
        "the stale pointer must be reported: {reported:?}"
    );
}
