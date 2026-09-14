//! P2-9: `state_brief` omits the events tail (the runtime's hot paths never
//! read it) while every other field matches a full `state` snapshot.

mod support;

use serde_json::json;
use support::*;

#[test]
fn state_brief_omits_events_and_keeps_the_rest() {
    isolated_state_home("brief");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("b", "worker")],
        "channels": [message_channel("leader", &["b"])],
    });
    let core = core_with_spec("s-brief", spec);
    let receipt = submit(&core, "ps1", "user", "pause_session", json!({}));
    assert!(receipt.ok);

    let full = core.state().unwrap();
    let brief = core.state_brief().unwrap();
    assert!(
        !full["events"].as_array().map(|e| e.is_empty()).unwrap_or(true),
        "the fixture must produce at least one event for the comparison to mean anything"
    );
    assert_eq!(brief["events"], json!([]));
    for key in ["session", "spec", "leader_id", "limits", "revision", "agents", "runs", "tasks", "pending_approvals"] {
        assert_eq!(brief.get(key), full.get(key), "field {key} must survive state_brief");
    }
}
