//! 规格↔代码的可执行对应（R2 控制面）。
//!
//! `verification/tla/*.tla` 用 TLC 穷举的是抽象状态机；本测试把**同一组不变量**在真实
//! `core::v2::Control` 上重算一遍：先对长度 ≤ 2 的命令序列做穷举，再做固定种子的随机游走，
//! 每一步之后检查 SQLite 里的事实。命令行：
//!
//! ```text
//! cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants
//! ```
//!
//! 覆盖（括号里是 TLA+ 里的同名性质）：
//! `TypeOK`、任务终态不可改写（`SettledIsFinal`）、窄返回能力随任务结清撤销
//! （`ReturnPathOnlyWhileOpen`）、依赖只指向更早创建的任务（`DependenciesPointBackwards`）、
//! 已终止实例名下无未结清任务（`NoOpenTaskOnDeadAssignee`）、实例不悬挂已结清目标
//! （`NoStaleActiveGoal`，V-G1）、请求关闭必释放预留（`ReservationReleased`）、
//! 单实例单活跃请求（`OneActiveRequest`）、选中尝试必为完整尝试（`SelectionIsComplete`）、
//! 等被解决必答其 tool_call（`ResolvedWaitIsAnswered`，V-W1）、未批准不产生效果
//! （`NoEffectBeforeApproval`）、LIVE 制品必有字节（`LiveIsPersisted`）。

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

/// 游走覆盖到的关键状态（证明探索不是空转）
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

/// 一步命令：名字（用于失败信息里的轨迹）与真正的提交。
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
    /// 任务 id -> 已见过的终态（`SettledIsFinal` 的跨步记忆）
    settled: HashMap<String, String>,
    /// 见过的上下文条目（`NoEntryIsEverLost` 的跨步记忆）
    entries_seen: HashSet<String>,
    /// 见过被覆盖的条目（`CoverageNeverLifted` 的跨步记忆）
    covered_seen: HashSet<String>,
    /// 批准 id -> 已见过的决定（`ApprovalDecisionIsFinal` 的跨步记忆）
    approvals_seen: HashMap<String, String>,
    /// 命令 id -> 已存回执（`ReceiptsAreStable` 的跨步记忆）
    receipts_seen: HashMap<String, String>,
    /// 上一次成功提交的命令与其身份（用于重放步骤）
    last_command: Option<(Command, Identity)>,
    /// 上一步之后的三张表规模（命令 / 事件 / 上下文条目），重放必须不动它们
    sizes: (usize, usize, usize),
    /// 上一步的标签（判断它是不是重放步骤）
    last_label: String,
    /// 重放覆盖：同 id 同 payload 成功过、同 id 异 payload 被拒过
    replayed: bool,
    divergent_refused: bool,
    /// 撤权覆盖：撤过一条有效授权、并且撤权后的派发被拒过
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
        // splitmix64：固定种子 ⇒ 可复现的游走
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

    /// 给"计划中的"资源分配一个稳定编号（请求 / 任务 id 之类）
    fn plan_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// 从表格行里挑一行（任务/请求这类多列对象）
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

    /// 执行一步并把结果记进轨迹；标签以 `!` 开头表示规格要求这一步被拒。
    fn run_step(&mut self, step: Step) -> Result<Json, String> {
        let Step { label, command, identity } = step;
        // 重放步骤自带命令 id（客户端在断连重连后重发同一个 id）；其余步骤分配新 id
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
            assert!(result.is_err(), "规格要求被拒但代码接受了：{label}");
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
            // 重放也要记住（同一个 id 可以再被重放）
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
                // 列可能有整数（计数/Revision）、实数（时间戳）或文本：统一读成字符串
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
    /// 这次游走是否走到了值得检查的状态（用来证明游走不是空转）
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

    /// 当前状态违反的不变量（空 = 全部成立）。检查器只看数据库里的事实。
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

        // SettledIsFinal：终态任务不再改写
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

        // ReservationReleased：请求一旦关闭，预留必须释放
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

        // OneActiveRequest：相位只由 turn 请求驱动（压缩请求并发且不动相位，见
        // begin_compression），所以每实例至多一个未关闭 turn 请求，并与相位一致
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

        // SelectionIsComplete：被选中的尝试必须是完整尝试
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

        // ReturnPathOnlyWhileOpen：存活的任务结果能力只属于未结清的任务
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

        // DependenciesPointBackwards：依赖必须先创建（行序即创建序）⇒ 依赖图无环
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

        // ResolvedWaitIsAnswered（V-W1）：等一旦结束，必定留下回答它自己 tool_call 的条目
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

        // NoEffectBeforeApproval / 终态操作必有回执
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

        // LiveIsPersisted：LIVE 制品必须有字节
        for row in self.rows("SELECT id, completeness, storage_ref FROM artifacts ORDER BY id", &[]) {
            if row[1] == "LIVE" && !std::path::Path::new(&row[2]).exists() {
                reports.push(format!("LiveIsPersisted: artifact {} is LIVE without bytes at {}", row[0], row[2]));
            }
        }

        // A28：命令回执稳定（同一个 command id 的存量回执不再改写），且重放步骤必须
        // 完全不动状态（命令/事件/上下文三张表的规模不变）
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

        // A25/RT-06：待批只属于未开动的操作（操作一旦终结，它的批准必须已过期）；
        // 被拒绝/过期的批准不会有已经发生的效果；批准决定一旦落下不再改写
        let approvals = self.rows("SELECT id, operation_id, status FROM approvals ORDER BY id", &[]);
        for row in &approvals {
            let (approval_id, operation_id, status) = (row[0].clone(), row[1].clone(), row[2].clone());
            // PENDING → 任何决定都合法（RT-06 的过期就是这一条）；决定落下之后不再改写
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

        // 上下文条目：只增不减、槽位连续（TailAppend）、覆盖不许回抬、覆盖指向更晚的总结
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
                // append_entry 从 idx = 1 开始，只增不减 ⇒ 条目数等于最大 idx
                if count != max {
                    reports.push(format!("TailAppend: {instance} epoch {epoch} has {count} entries but max idx {max}"));
                }
            }
            // 最新的总结必须可见（更早的总结可以被更晚的总结覆盖）
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

        // context_epoch：条目不能超出实例当前 epoch
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
            panic!("不变量违反（步 {label}）：\n  {}\n轨迹：\n  {trace}", reports.join("\n  "));
        }
    }
}

// ------------------------------------------------------------- step kinds --
const STEPS: usize = 38;

/// 按当前状态生成第 `kind` 种命令；前置不成立时返回 None（该步跳过）。
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
    // turn 请求：import_response 只接受它们（压缩请求由 compress_context 提交）
    let pending_requests: Vec<Vec<String>> =
        requests.iter().filter(|row| row[2] == "PENDING" && row[4] == "turn").cloned().collect();
    // 任何未关闭请求：record_attempt 对两种请求都适用
    let pending_any: Vec<Vec<String>> = requests.iter().filter(|row| row[2] == "PENDING").cloned().collect();
    // 可以导入的 turn 请求：已选中一个完整尝试（driver 就是在选中之后导入的）
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
        // 0: 建实例（上限 3）
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
        // 1: 建目标并挂到实例上
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
        // 2: 开会话（begin_request）
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
        // 3: 记录完整尝试（turn 与 compression 请求都有尝试）
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
        // 4: 导入普通回复
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
        // 5: 导入等待（未满足 ⇒ 停放）。有未结清任务时等它（可被结清唤醒），
        // 否则等一条永远不会来的用户消息（永久停放也是要检查的状态）
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
        // 6: 导入等待 + 计时器（可被 fire_timer 关闭）
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
        // 7: 导入完成候选（进 COMPLETION_PENDING）
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
        // 8: 结清目标
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
        // 9: 系统阻断目标
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
        // 10: 委派任务（合法目标）
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
        // 11: 承接者启动任务
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
        // 12: 承接者结清任务
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
        // 13: 委派者取消任务
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
        // 14: 用户输入（可取代等待）
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
        // 15: 停放 drain（等待唤醒路径）
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
        // 16: 计时器
        16 => Some(Step {
            label: "fire_timer".into(),
            command: Command { command_id: String::new(), method: "fire_timer".into(), params: json!({"now": 4e9}) },
            identity: Identity::System,
        }),
        // 17: 授权（message / delegate）
        17 => {
            if ids.is_empty() {
                return None;
            }
            let subject = harness.pick(&ids)?.clone();
            let target = harness.pick(&ids)?.clone();
            let action = ["message", "delegate", "shell"][(harness.next_rand() % 3) as usize];
            // shell 授权用与 create_instance 相同的范围，撤销之后还能重新授予
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
        // 18: 发消息（可能需要授权；被拒也是合法结果）
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
        // 19: 派发操作。始终要求批准：先走批准闸门（A25），批准后再派发成功；
        // 不要求批准的直派路径由 core 的单元测试覆盖，这里让游走稳定走到批准链路
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
        // 20: 结清操作
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
        // 21: 批准 / 拒绝
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
        // 22: 重置 epoch（安全边界：实例不再有在途回合时才重置，与 driver 一致）
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
        // 23: 生命周期（暂停 / 恢复 / 终止）。终止同样落在安全边界上：实例处于 READY
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
        // 24: 制品 stage / publish
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
        // 28: 导入工具意图（产生 PREPARED 操作，供派发/批准/回执路径使用）
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
        // 29: 重新授权（撤权/换版后把 PREPARED 操作重新盖上当前权限版本）
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
        // 30: 开门压缩（与 turn 请求共用准入闸门）
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
        // 31: 提交压缩（追加总结、覆盖旧条目、关请求、释放预留）
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
            // 一半情况保留最早一条可见条目（覆盖其余），另一半全部覆盖
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
        // 32: 压缩失败（上下文不动，只关请求并释放预留）
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
        // 33: 取消一个 PREPARED 操作（待批批准随之过期，RT-06 的过期路径）
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
        // 34: 重放上一条命令（同 id 同 payload）：返回存量回执，状态不动
        34 => {
            let (last, identity) = harness.last_command.clone()?;
            Some(Step {
                label: format!("replay_same({})", last.command_id),
                command: Command { command_id: last.command_id.clone(), ..last },
                identity,
            })
        }
        // 35: 反例探针——同一个 command id 换了 payload（规格要求拒绝）
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
        // 36: 撤销一条仍然有效的 shell 授权（A03/A04 的撤权面）
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
        // 37: 反例探针——授权已被撤销的实例不得再派发（规格要求被拒）
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
                return None; // 还有有效授权时这条探针不成立
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
        // 26: 反例探针——委派到已结清的目标（规格要求被拒）
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
        // 27: 反例探针——非承接者结清任务（规格要求被拒绝）
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
        // 25: GC 认领
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
    // 长度 ≤ 2 的穷举：每条序列从全新库开始（覆盖所有命令对，包括被拒绝的组合）
    for first in 0..STEPS {
        run_sequence("exh1", &[first]);
        for second in 0..STEPS {
            run_sequence("exh2", &[first, second]);
        }
    }
}

#[test]
fn spec_invariants_hold_over_random_walks() {
    // 固定种子的随机游走：更深的状态空间
    let mut reached = Coverage::default();
    for walk in 0..60u64 {
        let mut harness = Harness::new("walk");
        harness.rng = 0x9E3779B97F4A7C15 ^ (walk.wrapping_mul(0xD1B54A32D192ED03) | 1);
        harness.check("init");
        let mut taken: HashMap<String, usize> = HashMap::new();
        for _ in 0..24 {
            // 只在"当前能用"的命令里挑，并优先挑本游走里用得最少的种类（覆盖驱动）：
            // 否则游走会反复做同一件安全的事，永远走不到深层链路
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
    // 游走必须真的走到这些状态，否则上面的"全部通过"只是空转
    for (name, seen) in [
        ("等被解决（SATISFIED/CANCELLED）", reached.resolved_wait),
        ("目标结清", reached.settled_goal),
        ("任务结清", reached.settled_task),
        ("操作终态", reached.settled_operation),
        ("epoch 重置", reached.reset_epoch),
        ("实例终止", reached.terminated),
        ("制品 LIVE", reached.artifact_live),
        ("压缩提交（有条目被覆盖）", reached.compressed),
        ("批准已决定（批准/拒绝/过期）", reached.approval_decided),
        ("命令重放（同 id 同 payload 返回存量回执、异 payload 被拒）", reached.replay_checked),
        ("撤权后派发被拒（A03/A04）", reached.revocation_checked),
    ] {
        assert!(seen, "随机游走没有覆盖到：{name}");
    }
}

/// 反向验证：人为破坏状态时检查器必须报出来，否则上面的"通过"没有意义。
#[test]
fn the_invariant_checker_detects_broken_states() {
    // 1) 未知任务状态
    let mut harness = Harness::new("broken-status");
    let step = make_step(&mut harness, 0).expect("create instance");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 1).expect("create goal");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 10).expect("delegate task");
    let _ = harness.run_step(step);
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "健康的起点不应报违反：{healthy:?}");
    harness.ctl.connection().execute("UPDATE tasks SET status = 'WAT'", []).unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("TypeOK: task")), "应报出未知状态：{reported:?}");

    // 2) 终态任务被改写（跨步记忆）：先经控制面正常结清，再手工改写
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
    assert!(settled.is_ok(), "正常结清应当成功：{settled:?}");
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "正常结清后不应报违反：{healthy:?}");
    harness.ctl.connection().execute("UPDATE tasks SET status = 'RUNNING'", []).unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("SettledIsFinal")), "应报出终态改写：{reported:?}");

    // 4) 覆盖被回抬（`CoverageNeverLifted`）与覆盖指向更早的条目（`CoveragePointsForward`）
    let mut harness = Harness::new("broken-compress");
    for kind in [0, 14, 30, 31] {
        let step = make_step(&mut harness, kind).expect("compression setup");
        let _ = harness.run_step(step);
    }
    let healthy = harness.violations();
    assert!(healthy.is_empty(), "正常压缩后不应报违反：{healthy:?}");
    let covered = harness.rows("SELECT id FROM context_entries WHERE compressed_by IS NOT NULL", &[]);
    assert!(!covered.is_empty(), "压缩提交必须覆盖到条目");
    harness
        .ctl
        .connection()
        .execute("UPDATE context_entries SET compressed_by = NULL WHERE compressed_by IS NOT NULL", [])
        .unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("CoverageNeverLifted")), "应报出覆盖被回抬：{reported:?}");

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
    // 手工把总结指向自己：索引不再严格大于被覆盖条目
    harness.ctl.connection().execute("UPDATE context_entries SET kind = 'note' WHERE kind = 'summary'", []).unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("CoveragePointsForward")), "应报出覆盖指向非总结：{reported:?}");

    // 3) 实例悬挂已结清目标（V-G1）
    let mut harness = Harness::new("broken-goal");
    let step = make_step(&mut harness, 0).expect("create instance");
    let _ = harness.run_step(step);
    let step = make_step(&mut harness, 1).expect("create goal");
    let _ = harness.run_step(step);
    harness.ctl.connection().execute("UPDATE goals SET status = 'SUCCEEDED'", []).unwrap();
    let reported = harness.violations();
    assert!(reported.iter().any(|line| line.contains("NoStaleActiveGoal")), "应报出悬挂指针：{reported:?}");
}
