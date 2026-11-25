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
    fn open(&self, agents: u32, budget: u32) -> Store {
        Store::open(&self.0.join("swarm.db"), agents, budget).unwrap()
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
        self.create_root(agent, prompt, check, review, tree.then_some(&ModelSettings::default()))
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
    let mut s = home.open(2, 10);
    let root = legacy(&mut s, "default", "private alpha prompt").unwrap();
    spawn(&mut s, &root, "peer");
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"send","agent":"peer","text":"public question"})).unwrap();
    claim(&mut s, &root);
    s.apply(&root, &json!({"op":"delegate","agent":"peer","text":"delegated scope"})).unwrap();
    drop(s);
    let mut s = home.open(50, 50);
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
    let mut s = home.open(2, 6);
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
fn recursive_work_shares_one_root_budget_even_after_restart() {
    let home = Session::new();
    let mut s = home.open(3, 5);
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
    drop(s);
    let mut s = home.open(8, 100);
    s.recover().unwrap();
    assert!(s.claim().unwrap().is_none());
    for id in [&root, &child.task, &leaf.task] {
        let t = s.task(id).unwrap();
        assert_eq!(t["status"], "failed");
        assert_eq!(t["turns_used"], 5);
        assert_eq!(t["turn_budget"], 5);
    }
    assert!(s.task(&leaf.task).unwrap()["result"].as_str().unwrap().contains("budget"));
    let next = legacy(&mut s, "c", "independent").unwrap();
    claim(&mut s, &next);
    assert_eq!(s.task(&next).unwrap()["turns_used"], 1);
    assert!(s.apply(&next, &json!({"op":"spawn","agent":"extra"})).is_err());
}

#[test]
fn two_connections_contend_for_the_same_last_slot() {
    let home = Session::new();
    let mut s = home.open(3, 8);
    let a = legacy(&mut s, "default", "A").unwrap();
    spawn(&mut s, &a, "b");
    let b = legacy(&mut s, "b", "B").unwrap();
    claim(&mut s, &a);
    claim(&mut s, &b);
    let s2 = home.open(100, 100);
    let gate = Arc::new(Barrier::new(2));
    let run = |mut s: Store, task: String, name: &'static str, gate: Arc<Barrier>| {
        thread::spawn(move || {
            gate.wait();
            s.apply(&task, &json!({"op":"spawn","agent":name}))
        })
    };
    let x = run(s, a, "third", gate.clone());
    let y = run(s2, b, "another third", gate);
    assert_ne!(x.join().unwrap().is_ok(), y.join().unwrap().is_ok());
    assert_eq!(home.open(10, 10).agents().unwrap().as_array().unwrap().len(), 3);
}

#[test]
fn same_agent_serialization_extends_until_cancelled_calls_return() {
    let home = Session::new();
    let mut s = home.open(2, 9);
    let first = legacy(&mut s, "default", "first").unwrap();
    spawn(&mut s, &first, "other");
    let second = legacy(&mut s, "default", "second").unwrap();
    let parallel = legacy(&mut s, "other", "parallel").unwrap();
    claim(&mut s, &first);
    let mut s2 = home.open(2, 9);
    claim(&mut s2, &parallel);
    assert!(s.claim().unwrap().is_none());
    s.cancel(&first).unwrap();
    assert!(s2.claim().unwrap().is_none());
    assert!(s.apply(&first, &json!({"op":"send","agent":"other","text":"stale"})).is_err());
    s.release_cancelled(&first).unwrap();
    claim(&mut s2, &second);
    s.request_tool(&second, &json!({"op":"read_file","path":"f"})).unwrap();
    s.claim_tool().unwrap().unwrap();
    let third = legacy(&mut s, "default", "third").unwrap();
    s.cancel(&second).unwrap();
    assert_eq!(s.task(&second).unwrap()["status"], "unknown");
    assert!(s2.claim().unwrap().is_none());
    s.release_cancelled(&second).unwrap();
    claim(&mut s2, &third);
    assert!(s.tool_result(&second, Ok("stale")).is_err());
    assert_eq!(s.task(&second).unwrap()["tool_state"], "unknown");
}

#[test]
fn approvals_are_one_shot_exact_and_survive_all_pre_start_boundaries() {
    let home = Session::new();
    let mut s = home.open(1, 8);
    let t = s.submit_root("default", "edit", None, true, false).unwrap();
    claim(&mut s, &t);
    let shell = json!({"op":"shell","command":"first command"});
    s.request_tool(&t, &shell).unwrap();
    assert!(s.claim_tool().unwrap().is_none());
    drop(s);
    let mut s = home.open(9, 99);
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
    let mut s = home.open(1, 1);
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
    let mut s = home.open(2, 8);
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
    let mut s = home.open(2, 8);
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
    let mut s = home.open(1, 1);
    let cmd = json!({"op":"verify","command":"my verifier"});
    for (check, expected_status, accepted) in
        [(Ok("pass output"), "done", true), (Err("check failed"), "failed", false)]
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
        s = home.open(1, 1);
        s.recover().unwrap();
        assert!(s.claim_tool().unwrap().is_none());
        let t = s.task(&task).unwrap();
        assert_eq!(t["status"], expected_status);
        assert_eq!(t["evidence"], json!([{"action":cmd,"ok":accepted,"result":check.unwrap_or_else(|e|e)}]));
        assert_eq!(t["verification"], if accepted { "passed" } else { "failed" });
        if accepted {
            assert_eq!(t["result"], "proposed result");
        } else {
            assert!(t["result"].as_str().unwrap().contains("check failed"));
        }
    }
}

#[test]
fn failed_checks_queue_durable_repairs_until_success_or_the_third_attempt() {
    for pass_last in [true, false] {
        let home = Session::new();
        let mut s = home.open(1, 8);
        let task = legacy_checked(&mut s, "default", "fix", Some("run checks")).unwrap();
        let verify = json!({"op":"verify","command":"run checks"});
        for attempt in 1..=3 {
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
            let success = attempt == 3 && pass_last;
            let diagnostics = format!("diagnostics {attempt}");
            s.tool_result(&task, if success { Ok("tests pass") } else { Err(&diagnostics) }).unwrap();
            drop(s);
            s = home.open(1, 100);
            s.recover().unwrap();
            assert!(s.claim_tool().unwrap().is_none());
            let status = s.task(&task).unwrap();
            assert_eq!(status["turns_used"], attempt);
            assert_eq!(status["evidence"].as_array().unwrap().len(), attempt as usize);
            assert_eq!(status["evidence"][attempt as usize - 1]["ok"], success);
            if attempt < 3 {
                assert_eq!(status["status"], "queued");
                assert_eq!(status["verification"], "pending");
                assert!(status["proposal"].is_null() && status["result"].is_null());
            } else if success {
                assert_eq!(status["status"], "done");
                assert_eq!(status["verification"], "passed");
                assert_eq!(status["result"], "proposal 3");
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
    let mut s = home.open(1, 8);
    let task = legacy_checked(&mut s, "default", "fix", Some("my check")).unwrap();
    claim(&mut s, &task);
    finish(&mut s, &task, "claim of success");
    assert_eq!(s.claim_tool().unwrap().unwrap().1, json!({"op":"verify","command":"my check"}));
    drop(s);
    let mut s = home.open(1, 8);
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
    let mut s = home.open(2, 6);
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

    let mut s = home.open(1, 1);
    s.recover().unwrap();
    assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 4);
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
    let mut s = home.open(1, 1);
    assert_eq!(s.task(&new).unwrap()["policy"], "auto");
    assert_eq!(s.task("c").unwrap()["policy"], "review");
}

#[test]
fn migration_rolls_back_and_unknown_versions_do_not_open() {
    let home = Session::new();
    let db = legacy_database(&home);
    db.execute_batch("DROP TABLE messages").unwrap();
    let path = home.0.join("swarm.db");
    assert!(Store::open(&path, 1, 1).is_err());
    assert_eq!(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        db.query_row("SELECT count(*) FROM sqlite_schema WHERE name='root_policies'", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );

    let other = Session::new();
    let path = other.0.join("swarm.db");
    let db = legacy_database(&other);
    for (app, version) in [(APP_ID, 5), (APP_ID, 0), (0, 1), (0, 2), (42, 2)] {
        db.pragma_update(None, "application_id", app).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        assert!(Store::open(&path, 1, 1).is_err());
        assert_eq!(db.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0)).unwrap(), "delete");
    }
}

#[test]
fn reject_foreign_state_bind_workspace_and_keep_invalid_requests_atomic() {
    let home = Session::new();
    let path = home.0.join("foreign.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE important(data);").unwrap();
    assert!(Store::open(&path, 1, 1).is_err());
    assert_eq!(db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "delete");
    let mut s = home.open(1, 2);
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
    let mut s = home.open(2, 99);
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
    let mut s = home.open(1, 4);
    let api = ModelSettings { model: Some("api-model".into()), effort: Some("high".into()) };
    let chatgpt = ModelSettings { model: None, effort: Some("medium".into()) };
    s.set_model_settings("api", &api).unwrap();
    s.set_model_settings("chatgpt", &chatgpt).unwrap();
    drop(s);

    let mut s = home.open(1, 4);
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
    assert_eq!(other.open(1, 4).model_settings("chatgpt", None).unwrap(), ModelSettings::default());
}

#[test]
fn submitted_settings_survive_changes_and_restart_through_handoff_repair_and_report() {
    let home = Session::new();
    let mut s = home.open(2, 8);
    let overrides = ModelSettings { model: None, effort: Some("low".into()) };
    s.set_model_settings("api", &overrides).unwrap();
    let submitted = s.model_settings("api", Some("at-submission")).unwrap();
    let root = s.submit_with_model("default", "fix", Some("check it"), false, &submitted).unwrap();
    s.set_model_settings("api", &ModelSettings { model: Some("later-model".into()), effort: None }).unwrap();
    drop(s);

    let mut s = home.open(2, 8);
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
    let mut s = home.open(1, 4);
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
    db.execute("INSERT INTO tree_runs(root,reporter) VALUES ('old','report-old')", []).unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
    drop(db);

    let mut s = home.open(2, 10);
    let new = s.submit_with_model("default", "new work", None, false, &ModelSettings::default()).unwrap();
    s.set_model_settings("api", &ModelSettings { model: Some("future-model".into()), effort: None }).unwrap();
    assert_eq!(claim(&mut s, "old").settings, None);
    finish(&mut s, "old", "old result");
    assert_eq!(claim(&mut s, "report-old").settings, None);
    finish(&mut s, "report-old", "old report");
    assert_eq!(claim(&mut s, &new).settings, Some(ModelSettings::default()));
    finish(&mut s, &new, "new result");
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
    db.execute("UPDATE root_policies SET policy='auto'", []).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);

    let mut s = home.open(1, 1);
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
    assert_eq!(claim(&mut s, &root).kind, TurnKind::Root);
    finish(&mut s, &root, "fresh public result");
    let reporter = turn(&mut s);
    assert!(!chat(&reporter).contains("legacy") && !chat(&reporter).contains("prior private history"));
}

#[test]
fn handoff_is_atomic_and_sibling_stages_settle_concurrently_without_upward_messages() {
    let home = Session::new();
    let mut s = home.open(4, 14);
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
    assert!(!chat(&b).contains("only c") && !chat(&b).contains("private original intent"));
    assert!(handoff(&mut s, &b.task, &[("c", "invalid sibling")]).is_err());
    assert!(s.apply(&b.task, &json!({"op":"ask","text":"invalid"})).is_err());
    assert!(s.claim().unwrap().is_none());
    let status = s.task(&root).unwrap();
    assert_eq!(status["status"], "waiting");
    assert_eq!(status["stage_status"], "done");
    let gate = Arc::new(Barrier::new(2));
    let closes = [(s, b, "published b"), (home.open(1, 1), c, "published c")].map(|(mut store, node, text)| {
        let gate = gate.clone();
        thread::spawn(move || {
            gate.wait();
            finish(&mut store, &node.task, text);
        })
    });
    for worker in closes {
        worker.join().unwrap();
    }
    let mut s = home.open(1, 1);
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
    assert_eq!(s.task(&root).unwrap()["result"], "independent report");
    assert_eq!(s.tasks().unwrap().as_array().unwrap().len(), 3);
    let next = s.submit("default", "new root").unwrap();
    let private = chat(&claim(&mut s, &next));
    assert!(!private.contains("published") && !private.contains("independent report"));
    finish(&mut s, &next, "new output");
    let next_report = turn(&mut s);
    assert!(!chat(&next_report).contains("private original intent") && !chat(&next_report).contains("published"));
}

#[test]
fn clarifications_wait_durably_and_are_in_reporter_intent_only_for_their_root() {
    let home = Session::new();
    let mut s = home.open(1, 12);
    let root = s.submit_checked("default", "choose a version", Some("exact check")).unwrap();
    let ask = json!({"op":"ask","text":"Which version?"});
    claim(&mut s, &root);
    s.apply(&root, &ask).unwrap();
    drop(s);
    let mut s = home.open(1, 1);
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
    finish(&mut s, &root, "public result");
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
    finish(&mut s, &next, "done");
    let reporter = turn(&mut s);
    assert_eq!(packet(&reporter)["clarifications"], json!([]));
    assert!(!chat(&reporter).contains("private diagnostic") && !chat(&reporter).contains("version 2"));
}

#[test]
fn failed_or_unknown_branch_allows_approved_sibling_to_settle_and_blocks_success() {
    for failed in [true, false] {
        let home = Session::new();
        let mut s = home.open(3, 12);
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
            s = home.open(1, 1);
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
fn tree_checks_use_forward_private_repair_jobs_and_stop_after_three_attempts() {
    for pass in [true, false] {
        let home = Session::new();
        let mut s = home.open(1, 10);
        let root = s.submit_with_policy("default", "user intent", Some("exact command"), true).unwrap();
        claim(&mut s, &root);
        finish(&mut s, &root, "first public result");
        let verify = json!({"op":"verify","command":"exact command"});
        for attempt in 1..=3 {
            assert!(s.claim().unwrap().is_none());
            assert_eq!(s.claim_tool().unwrap(), Some((root.clone(), verify.clone())));
            let diag = format!("private check evidence {attempt}");
            s.tool_result(&root, if attempt == 3 && pass { Ok(&diag) } else { Err(&diag) }).unwrap();
            drop(s);
            s = home.open(1, 1);
            s.recover().unwrap();
            assert!(s.claim_tool().unwrap().is_none());
            let status = s.task(&root).unwrap();
            assert_eq!(status["stage_status"], "done");
            assert_eq!(status["status"], "waiting");
            if attempt < 3 {
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
        assert_eq!(status["evidence"].as_array().unwrap().len(), 3);
        assert_eq!(status["tree_nodes"][2]["kind"], "repair");
        assert_eq!(status["turns_used"], 4);
        assert_eq!(status["status"], body["outcome"]);
        assert_eq!(status["verification"], body["verification"]);
        assert!(s.claim_tool().unwrap().is_none());
        let later = s.submit("default", "later work").unwrap();
        assert!(!chat(&claim(&mut s, &later)).contains("private check evidence"));
    }
}

#[test]
fn tree_unknown_check_never_replays_and_reporter_never_turns_it_into_success() {
    let home = Session::new();
    let mut s = home.open(1, 8);
    let root = s.submit_checked("default", "checked", Some("check")).unwrap();
    claim(&mut s, &root);
    finish(&mut s, &root, "work complete");
    s.claim_tool().unwrap().unwrap();
    drop(s);
    let mut s = home.open(1, 1);
    s.recover().unwrap();
    assert!(s.tool_result(&root, Ok("late pass")).is_err());
    let reporter = turn(&mut s);
    assert_eq!(packet(&reporter)["verification"], "unknown");
    assert_eq!(packet(&reporter)["outcome"], "unknown");
    finish(&mut s, &reporter.task, "model claims success");
    let status = s.task(&root).unwrap();
    assert_eq!(status["stage_status"], "done");
    assert_eq!(status["status"], "unknown");
    assert!(s.claim_tool().unwrap().is_none() && s.claim().unwrap().is_none());
}

#[test]
fn tree_cancellation_fences_late_model_tool_check_and_reporter_callbacks() {
    for boundary in ["model", "approval", "started", "check_ready", "check_started", "reporter"] {
        let home = Session::new();
        let mut s = home.open(2, 10);
        let root = s.submit_with_policy("default", "intent", Some("exact"), true).unwrap();
        claim(&mut s, &root);
        handoff(&mut s, &root, &[("b", "task")]).unwrap();
        let child = turn(&mut s);
        let mut stale = child.task.clone();
        let mut approval = None;
        match boundary {
            "model" => (),
            "approval" | "started" => {
                s.request_tool(&child.task, &json!({"op":"shell","command":"model command"})).unwrap();
                let approval_id = s.task(&child.task).unwrap()["approval_id"].as_str().unwrap().to_owned();
                if boundary == "started" {
                    s.approve(&approval_id, true).unwrap();
                    s.claim_tool().unwrap().unwrap();
                }
                approval = Some(approval_id);
            }
            _ => {
                finish(&mut s, &child.task, "complete");
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
fn tree_reports_have_one_durable_claim_shared_budget_and_deterministic_partial_results() {
    for mode in ["budget_one", "report_reserved", "interrupted", "provider_error", "oversize"] {
        let home = Session::new();
        let budget = if mode == "budget_one" { 1 } else { 2 };
        let mut s = home.open(2, budget);
        let root = s.submit("default", "intent").unwrap();
        claim(&mut s, &root);
        if mode == "report_reserved" {
            handoff(&mut s, &root, &[("peer", "branch")]).unwrap();
        } else {
            let output = if mode == "oversize" { "羽".repeat(100_000) } else { "public result".into() };
            finish(&mut s, &root, &output);
        }
        match mode {
            "budget_one" | "oversize" => assert!(s.claim().unwrap().is_none()),
            _ => {
                let reporter = turn(&mut s);
                if mode == "interrupted" {
                    drop(s);
                    s = home.open(1, 1);
                    s.recover().unwrap();
                } else if mode == "report_reserved" {
                    assert_eq!(packet(&reporter)["outcome"], "failed");
                    finish(&mut s, &reporter.task, "budget insufficient for branch");
                } else {
                    s.fail(&reporter.task, "private provider diagnostic").unwrap();
                }
                assert!(s.apply(&reporter.task, &json!({"op":"finish","text":"second attempt"})).is_err());
            }
        }
        let status = s.task(&root).unwrap();
        assert_eq!(status["status"], "failed");
        if mode != "report_reserved" {
            assert!(status["result"].as_str().unwrap().contains("Partial report"));
        }
        assert_eq!(status["turns_used"], if mode == "oversize" { 1 } else { budget });
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
    let mut s = home.open(4, 16);
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
    finish(&mut s, &reporter.task, "all green");

    // Create a sibling root before deleting target.
    let sibling = s.submit_with_model("default", "keep sibling", None, false, &overrides).unwrap();

    // Delete by child ID resolves and deletes the entire root tree and its internal repair agent.
    assert_eq!(s.delete(&child.task).unwrap(), target);
    assert!(s.task(&target).is_err());
    assert!(s.task(&child.task).is_err());
    assert_eq!(
        s.db.query_row("SELECT count(*) FROM agents WHERE name=?1", [&repair_agent], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    assert_eq!(
        s.db.query_row("SELECT count(*) FROM clarifications WHERE root=?1", [&target], |r| r.get::<_, i64>(0)).unwrap(),
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
    finish(&mut s, &sibling, "done");
    let _rep = turn(&mut s); // phase='reporting'
    assert!(s.delete(&sibling).unwrap_err().contains("in flight"));
    assert!(s.delete("nonexistent-id").unwrap_err().contains("unknown task"));
}

#[test]
fn reject_action_queues_bounded_corrections_respects_budget_and_falls_back_for_reporter() {
    let home = Session::new();
    // Budget 4 allows root turn 1 + correction turn 2 + correction turn 3? No, turn 4 is reserved for reporter,
    // so with budget 4 worker can run at most 3 turns total (turns_used = 1, 2 -> budget_left = true; turns_used = 3 -> false).
    // Use budget 6 so MAX_ACTION_REJECTIONS (2) caps before budget runs out.
    let mut s = home.open(2, 6);
    let r = s.submit("default", "plain prompt with non-json text").unwrap();
    claim(&mut s, &r);
    s.reject_action(&r, "err-1").unwrap();
    let t2 = claim(&mut s, &r);
    assert!(chat(&t2).contains("err-1"));
    let last_msg = t2.messages.last().unwrap()["content"].as_str().unwrap();
    assert!(last_msg.contains("Continue task") && last_msg.contains("protocol_error"));
    s.reject_action(&r, "err-2").unwrap();
    let t3 = claim(&mut s, &r);
    assert!(chat(&t3).contains("err-2"));
    // 3rd rejection exceeds MAX_ACTION_REJECTIONS (2) and fails the stage, queuing the reporter.
    s.reject_action(&r, "err-3").unwrap();
    let st = s.task(&r).unwrap();
    assert_eq!(st["stage_status"], "failed");
    assert_eq!(st["stage_error"], "err-3");
    let rep = turn(&mut s);
    assert_eq!(rep.kind, TurnKind::Reporter);
    // Reporter rejection never retries; it falls back immediately to a partial failed report.
    s.reject_action(&rep.task, "reporter err").unwrap();
    let settled = s.task(&r).unwrap();
    assert_eq!(settled["status"], "failed");
    assert!(settled["result"].as_str().unwrap().contains("Partial report"));

    // Worker rejection fails immediately when remaining budget only covers the reserved reporter call.
    let tight_home = Session::new();
    let mut tight = tight_home.open(1, 2);
    let r2 = tight.submit("default", "tight budget").unwrap();
    claim(&mut tight, &r2);
    tight.reject_action(&r2, "no room for retry").unwrap();
    assert_eq!(tight.task(&r2).unwrap()["stage_status"], "failed");
    assert_eq!(tight.task(&r2).unwrap()["stage_error"], "no room for retry");
}
