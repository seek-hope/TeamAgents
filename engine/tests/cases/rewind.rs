//! The new conversation prefix is live; historical envelopes, work, usage and immutable files are not rewound.
use super::*;

#[tokio::test]
async fn file_preview_checks_committed_edits_against_workspace_bytes() {
    let root = root("rewind-files");
    let workspace = root.dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join("fact.txt");
    std::fs::write(&path, "prefix αα").unwrap();
    let edit = |tool: &str, id: &str, old: &str, new: &str| json!({"role":"assistant","content":"","tool_calls":[{"id":id,"type":"function","function":{"name":tool,"arguments":json!({"path":"fact.txt","old_string":old,"new_string":new}).to_string()}}]});
    let scripts = HashMap::from([(
        "i-leader".into(),
        vec![edit("edit", "first", "αα", "beta"), edit("edit_file", "second", "beta", ""), finish_call("edited")],
    )]);
    let seen = Default::default();
    let handle = serve(config_recording(&root, scripts, UserConfig::default(), vec!["files".into()], json!({}), &seen))
        .await
        .unwrap();
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let input = client
        .command(
            "original",
            "submit_input",
            json!({"instance_id":"i-leader","envelope_id":"point","text":"Edit fact.txt"}),
        )
        .await;
    assert_eq!(input["ok"], true, "{input}");
    assert_eq!(wait_work(&mut client, 0).await.0, "SUCCEEDED");
    assert_eq!(std::fs::read(&path).unwrap(), b"prefix ");
    let params = json!({"envelope":"point","files":true});
    let result = client.call("rewind_points", params.clone()).await;
    let file = &result["result"]["files"][0];
    assert_eq!(file["path"], "fact.txt", "{result}");
    assert_eq!(file["edits"], 2);
    assert_eq!(file["reconstructed_bytes"], "prefix αα".len());
    assert_eq!(file["status"], "matches_recorded_edit");
    assert_eq!(result["result"]["apply_available"], false);
    let apply = client.command("wrong-mode", "rewind_conversation", result["result"].clone()).await;
    assert_eq!(apply["ok"], false);
    assert!(client.call("rewind_points", json!({"draft":true})).await["result"].is_null());
    std::fs::write(&path, "operator edit").unwrap();
    assert_eq!(client.call("rewind_points", params.clone()).await["result"]["files"][0]["status"], "conflict");
    assert_eq!(std::fs::read(&path).unwrap(), b"operator edit");
    let outside = root.dir.join("outside");
    std::fs::write(&outside, "prefix ").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(outside, &path).unwrap();
    assert_eq!(client.call("rewind_points", params).await["result"]["files"][0]["status"], "unavailable");
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn tracked_shell_change_restores_the_delivered_dirty_worktree_with_a_reviewed_preview() {
    let name = "rewind::tracked_shell_change_restores_the_delivered_dirty_worktree_with_a_reviewed_preview";
    if std::env::var("TA_TRACKED_REWIND_CHILD").as_deref() != Ok(name) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TA_TRACKED_REWIND_CHILD", name)
            .env("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let root = root("tracked-shell");
    let ws = root.dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git").current_dir(&ws).args(args).status().unwrap().success())
    };
    git(&["init", "-q"]);
    let path = ws.join("tracked.bin");
    std::fs::write(&path, b"index version").unwrap();
    git(&["add", "tracked.bin"]);
    std::fs::write(&path, b"dirty\0delivery").unwrap();
    std::fs::write(ws.join("local"), b"local").unwrap();
    let seen = Default::default();
    let handle = serve(config_recording(
        &root,
        HashMap::from([(
            "i-leader".into(),
            vec![
                shell_call("write", "printf 'shell result' > tracked.bin; printf 'local edit' > local"),
                finish_call("closed"),
            ],
        )]),
        UserConfig::default(),
        vec!["shell".into()],
        json!({}),
        &seen,
    ))
    .await
    .unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let input = json!({"instance_id":"i-leader","envelope_id":"point","text":"Start from the working copy"});
    assert_eq!(client.command("input", "submit_input", input).await["ok"], true);
    assert_eq!(wait_work(&mut client, 0).await.0, "SUCCEEDED");
    assert_eq!(std::fs::read(&path).unwrap(), b"shell result");
    let params = json!({"envelope":"point","files":true});
    let preview = client.call("rewind_points", params.clone()).await["result"].clone();
    assert_eq!(preview["files"].as_array().unwrap().len(), 1);
    assert_eq!(preview["files"][0]["status"], "snapshot_difference");
    assert_eq!(preview["apply_available"], true);
    std::fs::write(&path, "later user edit").unwrap();
    assert_eq!(client.command("stale", "rewind_conversation", preview).await["ok"], false);
    assert_eq!(std::fs::read(&path).unwrap(), b"later user edit");
    assert!(client.call("rewind_points", json!({"draft":true})).await["result"].is_null());
    let fresh = client.call("rewind_points", params).await["result"].clone();
    handle.shutdown().await.unwrap();
    let handle =
        serve(config_recording(&root, HashMap::new(), UserConfig::default(), vec![], json!({}), &seen)).await.unwrap();
    let mut client = Client::connect(&socket).await;
    let applied = client.command("apply", "rewind_conversation", fresh.clone()).await;
    assert_eq!(applied["result"]["restored_files"], 1, "{applied}");
    assert_eq!(applied["result"]["draft"]["text"], "Start from the working copy");
    assert_eq!(std::fs::read(&path).unwrap(), b"dirty\0delivery");
    assert_eq!(std::fs::read(ws.join("local")).unwrap(), b"local edit");
    std::fs::write(&path, "after rewind").unwrap();
    assert_eq!(client.command("apply", "rewind_conversation", fresh).await["result"], applied["result"]);
    assert_eq!(std::fs::read(&path).unwrap(), b"after rewind");
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn rewind_reopens_the_delivered_prompt_without_a_request_and_new_input_uses_its_snapshot() {
    let root = root("rewind-snapshot");
    let ws = root.dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let path = ws.join("selection.txt");
    std::fs::write(&path, "immutable selection").unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let handle = serve(config_recording(
        &root,
        HashMap::from([("i-leader".into(), vec![finish_call("abandoned response")])]),
        UserConfig::default(),
        vec![],
        json!({}),
        &seen,
    ))
    .await
    .unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    let input = json!({"instance_id":"i-leader","envelope_id":"target","text":"original prompt","attachments":["selection.txt"]});
    let original = client.command("orig", "submit_input", input.clone()).await;
    assert_eq!(original["ok"], true, "{original}");
    assert_eq!(wait_work(&mut client, 0).await.0, "SUCCEEDED");
    std::fs::remove_file(&path).unwrap();
    let preview = client.call("rewind_points", json!({"envelope":"target"})).await["result"].clone();
    let apply = client.command("rewind", "rewind_conversation", preview.clone()).await;
    assert_eq!(apply["ok"], true, "{apply}");
    assert_eq!(apply["result"]["draft"]["attachments"], preview["draft"]["attachments"]);
    assert_eq!(client.command("rewind", "rewind_conversation", preview).await["result"], apply["result"]);
    assert_eq!(
        client.call("history", json!({"instance_id":"i-leader","current_epoch":true})).await["result"]["entries"],
        json!([])
    );
    assert_eq!(client.command("orig", "submit_input", input).await["result"], original["result"]);
    handle.shutdown().await.unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let handle = serve(config_recording(
        &root,
        HashMap::from([("i-leader".into(), vec![finish_call("new response")])]),
        UserConfig::default(),
        vec![],
        json!({}),
        &seen,
    ))
    .await
    .unwrap();
    let mut client = Client::connect(&socket).await;
    let draft = client.call("rewind_points", json!({"draft":true})).await;
    assert_eq!(draft["result"]["draft"], apply["result"]["draft"]);
    let since = client.call("checkpoint", json!({})).await["result"]["watermark"].as_i64().unwrap();
    assert!(seen.lock().unwrap().is_empty());
    let edited = json!({"source":"target","text":"edited question","envelope_id":"edited"});
    let sent = client.command("new", "send_rewind_draft", edited.clone()).await;
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(client.command("new", "send_rewind_draft", edited).await["result"], sent["result"]);
    // The first fixture work is closed. This edited prompt is a fresh turn, not a
    // request to reopen the old billing scope or manufacture another work completion.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = client.call("events", json!({"since":since})).await;
            if page["result"]["events"].as_array().unwrap().iter().any(|e| e["kind"] == "completion_closed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the edited turn settled");
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "reopening a draft never schedules an extra turn");
    let body = json!(requests[0]).to_string();
    assert!(body.contains("immutable selection") && body.contains("edited question"), "{body}");
    assert!(!body.contains("original prompt") && !body.contains("abandoned response"), "{body}");
    let old = client.call("history", json!({"instance_id":"i-leader"})).await;
    assert!(old["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["epoch"] == 0 && r["envelope_id"] == "target"));
    let conn = rusqlite::Connection::open_with_flags(
        root.dir.join("session.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*),count(DISTINCT artifact_id) FROM input_attachments", [], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?
        )))
        .unwrap(),
        (2, 1)
    );
    assert!(!path.exists());
    handle.shutdown().await.unwrap();
}
