//! Desired restrictions survive restart; observation is independent evidence from a real client.
use super::*;

async fn observed(client: &mut Client, revision: i64, state: &str) -> Json {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = client.call("checkpoint", json!({})).await;
            assert_eq!(reply["ok"], true, "{reply}");
            let snapshot = &reply["result"]["snapshot"];
            let driver = &snapshot["mcp_drivers"]["i-leader"];
            if driver["revision"] == revision && driver["services"]["echo_key"] == state {
                assert_eq!(snapshot["mcp_policy"]["revision"], revision);
                return snapshot.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("driver did not apply the requested policy")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_changes_reconcile_at_boundaries_and_stay_stopped_after_restart() {
    let root = root("session-service");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let starts = root.dir.join("starts");
    let gate = root.dir.join("continue");
    let mut catalog = UserConfig::default();
    catalog.tools.insert("echo_key".into(),serde_json::from_value(json!({"kind":"mcp","mcp_server":"echo","required":true,
        "command":"/bin/sh","args":["-c", "echo $$ >> \"$1\"; exec \"$2\"", "service", &starts,env!("CARGO_BIN_EXE_fake-mcp-server")]})).unwrap());
    let mut finish = finish_call("done");
    finish["__gate_path__"] = json!(gate);
    let seen = Arc::new(Mutex::new(vec![]));
    let handle = serve(config_recording(
        &root,
        HashMap::from([("i-leader".into(), vec![finish])]),
        catalog.clone(),
        vec![],
        json!({}),
        &seen,
    ))
    .await
    .unwrap();
    let mut client = Client::connect(&socket).await;
    let p = |action: &str| json!({"service":"echo_key","action":action});
    observed(&mut client, 0, "connected").await;
    assert_eq!(client.command("bad", "set_mcp_service", json!({"service":"echo","action":"hide"})).await["ok"], false);
    let rc = client.command("hide", "set_mcp_service", p("hide")).await;
    assert_eq!(rc["result"]["desired"]["exposed"], false, "{rc}");
    assert_eq!(client.command("hide", "set_mcp_service", p("hide")).await["result"], rc["result"]);
    observed(&mut client, 1, "connected").await;
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 1);
    assert_eq!(client.command("reconnect", "set_mcp_service", p("reconnect")).await["ok"], true);
    observed(&mut client, 2, "connected").await;
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 2);
    assert_eq!(client.command("stop", "set_mcp_service", p("stop")).await["ok"], true);
    observed(&mut client, 3, "stopped").await;
    assert_eq!(client.command("not-running", "set_mcp_service", p("reconnect")).await["ok"], false);
    let start = client.command("start", "set_mcp_service", p("start")).await;
    assert_eq!(start["result"]["desired"]["exposed"], false, "{start}");
    observed(&mut client, 4, "connected").await;
    let watermark = client.call("checkpoint", json!({})).await["result"]["watermark"].as_i64().unwrap();
    assert_eq!(client.command("input", "submit_input", input_params("reply when ready")).await["ok"], true);
    tokio::time::timeout(Duration::from_secs(6), async {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let denied = client.command("busy", "set_mcp_service", p("expose")).await;
    assert_eq!(denied["ok"], false, "{denied}");
    assert!(denied["error"].as_str().unwrap().contains("pending"));
    std::fs::write(&gate, []).unwrap();
    assert_eq!(wait_work(&mut client, watermark).await.0, "SUCCEEDED");
    assert_eq!(client.command("expose", "set_mcp_service", p("expose")).await["ok"], true);
    observed(&mut client, 5, "connected").await;
    assert_eq!(client.command("stop-final", "set_mcp_service", p("stop")).await["ok"], true);
    observed(&mut client, 6, "stopped").await;
    handle.shutdown().await.unwrap();
    let handle = serve(config_recording(&root, HashMap::new(), catalog, vec![], json!({}), &seen)).await.unwrap();
    let mut client = Client::connect(&socket).await;
    observed(&mut client, 6, "stopped").await;
    assert_eq!(
        std::fs::read_to_string(&starts).unwrap().lines().count(),
        3,
        "restart must not connect a stopped service"
    );
    handle.shutdown().await.unwrap();
}
