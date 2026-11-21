//! D-529: role changes and Worker creation share the same transactional store.
#[path = "../../test_scratch_owner.rs"]
mod scratch_owner;

use serde_json::{json, Value};
use std::path::Path;
use teamagents_core::v2::{control::role_model_snapshot, Command, Control, Identity};

fn submit(c: &mut Control, method: &str, params: Value, actor: Identity) -> Result<Value, String> {
    c.submit(Command { command_id: uuid::Uuid::new_v4().to_string(), method: method.into(), params }, actor)
}
fn user(c: &mut Control, method: &str, params: Value) -> Value {
    submit(c, method, params, Identity::User).unwrap()
}
fn session(path: &Path) -> Control {
    let mut c = Control::open(path, "s", true).unwrap();
    user(
        &mut c,
        "create_instance",
        json!({"id":"owner","is_leader":true,"profile":{"model":"wire","catalog_key":"account-a"}}),
    );
    c
}
fn profiles(c: &Control) -> Vec<(String, String, i64)> {
    c.connection()
        .prepare("SELECT id,profile_json,revision FROM instances ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn key(c: &Control, id: &str) -> String {
    c.connection()
        .query_row("SELECT json_extract(profile_json,'$.catalog_key') FROM instances WHERE id=?1", [id], |r| r.get(0))
        .unwrap()
}
fn event_count(c: &Control) -> i64 {
    c.connection().query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap()
}

#[test]
fn a_pin_survives_a_leader_change_and_only_live_workers_resume_following() {
    let mut c = session(Path::new(":memory:"));
    assert_eq!(
        role_model_snapshot(c.connection()).unwrap(),
        json!({"leader_id":"owner","leader":"account-a","workers":{"mode":"follow"}})
    );
    let pin = user(&mut c, "set_role_model", json!({"role":"workers","model":"account-a"}));
    assert_eq!(pin["workers"], json!({"mode":"pinned","key":"account-a"}));
    user(&mut c, "create_instance", json!({"id":"existing"}));
    user(&mut c, "spawn_instance", json!({"instance_id":"retired","instructions":"review"}));
    user(&mut c, "set_lifecycle", json!({"instance_id":"retired","lifecycle":"TERMINATED"}));
    user(&mut c, "set_role_model", json!({"role":"leader","model":"account-b"}));
    user(&mut c, "spawn_instance", json!({"instance_id":"new"}));
    assert_eq!(key(&c, "owner"), "account-b");
    for id in ["existing", "new", "retired"] {
        assert_eq!(key(&c, id), "account-a");
    }
    user(&mut c, "set_role_model", json!({"role":"workers","follow":true}));
    for id in ["existing", "new"] {
        assert_eq!(key(&c, id), "account-b");
    }
    user(&mut c, "set_role_model", json!({"role":"leader","model":"account-c"}));
    for id in ["owner", "existing", "new"] {
        assert_eq!(key(&c, id), "account-c");
    }
    assert_eq!(key(&c, "retired"), "account-a");
}

#[test]
fn role_changes_refuse_non_users_private_overrides_and_busy_batches_atomically() {
    for begin in ["begin_request", "begin_compression"] {
        let mut c = session(Path::new(":memory:"));
        for id in ["w1", "w2"] {
            user(&mut c, "create_instance", json!({"id":id}));
        }
        for actor in [Identity::Instance("owner".into()), Identity::System] {
            assert!(submit(&mut c, "set_role_model", json!({"role":"workers","model":"account-b"}), actor).is_err());
        }
        for (method, params) in [
            ("create_instance", json!({"id":"bad","profile":{"options":{"reasoning_effort":"high"}}})),
            ("spawn_instance", json!({"instance_id":"bad","profile":{"catalog_key":"account-b"}})),
            ("spawn_instance", json!({"instance_id":"bad","model":"account-b"})),
            ("set_instance_profile", json!({"instance_id":"w1","model":"account-b"})),
        ] {
            assert!(submit(&mut c, method, params, Identity::User).is_err());
        }
        submit(
            &mut c,
            begin,
            json!({"instance_id":"w2","request_id":"prepared","revision":0,"request_ref":"original"}),
            Identity::System,
        )
        .unwrap();
        let before = (profiles(&c), event_count(&c), role_model_snapshot(c.connection()).unwrap());
        for role in ["leader", "workers"] {
            let error =
                submit(&mut c, "set_role_model", json!({"role":role,"model":"account-b"}), Identity::User).unwrap_err();
            assert!(error.contains("w2") && error.contains("in flight"), "{error}");
            assert_eq!((profiles(&c), event_count(&c), role_model_snapshot(c.connection()).unwrap()), before);
        }
        let row: (String, String) = c
            .connection()
            .query_row("SELECT status,request_ref FROM model_requests WHERE request_id='prepared'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("PENDING".into(), "original".into()));
    }
}

#[test]
fn migration_preserves_v9_profiles_and_requires_a_decision_for_live_workers() {
    let root = scratch_owner::owned_temp_dir("ta-role-migration", &uuid::Uuid::new_v4().to_string());
    for live in [true, false] {
        let path = root.join(format!("{live}.sqlite"));
        let mut c = session(&path);
        user(&mut c, "create_instance", json!({"id":"w"}));
        if !live {
            user(&mut c, "set_lifecycle", json!({"instance_id":"w","lifecycle":"TERMINATED"}));
        }
        c.connection().execute_batch("DELETE FROM meta WHERE key='worker_models'; UPDATE meta SET value='9' WHERE key='schema_version'; UPDATE instances SET profile_json='{\"model\":\"legacy-wire\"}' WHERE id='w';").unwrap();
        let before = profiles(&c);
        drop(c);
        let mut c = Control::open(&path, "s", false).unwrap();
        let mode = if live { "unresolved" } else { "follow" };
        assert_eq!(role_model_snapshot(c.connection()).unwrap()["workers"], json!({"mode":mode}));
        assert_eq!(profiles(&c), before);
        if live {
            for (method, params) in [
                ("begin_request", json!({"instance_id":"w","revision":0,"request_id":"r"})),
                ("begin_compression", json!({"instance_id":"w","request_id":"s"})),
                ("create_instance", json!({"id":"w2"})),
            ] {
                assert!(submit(&mut c, method, params, Identity::User)
                    .unwrap_err()
                    .contains("select a shared Worker model"));
            }
            user(&mut c, "set_role_model", json!({"role":"workers","follow":true}));
            user(&mut c, "create_instance", json!({"id":"w2"}));
            assert_eq!(key(&c, "w"), key(&c, "w2"));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
