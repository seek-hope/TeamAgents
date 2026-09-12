//! The single serialized entry point for team transactions per session,
//! ported from src/teamagents/control.py.
//!
//! Milestone 1: submit/transaction/idempotency core. Reduction ports land
//! per action kind with their Python scenario tests as oracles (see
//! docs/RECONSTRUCT.md for the ledger).

use crate::models::*;
use crate::storage::Store;
use sha2::{Digest, Sha256};

pub struct Control {
    pub store: Store,
    pub session_id: String,
}

/// control.py::Control._derived_task_id — deterministic per action id.
pub fn derived_task_id(action: &TeamAction) -> String {
    if let Some(explicit) = action.payload.get("task_id").and_then(|v| v.as_str()) {
        return explicit.to_string();
    }
    let digest = hex_prefix(&Sha256::digest(action.action_id.as_bytes()), 12);
    format!("task_{digest}")
}

/// control.py::Control._payload_hash — sha256 of canonical payload json.
pub fn payload_hash(action: &TeamAction) -> String {
    // json.dumps(payload, sort_keys=True, ensure_ascii=False): serde_json maps
    // are BTreeMap-backed? No — serde_json::Value::Object preserves insertion
    // order unless preserve_order is off. Canonical form: serialize via a
    // sorted representation.
    let canonical = canonical_json(&action.payload);
    hex_prefix(&Sha256::digest(canonical.as_bytes()), 32)
}

fn canonical_json(v: &Json) -> String {
    match v {
        Json::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Json::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

fn hex_prefix(bytes: &[u8], n: usize) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()[..n].to_string()
}

impl Control {
    pub fn new(store: Store, session_id: impl Into<String>) -> Self {
        Self { store, session_id: session_id.into() }
    }

    /// control.py::Control.submit — one transaction; a failed attempt rolls
    /// back, then the failure receipt commits in a clean transaction.
    pub fn submit(&mut self, action: &TeamAction) -> Receipt {
        match self.submit_once(action) {
            Ok(r) => r,
            Err(e) => {
                let receipt = Receipt::failure(action, format!("{}: {e}", std::any::type_name_of_val(&e)));
                if let Ok(Some(prior)) = self.store.get_action_receipt(&action.action_id) {
                    return prior;
                }
                let _ = self.store.record_action(
                    &action.action_id,
                    &self.session_id,
                    &action.actor_id,
                    action.run_id.as_deref(),
                    action.kind,
                    &payload_hash(action),
                    &receipt,
                );
                receipt
            }
        }
    }

    fn submit_once(&mut self, action: &TeamAction) -> Result<Receipt, String> {
        if let Some(prior) = self.store.get_action_receipt(&action.action_id).map_err(|e| e.to_string())? {
            return Ok(prior);
        }
        let receipt = self.reduce(action)?;
        self.store
            .record_action(
                &action.action_id,
                &self.session_id,
                &action.actor_id,
                action.run_id.as_deref(),
                action.kind,
                &payload_hash(action),
                &receipt,
            )
            .map_err(|e| e.to_string())?;
        Ok(receipt)
    }

    fn reduce(&mut self, action: &TeamAction) -> Result<Receipt, String> {
        // ponytail: reduction ports land action-by-action with their Python
        // scenario tests as oracles; unported kinds are a readable refusal.
        Err(format!("action kind {:?} not yet ported to teamagents-core", action.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: &str) -> TeamAction {
        TeamAction {
            action_id: id.into(),
            session_id: "s1".into(),
            actor_id: "user".into(),
            run_id: None,
            kind: ActionKind::UserMessage,
            payload: serde_json::json!({"text": "hi"}),
        }
    }

    #[test]
    fn payload_hash_is_sorted_and_stable() {
        let a = TeamAction { payload: serde_json::json!({"b": 1, "a": {"y": 2, "x": 1}}), ..action("a") };
        let b = TeamAction { payload: serde_json::json!({"a": {"x": 1, "y": 2}, "b": 1}), ..action("b") };
        assert_eq!(payload_hash(&a), payload_hash(&b));
        assert_eq!(payload_hash(&a).len(), 32);
    }

    #[test]
    fn derived_task_id_prefers_explicit() {
        let a = TeamAction { payload: serde_json::json!({"task_id": "task_x"}), ..action("a") };
        assert_eq!(derived_task_id(&a), "task_x");
        assert!(derived_task_id(&action("a")).starts_with("task_"));
    }

    #[test]
    fn failure_receipt_commits_and_replays() {
        let mut ctl = Control::new(Store::open_memory().unwrap(), "s1");
        let r1 = ctl.submit(&action("act_1"));
        assert!(!r1.ok);
        let r2 = ctl.submit(&action("act_1"));
        assert_eq!(r1.action_id, r2.action_id);
        assert!(!r2.ok);
    }
}
