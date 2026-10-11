//! D-451: pending input revisions race only at the authoritative control transaction.
use serde_json::{json, Value};
use std::path::PathBuf;
use teamagents_core::v2::{Command, Control, Identity};

struct Session(PathBuf);
impl Session {
    fn new() -> (Self, Control) {
        let path = std::env::temp_dir().join(format!("teamagents-input-{}.db", uuid::Uuid::new_v4()));
        let mut ctl = Control::open(&path, "s1", true).unwrap();
        for id in ["i1", "i2"] {
            apply(&mut ctl, id, "create_instance", json!({"id": id})).unwrap();
        }
        apply(&mut ctl, "start", "begin_request", json!({"instance_id":"i1", "request_id":"r1", "revision":0}))
            .unwrap();
        (Self(path), ctl)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
fn command(id: &str, method: &str, params: Value) -> Command {
    Command { command_id: id.into(), method: method.into(), params }
}
fn apply(ctl: &mut Control, id: &str, method: &str, params: Value) -> Result<Value, String> {
    ctl.submit(command(id, method, params), Identity::User)
}
fn queue(ctl: &mut Control, id: &str, text: &str) {
    assert_eq!(
        apply(ctl, id, "submit_input", json!({"instance_id":"i1","envelope_id":id,"text":text})).unwrap()["queued"],
        true
    );
}
fn revise(id: &str, revision: i64) -> Value {
    json!({"instance_id":"i1","envelope_id":id,"revision":revision,"text":"revised"})
}
fn drain(ctl: &mut Control, id: &str) -> Result<Value, String> {
    apply(ctl, id, "drain_inbox", json!({"instance_id":"i1"}))
}

#[test]
fn edits_and_cancellation_survive_reopen_without_reordering_or_double_consumption() {
    let (session, mut ctl) = Session::new();
    queue(&mut ctl, "a", "original");
    queue(&mut ctl, "b", "cancel this");
    queue(&mut ctl, "c", "last");
    let edited = apply(&mut ctl, "edit", "edit_input", revise("a", 0)).unwrap();
    assert_eq!(edited["revision"], 1);
    assert_eq!(apply(&mut ctl, "edit", "edit_input", revise("a", 0)).unwrap(), edited);
    assert!(apply(&mut ctl, "stale", "edit_input", revise("a", 0)).is_err());
    apply(&mut ctl, "cancel", "cancel_input", revise("b", 0)).unwrap();
    assert!(apply(&mut ctl, "uncancel", "edit_input", revise("b", 1)).is_err());
    drop(ctl);
    let mut ctl = Control::open(&session.0, "s1", false).unwrap();
    assert_eq!(drain(&mut ctl, "drain").unwrap()["applied"], 2);
    assert_eq!(drain(&mut ctl, "again").unwrap()["applied"], 0);
    let entries: Vec<(String, String)> = ctl
        .connection()
        .prepare("SELECT envelope_id, message_json FROM context_entries ORDER BY idx")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(entries.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    assert!(entries[0].1.contains("revised"));
    assert!(!entries[0].1.contains("original"));
    let original: String = ctl
        .connection()
        .query_row("SELECT payload_json FROM input_revisions WHERE envelope_id='a' AND revision=0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&original).unwrap()["text"], "original");
    assert!(apply(&mut ctl, "late", "edit_input", revise("a", 1)).is_err());
    assert!(apply(&mut ctl, "late-cancel", "cancel_input", revise("c", 0)).is_err());
}

#[test]
fn input_revision_refuses_wrong_actor_recipient_session_epoch_and_malformed_updates() {
    let (session, mut ctl) = Session::new();
    queue(&mut ctl, "a", "original");
    for identity in [Identity::Instance("i1".into()), Identity::System] {
        assert!(ctl.submit(command("actor", "edit_input", revise("a", 0)), identity).is_err());
    }
    for patch in [json!({"instance_id":"i2"}), json!({"revision":-1}), json!({"revision":"0"}), json!({"text":" "})] {
        let mut params = revise("a", 0);
        for (key, value) in patch.as_object().unwrap() {
            params[key] = value.clone();
        }
        assert!(apply(&mut ctl, "invalid", "edit_input", params).is_err());
    }
    let mut other = Control::open(&session.0, "different", false).unwrap();
    assert!(apply(&mut other, "foreign", "cancel_input", revise("a", 0)).is_err());
    apply(&mut ctl, "reset", "reset_instance", json!({"instance_id":"i1"})).unwrap();
    assert!(apply(&mut ctl, "epoch", "edit_input", revise("a", 0)).is_err());
}

#[test]
fn concurrent_cancel_and_drain_never_apply_cancelled_input() {
    let (session, mut ctl) = Session::new();
    queue(&mut ctl, "a", "effect");
    drop(ctl);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [true, false]
        .into_iter()
        .map(|cancel| {
            let path = session.0.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut ctl = Control::open(&path, "s1", false).unwrap();
                barrier.wait();
                if cancel {
                    apply(&mut ctl, "cancel", "cancel_input", revise("a", 0))
                } else {
                    drain(&mut ctl, "drain")
                }
            })
        })
        .collect();
    let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(outcomes.iter().any(Result::is_ok));
    let mut ctl = Control::open(&session.0, "s1", false).unwrap();
    drain(&mut ctl, "finish-drain").unwrap();
    let state: String =
        ctl.connection().query_row("SELECT state FROM envelopes WHERE id='a'", [], |r| r.get(0)).unwrap();
    let count: i64 = ctl
        .connection()
        .query_row("SELECT COUNT(*) FROM context_entries WHERE envelope_id='a'", [], |r| r.get(0))
        .unwrap();
    assert!(matches!((state.as_str(), count), ("CANCELLED", 0) | ("APPLIED", 1)), "{state} {count}");
}

#[test]
fn migration_preserves_legacy_pending_input_and_captures_original_before_edit() {
    let (session, mut ctl) = Session::new();
    queue(&mut ctl, "a", "legacy");
    ctl.connection()
        .execute_batch("DROP TABLE input_revisions; UPDATE meta SET value='4' WHERE key='schema_version'")
        .unwrap();
    drop(ctl);
    let mut ctl = Control::open(&session.0, "s1", false).unwrap();
    apply(&mut ctl, "edit", "edit_input", revise("a", 0)).unwrap();
    let raw: String = ctl
        .connection()
        .query_row("SELECT payload_json FROM input_revisions WHERE envelope_id='a' AND revision=0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&raw).unwrap()["text"], "legacy");
}
