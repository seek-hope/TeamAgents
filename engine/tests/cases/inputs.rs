//! D-451: real daemon/CLI pending-input receipts, restart, and request attribution.
use super::*;

#[tokio::test]
async fn pending_inputs_edit_cancel_and_restart_through_daemon_and_cli() {
    let root = root("pending-inputs");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut slow = reply("first reply");
    slow["__slow_ms__"] = json!(1500);
    let scripts = HashMap::from([("i-leader".into(), vec![slow])]);
    let handle =
        serve(config_recording(&root, scripts, UserConfig::default(), vec![], json!({}), &seen)).await.unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    assert_eq!(client.command("first", "submit_input", input_params("first")).await["ok"], true);
    tokio::time::timeout(Duration::from_secs(3), async {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        client.command("pause", "set_lifecycle", json!({"instance_id":"i-leader", "lifecycle":"PAUSED"})).await["ok"],
        true
    );
    let attachment = "queued file \u{03bb}.txt";
    std::fs::write(root.dir.join("ws").join(attachment), "snapshot before restart").unwrap();
    for id in ["a", "b", "c"] {
        let reply = client
            .command(
                id,
                "submit_input",
                json!({"instance_id":"i-leader", "envelope_id":id, "text":format!("original-{id}"), "attachments":[attachment]}),
            )
            .await;
        assert_eq!(reply["result"]["queued"], true, "{reply}");
    }
    std::fs::remove_file(root.dir.join("ws").join(attachment)).unwrap();
    assert_eq!(
        client
            .command(
                "a",
                "submit_input",
                json!({"instance_id":"i-leader", "envelope_id":"a", "text":"original-a", "attachments":[attachment]})
            )
            .await["result"]["queued"],
        true
    );
    let page = client.call("inputs", json!({"limit":2})).await;
    assert_eq!(page["result"]["inputs"].as_array().unwrap().len(), 2);
    let cursor = page["result"]["next_after"].as_i64().unwrap();
    let last = client.call("inputs", json!({"after":cursor,"limit":2})).await;
    assert_eq!(last["result"]["inputs"][0]["envelope_id"], "c");
    for bad in [json!({"limit":0}), json!({"limit":101}), json!({"after":-1})] {
        assert_eq!(client.call("inputs", bad).await["ok"], false);
    }
    let edit = json!({"instance_id":"i-leader", "envelope_id":"a", "revision":0,"text":"edited-a"});
    let edited = client.command("edit-a", "edit_input", edit.clone()).await;
    assert_eq!(edited["ok"], true, "{edited}");
    drop(client);
    let mut client = Client::connect(&socket).await;
    assert_eq!(client.command("edit-a", "edit_input", edit.clone()).await["result"], edited["result"]);
    assert_eq!(client.command("stale-a", "edit_input", edit).await["ok"], false);
    assert_eq!(
        client
            .command("wrong-member", "cancel_input", json!({"instance_id":"wrong", "envelope_id":"a", "revision":1}))
            .await["ok"],
        false
    );
    let state = root.dir.join("state");
    let config_home = root.dir.join("config");
    let cli = tokio::task::spawn_blocking(move || {
        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_teamagents"))
                .arg("--state-root")
                .arg(&state)
                .args(args)
                .env("XDG_CONFIG_HOME", &config_home)
                .env("XDG_STATE_HOME", &state)
                .output()
                .unwrap()
        };
        let listed = run(&["inputs", "list", "--json"]);
        assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
        let data: Json = serde_json::from_slice(&listed.stdout).unwrap();
        assert_eq!(data["inputs"][0]["revision"], 1);
        let edit = run(&["inputs", "edit", "--id", "a", "--revision", "1", "--prompt", "cli-edited-a", "--json"]);
        assert!(edit.status.success(), "{}", String::from_utf8_lossy(&edit.stderr));
        let cancelled = run(&["inputs", "cancel", "--id", "b", "--revision", "0", "--json"]);
        assert!(cancelled.status.success(), "{}", String::from_utf8_lossy(&cancelled.stderr));
        let repeat = run(&["inputs", "cancel", "--id", "b", "--revision", "0"]);
        assert_eq!(repeat.status.code(), Some(1));
    });
    cli.await.unwrap();
    handle.shutdown().await.unwrap();
    drop(client);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handle = serve(config_recording(
        &root,
        HashMap::from([("i-leader".into(), vec![reply("queued reply")])]),
        UserConfig::default(),
        vec![],
        json!({}),
        &seen,
    ))
    .await
    .unwrap();
    let mut client = Client::connect(&socket).await;
    let pending = client.call("inputs", json!({})).await;
    let rows = pending["result"]["inputs"].as_array().unwrap();
    assert_eq!(rows.iter().map(|r| r["envelope_id"].as_str().unwrap()).collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(rows[0]["revision"], 2);
    assert_eq!(rows[0]["payload"]["text"], "cli-edited-a");
    assert_eq!(rows[0]["payload"]["attachments"][0]["label"], attachment);
    client.command("resume", "set_lifecycle", json!({"instance_id":"i-leader", "lifecycle":"ACTIVE"})).await;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let page = client.call("inputs", json!({})).await;
            if page["result"]["inputs"].as_array().is_some_and(Vec::is_empty) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let history = client.call("history", json!({"instance_id":"i-leader"})).await;
    let entries = history["result"]["entries"].as_array().unwrap();
    assert_eq!(entries.iter().filter(|e| e["envelope_id"] == "a").count(), 1);
    let entry = entries.iter().find(|e| e["envelope_id"] == "a").unwrap();
    assert_eq!(entry["message"]["attachments"][0]["label"], attachment);
    assert!(!serde_json::to_string(&history).unwrap().contains("snapshot before restart"));
    assert!(!entries.iter().any(|e| e["envelope_id"] == "b"));
    assert!(entries.iter().find(|e| e["envelope_id"] == "a").unwrap()["message"]["content"]
        .as_str()
        .unwrap()
        .contains("cli-edited-a"));
    assert_eq!(
        client
            .command(
                "late",
                "edit_input",
                json!({"instance_id":"i-leader","envelope_id":"a","revision":2,"text":"too late"})
            )
            .await["ok"],
        false
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let requests = seen.lock().unwrap().clone();
    assert!(requests[0].to_vec().iter().any(|m| m["content"].as_str().is_some_and(|t| t.contains("cli-edited-a"))));
    assert!(!serde_json::to_string(&requests).unwrap().contains("original-b"));
    assert!(serde_json::to_string(&requests).unwrap().contains("snapshot before restart"));
    assert!(!serde_json::to_string(&requests).unwrap().contains("\"attachments\""));
    handle.shutdown().await.unwrap();
}
