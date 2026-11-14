use super::*;

fn seed_events(db: &Path, count: usize, payload: &str) -> (i64, i64) {
    let mut conn = rusqlite::Connection::open(db).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    let start: i64 = tx.query_row("SELECT COALESCE(MAX(sequence),0) FROM events", [], |r| r.get(0)).unwrap();
    for index in 1..=count {
        tx.execute("INSERT INTO events(session_id,sequence,kind,scope,payload_json,created) VALUES('s-test',?1,'page_probe','i-leader',?2,0)",
            rusqlite::params![start + index as i64, payload]).unwrap();
    }
    tx.commit().unwrap();
    (start, start + count as i64)
}

#[tokio::test]
async fn event_pages_bound_rows_bytes_and_refuse_reclaimed_ranges() {
    let (root, handle) = boot("event-pages", HashMap::new()).await;
    let db = root.dir.join("session.sqlite");
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let (start, end) = seed_events(&db, 1100, "{}");
    let mut cursor = start;
    let mut seen = Vec::new();
    while cursor < end {
        let reply = client.call("events", json!({"since":cursor,"until":end,"limit":97})).await;
        assert_eq!(reply["ok"], true, "{reply}");
        let page = &reply["result"];
        let (next, more) = teamagents_core::v2::wire::event_cursor(page, cursor).unwrap();
        assert!(page["events"].as_array().unwrap().len() <= 97);
        seen.extend(page["events"].as_array().unwrap().iter().map(|e| e["sequence"].as_i64().unwrap()));
        assert_eq!(more, next < end);
        cursor = next;
    }
    assert_eq!(seen, ((start + 1)..=end).collect::<Vec<_>>());
    for limit in [json!(-1), json!(0), json!(1001), json!("10")] {
        assert_eq!(client.call("events", json!({"limit":limit})).await["ok"], false);
        assert_eq!(client.call("surfaces", json!({"limit":limit})).await["ok"], false);
    }
    assert_eq!(client.call("events", json!({"since":"bad"})).await["ok"], false);
    // UTF-8 byte size, not character count, determines how many rows fit.
    let payload = json!({"text":"\u{00e9}".repeat(600_000)}).to_string();
    let (large_start, _) = seed_events(&db, 4, &payload);
    let reply = client.call("events", json!({"since":large_start})).await;
    assert_eq!(reply["result"]["events"].as_array().unwrap().len(), 3);
    assert_eq!(reply["result"]["has_more"], true);
    let (oversize_start, _) = seed_events(&db, 1, &json!({"text":"x".repeat(4*1024*1024)}).to_string());
    let reply = client.call("events", json!({"since":oversize_start})).await;
    assert_eq!(reply["ok"], true, "the error must carry an explicit resync instruction");
    assert_eq!(reply["result"]["resync_required"], true);
    assert_eq!(reply["result"]["watermark"], oversize_start);
    assert_eq!(reply["result"]["events"], json!([]));
    let error = teamagents_core::v2::wire::event_cursor(&reply["result"], oversize_start).unwrap_err();
    assert!(error.contains("4 MiB"), "{error}");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("DELETE FROM events WHERE sequence=?1", [start + 2]).unwrap();
    let reply = client.call("events", json!({"since":start})).await;
    assert_eq!(reply["result"]["resync_required"], true);
    assert_eq!(reply["result"]["watermark"], start);
    assert_eq!(reply["result"]["events"], json!([]));
    // An incompatible client must fail explicitly instead of misreading the new cursor contract.
    let old = client.request(json!({"protocol_version":1,"request_id":"old","method":"checkpoint"})).await;
    assert_eq!(old["ok"], false);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn history_pages_preserve_order_and_bound_utf8_payloads() {
    let (root, handle) = boot("history-pages", HashMap::new()).await;
    let conn = rusqlite::Connection::open(root.dir.join("session.sqlite")).unwrap();
    let payload = json!({"role":"user","content":"\u{00e9}".repeat(600_000)}).to_string();
    for (epoch, idx) in [(0, 100), (1, 1), (1, 2), (1, 3)] {
        conn.execute("INSERT INTO context_entries(instance_id,epoch,idx,id,kind,message_json,created) VALUES('i-leader',?1,?2,?3,'user',?4,0)",
            rusqlite::params![epoch,idx,format!("page-{epoch}-{idx}"),payload]).unwrap();
    }
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    let first = client.call("history", json!({"instance_id":"i-leader"})).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["result"]["entries"].as_array().unwrap().len(), 3);
    assert_eq!(first["result"]["next_before"], json!({"epoch":1,"idx":1}));
    let second = client
        .call("history", json!({"instance_id":"i-leader","before":first["result"]["next_before"],"limit":1}))
        .await;
    assert_eq!(second["result"]["entries"][0]["epoch"], 0);
    assert_eq!(second["result"]["entries"][0]["idx"], 100);
    assert_eq!(client.call("history", json!({"instance_id":"i-leader","before":{"epoch":1}})).await["ok"], false);
    conn.execute(
        "UPDATE context_entries SET message_json=?1 WHERE id='page-1-3'",
        [json!({"content":"x".repeat(4*1024*1024)}).to_string()],
    )
    .unwrap();
    let too_big = client.call("history", json!({"instance_id":"i-leader"})).await;
    assert_eq!(too_big["ok"], false);
    assert!(too_big["error"].as_str().unwrap().contains("session export"));
    handle.shutdown().await.unwrap();
}

async fn headless_large_history(oversized: bool) -> Result<teamagents_engine::v2::exec::ExecRun, (i32, String)> {
    let root = root("exec-large-history");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let gate = root.dir.join("release-provider");
    let text = if oversized { "x".repeat(4 * 1024 * 1024) } else { "finished after large tool results".into() };
    let answer = json!({"role":"assistant","content":text,"__gate_path__":gate});
    let handle = serve(config(&root, HashMap::from([("i-leader".into(), vec![answer])]))).await.unwrap();
    let mut options =
        exec_options(&root.dir.join("state/daemon.sock"), &root.dir.join("ws"), "inspect the results", vec![]);
    options.timeout_s = 3;
    let run = tokio::task::spawn_blocking(move || teamagents_engine::v2::exec::execute(&options));
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let cp = client.call("checkpoint", json!({})).await;
            if cp["result"]["snapshot"]["instances"][0]["phase"] == "MODEL_PENDING" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("provider reached gate");
    if !oversized {
        let conn = rusqlite::Connection::open(root.dir.join("session.sqlite")).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        let (epoch, idx): (i64, i64) = conn
            .query_row("SELECT epoch,MAX(idx) FROM context_entries WHERE instance_id='i-leader'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        for n in 1..=4 {
            conn.execute("INSERT INTO context_entries(instance_id,epoch,idx,id,kind,message_json,created) VALUES('i-leader',?1,?2,?3,'tool',?4,0)",
                rusqlite::params![epoch, idx+n, format!("large-result-{n}"), json!({"role":"tool","content":"x".repeat(1_200_000),"tool_call_id":format!("call-{n}")}).to_string()]).unwrap();
        }
    }
    std::fs::write(&gate, b"go").unwrap();
    let result = run.await.unwrap();
    handle.shutdown().await.unwrap();
    result
}

#[tokio::test]
async fn headless_attribution_reads_older_history_pages() {
    let run = headless_large_history(false).await.unwrap();
    assert_eq!(run.end, End::Reply, "{}", run.report);
    assert_eq!(run.report["reply"], "finished after large tool results");
}

#[tokio::test]
async fn headless_history_errors_are_not_reported_as_timeouts() {
    match headless_large_history(true).await {
        Err((code, message)) => {
            assert_eq!(code, 2);
            assert!(message.contains("4 MiB"), "{message}");
        }
        Ok(run) => panic!("expected a history-read error, got {}", run.report),
    }
}

#[tokio::test]
async fn headless_timeout_never_claims_complete_cost_for_a_pending_call() {
    let root = root("exec-pending-cost");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let gate = root.dir.join("release-provider");
    let answer = json!({"role":"assistant","content":"later","__gate_path__":gate});
    let handle = serve(config(&root, HashMap::from([("i-leader".into(), vec![answer])]))).await.unwrap();
    let mut options = exec_options(&root.dir.join("state/daemon.sock"), &root.dir.join("ws"), "do the work", vec![]);
    options.timeout_s = 1;
    let run =
        tokio::task::spawn_blocking(move || teamagents_engine::v2::exec::execute(&options)).await.unwrap().unwrap();
    std::fs::write(&gate, b"go").unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(run.end, End::Timeout);
    assert_eq!(run.report["usage"]["requests"], 0);
    assert_eq!(run.report["usage"]["cost"]["complete"], false, "{}", run.report);
}

#[tokio::test]
async fn headless_usage_excludes_queued_turns_and_unrelated_members() {
    let root = root("exec-owned-usage");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let gate = root.dir.join("release-old-requests");
    let scripts = HashMap::from([
        (
            "i-leader".into(),
            vec![json!({"role":"assistant","content":"old answer","__gate_path__":gate}), reply("this run's answer")],
        ),
        ("i-other".into(), vec![json!({"role":"assistant","content":"unrelated answer","__gate_path__":gate})]),
    ]);
    let mut cfg = config(&root, scripts);
    cfg.supervisor.catalog.models.insert(
        "scripted".into(),
        serde_json::from_value(json!({
        "provider":"fixture","model":"scripted","price":{"input_usd_per_million":2.0,
        "output_usd_per_million":8.0,"source":"fixture tariff"}}))
        .unwrap(),
    );
    let handle = serve(cfg).await.unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let mut client = Client::connect(&socket).await;
    assert_eq!(
        client.command("other", "create_instance", json!({"id":"i-other","profile":{"model":"scripted"}})).await["ok"],
        true
    );
    assert_eq!(
        client.command("other-work", "create_work", json!({"id":"other-work","instance_id":"i-other"})).await["ok"],
        true
    );
    for (id, instance) in [("old-input", "i-leader"), ("other-input", "i-other")] {
        assert_eq!(
            client
                .command(id, "submit_input", json!({"instance_id":instance,"envelope_id":id,"text":"earlier work"}))
                .await["ok"],
            true
        );
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let cp = client.call("checkpoint", json!({})).await;
            if cp["result"]["snapshot"]["instances"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["phase"] == "MODEL_PENDING")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both earlier requests reached their gates");
    let options = exec_options(&socket, &root.dir.join("ws"), "answer this input", vec![]);
    let task = tokio::task::spawn_blocking(move || teamagents_engine::v2::exec::execute(&options));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let events = client.call("events", json!({"since":0})).await;
            if events["result"]["events"].as_array().unwrap().iter().any(|event| event["kind"] == "input_queued") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exec input queued behind the old leader request");
    std::fs::write(&gate, b"go").unwrap();
    let run = task.await.unwrap().unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(run.end, End::Reply, "{}", run.report);
    assert_eq!(run.report["reply"], "this run's answer");
    assert_eq!(run.report["usage"]["requests"], 1, "{}", run.report);
    assert_eq!(run.report["usage"]["total_tokens"], 5, "{}", run.report);
    assert_eq!(run.report["usage"]["cost"]["complete"], true, "{}", run.report);
    assert!((run.report["usage"]["cost"]["known_estimated_usd"].as_f64().unwrap() - 0.000022).abs() < 1e-12);
}

#[tokio::test]
async fn headless_usage_retains_related_member_requests() {
    let root = root("exec-related-usage");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let leader = vec![
        json!({"role":"assistant","content":"","tool_calls":[
            {"id":"spawn","type":"function","function":{"name":"spawn","arguments":json!({"instance_id":"i-worker","instructions":"do the task"}).to_string()}},
            {"id":"delegate","type":"function","function":{"name":"delegate","arguments":json!({"assignee":"i-worker","task_id":"related-task","description":"finish the task"}).to_string()}}
        ]}),
        json!({"role":"assistant","content":"","tool_calls":[
            {"id":"wait","type":"function","function":{"name":"wait","arguments":json!({"mode":"ANY","conditions":[{"kind":"task","task_id":"related-task"}]}).to_string()}}
        ]}),
        finish_call("the member finished"),
    ];
    let mut cfg = config(
        &root,
        HashMap::from([("i-leader".into(), leader), ("i-worker".into(), vec![finish_call("member result")])]),
    );
    cfg.supervisor.catalog.models.insert(
        "scripted".into(),
        serde_json::from_value(json!({
        "provider":"fixture","model":"scripted","price":{"input_usd_per_million":2.0,
        "output_usd_per_million":8.0,"source":"fixture tariff"}}))
        .unwrap(),
    );
    let handle = serve(cfg).await.unwrap();
    let options =
        exec_options(&root.dir.join("state/daemon.sock"), &root.dir.join("ws"), "delegate and finish", vec![]);
    let run =
        tokio::task::spawn_blocking(move || teamagents_engine::v2::exec::execute(&options)).await.unwrap().unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(run.end, End::Completed, "{}", run.report);
    assert_eq!(run.report["usage"]["requests"], 4, "{}", run.report);
    assert_eq!(run.report["usage"]["total_tokens"], 20, "{}", run.report);
    assert_eq!(run.report["usage"]["cost"]["complete"], true, "{}", run.report);
    assert!((run.report["usage"]["cost"]["known_estimated_usd"].as_f64().unwrap() - 0.000088).abs() < 1e-12);
}

#[tokio::test]
async fn headless_owns_its_reply_while_later_inputs_keep_the_leader_busy() {
    let root = root("exec-busy-followups");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let gates: Vec<_> = (0..3).map(|n| root.dir.join(format!("release-{n}"))).collect();
    let script = gates
        .iter()
        .enumerate()
        .map(|(n, gate)| json!({"role":"assistant","content":format!("answer-{n}"),"__gate_path__":gate}))
        .collect();
    let handle = serve(config(&root, HashMap::from([("i-leader".into(), script)]))).await.unwrap();
    let socket = root.dir.join("state/daemon.sock");
    let mut options = exec_options(&socket, &root.dir.join("ws"), "first input", vec![]);
    options.timeout_s = 5;
    let db = root.dir.join("session.sqlite");
    let held_gate = gates[2].clone();
    let task = tokio::task::spawn_blocking(move || {
        let mut advanced = false;
        teamagents_engine::v2::exec::execute_into(
            &options,
            Some(&mut |event: &Json| {
                if !advanced && event["kind"] == "request_began" && event["scope"] == "i-leader" {
                    advanced = true;
                    let mut previous = event["payload"]["request_id"].as_str().unwrap().to_string();
                    let mut rpc = teamagents_engine::v2::exec::Client::connect(&socket).unwrap();
                    let conn = rusqlite::Connection::open(&db).unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();
                    for (index, gate) in gates.iter().take(2).enumerate() {
                        let envelope = format!("followup-{index}");
                        let result = rpc
                            .command(
                                &envelope,
                                "submit_input",
                                json!({"instance_id":"i-leader","envelope_id":envelope,"text":"later input"}),
                            )
                            .unwrap();
                        assert_eq!(result["queued"], true);
                        std::fs::write(gate, b"go").unwrap();
                        let deadline = std::time::Instant::now() + Duration::from_secs(5);
                        loop {
                            let (id, phase): (Option<String>, String) = conn
                                .query_row(
                                    "SELECT active_request_id,phase FROM instances WHERE id='i-leader'",
                                    [],
                                    |row| Ok((row.get(0)?, row.get(1)?)),
                                )
                                .unwrap();
                            if phase == "MODEL_PENDING" && id.as_deref().is_some_and(|id| id != previous) {
                                previous = id.unwrap();
                                break;
                            }
                            assert!(std::time::Instant::now() < deadline, "later input did not start");
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                Ok(())
            }),
        )
    });
    let result = task.await;
    std::fs::write(&held_gate, b"go").unwrap();
    handle.shutdown().await.unwrap();
    let run = result.unwrap().unwrap();
    assert_eq!(run.end, End::Reply, "{}", run.report);
    assert_eq!(run.report["reply"], "answer-0");
    assert_eq!(run.report["usage"]["requests"], 1, "{}", run.report);
    assert_eq!(run.report["usage"]["total_tokens"], 5, "{}", run.report);
}

#[tokio::test]
async fn headless_stream_drains_many_pages_and_skips_preexisting_history() {
    let root = root("event-stream-pages");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let gate = root.dir.join("release-provider");
    let mut answer = finish_call("paged completion");
    answer["__gate_path__"] = json!(gate);
    let mut cfg = config(&root, HashMap::from([("i-leader".into(), vec![answer])]));
    cfg.supervisor.catalog.models.insert(
        "scripted".into(),
        serde_json::from_value(json!({
        "provider":"fixture","model":"scripted","price":{"input_usd_per_million":2.0,
        "output_usd_per_million":8.0,"source":"fixture tariff"}}))
        .unwrap(),
    );
    let handle = serve(cfg).await.unwrap();
    let db = root.dir.join("session.sqlite");
    let (_, old_head) = seed_events(&db, 800, "{}");
    let options = exec_options(&root.dir.join("state/daemon.sock"), &root.dir.join("ws"), "finish the task", vec![]);
    let run = tokio::task::spawn_blocking(move || {
        let mut events = Vec::new();
        let report = teamagents_engine::v2::exec::execute_into(
            &options,
            Some(&mut |event: &Json| {
                events.push(event.clone());
                Ok(())
            }),
        );
        (report, events)
    });
    let mut client = Client::connect(&root.dir.join("state/daemon.sock")).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let cp = client.call("checkpoint", json!({})).await;
            if cp["result"]["snapshot"]["instances"][0]["phase"] == "MODEL_PENDING" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("provider reached its gate");
    seed_events(&db, 800, "{}");
    std::fs::write(&gate, b"go").unwrap();
    let (report, events) = run.await.unwrap();
    let report = report.unwrap();
    assert_eq!(report.end, End::Completed, "{}", report.report);
    assert_eq!(events.iter().filter(|e| e["kind"] == "page_probe").count(), 800);
    assert!(events.iter().all(|e| e["sequence"].as_i64().unwrap() > old_head));
    assert!(events
        .windows(2)
        .all(|pair| pair[1]["sequence"].as_i64().unwrap() == pair[0]["sequence"].as_i64().unwrap() + 1));
    assert!(events.iter().any(|e| e["kind"] == "work_completed"));
    assert_eq!(report.report["usage"]["requests"], 1);
    assert_eq!(report.report["usage"]["cost"]["complete"], true);
    assert!((report.report["usage"]["cost"]["known_estimated_usd"].as_f64().unwrap() - 0.000022).abs() < 1e-12);
    let scopes = client.call("work_scopes", json!({})).await;
    assert_eq!(scopes["result"]["work_scopes"][0]["cost"]["complete"], true);
    assert_eq!(scopes["result"]["work_scopes"][0]["cost"]["attempts"], 1);
    assert_eq!(report.report["watermark"], events.last().unwrap()["sequence"]);
    handle.shutdown().await.unwrap();
}
