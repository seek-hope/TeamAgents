use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

struct Session(PathBuf);
impl Session {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("swarm-store-{}", id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> Store {
        Store::open(&self.0.join("swarm.db")).unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Store {
    fn submit_root(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        tree: bool,
    ) -> Result<String> {
        self.create_root(agent, prompt, check, review, false, tree.then_some(&ModelSettings::default()))
    }
}

fn legacy(s: &mut Store, agent: &str, prompt: &str) -> Result<String> {
    s.submit_root(agent, prompt, None, false, false)
}
fn legacy_checked(s: &mut Store, agent: &str, prompt: &str, check: Option<&str>) -> Result<String> {
    s.submit_root(agent, prompt, check, false, false)
}
fn turn(s: &mut Store) -> Turn {
    s.claim().unwrap().unwrap()
}
fn claim(s: &mut Store, task: &str) -> Turn {
    let turn = turn(s);
    assert_eq!(turn.task, task);
    turn
}
fn spawn(s: &mut Store, task: &str, agent: &str) {
    claim(s, task);
    s.apply(task, &json!({"op":"spawn","agent":agent})).unwrap();
}
fn finish(s: &mut Store, task: &str, text: &str) {
    s.apply(task, &json!({"op":"finish","text":text})).unwrap();
}
fn chat(turn: &Turn) -> String {
    turn.messages
        .iter()
        .map(|m| {
            assert!(matches!(m["role"].as_str(), Some("assistant" | "user")));
            m["content"].as_str().unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn private_histories_persist_and_only_explicit_messages_cross_agents() {
    let home = Session::new();
    let mut s = home.open();
    let root = legacy(&mut s, "default", "private alpha prompt").unwrap();
    spawn(&mut s, &root, "peer");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"send","agent":"peer","text":"public question"})).unwrap();
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate","agent":"peer","text":"delegated scope"})).unwrap();
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    let child = turn(&mut s);
    assert_eq!(child.agent, "peer");
    let context = chat(&child);
    assert!(context.contains("public question") && context.contains("delegated scope"));
    assert!(!context.contains("private alpha prompt"));
    let action = json!({"op":"edit_file","path":"file","old":"","new":"private edit body"});
    s.request_tool(&child.task, &action).unwrap();
    assert_eq!(s.claim_tool().unwrap().unwrap(), (child.task.clone(), action));
    s.tool_result(&child.task, Ok("private beta receipt")).unwrap();
    let root_status = s.task(&root).unwrap();
    assert_eq!(root_status["policy"], "auto");
    assert_eq!(root_status["evidence"], json!([]));
    assert_eq!(
        root_status["subtree_receipts"],
        json!([{"task":child.task,"agent":"peer","op":"edit_file","label":"file","ok":true}])
    );
    assert!(!root_status.to_string().contains("private edit body"));
    assert!(!root_status.to_string().contains("private beta receipt"));
    let child_status = s.task(&child.task).unwrap();
    assert_eq!(child_status["evidence"][0]["result"], "private beta receipt");
    assert!(child_status.get("subtree_receipts").is_none());
    claim(&mut s, &child.task);
    finish(&mut s, &child.task, "published child result");
    let parent = claim(&mut s, &root);
    let context = chat(&parent);
    assert!(context.contains("published child result"));
    assert!(!context.contains("private beta receipt"));
    assert!(!context.contains("subtree_receipts") && !context.contains("private edit body"));
    finish(&mut s, &root, "private alpha result");
    assert_eq!(s.task(&root).unwrap()["verification"], "not_requested");
    let later = legacy(&mut s, "peer", "follow up").unwrap();
    let context = chat(&claim(&mut s, &later));
    assert!(context.contains("private beta receipt") && context.contains("published child result"));
    assert!(!context.contains("private alpha"));
}

#[test]
fn failure_of_a_delegate_propagates_state_not_private_diagnostics() {
    let home = Session::new();
    let mut s = home.open();
    let root = legacy(&mut s, "default", "root").unwrap();
    spawn(&mut s, &root, "peer");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate", "agent":"peer", "text":"part"})).unwrap();
    let child = turn(&mut s).task;
    s.fail(&child, "peer private diagnostic").unwrap();
    assert_eq!(s.task(&child).unwrap()["result"], "peer private diagnostic");
    assert_eq!(s.task(&root).unwrap()["status"], "failed");
    let next = legacy(&mut s, "default", "retry another way").unwrap();
    let history = chat(&claim(&mut s, &next));
    assert!(history.contains(&child) && !history.contains("peer private diagnostic"));
}

#[test]
fn legacy_work_has_no_turn_budget_or_member_cap_across_restarts() {
    let home = Session::new();
    let mut s = home.open();
    let root = legacy(&mut s, "default", "root").unwrap();
    spawn(&mut s, &root, "b");
    spawn(&mut s, &root, "c");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate","agent":"b","text":"part"})).unwrap();
    let child = turn(&mut s);
    assert!(s.apply(&child.task, &json!({"op":"delegate","agent":"default","text":"cycle"})).is_err());
    s.apply(&child.task, &json!({"op":"delegate","agent":"c","text":"part two"})).unwrap();
    let leaf = turn(&mut s);
    assert_eq!(s.task(&leaf.task).unwrap()["turns_used"], 5);
    assert!(s.task(&leaf.task).unwrap()["turn_budget"].is_null());
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    let reclaimed = claim(&mut s, &leaf.task);
    assert_eq!(s.task(&leaf.task).unwrap()["turns_used"], 6);
    assert!(s.task(&leaf.task).unwrap()["turn_budget"].is_null());
    finish(&mut s, &reclaimed.task, "completed after restart");
    assert_eq!(s.task(&leaf.task).unwrap()["status"], "done");
    let resumed_child = claim(&mut s, &child.task);
    finish(&mut s, &resumed_child.task, "child complete");
    let resumed_root = claim(&mut s, &root);
    finish(&mut s, &resumed_root.task, "root complete");
    assert_eq!(s.task(&root).unwrap()["status"], "done");
    assert_eq!(s.task(&root).unwrap()["turns_used"], 8);
    let next = legacy(&mut s, "c", "independent").unwrap();
    claim(&mut s, &next);
    assert_eq!(s.task(&next).unwrap()["turns_used"], 1);
    assert!(s.task(&next).unwrap()["turn_budget"].is_null());
    s.apply(&next, &json!({"op":"spawn","agent":"extra"})).unwrap();
    assert_eq!(s.agents().unwrap().as_array().unwrap().len(), 4);
}

#[test]
fn two_connections_spawn_new_members_concurrently_without_cap() {
    let home = Session::new();
    let mut s = home.open();
    let a = legacy(&mut s, "default", "A").unwrap();
    spawn(&mut s, &a, "b");
    let b = legacy(&mut s, "b", "B").unwrap();
    claim(&mut s, &a);
    claim(&mut s, &b);
    let s2 = home.open();
    let gate = Arc::new(Barrier::new(2));
    let run = |mut s: Store, task: String, name: &'static str, gate: Arc<Barrier>| {
        thread::spawn(move || {
            gate.wait();
            s.apply(&task, &json!({"op":"spawn","agent":name}))
        })
    };
    let x = run(s, a, "third", gate.clone());
    let y = run(s2, b, "another third", gate);
    assert!(x.join().unwrap().is_ok());
    assert!(y.join().unwrap().is_ok());
    assert_eq!(home.open().agents().unwrap().as_array().unwrap().len(), 4);
}

#[test]
fn same_agent_calls_run_concurrently_while_cancelled_calls_stay_fenced() {
    let home = Session::new();
    let mut s = home.open();
    let first = legacy(&mut s, "default", "first").unwrap();
    spawn(&mut s, &first, "other");
    let second = legacy(&mut s, "default", "second").unwrap();
    let parallel = legacy(&mut s, "other", "parallel").unwrap();
    claim(&mut s, &first);
    let mut s2 = home.open();
    claim(&mut s2, &second);
    claim(&mut s2, &parallel);
    s.cancel(&first).unwrap();
    assert!(s.apply(&first, &json!({"op":"send","agent":"other","text":"stale"})).is_err());
    s.release_cancelled(&first).unwrap();
    s.request_tool(&second, &json!({"op":"read_file","path":"f"})).unwrap();
    s.claim_tool().unwrap().unwrap();
    let third = legacy(&mut s, "default", "third").unwrap();
    s.cancel(&second).unwrap();
    assert_eq!(s.task(&second).unwrap()["status"], "unknown");
    claim(&mut s2, &third);
    s.release_cancelled(&second).unwrap();
    assert!(s.tool_result(&second, Ok("stale")).is_err());
    assert_eq!(s.task(&second).unwrap()["tool_state"], "unknown");
}

#[test]
fn approvals_are_one_shot_exact_and_survive_all_pre_start_boundaries() {
    let home = Session::new();
    let mut s = home.open();
    let t = s.submit_root("default", "edit", None, true, false).unwrap();
    claim(&mut s, &t);
    let shell = json!({"op":"shell","command":"first command"});
    s.request_tool(&t, &shell).unwrap();
    assert!(s.claim_tool().unwrap().is_none());
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    let task = s.task(&t).unwrap();
    let first = task["approval_id"].as_str().unwrap();
    assert_eq!(task["action"], shell);
    assert!(s.approve(&t, true).is_err());
    s.approve(first, false).unwrap();
    assert!(s.claim_tool().unwrap().is_none());
    assert!(chat(&claim(&mut s, &t)).contains("denied"));
    let edit = json!({"op":"edit_file","path":"new.txt","old":"","new":"some bytes\n"});
    s.request_tool(&t, &edit).unwrap();
    let listed = s.tasks().unwrap();
    let second = listed[0]["approval_id"].as_str().unwrap();
    assert_ne!(first, second);
    assert!(s.approve(first, true).is_err());
    s.approve(second, true).unwrap();
    assert!(s.approve(second, true).is_err());
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    assert_eq!(s.claim_tool().unwrap().unwrap(), (t.clone(), edit.clone()));
    assert!(s.claim_tool().unwrap().is_none());
    s.tool_result(&t, Err("conflicting edit")).unwrap();
    assert!(s.tool_result(&t, Ok("not applied")).is_err());
    let task = s.task(&t).unwrap();
    assert_eq!(task["evidence"], json!([{"action":edit,"ok":false,"result":"conflicting edit"}]));
    assert!(chat(&claim(&mut s, &t)).contains("conflicting edit"));
}

#[test]
fn uncertain_tool_recovers_once_and_invalidates_all_ancestors() {
    let home = Session::new();
    let mut s = home.open();
    let root = legacy(&mut s, "default", "root").unwrap();
    spawn(&mut s, &root, "peer");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate","agent":"peer","text":"child"})).unwrap();
    let child = turn(&mut s).task;
    let cmd = json!({"op":"shell","command":"write something"});
    s.request_tool(&child, &cmd).unwrap();
    let task = s.task(&child).unwrap();
    assert_eq!(task["policy"], "auto");
    assert_eq!(task["tool_state"], "ready");
    assert!(task["approval_id"].is_null());
    assert_eq!(s.claim_tool().unwrap().unwrap(), (child.clone(), cmd.clone()));
    drop(s);
    let mut s = home.open();
    for _ in 0..2 {
        s.recover().unwrap();
        assert!(s.claim_tool().unwrap().is_none());
        assert!(s.claim().unwrap().is_none());
        let leaf = s.task(&child).unwrap();
        assert_eq!(leaf["status"], "unknown");
        assert_eq!(leaf["action"], cmd);
        assert_eq!(leaf["evidence"], json!([]));
        assert_eq!(s.task(&root).unwrap()["status"], "unknown");
    }
}

#[test]
fn checked_finish_is_gated_only_by_the_submitted_command_and_persists_its_receipt() {
    let home = Session::new();
    let mut s = home.open();
    let cmd = json!({"op":"verify","command":"my verifier"});
    for (check, expected_status, accepted) in
        [(Ok("pass output"), "done", true), (Err("check failed"), "queued", false)]
    {
        let task = legacy_checked(&mut s, "default", "assignment", Some("my verifier")).unwrap();
        claim(&mut s, &task);
        assert!(s.request_tool(&task, &cmd).is_err());
        assert!(s.apply(&task, &cmd).is_err());
        finish(&mut s, &task, "proposed result");
        let pending = s.task(&task).unwrap();
        assert_eq!(pending["status"], "waiting");
        assert_eq!(pending["proposal"], "proposed result");
        assert!(pending["result"].is_null());
        assert!(s.claim().unwrap().is_none());
        assert_eq!(s.claim_tool().unwrap().unwrap(), (task.clone(), cmd.clone()));
        s.tool_result(&task, check).unwrap();
        drop(s);
        s = home.open();
        s.recover().unwrap();
        assert!(s.claim_tool().unwrap().is_none());
        let t = s.task(&task).unwrap();
        assert_eq!(t["status"], expected_status);
        assert_eq!(t["evidence"], json!([{"action":cmd,"ok":accepted,"result":check.unwrap_or_else(|e|e)}]));
        assert_eq!(t["verification"], if accepted { "passed" } else { "pending" });
        if accepted {
            assert_eq!(t["result"], "proposed result");
        } else {
            assert!(t["result"].is_null());
            let repaired = claim(&mut s, &task).task;
            finish(&mut s, &repaired, "aborted");
        }
    }
}

#[test]
fn failed_checks_queue_durable_repairs_until_success_or_the_tenth_attempt() {
    for pass_last in [true, false] {
        let home = Session::new();
        let mut s = home.open();
        let task = legacy_checked(&mut s, "default", "fix", Some("run checks")).unwrap();
        let verify = json!({"op":"verify","command":"run checks"});
        for attempt in 1..=10 {
            let turn = claim(&mut s, &task);
            if attempt > 1 {
                let previous = attempt - 1;
                assert!(turn.messages.iter().any(|m| {
                    let Ok(body) = serde_json::from_str::<Value>(m["content"].as_str().unwrap()) else { return false };
                    body["tool"] == "verify"
                        && body["ok"] == false
                        && body["attempt"] == previous
                        && body["result"] == format!("diagnostics {previous}")
                        && body["next"].is_string()
                }));
            }
            finish(&mut s, &task, &format!("proposal {attempt}"));
            assert_eq!(s.claim_tool().unwrap(), Some((task.clone(), verify.clone())));
            let success = attempt == 10 && pass_last;
            let diagnostics = format!("diagnostics {attempt}");
            s.tool_result(&task, if success { Ok("tests pass") } else { Err(&diagnostics) }).unwrap();
            drop(s);
            s = home.open();
            s.recover().unwrap();
            assert!(s.claim_tool().unwrap().is_none());
            let status = s.task(&task).unwrap();
            assert_eq!(status["turns_used"], attempt);
            assert_eq!(status["evidence"].as_array().unwrap().len(), attempt as usize);
            assert_eq!(status["evidence"][attempt as usize - 1]["ok"], success);
            if attempt < 10 {
                assert_eq!(status["status"], "queued");
                assert_eq!(status["verification"], "pending");
                assert!(status["proposal"].is_null() && status["result"].is_null());
            } else if success {
                assert_eq!(status["status"], "done");
                assert_eq!(status["verification"], "passed");
                assert_eq!(status["result"], "proposal 10");
            } else {
                assert_eq!(status["status"], "failed");
                assert_eq!(status["verification"], "failed");
                assert!(status["result"].as_str().unwrap().contains("check attempt limit"));
                assert!(status["proposal"].is_null());
            }
        }
        assert!(s.claim().unwrap().is_none());
    }
}

#[test]
fn lost_check_never_accepts_or_requeues_the_proposal() {
    let home = Session::new();
    let mut s = home.open();
    let task = legacy_checked(&mut s, "default", "fix", Some("my check")).unwrap();
    claim(&mut s, &task);
    finish(&mut s, &task, "claim of success");
    assert_eq!(s.claim_tool().unwrap().unwrap().1, json!({"op":"verify","command":"my check"}));
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    assert!(s.tool_result(&task, Ok("late pass")).is_err());
    assert!(s.claim().unwrap().is_none() && s.claim_tool().unwrap().is_none());
    let status = s.task(&task).unwrap();
    assert_eq!(status["status"], "unknown");
    assert_eq!(status["tool_state"], "unknown");
    assert_eq!(status["verification"], "failed");
    assert!(status["verification_note"].as_str().unwrap().contains("outcome is unknown"));
    assert_eq!(status["subtree_receipts"], json!([]));
    assert!(status["result"].as_str().unwrap().contains("outcome unknown"));

    let skipped = legacy_checked(&mut s, "default", "next", Some("my check")).unwrap();
    claim(&mut s, &skipped);
    finish(&mut s, &skipped, "not yet verified");
    s.cancel(&skipped).unwrap();
    let status = s.task(&skipped).unwrap();
    assert_eq!(status["status"], "cancelled");
    assert_eq!(status["verification"], "pending");
    assert!(status["verification_note"].as_str().unwrap().contains("before the submitted check ran"));
    assert!(s.claim_tool().unwrap().is_none());
}

#[test]
fn cancelled_tree_revokes_unstarted_effects_and_cannot_publish_a_late_result() {
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit_root("default", "root", None, true, false).unwrap();
    spawn(&mut s, &root, "peer");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate","agent":"peer","text":"part"})).unwrap();
    let child = turn(&mut s).task;
    s.request_tool(&child, &json!({"op":"shell","command":"test"})).unwrap();
    let t = s.task(&child).unwrap();
    s.cancel(&root).unwrap();
    assert!(s.approve(t["approval_id"].as_str().unwrap(), true).is_err());
    assert_eq!(s.task(&child).unwrap()["status"], "cancelled");
    assert_eq!(s.task(&root).unwrap()["status"], "cancelled");
    assert!(s.claim_tool().unwrap().is_none());
    assert!(s.claim().unwrap().is_none());
}

fn legacy_database(home: &Session) -> Connection {
    let db = Connection::open(home.0.join("swarm.db")).unwrap();
    db.execute_batch(SCHEMA_V1).unwrap();
    db.pragma_update(None, "application_id", APP_ID).unwrap();
    db.pragma_update(None, "user_version", 1).unwrap();
    db.execute_batch("INSERT INTO settings VALUES (1,2,10,NULL); INSERT INTO agents VALUES ('default'),('peer');")
        .unwrap();
    db
}

#[test]
fn migration_preserves_legacy_root_policy_pending_and_ready_operations() {
    let home = Session::new();
    let db = legacy_database(&home);
    db.execute_batch(
        "INSERT INTO tasks(id,root,parent,agent,prompt,status) VALUES
        ('a','a',NULL,'default','review me','approval'),
        ('b','b',NULL,'default','delegated','waiting'),
        ('c','b','b','peer','execute approved edit','waiting');",
    )
    .unwrap();
    let shell = json!({"op":"shell","command":"list files"});
    let edit = json!({"op":"edit_file","path":"file","old":"","new":"update"});
    for (op, task, action, state) in [("one", "a", &shell, "approval"), ("two", "c", &edit, "ready")] {
        db.execute(
            "INSERT INTO operations(id,task,action,state) VALUES (?1,?2,?3,?4)",
            params![op, task, action.to_string(), state],
        )
        .unwrap();
    }
    drop(db);

    let mut s = home.open();
    s.recover().unwrap();
    assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 8);
    for t in s.tasks().unwrap().as_array().unwrap() {
        assert_eq!(t["policy"], "review");
    }
    let pending = s.task("a").unwrap();
    assert_eq!(pending["action"], shell);
    assert_eq!(pending["approval_id"], "one");
    assert!(s.approve("two", true).is_err());
    assert_eq!(s.claim_tool().unwrap(), Some(("c".into(), edit.clone())));
    assert!(s.claim_tool().unwrap().is_none());
    s.tool_result("c", Ok("edited")).unwrap();
    claim(&mut s, "c");
    s.request_tool("c", &shell).unwrap();
    assert_eq!(s.task("c").unwrap()["tool_state"], "approval");
    assert!(s.claim_tool().unwrap().is_none());
    s.approve("one", true).unwrap();
    assert_eq!(s.claim_tool().unwrap(), Some(("a".into(), shell)));
    let new = s.submit("default", "new work").unwrap();
    drop(s);
    let mut s = home.open();
    assert_eq!(s.task(&new).unwrap()["policy"], "auto");
    assert_eq!(s.task("c").unwrap()["policy"], "review");
}

#[test]
fn migration_rolls_back_and_unknown_versions_do_not_open() {
    let home = Session::new();
    let db = legacy_database(&home);
    db.execute_batch("DROP TABLE messages").unwrap();
    let path = home.0.join("swarm.db");
    assert!(Store::open(&path).is_err());
    assert_eq!(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        db.query_row("SELECT count(*) FROM sqlite_schema WHERE name='root_policies'", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );

    let other = Session::new();
    let path = other.0.join("swarm.db");
    let db = legacy_database(&other);
    for (app, version) in [(APP_ID, 9), (APP_ID, 0), (0, 1), (0, 2), (42, 2)] {
        db.pragma_update(None, "application_id", app).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        assert!(Store::open(&path).is_err());
        assert_eq!(db.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0)).unwrap(), "delete");
    }
}

#[test]
fn reject_foreign_state_bind_workspace_and_keep_invalid_requests_atomic() {
    let home = Session::new();
    let path = home.0.join("foreign.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE important(data);").unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "delete");
    let mut s = home.open();
    assert_eq!(s.db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
    assert_eq!(s.db.query_row("PRAGMA synchronous", [], |r| r.get::<_, i32>(0)).unwrap(), 2);
    s.bind_workspace(&home.0).unwrap();
    s.bind_workspace(&home.0.join(".")).unwrap();
    let other = Session::new();
    assert!(s.bind_workspace(&other.0).is_err());
    assert!(s.submit("missing", "work").is_err());
    assert!(s.submit("default", " ").is_err());
    assert!(s.submit_checked("default", "x", Some(" ")).is_err());
    assert_eq!(s.tasks().unwrap(), json!([]));
    let t = legacy(&mut s, "default", "ok").unwrap();
    claim(&mut s, &t);
    let before = s.task(&t).unwrap();
    assert!(s.request_tool(&t, &json!({"op":"edit_file","path":"a"})).is_err());
    assert!(s.apply(&t, &json!({"op":"send","agent":"not here","text":"invalid attempt"})).is_err());
    assert_eq!(s.task(&t).unwrap(), before);
    drop(s);
    let mut s = home.open();
    assert!(s.bind_workspace(&other.0).is_err());
    s.recover().unwrap();
    let retried = claim(&mut s, &t);
    assert!(!chat(&retried).contains("invalid attempt"));
    assert_eq!(s.task(&t).unwrap()["turns_used"], 2);
}

fn handoff(s: &mut Store, task: &str, children: &[(&str, &str)]) -> Result<()> {
    let children: Vec<_> = children.iter().map(|(agent, text)| json!({"agent":agent,"text":text})).collect();
    s.apply(task, &json!({"op":"handoff","text":"shared forward brief","children":children}))
}

fn packet(reporter: &Turn) -> Value {
    assert_eq!(reporter.kind, TurnKind::Reporter);
    assert_eq!(reporter.messages.len(), 1);
    serde_json::from_str(reporter.messages[0]["content"].as_str().unwrap()).unwrap()
}

#[test]
fn model_overrides_are_scoped_to_session_and_provider() {
    let home = Session::new();
    let mut s = home.open();
    let api = ModelSettings { model: Some("api-model".into()), effort: Some("high".into()) };
    let chatgpt = ModelSettings { model: None, effort: Some("medium".into()) };
    s.set_model_settings("api", &api).unwrap();
    s.set_model_settings("chatgpt", &chatgpt).unwrap();
    drop(s);

    let mut s = home.open();
    assert_eq!(s.model_settings("api", Some("env-model")).unwrap(), api);
    assert_eq!(s.model_settings("chatgpt", None).unwrap(), chatgpt);
    assert_eq!(
        s.model_settings("chatgpt", Some("env-model")).unwrap(),
        ModelSettings { model: Some("env-model".into()), ..chatgpt.clone() },
    );
    s.set_model_settings("api", &ModelSettings::default()).unwrap();
    assert_eq!(s.model_settings("api", None).unwrap(), ModelSettings::default());
    assert_eq!(s.model_settings("chatgpt", None).unwrap(), chatgpt);

    let other = Session::new();
    assert_eq!(other.open().model_settings("chatgpt", None).unwrap(), ModelSettings::default());
}

#[test]
fn submitted_settings_survive_changes_and_restart_through_handoff_repair_and_report() {
    let home = Session::new();
    let mut s = home.open();
    let overrides = ModelSettings { model: None, effort: Some("low".into()) };
    s.set_model_settings("api", &overrides).unwrap();
    let submitted = s.model_settings("api", Some("at-submission")).unwrap();
    let root = s.submit_with_model("default", "fix", Some("check it"), false, &submitted).unwrap();
    s.set_model_settings("api", &ModelSettings { model: Some("later-model".into()), effort: None }).unwrap();
    drop(s);

    let mut s = home.open();
    s.recover().unwrap();
    assert_eq!(claim(&mut s, &root).settings.as_ref(), Some(&submitted));
    handoff(&mut s, &root, &[("worker", "implement")]).unwrap();
    let child = turn(&mut s);
    assert_eq!(child.agent, "worker");
    assert_eq!(child.settings.as_ref(), Some(&submitted));
    finish(&mut s, &child.task, "first pass");
    assert_eq!(s.claim_tool().unwrap().unwrap().0, root);
    s.tool_result(&root, Err("fix required")).unwrap();
    let repair = turn(&mut s);
    assert!(repair.agent.starts_with("repair-"));
    assert_eq!(repair.settings.as_ref(), Some(&submitted));
    finish(&mut s, &repair.task, "fixed");
    assert_eq!(s.claim_tool().unwrap().unwrap().0, root);
    s.tool_result(&root, Ok("passed")).unwrap();
    let reporter = turn(&mut s);
    assert_eq!(reporter.kind, TurnKind::Reporter);
    assert_eq!(reporter.settings.as_ref(), Some(&submitted));
    finish(&mut s, &reporter.task, "verified");
    assert_eq!(s.task(&root).unwrap()["status"], "done");
}

#[test]
fn failed_snapshot_write_rolls_back_root_submission() {
    let home = Session::new();
    let mut s = home.open();
    s.db.execute_batch(
        "CREATE TRIGGER reject_snapshot BEFORE INSERT ON root_model_settings
        BEGIN SELECT RAISE(ABORT,'snapshot write failed'); END;",
    )
    .unwrap();
    let error = s.submit_with_model("default", "work", None, false, &ModelSettings::default()).unwrap_err();
    assert!(error.contains("snapshot write failed"));
    assert_eq!(s.tasks().unwrap(), json!([]));
    assert!(s.claim().unwrap().is_none());
}

#[test]
fn schema_three_tree_keeps_legacy_model_selection_alongside_an_empty_snapshot() {
    let home = Session::new();
    let db = legacy_database(&home);
    db.execute_batch("INSERT INTO tasks(id,root,agent,prompt,status) VALUES ('old','old','default','work','queued')")
        .unwrap();
    db.execute_batch(MIGRATE_V2).unwrap();
    db.execute_batch(tree::MIGRATE_V3).unwrap();
    db.execute("UPDATE settings SET max_agents=6", []).unwrap();
    db.execute("INSERT INTO tree_runs(root,reporter) VALUES ('old','report-old')", []).unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
    drop(db);

    let mut s = home.open();
    let new = s.submit_with_model("default", "new work", None, false, &ModelSettings::default()).unwrap();
    s.set_model_settings("api", &ModelSettings { model: Some("future-model".into()), effort: None }).unwrap();
    assert_eq!(claim(&mut s, "old").settings, None);
    handoff(&mut s, "old", &[("o1", "1"), ("o2", "2")]).unwrap();
    assert_eq!(claim(&mut s, &new).settings, Some(ModelSettings::default()));
    let o1 = turn(&mut s).task;
    let o2 = turn(&mut s).task;
    finish(&mut s, &o1, "o1 result");
    finish(&mut s, &o2, "o2 result");
    assert_eq!(claim(&mut s, "report-old").settings, None);
    finish(&mut s, "report-old", "old report");
    handoff(&mut s, &new, &[("n1", "1"), ("n2", "2")]).unwrap();
    let n1 = turn(&mut s).task;
    let n2 = turn(&mut s).task;
    finish(&mut s, &n1, "n1 result");
    finish(&mut s, &n2, "n2 result");
    let reporter = turn(&mut s);
    assert_eq!(reporter.kind, TurnKind::Reporter);
    assert_eq!(reporter.settings, Some(ModelSettings::default()));
}

#[test]
fn schema_two_keeps_private_history_active_operations_and_legacy_actions() {
    let home = Session::new();
    let db = legacy_database(&home);
    db.execute_batch("INSERT INTO tasks(id,root,agent,prompt,status) VALUES ('old','old','default','prior intent','waiting');
        INSERT INTO messages VALUES ('default','user','prior private history');
        INSERT INTO operations(id,task,action,state) VALUES ('op','old','{\"op\":\"shell\",\"command\":\"prior command\"}','ready');").unwrap();
    db.execute_batch(MIGRATE_V2).unwrap();
    db.execute("UPDATE settings SET max_agents=4", []).unwrap();
    db.execute("UPDATE root_policies SET policy='auto'", []).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);

    let mut s = home.open();
    s.recover().unwrap();
    assert_eq!(s.task("old").unwrap()["policy"], "auto");
    assert_eq!(s.claim_tool().unwrap(), Some(("old".into(), json!({"op":"shell","command":"prior command"}))));
    s.tool_result("old", Ok("legacy receipt")).unwrap();
    let old = claim(&mut s, "old");
    assert_eq!(old.kind, TurnKind::Legacy);
    assert_eq!(old.settings, None);
    assert!(chat(&old).contains("prior private history"));
    s.apply("old", &json!({"op":"delegate","agent":"peer","text":"legacy assignment"})).unwrap();
    let child = turn(&mut s);
    assert_eq!(child.kind, TurnKind::Legacy);
    assert_eq!(child.settings, None);
    finish(&mut s, &child.task, "legacy result");
    assert!(chat(&claim(&mut s, "old")).contains("legacy result"));
    finish(&mut s, "old", "old finished");
    let root = s.submit("default", "fresh intent").unwrap();
    let fresh_turn = claim(&mut s, &root);
    assert_eq!(fresh_turn.kind, TurnKind::Root);
    assert!(!chat(&fresh_turn).contains("prior private history") && !chat(&fresh_turn).contains("legacy"));
    handoff(&mut s, &root, &[("w1", "1"), ("w2", "2")]).unwrap();
    let w1 = turn(&mut s).task;
    let w2 = turn(&mut s).task;
    finish(&mut s, &w1, "fresh 1");
    finish(&mut s, &w2, "fresh 2");
    let reporter = turn(&mut s);
    assert!(!chat(&reporter).contains("legacy") && !chat(&reporter).contains("prior private history"));
}

#[test]
fn handoff_is_atomic_and_sibling_stages_settle_concurrently_without_upward_messages() {
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit("default", "private original intent").unwrap();
    claim(&mut s, &root);
    for op in ["spawn", "delegate", "send"] {
        assert!(s.apply(&root, &json!({"op":op,"agent":"worker","text":"bad"})).is_err());
    }
    for children in
        [vec![("b", "new"), ("b", "duplicate")], vec![("b", "new"), ("default", "cycle")], vec![("b", "too many"); 17]]
    {
        assert!(handoff(&mut s, &root, &children).is_err());
        assert_eq!(s.agents().unwrap(), json!([{"name":"default"}]));
    }
    handoff(&mut s, &root, &[("b", "only b"), ("c", "only c")]).unwrap();
    let b = turn(&mut s);
    let c = turn(&mut s);
    assert_eq!((b.kind, c.kind), (TurnKind::Node, TurnKind::Node));
    assert!(chat(&b).contains("shared forward brief") && chat(&b).contains("only b"));
    // Default forked handoff shares the parent's immutable prefix while keeping sibling assignments isolated.
    assert!(chat(&b).contains("private original intent") && chat(&c).contains("private original intent"));
    assert_eq!(b.messages[..b.messages.len() - 1], c.messages[..c.messages.len() - 1]);
    let b_last = b.messages.last().unwrap()["content"].as_str().unwrap();
    let c_last = c.messages.last().unwrap()["content"].as_str().unwrap();
    assert!(b_last.contains("only b") && !b_last.contains("only c"));
    assert!(c_last.contains("only c") && !c_last.contains("only b"));
    assert!(handoff(&mut s, &b.task, &[("c", "invalid sibling")]).is_err());
    assert!(s.apply(&b.task, &json!({"op":"ask","text":"invalid"})).is_err());
    assert!(s.claim().unwrap().is_none());
    let status = s.task(&root).unwrap();
    assert_eq!(status["status"], "waiting");
    assert_eq!(status["stage_status"], "done");
    let gate = Arc::new(Barrier::new(2));
    let closes = [(s, b, "published b"), (home.open(), c, "published c")].map(|(mut store, node, text)| {
        let gate = gate.clone();
        thread::spawn(move || {
            gate.wait();
            finish(&mut store, &node.task, text);
        })
    });
    for worker in closes {
        worker.join().unwrap();
    }
    let mut s = home.open();
    let reporter = turn(&mut s);
    let body = packet(&reporter);
    assert_eq!(body["intent"], "private original intent");
    assert_eq!(body["leaf_results"].as_array().unwrap().len(), 2);
    assert_eq!(body["leaf_results"][1]["result"], "published c");
    assert!(!chat(&reporter).contains("shared forward brief"));
    assert!(s.claim().unwrap().is_none());
    for action in [json!({"op":"members"}), json!({"op":"handoff","text":"bad","children":[]})] {
        assert!(s.apply(&reporter.task, &action).is_err());
    }
    assert!(s.request_tool(&reporter.task, &json!({"op":"shell","command":"bad"})).is_err());
    finish(&mut s, &reporter.task, "independent report");
    let settled = s.task(&root).unwrap();
    assert_eq!(settled["result"], "independent report");
    assert_eq!(settled["continuation_node"], reporter.task);
    assert_eq!(settled["continuation_kind"], "summarizer");
    assert_eq!(settled["leaf_results"].as_array().unwrap().len(), 2);
    assert_eq!(s.tasks().unwrap().as_array().unwrap().len(), 4);
    assert_eq!(s.task(&reporter.task).unwrap()["kind"], "summarizer");
    let next = s.submit("default", "new root").unwrap();
    let private = chat(&claim(&mut s, &next));
    assert!(!private.contains("published") && !private.contains("independent report"));
    finish(&mut s, &next, "new output");
    // Single-leaf settlement skips reporter and keeps history isolated from prior sessions.
    assert!(s.claim().unwrap().is_none());
    let next_status = s.task(&next).unwrap();
    assert_eq!(next_status["status"], "done");
    assert_eq!(next_status["result"], "new output");
    assert_eq!(next_status["continuation_node"], next);
}

#[test]
fn handoff_opt_out_isolates_parent_history_and_usage_aggregates_per_root() {
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit("default", "secret parent prompt").unwrap();
    claim(&mut s, &root);
    s.record_usage(
        &root,
        &crate::model::UsageStats { input_tokens: 1000, cached_tokens: 200, output_tokens: 50 },
        3000,
    )
    .unwrap();
    let meta = json!({"reasoning":[{"type":"reasoning","id":"rs1"}]});
    let action = json!({
        "op":"handoff",
        "text":"fresh brief",
        "inherit_context":false,
        "children":[{"agent":"b","text":"only b"},{"agent":"c","text":"only c"}]
    });
    s.apply_with_meta(&root, &action, Some(&meta)).unwrap();
    let b = turn(&mut s);
    let c = turn(&mut s);
    assert!(chat(&b).contains("fresh brief") && chat(&b).contains("only b"));
    assert!(!chat(&b).contains("secret parent prompt") && !chat(&b).contains("only c"));
    assert_eq!(b.last_input_tokens, 0);
    s.record_usage(
        &b.task,
        &crate::model::UsageStats { input_tokens: 400, cached_tokens: 300, output_tokens: 20 },
        1200,
    )
    .unwrap();
    s.record_usage(
        &c.task,
        &crate::model::UsageStats { input_tokens: 400, cached_tokens: 300, output_tokens: 20 },
        1200,
    )
    .unwrap();
    finish(&mut s, &b.task, "b done");
    finish(&mut s, &c.task, "c done");
    let reporter = turn(&mut s);
    s.record_usage(
        &reporter.task,
        &crate::model::UsageStats { input_tokens: 200, cached_tokens: 0, output_tokens: 10 },
        600,
    )
    .unwrap();
    finish(&mut s, &reporter.task, "overall done");
    let status = s.task(&root).unwrap();
    assert_eq!(status["usage"]["input_tokens"], 2000);
    assert_eq!(status["usage"]["cached_tokens"], 800);
    assert_eq!(status["usage"]["output_tokens"], 100);
}

#[test]
fn clarifications_wait_durably_and_are_in_reporter_intent_only_for_their_root() {
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit_checked("default", "choose a version", Some("exact check")).unwrap();
    let ask = json!({"op":"ask","text":"Which version?"});
    claim(&mut s, &root);
    s.apply(&root, &ask).unwrap();
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    let status = s.task(&root).unwrap();
    assert_eq!(status["status"], "input");
    assert_eq!(status["question"], "Which version?");
    assert!(s.claim().unwrap().is_none() && s.claim_tool().unwrap().is_none());
    assert!(s.answer(&root, " ").is_err());
    s.answer(&root, "version 2").unwrap();
    assert!(s.answer(&root, "version 3").is_err());
    assert!(chat(&claim(&mut s, &root)).contains("version 2"));
    s.apply(&root, &json!({"op":"members"})).unwrap();
    claim(&mut s, &root);
    assert!(s.apply(&root, &ask).is_err());
    handoff(&mut s, &root, &[("w1", "part 1"), ("w2", "part 2")]).unwrap();
    let w1 = turn(&mut s);
    let w2 = turn(&mut s);
    finish(&mut s, &w1.task, "public result 1");
    finish(&mut s, &w2.task, "public result 2");
    assert!(s.claim().unwrap().is_none());
    s.claim_tool().unwrap().unwrap();
    s.tool_result(&root, Ok("private check evidence")).unwrap();
    let reporter = turn(&mut s);
    let body = packet(&reporter);
    assert_eq!(body["clarifications"], json!([{"question":"Which version?","answer":"version 2"}]));
    assert_eq!(body["verification"], "passed");
    assert!(!chat(&reporter).contains("private check evidence"));
    finish(&mut s, &reporter.task, "finished");
    let next = s.submit("default", "another").unwrap();
    claim(&mut s, &next);
    s.request_tool(&next, &json!({"op":"read_file","path":"file"})).unwrap();
    s.claim_tool().unwrap().unwrap();
    s.tool_result(&next, Err("private diagnostic")).unwrap();
    claim(&mut s, &next);
    assert!(s.apply(&next, &ask).is_err());
    handoff(&mut s, &next, &[("w1", "a"), ("w2", "b")]).unwrap();
    let nw1 = turn(&mut s);
    let nw2 = turn(&mut s);
    finish(&mut s, &nw1.task, "done 1");
    finish(&mut s, &nw2.task, "done 2");
    let reporter = turn(&mut s);
    assert_eq!(packet(&reporter)["clarifications"], json!([]));
    assert!(!chat(&reporter).contains("private diagnostic") && !chat(&reporter).contains("version 2"));
}

#[test]
fn failed_or_unknown_branch_allows_approved_sibling_to_settle_and_blocks_success() {
    for failed in [true, false] {
        let home = Session::new();
        let mut s = home.open();
        let root = s.submit_with_policy("default", "parent", Some("gated check"), true).unwrap();
        claim(&mut s, &root);
        handoff(&mut s, &root, &[("b", "first"), ("c", "second")]).unwrap();
        let b = turn(&mut s);
        let c = turn(&mut s);
        s.request_tool(&c.task, &json!({"op":"edit_file","path":"w","old":"","new":"private edit"})).unwrap();
        let approval = s.task(&c.task).unwrap()["approval_id"].as_str().unwrap().to_owned();
        if failed {
            s.fail(&b.task, "private provider failure").unwrap();
        } else {
            s.request_tool(&b.task, &json!({"op":"read_file","path":"private path"})).unwrap();
            assert_eq!(s.claim_tool().unwrap().unwrap().0, b.task);
            drop(s);
            s = home.open();
            s.recover().unwrap();
            s.recover().unwrap();
            assert!(s.tool_result(&b.task, Ok("late")).is_err());
        }
        assert!(s.claim().unwrap().is_none() && s.claim_tool().unwrap().is_none());
        s.approve(&approval, true).unwrap();
        s.claim_tool().unwrap().unwrap();
        assert!(s.claim().unwrap().is_none());
        s.tool_result(&c.task, Ok("private tool receipt")).unwrap();
        claim(&mut s, &c.task);
        finish(&mut s, &c.task, "published independent success");
        assert!(s.claim_tool().unwrap().is_none());
        let reporter = turn(&mut s);
        let status = if failed { "failed" } else { "unknown" };
        let body = packet(&reporter);
        assert_eq!(body["verification"], "not_run");
        assert_eq!(body["outcome"], status);
        assert_eq!(body["leaf_results"][0]["status"], status);
        assert!(body["leaf_results"][0]["result"].is_null());
        assert!(chat(&reporter).contains("published independent success") && !chat(&reporter).contains("private"));
        finish(&mut s, &reporter.task, "partial result");
        assert_eq!(s.task(&root).unwrap()["status"], status);
    }
}

#[test]
fn tree_checks_use_forward_private_repair_jobs_and_stop_after_ten_attempts() {
    for pass in [true, false] {
        let home = Session::new();
        let mut s = home.open();
        let root = s.submit_with_policy("default", "user intent", Some("exact command"), true).unwrap();
        claim(&mut s, &root);
        finish(&mut s, &root, "first public result");
        let verify = json!({"op":"verify","command":"exact command"});
        for attempt in 1..=10 {
            assert!(s.claim().unwrap().is_none());
            assert_eq!(s.claim_tool().unwrap(), Some((root.clone(), verify.clone())));
            let diag = format!("private check evidence {attempt}");
            s.tool_result(&root, if attempt == 10 && pass { Ok(&diag) } else { Err(&diag) }).unwrap();
            drop(s);
            s = home.open();
            s.recover().unwrap();
            assert!(s.claim_tool().unwrap().is_none());
            let status = s.task(&root).unwrap();
            assert_eq!(status["stage_status"], "done");
            assert_eq!(status["status"], "waiting");
            if attempt < 10 {
                let repair = turn(&mut s);
                assert_eq!(repair.kind, TurnKind::Node);
                assert_ne!(repair.agent, "default");
                assert!(chat(&repair).contains(&diag) && chat(&repair).contains("user intent"));
                assert_eq!(s.task(&repair.task).unwrap()["policy"], "review");
                assert_eq!(s.agents().unwrap(), json!([{"name":"default"}]));
                assert!(s.submit(&repair.agent, "new root").is_err());
                finish(&mut s, &repair.task, &format!("repair completed {attempt}"));
            }
        }
        let reporter = turn(&mut s);
        let body = packet(&reporter);
        assert_eq!(body["outcome"], if pass { "done" } else { "failed" });
        assert_eq!(body["verification"], if pass { "passed" } else { "failed" });
        assert!(!chat(&reporter).contains("private check evidence"));
        finish(&mut s, &reporter.task, "check report");
        let status = s.task(&root).unwrap();
        assert_eq!(status["evidence"].as_array().unwrap().len(), 10);
        assert_eq!(status["tree_nodes"][2]["kind"], "repair");
        assert_eq!(status["turns_used"], 11);
        assert_eq!(status["status"], body["outcome"]);
        assert_eq!(status["verification"], body["verification"]);
        assert!(s.claim_tool().unwrap().is_none());
        let later = s.submit("default", "later work").unwrap();
        assert!(!chat(&claim(&mut s, &later)).contains("private check evidence"));
    }
}

#[test]
fn tree_unknown_check_never_replays_and_reporter_never_turns_it_into_success() {
    // 1-leaf lost check settles directly as unknown without continuation.
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit_checked("default", "checked", Some("check")).unwrap();
    claim(&mut s, &root);
    finish(&mut s, &root, "work complete");
    s.claim_tool().unwrap().unwrap();
    drop(s);
    let mut s = home.open();
    s.recover().unwrap();
    assert!(s.tool_result(&root, Ok("late pass")).is_err());
    assert!(s.claim().unwrap().is_none());
    let status = s.task(&root).unwrap();
    assert_eq!(status["stage_status"], "done");
    assert_eq!(status["status"], "unknown");
    assert_eq!(status["verification"], "failed");
    assert!(status["verification_note"].as_str().unwrap().contains("outcome is unknown"));
    assert!(status["continuation_node"].is_null());
    assert!(s.claim_tool().unwrap().is_none());

    // 2-leaf lost check invokes reporter with unknown outcome and blocks continuation.
    let home2 = Session::new();
    let mut s2 = home2.open();
    let r2 = s2.submit_checked("default", "checked two", Some("check")).unwrap();
    claim(&mut s2, &r2);
    handoff(&mut s2, &r2, &[("a", "1"), ("b", "2")]).unwrap();
    let a = turn(&mut s2);
    let b = turn(&mut s2);
    finish(&mut s2, &a.task, "a complete");
    finish(&mut s2, &b.task, "b complete");
    s2.claim_tool().unwrap().unwrap();
    drop(s2);
    let mut s2 = home2.open();
    s2.recover().unwrap();
    let reporter = turn(&mut s2);
    assert_eq!(packet(&reporter)["verification"], "unknown");
    assert_eq!(packet(&reporter)["outcome"], "unknown");
    finish(&mut s2, &reporter.task, "model claims success");
    let st2 = s2.task(&r2).unwrap();
    assert_eq!(st2["status"], "unknown");
    assert!(st2["continuation_node"].is_null());
    assert!(s2.claim_tool().unwrap().is_none() && s2.claim().unwrap().is_none());
}

#[test]
fn tree_cancellation_fences_late_model_tool_check_and_reporter_callbacks() {
    for boundary in ["model", "approval", "started", "check_ready", "check_started", "reporter"] {
        let home = Session::new();
        let mut s = home.open();
        let root = s.submit_with_policy("default", "intent", Some("exact"), true).unwrap();
        claim(&mut s, &root);
        handoff(&mut s, &root, &[("b", "task"), ("c", "task 2")]).unwrap();
        let first = turn(&mut s);
        let second = turn(&mut s);
        let mut stale = first.task.clone();
        let mut approval = None;
        match boundary {
            "model" => (),
            "approval" | "started" => {
                s.request_tool(&first.task, &json!({"op":"shell","command":"model command"})).unwrap();
                let approval_id = s.task(&first.task).unwrap()["approval_id"].as_str().unwrap().to_owned();
                if boundary == "started" {
                    s.approve(&approval_id, true).unwrap();
                    s.claim_tool().unwrap().unwrap();
                }
                approval = Some(approval_id);
            }
            _ => {
                finish(&mut s, &first.task, "complete 1");
                finish(&mut s, &second.task, "complete 2");
                stale = root.clone();
                if boundary != "check_ready" {
                    s.claim_tool().unwrap().unwrap();
                }
                if boundary == "reporter" {
                    s.tool_result(&root, Ok("pass")).unwrap();
                    stale = turn(&mut s).task;
                }
            }
        }
        s.cancel(&root).unwrap();
        if let Some(id) = approval {
            assert!(s.approve(&id, true).is_err());
        }
        if boundary != "check_ready" {
            assert!(s.discard_late_completion(&stale).unwrap());
        }
        assert!(s.apply(&stale, &json!({"op":"finish","text":"late"})).is_err());
        assert!(s.request_tool(&stale, &json!({"op":"shell","command":"late"})).is_err());
        assert!(s.tool_result(&stale, Ok("late pass")).is_err());
        assert!(s.claim().unwrap().is_none() && s.claim_tool().unwrap().is_none());
        let expected = if matches!(boundary, "started" | "check_started") { "unknown" } else { "cancelled" };
        assert_eq!(s.task(&root).unwrap()["status"], expected, "{boundary}");
    }
}

#[test]
fn tree_reports_have_one_durable_claim_no_budget_cap_and_deterministic_partial_results() {
    // A single leaf settles directly without invoking a summarizer turn.
    let home_one = Session::new();
    let mut s1 = home_one.open();
    let r1 = s1.submit("default", "single direct").unwrap();
    claim(&mut s1, &r1);
    finish(&mut s1, &r1, "direct leaf result");
    assert!(s1.claim().unwrap().is_none());
    let st1 = s1.task(&r1).unwrap();
    assert_eq!(st1["status"], "done");
    assert_eq!(st1["result"], "direct leaf result");
    assert_eq!(st1["turns_used"], 1);
    assert!(st1["turn_budget"].is_null());
    assert_eq!(st1["continuation_node"], r1);

    for mode in ["interrupted", "provider_error", "oversize"] {
        let home = Session::new();
        let mut s = home.open();
        let root = s.submit("default", "intent").unwrap();
        claim(&mut s, &root);
        handoff(&mut s, &root, &[("p1", "branch 1"), ("p2", "branch 2")]).unwrap();
        match mode {
            "oversize" => {
                let p1 = turn(&mut s);
                let p2 = turn(&mut s);
                finish(&mut s, &p1.task, &"羽".repeat(1_000_000));
                finish(&mut s, &p2.task, "other");
                assert!(s.claim().unwrap().is_none());
            }
            "interrupted" | "provider_error" => {
                let p1 = turn(&mut s);
                let p2 = turn(&mut s);
                finish(&mut s, &p1.task, "public result 1");
                finish(&mut s, &p2.task, "public result 2");
                let reporter = turn(&mut s);
                if mode == "interrupted" {
                    drop(s);
                    s = home.open();
                    s.recover().unwrap();
                } else {
                    s.fail(&reporter.task, "private provider diagnostic").unwrap();
                }
                assert!(s.apply(&reporter.task, &json!({"op":"finish","text":"second attempt"})).is_err());
            }
            _ => unreachable!(),
        }
        let status = s.task(&root).unwrap();
        assert_eq!(status["status"], "failed");
        assert!(status["result"].as_str().unwrap().contains("Partial report"));
        assert_eq!(status["turns_used"], if mode == "oversize" { 3 } else { 4 });
        assert!(status["turn_budget"].is_null());
        assert!(!status.to_string().contains("private provider diagnostic"));
        if mode == "oversize" {
            assert!(status["leaf_results"][0]["result"].as_str().unwrap().contains("truncated"));
        }
        s.recover().unwrap();
        assert!(s.claim().unwrap().is_none());
        assert_eq!(s.task(&root).unwrap(), status);
    }
}

#[test]
fn delete_removes_completed_and_idle_trees_while_preserving_siblings_and_refusing_inflight() {
    let home = Session::new();
    let mut s = home.open();
    let overrides = ModelSettings { model: Some("m1".into()), effort: Some("high".into()) };
    let target = s.submit_with_model("default", "delete me", Some("cargo test"), true, &overrides).unwrap();

    // Clarify and handoff target tree with child + repair stage + clarifications + operations + model settings.
    claim(&mut s, &target);
    s.apply(&target, &json!({"op":"ask","text":"Which module?"})).unwrap();
    s.answer(&target, "parser").unwrap();
    claim(&mut s, &target);
    handoff(&mut s, &target, &[("worker", "build fix")]).unwrap();
    let child = turn(&mut s);
    assert_eq!(child.agent, "worker");
    s.request_tool(&child.task, &json!({"op":"shell","command":"echo hi"})).unwrap();
    let aid = s.task(&child.task).unwrap()["approval_id"].as_str().unwrap().to_owned();
    s.approve(&aid, true).unwrap();
    s.claim_tool().unwrap().unwrap();
    s.tool_result(&child.task, Ok("hi")).unwrap();
    claim(&mut s, &child.task);
    finish(&mut s, &child.task, "built");
    // Fail first check to spawn internal repair agent, then pass check and finish report.
    assert_eq!(s.claim_tool().unwrap().unwrap().0, target);
    s.tool_result(&target, Err("test failed")).unwrap();
    let repair = turn(&mut s);
    assert!(repair.agent.starts_with("repair-"));
    let repair_agent = repair.agent.clone();
    finish(&mut s, &repair.task, "repaired");
    assert_eq!(s.claim_tool().unwrap().unwrap().0, target);
    s.tool_result(&target, Ok("passed")).unwrap();
    let reporter = turn(&mut s);
    let reporter_id = reporter.task.clone();
    finish(&mut s, &reporter_id, "all green");
    let summarizer_agent = s.task(&reporter_id).unwrap()["agent"].as_str().unwrap().to_owned();
    assert!(summarizer_agent.starts_with("summarizer-"));

    // Create a sibling root before deleting target.
    let sibling = s.submit_with_model("default", "keep sibling", None, false, &overrides).unwrap();

    // Delete by child ID resolves and deletes the entire root tree and its internal repair/summarizer agents.
    assert_eq!(s.delete(&child.task).unwrap(), target);
    assert!(s.task(&target).is_err());
    assert!(s.task(&child.task).is_err());
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM agents WHERE name IN (?1,?2)",
            params![repair_agent, summarizer_agent],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        s.db.query_row("SELECT count(*) FROM clarifications WHERE root=?1", [&target], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    assert_eq!(
        s.db.query_row("SELECT count(*) FROM tree_rounds WHERE root=?1", [&target], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    // Sibling root and persistent members ("default", "worker") remain intact.
    assert_eq!(s.task(&sibling).unwrap()["prompt"], "keep sibling");
    assert!(s.agents().unwrap().as_array().unwrap().iter().any(|m| m["name"] == "worker"));

    // Idle waiting-input and approval roots on persistent member "worker" can be deleted safely.
    let waiting = s.submit_with_model("worker", "ask me", None, false, &overrides).unwrap();
    // Claim sibling off "default" so the next queued claim targets "worker" -> wait, submit sibling after or claim both:
    claim(&mut s, &sibling); // model_reserved=1 on sibling ("default")
    let w_turn = turn(&mut s);
    assert_eq!(w_turn.task, waiting);
    s.apply(&waiting, &json!({"op":"ask","text":"Need detail?"})).unwrap();
    assert_eq!(s.delete(&waiting).unwrap(), waiting);

    let approval_root = s.submit_with_model("worker", "review op", None, true, &overrides).unwrap();
    let a_turn = turn(&mut s);
    assert_eq!(a_turn.task, approval_root);
    s.request_tool(&approval_root, &json!({"op":"shell","command":"ls"})).unwrap();
    assert_eq!(s.delete(&approval_root).unwrap(), approval_root);

    // Refuse deletion whenever a model turn, tool operation, or reporter turn is in flight on sibling.
    assert!(s.delete(&sibling).unwrap_err().contains("in flight")); // model_reserved=1
    assert_eq!(s.task(&sibling).unwrap()["id"], sibling);
    s.request_tool(&sibling, &json!({"op":"shell","command":"ls"})).unwrap();
    s.claim_tool().unwrap().unwrap(); // operations.reserved=1
    assert!(s.delete(&sibling).unwrap_err().contains("in flight"));
    s.tool_result(&sibling, Ok("ok")).unwrap();
    claim(&mut s, &sibling);
    handoff(&mut s, &sibling, &[("w2", "work 2"), ("w3", "work 3")]).unwrap();
    let sw2 = turn(&mut s);
    let sw3 = turn(&mut s);
    finish(&mut s, &sw2.task, "done 2");
    finish(&mut s, &sw3.task, "done 3");
    let _rep = turn(&mut s); // phase='reporting'
    assert!(s.delete(&sibling).unwrap_err().contains("in flight"));
    assert!(s.delete("nonexistent-id").unwrap_err().contains("unknown task"));
}

#[test]
fn reject_action_queues_bounded_corrections_and_falls_back_for_reporter() {
    let home = Session::new();
    let mut s = home.open();
    let r = s.submit("default", "plain prompt with non-json text").unwrap();
    claim(&mut s, &r);
    for attempt in 1..=5 {
        let err = format!("err-{attempt}");
        s.reject_action(&r, &err).unwrap();
        let rejected = claim(&mut s, &r);
        assert!(chat(&rejected).contains(&err));
        let last_msg = rejected.messages.last().unwrap()["content"].as_str().unwrap();
        assert!(last_msg.contains("Continue task") && last_msg.contains("protocol_error"));
    }
    // 6th rejection exceeds MAX_ACTION_REJECTIONS (5) and settles 1-leaf root directly as failed.
    s.reject_action(&r, "err-6").unwrap();
    let st = s.task(&r).unwrap();
    assert_eq!(st["stage_status"], "failed");
    assert_eq!(st["stage_error"], "err-6");
    assert_eq!(st["status"], "failed");
    assert!(s.claim().unwrap().is_none());

    // Reporter rejection never retries; it falls back immediately to a partial failed report.
    let r_multi = s.submit("default", "multi leaf").unwrap();
    claim(&mut s, &r_multi);
    handoff(&mut s, &r_multi, &[("a", "part a"), ("b", "part b")]).unwrap();
    let ta = turn(&mut s);
    let tb = turn(&mut s);
    finish(&mut s, &ta.task, "done a");
    finish(&mut s, &tb.task, "done b");
    let rep = turn(&mut s);
    assert_eq!(rep.kind, TurnKind::Reporter);
    s.reject_action(&rep.task, "reporter err").unwrap();
    let settled = s.task(&r_multi).unwrap();
    assert_eq!(settled["status"], "failed");
    assert!(settled["result"].as_str().unwrap().contains("Partial report"));

    // Interleaved valid actions reset the consecutive rejection streak so >5 total rejections across a long run succeed.
    let r_reset = s.submit("default", "long running doc edit").unwrap();
    claim(&mut s, &r_reset);
    for round_err in 1..=8 {
        s.reject_action(&r_reset, &format!("blip-{round_err}")).unwrap();
        let retried = claim(&mut s, &r_reset);
        let msg = retried.messages.last().unwrap()["content"].as_str().unwrap();
        assert!(msg.contains("attempt") || msg.contains("Continue task"));
        // Every valid non-terminal action resets the streak before the next turn.
        s.apply(&r_reset, &json!({"op":"members"})).unwrap();
        claim(&mut s, &r_reset);
    }
    finish(&mut s, &r_reset, "recovered across 8 intermittent blips");
    assert_eq!(s.task(&r_reset).unwrap()["status"], "done");
}

#[test]
fn new_sessions_have_isolated_node_histories() {
    let home = Session::new();
    let mut s = home.open();
    let secret = "SECRET-SESSION-ONE-CONTEXT";
    let first = s.submit("default", secret).unwrap();
    let first_turn = claim(&mut s, &first);
    assert!(chat(&first_turn).contains(secret));
    finish(&mut s, &first, "first finished");

    let second = s.submit("default", "second unrelated task").unwrap();
    let second_turn = claim(&mut s, &second);
    assert!(!chat(&second_turn).contains(secret) && !chat(&second_turn).contains("first finished"));
    assert_eq!(second_turn.messages.len(), 1);
}

#[test]
fn single_leaf_and_summarizer_continue_as_next_subtree_root_with_fresh_budget_and_member_reuse() {
    let home = Session::new();
    let mut s = home.open();

    // Part A: 1-leaf branch settles at leaf `b`, which continues as Round-2 subtree root with fresh budget & member reuse.
    let r1 = s.submit_checked("default", "round 1 initial", Some("verify r1")).unwrap();
    claim(&mut s, &r1);
    handoff(&mut s, &r1, &[("b", "do b")]).unwrap();
    let b1 = turn(&mut s);
    finish(&mut s, &b1.task, "b1 output");
    assert_eq!(s.claim_tool().unwrap().unwrap().0, r1);
    s.tool_result(&r1, Ok("r1 ok")).unwrap();
    assert!(s.claim().unwrap().is_none()); // Skipped reporter cleanly!
    let settled1 = s.task(&r1).unwrap();
    assert_eq!(settled1["status"], "done");
    assert_eq!(settled1["verification"], "passed");
    assert_eq!(settled1["continuation_node"], b1.task);
    assert_eq!(settled1["continuation_kind"], "leaf");
    assert_eq!(settled1["continuation_agent"], "b");

    // Continue at `b1` without --check: must NOT inherit Round 1's verification=passed, must allow `ask`, fresh budget, & reuse `default`/`b`!
    let (cont_root, cont_node) =
        s.continue_tree(&b1.task, "round 2 follow up", None, false, &ModelSettings::default()).unwrap();
    assert_eq!((&cont_root, &cont_node), (&r1, &b1.task));
    let mid_r2 = s.task(&r1).unwrap();
    assert_eq!(mid_r2["round"], 2);
    assert_eq!(mid_r2["verification"], "not_requested");
    assert_eq!(mid_r2["turns_used"], 0);

    let r2_root_turn = claim(&mut s, &b1.task);
    assert_eq!(r2_root_turn.kind, TurnKind::Root);
    assert!(chat(&r2_root_turn).contains("b1 output") && chat(&r2_root_turn).contains("round 2 follow up"));
    // Can ask before using tools as the new subtree root, then reuse `default` in Round 2!
    s.apply(&b1.task, &json!({"op":"ask","text":"Need Round 2 choice?"})).unwrap();
    assert_eq!(s.task(&r1).unwrap()["status"], "input");
    s.answer(&r1, "Use path X").unwrap();
    let r2_resumed = claim(&mut s, &b1.task);
    assert!(chat(&r2_resumed).contains("Use path X"));
    handoff(&mut s, &b1.task, &[("default", "reused default"), ("c", "two leaves")]).unwrap();
    let r2_def = turn(&mut s);
    let r2_c = turn(&mut s);
    finish(&mut s, &r2_def.task, "default r2 done");
    finish(&mut s, &r2_c.task, "c r2 done");
    let sum = turn(&mut s);
    assert_eq!(sum.kind, TurnKind::Reporter);
    assert!(packet(&sum)["clarifications"].to_string().contains("Use path X"));
    finish(&mut s, &sum.task, "Round 2 summarized");
    let settled2 = s.task(&r1).unwrap();
    assert_eq!(settled2["status"], "done");
    assert_eq!(settled2["prompt"], "round 1 initial");
    assert_eq!(settled2["result"], "Round 2 summarized");
    assert_eq!(settled2["continuation_node"], sum.task);
    assert_eq!(settled2["continuation_kind"], "summarizer");
    assert_eq!(settled2["rounds"].as_array().unwrap().len(), 2);

    // Part B: Summarizer node itself becomes Round 3 subtree root and can hand off again & cancel cleanly across deep rounds.
    let (_, sum_node) =
        s.continue_tree(&r1, "round 3 from summarizer", None, false, &ModelSettings::default()).unwrap();
    assert_eq!(sum_node, sum.task);
    let r3_turn = claim(&mut s, &sum.task);
    assert_eq!(r3_turn.kind, TurnKind::Root);
    assert!(chat(&r3_turn).contains("Round 2 summarized") && chat(&r3_turn).contains("round 3 from summarizer"));
    handoff(&mut s, &sum.task, &[("b", "reused b"), ("d", "extra worker")]).unwrap();
    let _r3_b = turn(&mut s);
    let _r3_d = turn(&mut s);
    s.cancel(&r1).unwrap();
    let settled3 = s.task(&r1).unwrap();
    assert_eq!(settled3["status"], "cancelled");
    assert!(settled3["continuation_node"].is_null()); // 2 leaves were cancelled -> no single leaf/summarizer.
    assert!(s.continue_tree(&r1, "blocked", None, false, &ModelSettings::default()).is_err());

    // Part C: `unknown` outcome cannot be continued.
    let home_u = Session::new();
    let mut su = home_u.open();
    let ru = su.submit("default", "unknown op").unwrap();
    claim(&mut su, &ru);
    su.request_tool(&ru, &json!({"op":"shell","command":"ls"})).unwrap();
    su.claim_tool().unwrap().unwrap();
    drop(su);
    let mut su = home_u.open();
    su.recover().unwrap();
    assert_eq!(su.task(&ru).unwrap()["status"], "unknown");
    assert!(su.continue_tree(&ru, "nope", None, false, &ModelSettings::default()).is_err());
}

#[test]
fn schema_four_database_migrates_cleanly_to_v6_and_allows_concurrent_same_agent() {
    let home = Session::new();
    let mut db = legacy_database(&home);
    let tx = db.transaction().unwrap();
    tx.execute_batch(MIGRATE_V2).unwrap();
    tx.execute_batch(crate::store::tree::MIGRATE_V3).unwrap();
    tx.execute_batch(MIGRATE_V4).unwrap();
    tx.pragma_update(None, "user_version", 4).unwrap();
    // Create a completed 1-leaf tree (r1 -> c1) and a completed 2-leaf tree (r2 -> d1, d2).
    tx.execute("INSERT INTO agents(name) VALUES ('b'),('c')", []).unwrap();
    tx.execute("INSERT INTO tasks(id,root,parent,agent,prompt,status,result) VALUES ('r1','r1',NULL,'default','one leaf','done','done root')", []).unwrap();
    tx.execute("INSERT INTO tasks(id,root,parent,agent,prompt,status,result) VALUES ('c1','r1','r1','b','child','done','leaf answer')", []).unwrap();
    tx.execute("INSERT INTO root_policies(root,policy) VALUES ('r1','auto')", []).unwrap();
    tx.execute("INSERT INTO tree_runs(root,reporter,phase,outcome,result) VALUES ('r1','rep1','complete','done','leaf answer')", []).unwrap();

    tx.execute("INSERT INTO tasks(id,root,parent,agent,prompt,status,result) VALUES ('r2','r2',NULL,'default','two leaves','done','done root 2')", []).unwrap();
    tx.execute(
        "INSERT INTO tasks(id,root,parent,agent,prompt,status,result) VALUES ('d1','r2','r2','b','c1','done','ans 1')",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO tasks(id,root,parent,agent,prompt,status,result) VALUES ('d2','r2','r2','c','c2','done','ans 2')",
        [],
    )
    .unwrap();
    tx.execute("INSERT INTO root_policies(root,policy) VALUES ('r2','auto')", []).unwrap();
    tx.execute("INSERT INTO tree_runs(root,reporter,phase,outcome,result) VALUES ('r2','rep2','complete','done','old ext report')", []).unwrap();
    tx.commit().unwrap();
    drop(db);

    let mut s = home.open();
    s.recover().unwrap();
    assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 8);
    let t1 = s.task("r1").unwrap();
    assert_eq!(t1["continuation_node"], "c1");
    assert_eq!(t1["rounds"].as_array().unwrap().len(), 1);
    let t2 = s.task("r2").unwrap();
    assert!(t2["continuation_node"].is_null());
    assert_eq!(t2["rounds"].as_array().unwrap().len(), 1);
    let q1 = s.submit("default", "same 1").unwrap();
    let q2 = s.submit("default", "same 2").unwrap();
    let c1 = claim(&mut s, &q1);
    let c2 = claim(&mut s, &q2);
    assert_ne!(c1.task, c2.task);
}

#[test]
fn collaborative_whiteboard_supports_cas_append_resolve_delete_digest_deadlock_free_wait_and_cleanup() {
    let home = Session::new();
    let mut s = home.open();
    let root = s.submit("default", "coordinate siblings via whiteboard").unwrap();
    claim(&mut s, &root);
    handoff(&mut s, &root, &[("producer", "write contract"), ("consumer", "wait and read")]).unwrap();
    let ta = turn(&mut s);
    let tb = turn(&mut s);
    assert_eq!(ta.agent, "producer");
    assert_eq!(tb.agent, "consumer");

    // `tb` parks on wb_wait while `ta` is still active (`other_active == 1`).
    s.apply(&tb.task, &json!({"op":"wb_wait","key":"contract:"})).unwrap();
    assert!(s.claim().unwrap().is_none());
    assert_eq!(s.task(&tb.task).unwrap()["status"], "waiting");

    // `ta` writes `contract:api`, waking `tb` with matching update.
    s.apply(
        &ta.task,
        &json!({"op":"wb_write","key":"contract:api","mode":"set","kind":"contract","content":"v1 payload"}),
    )
    .unwrap();
    let ta2 = claim(&mut s, &ta.task);
    assert!(chat(&ta2).contains(r#""op":"wb_write""#) && chat(&ta2).contains(r#""version":1"#));

    // CAS mismatch returns conflict without failing or mutating entry.
    s.apply(
        &ta.task,
        &json!({"op":"wb_write","key":"contract:api","mode":"set","content":"bad overwrite","expected_version":0}),
    )
    .unwrap();
    let ta3 = claim(&mut s, &ta.task);
    assert!(chat(&ta3).contains(r#""conflict":true"#) && chat(&ta3).contains(r#""actual_version":1"#));

    // Append, temporary delete, and resolve.
    s.apply(&ta.task, &json!({"op":"wb_write","key":"contract:api","mode":"append","content":"line 2"})).unwrap();
    claim(&mut s, &ta.task);
    s.apply(&ta.task, &json!({"op":"wb_write","key":"temp:scratch","mode":"set","content":"scratch"})).unwrap();
    claim(&mut s, &ta.task);
    s.apply(&ta.task, &json!({"op":"wb_write","key":"temp:scratch","mode":"delete"})).unwrap();
    claim(&mut s, &ta.task);
    s.apply(
        &ta.task,
        &json!({"op":"wb_write","key":"contract:api","mode":"resolve","content":"v1 payload\nline 2 [locked]"}),
    )
    .unwrap();
    claim(&mut s, &ta.task);

    // `tb` woke with `updated` and receives the transient Collaborative Whiteboard Digest.
    let tb2 = claim(&mut s, &tb.task);
    let tb2_chat = chat(&tb2);
    assert!(tb2_chat.contains("updated") && tb2_chat.contains("Collaborative Whiteboard Digest"));
    s.apply(&tb.task, &json!({"op":"wb_read","prefix":"contract:","since_seq":0})).unwrap();
    let tb3 = claim(&mut s, &tb.task);
    assert!(chat(&tb3).contains("line 2 [locked]") && chat(&tb3).contains("resolved"));

    // Finish `ta`, then `tb` waits on an unwritten key -> immediately wakes with `no_other_writers`.
    finish(&mut s, &ta.task, "producer done");
    s.apply(&tb.task, &json!({"op":"wb_wait","key":"never:written"})).unwrap();
    let tb4 = claim(&mut s, &tb.task);
    assert!(chat(&tb4).contains("no_other_writers"));
    finish(&mut s, &tb.task, "consumer done");

    // In-tree summarizer receives read-only `whiteboard` snapshot.
    let sum = turn(&mut s);
    assert_eq!(sum.kind, TurnKind::Reporter);
    let pkt = packet(&sum);
    let wb = pkt["whiteboard"].as_array().unwrap();
    assert_eq!(wb.len(), 1);
    assert_eq!(wb[0]["key"], "contract:api");
    assert_eq!(wb[0]["status"], "resolved");
    finish(&mut s, &sum.task, "round 1 summarized");

    // Whiteboard persists across continuation rounds and accepts user writes, then cleans up on delete.
    s.continue_tree(&sum.task, "round 2", None, false, &ModelSettings::default()).unwrap();
    s.user_wb_write(&root, "user:note", "set", Some("decision"), Some("approved by user")).unwrap();
    let board = s.whiteboard(&root).unwrap();
    assert_eq!(board["entries"].as_array().unwrap().len(), 2);
    assert_eq!(s.task(&root).unwrap()["whiteboard"].as_array().unwrap().len(), 2);
    claim(&mut s, &sum.task);
    finish(&mut s, &sum.task, "round 2 done");
    s.delete(&root).unwrap();
    let remaining: i64 =
        s.db.query_row("SELECT count(*) FROM whiteboard_entries WHERE root=?1", [&root], |r| r.get(0)).unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn compaction_prepares_and_applies_checkpoint_and_staged_tool_job_preserves_mode() {
    let home = Session::new();
    let mut s = home.open();
    let r_direct = s.submit("default", "direct mode").unwrap();
    assert_eq!(s.task(&r_direct).unwrap()["sandbox"], "direct");

    claim(&mut s, &r_direct);
    s.request_tool(&r_direct, &json!({"op":"shell","command":"echo hi"})).unwrap();
    let job_direct = s.claim_tool_job().unwrap().unwrap();
    assert!(!job_direct.staged);
    s.tool_result(&r_direct, Ok("hi")).unwrap();

    // Grow `r_direct`'s node_messages to >= 6 messages and verify prepare/apply compaction.
    for i in 0..3 {
        claim(&mut s, &r_direct);
        s.request_tool(&r_direct, &json!({"op":"shell","command":format!("echo step {i}")})).unwrap();
        let _ = s.claim_tool_job().unwrap().unwrap();
        s.tool_result(&r_direct, Ok("ok")).unwrap();
    }
    let before = claim(&mut s, &r_direct);
    let before_count = before.messages.len();
    assert!(before_count >= 6);
    let cjob = s.prepare_compaction(&r_direct).unwrap().unwrap();
    assert!(!cjob.middle.is_empty());
    s.apply_compaction(&r_direct, cjob.head_keep, cjob.tail_keep, "condensed middle steps").unwrap();
    let after = claim(&mut s, &r_direct);
    assert!(after.messages.len() < before_count);
    assert!(chat(&after).contains("compacted_history") && chat(&after).contains("condensed middle steps"));
    finish(&mut s, &r_direct, "direct compacted done");

    let r_staged =
        s.submit_with_options("default", "staged mode", None, false, true, &ModelSettings::default()).unwrap();
    assert_eq!(s.task(&r_staged).unwrap()["sandbox"], "staged");
    claim(&mut s, &r_staged);
    s.request_tool(&r_staged, &json!({"op":"shell","command":"echo staged"})).unwrap();
    let job_staged = s.claim_tool_job().unwrap().unwrap();
    assert!(job_staged.staged);
    s.tool_result(&r_staged, Ok("staged")).unwrap();
}

#[test]
fn tree_continuation_skips_prompt_echo_nudges_repetition_and_keeps_compaction_pairs() {
    let home = Session::new();
    let mut s = home.open();
    let r = s.submit("default", "VERY_UNIQUE_INITIAL_PROMPT_MARKER").unwrap();
    let t0 = claim(&mut s, &r);
    assert!(chat(&t0).contains("VERY_UNIQUE_INITIAL_PROMPT_MARKER"));

    // Paginated read_file with distinct offsets must NOT re-echo the original prompt or fire repetition nudges.
    for offset in [1, 101, 201, 301] {
        s.request_tool(&r, &json!({"op":"read_file","path":"doc.md","offset":offset,"limit":100})).unwrap();
        let _ = s.claim_tool().unwrap().unwrap();
        s.tool_result(&r, Ok("page")).unwrap();
        let next = claim(&mut s, &r);
        let tail = next.messages.last().unwrap()["content"].as_str().unwrap();
        assert!(!tail.contains("Original request:"), "unexpected prompt echo: {tail}");
        assert!(!tail.contains("repeated similar"), "unexpected nudge during pagination: {tail}");
    }

    // Protocol error retries still focus cleanly without repeating the full prompt.
    s.reject_action(&r, "bad format").unwrap();
    let retry = claim(&mut s, &r);
    let retry_tail = retry.messages.last().unwrap()["content"].as_str().unwrap();
    assert!(retry_tail.contains("Continue task") && !retry_tail.contains("VERY_UNIQUE_INITIAL_PROMPT_MARKER"));

    // 4 consecutive read-only shell probes trigger a transient repetition nudge.
    let mut nudged = retry;
    for cmd in ["ls -la", "wc -l doc.md", "grep -n '#' doc.md", "cat doc.md | head -5"] {
        s.request_tool(&r, &json!({"op":"shell","command":cmd})).unwrap();
        let _ = s.claim_tool().unwrap().unwrap();
        s.tool_result(&r, Ok("out")).unwrap();
        nudged = claim(&mut s, &r);
    }
    assert!(nudged.messages.last().unwrap()["content"].as_str().unwrap().contains("repeated similar"));

    // An intervening edit_file resets the repetition detector cleanly.
    s.request_tool(&r, &json!({"op":"edit_file","path":"doc.md","old":"a","new":"b"})).unwrap();
    let _ = s.claim_tool().unwrap().unwrap();
    s.tool_result(&r, Ok("edited")).unwrap();
    let clean_after_edit = claim(&mut s, &r);
    assert!(!clean_after_edit.messages.last().unwrap()["content"].as_str().unwrap().contains("repeated similar"));

    // Compaction keeps head_keep == 1 on a root task so assistant/user tool pairs are never split.
    let cjob = s.prepare_compaction(&r).unwrap().unwrap();
    assert_eq!(cjob.head_keep, 1);
}
