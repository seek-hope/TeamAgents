use super::{json_response, runtime, FakeServer};
use serde_json::{json, Value};
use std::process::Command;

#[path = "../common/mod.rs"]
mod common;

#[test]
fn local_router_cli_checks_consent_state_and_the_exact_model_before_sending() {
    let rt = runtime();
    let root = common::TempRoot::new("local-router-cli");
    let row = |id, status| json!({"id":id,"path":"private-path-fixture","status":{"value":status,"args":["private-argument-fixture"]}});
    let list = |rows: Vec<Value>| json_response("200 OK", &json!({"data":rows}));
    let idle = list(vec![row("pkg/model:Q4", "unloaded")]);
    let ack = json_response("200 OK", &json!({"success":true}));
    let server = rt.block_on(FakeServer::start(vec![
        idle.clone(),
        list(vec![row("pkg/model:Q4", "unloaded"), row("other", "sleeping")]),
        idle.clone(),
        ack.clone(),
        list(vec![row("pkg/model:Q4", "loaded")]),
        ack,
        idle,
        String::new(),
        list(vec![row("pkg/model:Q4", "loading")]),
    ]));
    let cfg = root.join("cfg/teamagents");
    std::fs::create_dir_all(&cfg).unwrap();
    let text=format!("local_router_profiles=['chosen']\n[models.chosen]\nprovider='local'\nmodel='pkg/model:Q4'\nbase_url='{}/v1'\ntimeout=2\napi_key_env='TA_ROUTER_TEST_KEY'\n",server.base);
    std::fs::write(cfg.join("config.toml"), &text).unwrap();
    std::fs::create_dir_all(root.join(".teamagents")).unwrap();
    std::fs::write(root.join(".teamagents/config.toml"),"local_router_profiles=['other']\n[models.other]\nprovider='local'\nmodel='unbound'\nbase_url='http://127.0.0.1:1/v1'\n").unwrap();
    let run = |args: &[&str], rc: i32| {
        let out = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("local-models")
            .args(args)
            .current_dir(&root.dir)
            .env("XDG_CONFIG_HOME", root.join("cfg"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_ROUTER_TEST_KEY", "fixture-bearer")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(rc), "{args:?}: {out:?}");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        for private in ["fixture-bearer", "private-path-fixture", "private-argument-fixture"] {
            assert!(!text.contains(private));
        }
        text
    };
    run(&["load", "--model", "chosen"], 2);
    run(&["list", "--model", "other"], 1);
    run(&["list", "--model", "chosen", "--yes"], 2);
    assert!(server.requests.lock().unwrap().is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&run(&["list", "--model", "chosen", "--json"], 0)).unwrap()["models"]
            ["pkg/model:Q4"],
        "unloaded"
    );
    let op = |verb, rc| run(&[verb, "--model", "chosen", "--yes", "--json"], rc);
    assert!(op("load", 1).contains("another resident model"));
    for (verb, state) in [("load", "unloaded"), ("unload", "loaded")] {
        let reply: Value = serde_json::from_str(&op(verb, 0)).unwrap();
        assert_eq!(reply["acknowledged"], true);
        assert_eq!(reply["prior_state"], state);
    }
    assert!(op("load", 1).contains("outcome unknown"));
    assert_eq!(server.requests.lock().unwrap().len(), 8);
    assert!(run(&["list", "--model", "chosen"], 0).contains("loading"));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 9);
    for (index, req) in requests.iter().enumerate() {
        assert!(req.to_lowercase().contains("authorization: bearer fixture-bearer\r\n"));
        if matches!(index, 3 | 5 | 7) {
            let path = if index == 5 { "unload" } else { "load" };
            assert!(req.starts_with(&format!("POST /models/{path} HTTP/")));
            assert_eq!(
                serde_json::from_str::<Value>(req.split_once("\r\n\r\n").unwrap().1).unwrap(),
                json!({"model":"pkg/model:Q4"})
            );
        } else {
            assert!(req.starts_with("GET /models HTTP/"));
        }
    }
    assert_eq!(std::fs::read_to_string(cfg.join("config.toml")).unwrap(), text);
    assert!(!root.join("state").exists());
}

#[test]
fn local_router_failures_never_redirect_or_replay_a_mutation() {
    let rt = runtime();
    let idle = json_response("200 OK", &json!({"data":[{"id":"m","status":{"value":"unloaded"}}]}));
    let other = rt.block_on(FakeServer::start(vec![idle.clone()]));
    let redirect = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: {}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        other.base
    );
    let inconsistent = json_response(
        "200 OK",
        &json!({"data":[
            {"id":"m","status":{"value":"unloaded"}}, {"id":"peer","status":{"value":"loaded","failed":true}}
        ]}),
    );
    let server = rt.block_on(FakeServer::start(vec![
        redirect.clone(),
        inconsistent,
        idle.clone(),
        redirect,
        idle.clone(),
        json_response("200 OK", &json!({"success":"true"})),
        idle,
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{".into(),
    ]));
    let cfg = teamagents_engine::config::parse_user_config(&format!(
        "local_router_profiles=['p']\n[models.p]\nprovider='local'\nmodel='m'\nbase_url='{}'\ntimeout=2",
        server.base
    ))
    .unwrap();
    for (message, post_count) in [
        ("307", 0),
        ("inconsistent router model state", 0),
        ("outcome unknown", 1),
        ("outcome unknown", 2),
        ("outcome unknown", 3),
    ] {
        let err = teamagents_engine::local_models::command(&cfg, "load", "p", true).unwrap_err();
        assert!(err.contains(message), "{err}");
        let seen = server.requests.lock().unwrap();
        assert_eq!(seen.iter().filter(|r| r.starts_with("POST ")).count(), post_count);
        assert!(other.requests.lock().unwrap().is_empty());
    }
    assert_eq!(server.requests.lock().unwrap().len(), 8);
}

#[test]
fn local_router_config_only_allows_an_explicit_numeric_loopback_route() {
    for url in [
        "https://example.invalid/v1",
        "http://localhost/v1",
        "http://user@127.0.0.1:9931/v1",
        "http://127.0.0.1/v1?q=x",
        "http://127.0.0.1:9931/arbitrary",
    ] {
        assert!(teamagents_engine::config::parse_user_config(&format!(
            "local_router_profiles=['m']\n[models.m]\nprovider='local'\nmodel='a'\nbase_url='{url}'"
        ))
        .is_err());
    }
}
