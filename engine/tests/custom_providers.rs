//! Custom providers use the existing adapters, persist and remain usable without GET /models.
mod support;

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use teamagents_engine::config::{load_user_config, save_custom_provider, user_config_path, CustomProvider};
use teamagents_engine::session::{open_session, OpenOptions};

fn input(name: &str, protocol: &str, base: &str) -> CustomProvider {
    serde_json::from_value(json!({"name":name,"protocol":protocol,"base_url":base,
        "model":"custom-model","api_key_env":"TA_CUSTOM_TEST_KEY","context_window":null}))
    .unwrap()
}

#[test]
fn custom_provider_validation_and_atomic_config_preservation() {
    let home = support::isolated_state_home("custom-provider-config");
    let path = user_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for original in [
        "# keep this comment\n[models.old]\nprovider = 'old' # inline comment\nmodel = 'old-model'\n[permissions]\nmode = 'approved_scope'\n",
        "# inline models table\nmodels = { old = { provider = 'old', model = 'old-model' } }\n[unknown]\nkeep = true\n",
        "",
    ] {
        std::fs::write(&path, original).unwrap();
        let (name, profile) = input("custom.模型", "response", "https://example.invalid/v1/").profile().unwrap();
        assert_eq!(profile.protocol, "responses");
        assert_eq!(profile.base_url.as_deref(), Some("https://example.invalid/v1"));
        save_custom_provider(&name, &profile).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains(original.lines().next().unwrap_or("")));
        if original.contains("# inline comment") { assert!(written.contains("# inline comment")); }
        let value: toml::Value = written.parse().unwrap();
        let before: toml::Value = original.parse().unwrap();
        for (key, value) in before.as_table().unwrap() {
            if key != "models" { assert_eq!(&written.parse::<toml::Value>().unwrap()[key], value); }
        }
        assert_eq!(value["models"][&name]["protocol"].as_str(), Some("responses"));
        let loaded = load_user_config(&path).unwrap();
        assert_eq!(loaded.models[&name].api_key_env.as_deref(), Some("TA_CUSTOM_TEST_KEY"));
        if !original.is_empty() { assert_eq!(loaded.models["old"].model, "old-model"); }
        assert!(save_custom_provider(&name, &profile).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }
    let (name, profile) = input("bad-config", "anthropic", "https://example.invalid").profile().unwrap();
    std::fs::write(&path, "[broken").unwrap();
    assert!(save_custom_provider(&name, &profile).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[broken");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(save_custom_provider(&name, &profile).is_err());
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
    let lock =
        std::fs::File::options().read(true).write(true).open(home.join("config/teamagents/config.lock")).unwrap();
    lock.lock().unwrap();
    assert!(save_custom_provider(&name, &profile).unwrap_err().contains("其他进程"));
    assert!(!path.exists());
    drop(lock);
    for (field, value) in [
        ("name", json!(" ")),
        ("name", json!("x\n")),
        ("model", json!("")),
        ("protocol", json!("unknown")),
        ("base_url", json!("file:///tmp/model")),
        ("base_url", json!("https://user:secret@example.invalid/v1")),
        ("base_url", json!("https://example.invalid/v1?key=secret")),
        ("base_url", json!("https://example.invalid/v1/responses")),
        ("api_key_env", json!("sk-do-not-paste-a-key")),
        ("context_window", json!(0)),
    ] {
        let mut data =
            json!({"name":"name", "model":"model", "protocol":"responses", "base_url":"https://example.invalid/v1"});
        data[field] = value;
        let invalid: CustomProvider = serde_json::from_value(data.clone()).unwrap();
        assert!(invalid.profile().is_err(), "accepted {data}");
    }
}

#[test]
fn added_providers_call_each_api_and_survive_reopen_without_model_discovery() {
    let mut home = support::isolated_state_home("custom-provider-runtime");
    home.set("TA_CUSTOM_TEST_KEY", "test-credential");
    let cwd = home.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let path = user_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[models.baseline]\nprovider='baseline'\nmodel='unused'\nbase_url='http://127.0.0.1:9/v1'\n")
        .unwrap();
    for (i, protocol) in ["responses", "anthropic", "chat/completions"].into_iter().enumerate() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            for round in 0..2 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10))
                        }
                        Err(e) => panic!("missing request: {e}"),
                    }
                };
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                let expected = match protocol {
                    "anthropic" => "messages",
                    other => other,
                };
                assert!(headers.starts_with(&format!("POST /v1/{expected} HTTP/1.1")), "{headers}");
                let lowercase = headers.to_ascii_lowercase();
                assert!(lowercase.contains(if protocol == "anthropic" {
                    "x-api-key: test-credential"
                } else {
                    "authorization: bearer test-credential"
                }));
                let length: usize = lowercase
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body: Json = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["model"], "custom-model");
                assert_eq!(body["stream"], true);
                assert!(body[if protocol == "responses" { "input" } else { "messages" }].is_array());
                if round == 1 {
                    assert!(body.to_string().contains("custom reply"), "history was not restored: {body}");
                }
                let reply = match protocol {
                    "responses" => json!({"id":"r1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"custom reply"}]}]}),
                    "anthropic" => json!({"id":"m1","type":"message","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"custom reply"}]}),
                    _ => json!({"choices":[{"message":{"role":"assistant","content":"custom reply"}}]}),
                }.to_string();
                write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
            }
        });
        let name = format!("custom-{i}");
        let open = || {
            open_session(OpenOptions {
            cwd: Some(cwd.clone()), session_id: Some(format!("custom-session-{i}")), full_auto: false,
            initial_spec: Some(json!({"leader_id":"leader","agents":[{"id":"leader","name":"Leader","role":"leader","runtime_kind":"deepagents","model_profile":"baseline"}]})),
            catalog: None, scripts: None,
        }).unwrap()
        };
        let first = open();
        let added = first.add_model_provider(input(&name, protocol, &base)).unwrap();
        assert_eq!(added["added_profile"], name);
        first.set_model_selection("leader", Some(name.clone()), None, None).unwrap();
        first.runtime.start();
        assert!(first.runtime.user_message("first", false).unwrap().ok);
        assert!(first.runtime.settle(5));
        first.close();
        drop(first);
        let reopened = open();
        assert_eq!(reopened.model_report()["agents"][0]["model_profile"], name);
        reopened.runtime.start();
        assert!(reopened.runtime.user_message("second", false).unwrap().ok);
        assert!(reopened.runtime.settle(5));
        let state = reopened.core.state().unwrap();
        assert!(state["runs"].as_array().unwrap().iter().all(|run| run["status"] == "COMPLETED"), "{state}");
        reopened.close();
        drop(reopened);
        server.join().unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("test-credential"));
    }
}
