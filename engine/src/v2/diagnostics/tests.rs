use super::*;
use std::path::PathBuf;

struct Fixture {
    dir: PathBuf,
    conn: Connection,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("teamagents-diagnostics-{}", uuid::Uuid::new_v4()));
        let conn = teamagents_core::v2::store::open(&dir.join("session.sqlite"), true).unwrap();
        conn.execute_batch(
            "INSERT INTO instances (id, session_id, profile_revision, lifecycle, phase, profile_json)
             VALUES ('worker', 's', 0, 'ACTIVE', 'READY', '{}');
             INSERT INTO tasks (id, goal_id, session_id, requester, assignee, dependencies_json,
                 acceptance_refs_json, status, result_refs_json)
             VALUES ('task', 'g', 's', 'user', 'worker', '[]', '[]', 'RUNNING', '[]');
             INSERT INTO context_entries (instance_id, epoch, idx, id, kind, message_json, created)
             VALUES ('worker', 0, 1, 'entry', 'assistant', '{}', 0);",
        )
        .unwrap();
        Self { dir, conn }
    }

    fn view(&self, all: bool) -> View {
        let tx = self.conn.unchecked_transaction().unwrap();
        let view = read(&tx, all).unwrap();
        tx.commit().unwrap();
        view
    }

    fn code(&self) -> String {
        self.view(false).activities["worker"]["code"].as_str().unwrap().into()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn idle_unsettled_diagnostics_are_read_only_and_survive_reopen() {
    let f = Fixture::new();
    f.conn.pragma_update(None, "query_only", true).unwrap();
    let before = f.conn.total_changes();
    let first = f.view(false);
    assert_eq!(first.activities["worker"]["code"], "idle_unsettled");
    assert_eq!(first.tasks[0]["status"], "RUNNING");
    assert_eq!(first.tasks[0]["diagnosis"]["code"], "idle_unsettled");
    assert!(first.tasks[0]["diagnosis"]["next_steps"].as_str().unwrap().contains("instances send"));
    assert_eq!(first.tasks, f.view(false).tasks);
    assert_eq!(before, f.conn.total_changes());
    let reopened = teamagents_core::v2::store::open_read_only(&f.dir.join("session.sqlite")).unwrap();
    let tx = reopened.unchecked_transaction().unwrap();
    let next = read(&tx, false).unwrap();
    assert_eq!(first.activities, next.activities);
    assert_eq!(first.tasks, next.tasks);
}

#[test]
fn ready_is_not_idle_when_work_is_unaddressed_and_uses_current_visible_epoch() {
    let f = Fixture::new();
    for kind in ["user", "note", "tool_result", "summary"] {
        f.conn.execute("UPDATE context_entries SET kind = ?1", [kind]).unwrap();
        assert_eq!(f.code(), "ready", "{kind}");
    }
    for kind in ["assistant", "runtime"] {
        f.conn.execute("UPDATE context_entries SET kind = ?1", [kind]).unwrap();
        assert_eq!(f.code(), "idle_unsettled", "{kind}");
    }
    f.conn
        .execute_batch(
            "INSERT INTO context_entries (instance_id, epoch, idx, id, kind, message_json, created)
         VALUES ('worker', 0, 2, 'summary', 'summary', '{}', 0),
                ('worker', 0, 3, 'covered', 'user', '{}', 0),
                ('worker', 1, 1, 'future-epoch', 'user', '{}', 0);
         UPDATE context_entries SET compressed_by = 'summary' WHERE id = 'covered';",
        )
        .unwrap();
    assert_eq!(f.code(), "idle_unsettled", "summary must precede the visible runtime tail");
    f.conn.execute_batch("UPDATE tasks SET status = 'PENDING'").unwrap();
    assert_eq!(f.code(), "ready");
    f.conn
        .execute_batch(
            "UPDATE tasks SET status = 'RUNNING';
         INSERT INTO envelopes (id, session_id, sender, recipient, epoch, kind, sequence, state)
         VALUES ('input', 's', 'user', 'worker', 9, 'user_input', 1, 'ACCEPTED');",
        )
        .unwrap();
    assert_eq!(f.code(), "idle_unsettled", "stale/future epoch input cannot drive this epoch");
    f.conn.execute_batch("UPDATE envelopes SET epoch = 0").unwrap();
    assert_eq!(f.code(), "ready", "queued input prevents an idle diagnosis");
}

#[test]
fn waits_report_conditions_and_timer_presence_without_claiming_deadlock() {
    let f = Fixture::new();
    f.conn.execute_batch("UPDATE instances SET phase = 'WAITING'").unwrap();
    assert_eq!(f.code(), "wait_missing");
    for mode in ["ALL", "ANY"] {
        f.conn.execute("INSERT OR REPLACE INTO waits VALUES ('wait', 'worker', 0, ?1, ?2, NULL, 'PENDING')",
            rusqlite::params![mode, json!([{"kind":"task","task_id":"task"},{"kind":"message","from":"external"},{"kind":"operation","operation_id":"job"}]).to_string()]).unwrap();
        let view = f.view(false);
        let d = &view.activities["worker"];
        assert_eq!(d["code"], "waiting");
        assert_eq!(d["attention"], false);
        assert!(d["summary"].as_str().unwrap().contains("no timer"));
        assert_eq!(d["waits"][0]["mode"], mode);
        assert_eq!(d["waits"][0]["conditions"].as_array().unwrap().len(), 3);
        f.conn.execute_batch("UPDATE waits SET timer_at = 1").unwrap();
        assert!(f.view(false).activities["worker"]["summary"].as_str().unwrap().contains("timer is recorded"));
    }
    f.conn.execute_batch("UPDATE waits SET epoch = 5").unwrap();
    assert_eq!(f.code(), "wait_missing");
}

#[test]
fn approval_tools_and_inactive_states_preserve_operation_facts() {
    let f = Fixture::new();
    f.conn
        .execute_batch(
            "UPDATE instances SET phase = 'TOOLS_PENDING', active_request_id = 'request';
         INSERT INTO model_requests (request_id, instance_id, epoch, request_ref, status)
         VALUES ('request', 'worker', 0, 'ref', 'SUCCEEDED');
         INSERT INTO decisions (decision_id, request_id) VALUES ('decision', 'request');
         INSERT INTO operations (operation_id, decision_id, tool_index, epoch, args_hash, status)
         VALUES ('job', 'decision', 0, 0, 'hash', 'RUNNING');
         INSERT INTO approvals (id, session_id, operation_id, args_hash, grant_revision, status)
         VALUES ('approval', 's', 'job', 'hash', 0, 'PENDING');",
        )
        .unwrap();
    assert_eq!(f.code(), "waiting_approval");
    f.conn.execute_batch("UPDATE approvals SET status = 'APPROVED'").unwrap();
    assert_eq!(f.code(), "tools_pending");
    let active = f.view(false).activities["worker"].clone();
    assert_eq!(active["attention"], false);
    assert_eq!(active["operations"][0]["status"], "RUNNING");
    assert!(active["next_steps"].as_str().unwrap().contains("terminate"));
    f.conn
        .execute_batch("UPDATE instances SET lifecycle = 'PAUSED'; UPDATE operations SET cancel_requested = 1")
        .unwrap();
    let paused = f.view(false).activities["worker"].clone();
    assert_eq!(paused["code"], "inactive");
    assert_eq!(paused["operations"][0]["cancel_requested"], true);
    f.conn
        .execute_batch(
            "UPDATE tasks SET status = 'BLOCKED';
         UPDATE operations SET status = 'OUTCOME_UNKNOWN';
         INSERT INTO events (session_id, sequence, kind, scope, payload_json, created)
         VALUES ('s', 1, 'task_blocked', 'task', '{\"reason\":\"outcome_unknown\"}', 0);",
        )
        .unwrap();
    let blocked = &f.view(false).tasks[0];
    assert_eq!(blocked["diagnosis"]["code"], "blocked");
    assert_eq!(blocked["diagnosis"]["reason"], "outcome_unknown");
    assert_eq!(blocked["assignee_activity"]["operations"][0]["status"], "OUTCOME_UNKNOWN");
    f.conn.execute_batch("UPDATE instances SET lifecycle = 'ACTIVE', phase = 'WAITING_CAPABILITY'").unwrap();
    assert_eq!(f.code(), "waiting_capability");
    f.conn.execute_batch("UPDATE instances SET phase = 'MODEL_PENDING'").unwrap();
    assert_eq!(f.code(), "model_pending");
}

#[test]
fn settled_task_does_not_claim_its_assignees_command_stopped() {
    let f = Fixture::new();
    f.conn
        .execute_batch("UPDATE tasks SET status = 'CANCELLED'; UPDATE instances SET phase = 'TOOLS_PENDING'")
        .unwrap();
    assert!(f.view(false).tasks.is_empty());
    let all = f.view(true);
    assert_eq!(all.tasks[0]["diagnosis"]["code"], "settled");
    assert_eq!(all.tasks[0]["assignee_activity"]["code"], "tools_pending");
    assert_eq!(all.tasks[0]["status"], "CANCELLED");
}
