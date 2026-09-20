//! Real Git regressions for workspace identity, recovery and result protection.

mod support;

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{isolated_state_home, TestEnv};
use teamagents_core::models::{AgentSpec, WorkspacePolicy};
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::{sessions, workspace};

fn environment(tag: &str) -> TestEnv {
    let mut env = isolated_state_home(tag);
    env.set("GIT_CONFIG_NOSYSTEM", "1");
    env.set("GIT_CONFIG_GLOBAL", "/dev/null");
    env
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn project(root: &Path) -> PathBuf {
    let path = root.join("project");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("README.md"), "original\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "base"]);
    path
}

fn agent(id: &str) -> AgentSpec {
    serde_json::from_value(json!({
        "id": id, "name": id, "role": "worker", "runtime_kind": "deepagents",
        "model_profile": "m", "workspace_policy": "git_worktree",
    }))
    .unwrap()
}

fn session(root: &Path, id: &str, project: &Path) -> PathBuf {
    let path = root.join(id);
    std::fs::create_dir_all(&path).unwrap();
    let core = CoreClient::open(path.join("team.db").to_str().unwrap(), id).unwrap();
    core.call("create_session", json!({"session_id": id, "cwd": project})).unwrap();
    path
}

#[test]
fn reopening_worktree_keeps_member_root_when_project_becomes_dirty() {
    let env = environment("worktree-dirty-resume");
    let project = project(&env);
    let member = env.join("s1/members/dev");
    let first = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    std::fs::write(first.path.join("draft.txt"), "member progress\n").unwrap();
    std::fs::write(project.join("README.md"), "user changes\n").unwrap();

    let resumed = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    assert_eq!(resumed.policy, WorkspacePolicy::GitWorktree);
    assert_eq!(resumed.path, first.path);
    assert_eq!(resumed.branch, first.branch);
    assert_eq!(std::fs::read_to_string(resumed.path.join("draft.txt")).unwrap(), "member progress\n");
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "user changes\n");

    // The recorded baseline must not follow a subsequently advanced project HEAD.
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "user changes"]);
    let resumed = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    assert_eq!(resumed.base_commit, first.base_commit);
}

#[test]
fn reopening_worktree_refuses_a_different_repository_and_damaged_git_metadata() {
    let env = environment("worktree-identity");
    let project = project(&env);
    let member = env.join("s1/members/dev");
    let first = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    let other = self::project(&env.join("other"));
    assert!(workspace::prepare(&agent("dev"), &other, &member).is_err());
    assert!(workspace::prepare(&agent("dev"), &env.join("missing"), &member).is_err());

    std::fs::write(first.path.join(".git"), "gitdir: /nonexistent/teamagents-worktree\n").unwrap();
    assert!(workspace::prepare(&agent("dev"), &project, &member).is_err());
    assert!(!workspace::cleanup(&first, &project, false).0);
    assert!(first.path.join("README.md").exists());
}

#[test]
fn different_sessions_get_distinct_member_branches() {
    let env = environment("worktree-distinct-sessions");
    let project = project(&env);
    let first = workspace::prepare(&agent("dev"), &project, &env.join("s1/members/dev")).unwrap();
    let second = workspace::prepare(&agent("dev"), &project, &env.join("s2/members/dev")).unwrap();
    assert_ne!(first.branch, second.branch);
    assert_ne!(first.path, second.path);
}

#[test]
fn cleanup_protects_the_actual_detached_head_until_merged() {
    let env = environment("worktree-detached");
    let project = project(&env);
    let work = workspace::prepare(&agent("dev"), &project, &env.join("s1/members/dev")).unwrap();
    git(&work.path, &["checkout", "--detach", "-q"]);
    std::fs::write(work.path.join("result.txt"), "detached result\n").unwrap();
    git(&work.path, &["add", "."]);
    git(&work.path, &["commit", "-q", "-m", "detached result"]);
    let head = git(&work.path, &["rev-parse", "HEAD"]);

    let (removed, reason) = workspace::cleanup(&work, &project, false);
    assert!(!removed, "unmerged detached HEAD was deleted: {reason}");
    assert_eq!(git(&work.path, &["rev-parse", "HEAD"]), head);
    assert!(work.path.join("result.txt").exists());
    git(&project, &["merge", "--ff-only", &head]);
    let (removed, reason) = workspace::cleanup(&work, &project, false);
    assert!(removed, "merged detached worktree should be removable: {reason}");
    assert!(!work.path.exists());
    assert_eq!(std::fs::read_to_string(project.join("result.txt")).unwrap(), "detached result\n");
}

#[test]
fn dirty_detection_fails_closed_on_git_errors_and_hidden_untracked_files() {
    let env = environment("worktree-status");
    let project = project(&env);
    git(&project, &["config", "status.showUntrackedFiles", "no"]);
    std::fs::write(project.join("user-input.txt"), "must not disappear from task input\n").unwrap();
    assert!(workspace::is_dirty(&project), "Git user preferences must not hide task inputs");
    assert!(workspace::is_dirty(&env.join("missing")), "failed status is not a clean repository");
}

#[test]
fn cleanup_never_deletes_a_user_branch_after_member_switches_to_it() {
    let env = environment("worktree-user-branch");
    let project = project(&env);
    let member = env.join("s1/members/dev");
    let work = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    git(&work.path, &["checkout", "-q", "-b", "user-feature"]);
    let reopened = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    let (removed, reason) = workspace::cleanup(&reopened, &project, false);
    assert!(removed, "{reason}");
    assert!(!git(&project, &["branch", "--list", "user-feature"]).is_empty(), "user branch was deleted");
}

#[test]
fn deleting_a_session_preflights_every_worktree_before_removing_any() {
    let env = environment("worktree-delete-preflight");
    let project = project(&env);
    let root = env.join("sessions");
    let session = session(&root, "s1", &project);
    let clean = workspace::prepare(&agent("a"), &project, &session.join("members/a")).unwrap();
    let dirty = workspace::prepare(&agent("z"), &project, &session.join("members/z")).unwrap();
    std::fs::write(dirty.path.join("draft.txt"), "keep\n").unwrap();

    assert!(sessions::delete_session("s1", Some(&root)).is_err());
    assert!(clean.path.exists(), "a refused deletion must not partially remove other member workspaces");
    assert!(dirty.path.join("draft.txt").exists());
    assert!(session.join("team.db").exists());
}

#[test]
fn archiving_worktrees_repairs_registration_and_preserves_unmerged_results() {
    let env = environment("worktree-archive");
    let project = project(&env);
    let root = env.join("sessions");
    let session = session(&root, "s1", &project);
    let work = workspace::prepare(&agent("dev"), &project, &session.join("members/dev")).unwrap();
    std::fs::write(work.path.join("result.txt"), "deliverable\n").unwrap();
    git(&work.path, &["add", "."]);
    git(&work.path, &["commit", "-q", "-m", "member result"]);
    let head = git(&work.path, &["rev-parse", "HEAD"]);

    let archived = PathBuf::from(sessions::archive_session("s1", Some(&root)).unwrap());
    let moved = archived.join("members/dev/work");
    let registered = git(&project, &["worktree", "list", "--porcelain"]);
    assert!(registered.contains(moved.to_str().unwrap()), "stale registration after archive: {registered}");
    assert!(!registered.contains(work.path.to_str().unwrap()), "old path is still registered: {registered}");
    assert_eq!(git(&moved, &["rev-parse", "HEAD"]), head);
    assert!(sessions::delete_session("s1", Some(&root.join("archived"))).is_err());
    assert!(moved.join("result.txt").exists());
    git(&project, &["merge", "--ff-only", &head]);
    sessions::delete_session("s1", Some(&root.join("archived"))).unwrap();
    assert!(!archived.exists());
}

#[test]
fn archive_collision_preserves_both_sessions_instead_of_overwriting() {
    let env = environment("worktree-archive-collision");
    let project = project(&env);
    let root = env.join("sessions");
    let active = session(&root, "s1", &project);
    let archived = session(&root.join("archived"), "s1", &project);
    std::fs::write(archived.join("irreplaceable.txt"), "previous result\n").unwrap();

    assert!(sessions::archive_session("s1", Some(&root)).is_err());
    assert!(active.join("team.db").exists());
    assert_eq!(std::fs::read_to_string(archived.join("irreplaceable.txt")).unwrap(), "previous result\n");
}

#[test]
fn old_sessions_without_origin_metadata_resume_but_keep_their_branch() {
    let env = environment("worktree-legacy");
    let project = project(&env);
    let member = env.join("s1/members/dev");
    let work = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    let branch = work.branch.clone().unwrap();
    std::fs::remove_file(member.join("worktree.json")).unwrap();
    std::fs::write(project.join("README.md"), "user changes\n").unwrap();

    let resumed = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    assert_eq!(resumed.path, work.path);
    let (removed, reason) = workspace::cleanup(&resumed, &project, false);
    assert!(removed, "{reason}");
    assert!(!git(&project, &["branch", "--list", &branch]).is_empty());
}

#[test]
fn manually_moved_worktrees_are_repaired_on_resume_and_missing_markers_block_deletion() {
    let env = environment("worktree-move-resume");
    let project = project(&env);
    let root = env.join("sessions");
    let original = session(&root, "s1", &project);
    let first = workspace::prepare(&agent("dev"), &project, &original.join("members/dev")).unwrap();
    let moved = root.join("s2");
    std::fs::rename(&original, &moved).unwrap();

    let resumed = workspace::prepare(&agent("dev"), &project, &moved.join("members/dev")).unwrap();
    let registered = git(&project, &["worktree", "list", "--porcelain"]);
    assert!(registered.contains(resumed.path.to_str().unwrap()));
    assert!(!registered.contains(first.path.to_str().unwrap()));
    std::fs::remove_file(resumed.path.join(".git")).unwrap();
    assert!(sessions::delete_session("s2", Some(&root)).is_err());
    assert!(resumed.path.join("README.md").exists());
    let result_backup = env.join("member-backup");
    std::fs::rename(&resumed.path, &result_backup).unwrap();
    assert!(workspace::prepare(&agent("dev"), &project, &moved.join("members/dev")).is_err());
    assert!(!resumed.path.exists(), "a missing recorded worktree must not be silently recreated");
    assert!(result_backup.join("README.md").exists());
}

#[test]
fn locked_or_corrupt_members_prevent_partial_session_deletion() {
    let env = environment("worktree-delete-locked");
    let project = project(&env);
    let root = env.join("sessions");
    let session = session(&root, "s1", &project);
    let clean = workspace::prepare(&agent("a"), &project, &session.join("members/a")).unwrap();
    let protected = workspace::prepare(&agent("z"), &project, &session.join("members/z")).unwrap();
    git(&project, &["worktree", "lock", protected.path.to_str().unwrap()]);

    assert!(sessions::delete_session("s1", Some(&root)).is_err());
    assert!(clean.path.exists() && protected.path.exists());
    git(&project, &["worktree", "unlock", protected.path.to_str().unwrap()]);
    std::fs::write(protected.path.join(".git"), "invalid metadata\n").unwrap();
    assert!(sessions::delete_session("s1", Some(&root)).is_err());
    assert!(sessions::archive_session("s1", Some(&root)).is_err());
    assert!(clean.path.exists() && protected.path.exists());
    assert!(session.join("team.db").exists());
}

#[test]
fn member_git_checks_ignore_inherited_git_environment_and_fsmonitor_hooks() {
    let mut env = environment("worktree-git-environment");
    let project = project(&env);
    let other = self::project(&env.join("other"));
    let member = env.join("s1/members/dev");
    let work = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    let marker = project.join("monitor-ran");
    git(&project, &["config", "core.fsmonitor", &format!("touch {}", marker.display())]);
    env.set("GIT_DIR", other.join(".git"));
    env.set("GIT_WORK_TREE", &other);
    env.set("GIT_INDEX_FILE", other.join(".git/index"));
    env.set("GIT_COMMON_DIR", other.join(".git"));

    assert!(!workspace::is_dirty(&project));
    let resumed = workspace::prepare(&agent("dev"), &project, &member).unwrap();
    assert_eq!(resumed.path, work.path);
    assert!(!marker.exists(), "workspace inspection must not launch a configured fsmonitor");
}

#[test]
fn session_mutations_respect_a_live_session_lock() {
    let env = environment("worktree-mutation-lock");
    let project = project(&env);
    let root = teamagents_engine::config::sessions_dir();
    let session = session(&root, "s1", &project);
    let work = workspace::prepare(&agent("dev"), &project, &session.join("members/dev")).unwrap();
    let lock = sessions::acquire_session_lock("s1").unwrap();
    assert!(sessions::archive_session("s1", Some(&root)).is_err());
    assert!(sessions::delete_session("s1", Some(&root)).is_err());
    assert!(work.path.exists());
    drop(lock);
    sessions::delete_session("s1", Some(&root)).unwrap();
    assert!(!session.exists());
}

#[test]
fn failed_archive_repair_rolls_back_and_releases_the_session_lock() {
    use std::os::unix::fs::PermissionsExt;

    let mut env = environment("worktree-archive-rollback");
    let project = project(&env);
    let root = env.join("sessions");
    let source = session(&root, "s1", &project);
    let first = workspace::prepare(&agent("a"), &project, &source.join("members/a")).unwrap();
    let second = workspace::prepare(&agent("z"), &project, &source.join("members/z")).unwrap();
    let marker = env.join("repair-failed");
    let ready = env.join("mutation-lock-checked");
    let wrapper_dir = env.join("bin");
    std::fs::create_dir_all(&wrapper_dir).unwrap();
    let actual_git = teamagents_engine::tools::which("git").unwrap();
    let wrapper = wrapper_dir.join("git");
    // Stop once at the second repaired member, after the first link changed.
    // While stopped, another mutation must see the moved inode's live lock.
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
case "$*" in
  *"worktree repair "*"/archived/"*"/members/z/work")
    if [ ! -f "$TA_TEST_FAILURE" ]; then
      : > "$TA_TEST_FAILURE"
      i=0
      while [ ! -f "$TA_TEST_READY" ] && [ "$i" -lt 200 ]; do
        sleep 0.01
        i=$((i + 1))
      done
      echo 'injected repair failure' >&2
      exit 1
    fi
    ;;
esac
exec "$TA_TEST_REAL_GIT" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.set("TA_TEST_REAL_GIT", actual_git);
    env.set("TA_TEST_FAILURE", &marker);
    env.set("TA_TEST_READY", &ready);
    let mut paths = vec![wrapper_dir];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    env.set("PATH", std::env::join_paths(paths).unwrap());

    let worker_root = root.clone();
    let pending = std::thread::spawn(move || sessions::archive_session("s1", Some(&worker_root)));
    let reached = support::wait_for(|| marker.exists(), 5000);
    let locked = reached.then(|| sessions::delete_session("s1", Some(&root.join("archived"))));
    std::fs::write(&ready, "continue").unwrap();
    let error = pending.join().unwrap().unwrap_err();
    assert!(reached, "failure injection was never reached");
    let locked = locked.unwrap().unwrap_err();
    assert!(locked.contains("already running"), "{locked}");
    assert!(error.contains("injected repair failure"), "{error}");
    assert!(source.join("team.db").exists());
    assert!(!root.join("archived/s1").exists());
    let registered = git(&project, &["worktree", "list", "--porcelain"]);
    assert!(registered.contains(first.path.to_str().unwrap()));
    assert!(registered.contains(second.path.to_str().unwrap()));
    assert!(!registered.contains("/archived/"));

    sessions::archive_session("s1", Some(&root)).unwrap();
    sessions::delete_session("s1", Some(&root.join("archived"))).unwrap();
}

#[test]
fn session_cleanup_preserves_ignored_results_until_explicitly_removed() {
    let env = environment("worktree-ignored-results");
    let project = project(&env);
    std::fs::write(project.join(".gitignore"), "private-result.txt\n").unwrap();
    git(&project, &["add", ".gitignore"]);
    git(&project, &["commit", "-q", "-m", "ignore local outputs"]);
    let root = env.join("sessions");
    let session = session(&root, "s1", &project);
    let work = workspace::prepare(&agent("dev"), &project, &session.join("members/dev")).unwrap();
    std::fs::write(work.path.join("private-result.txt"), "untracked deliverable\n").unwrap();

    assert!(sessions::delete_session("s1", Some(&root)).is_err(), "ignored does not mean safe to delete");
    assert_eq!(std::fs::read_to_string(work.path.join("private-result.txt")).unwrap(), "untracked deliverable\n");
    std::fs::remove_file(work.path.join("private-result.txt")).unwrap();
    let (removed, reason) = workspace::cleanup(&work, &project, false);
    assert!(removed, "{reason}");
    assert!(!session.join("members/dev/worktree.json").exists(), "successful cleanup removes stale origin");
    // A later session deletion remains possible after a completed member cleanup.
    sessions::delete_session("s1", Some(&root)).unwrap();
}

#[test]
fn resumed_session_file_tools_write_to_the_member_worktree_not_dirty_project() {
    use std::collections::HashMap;
    use teamagents_engine::scripted::Step;
    use teamagents_engine::session::{open_session, OpenOptions};

    let env = environment("worktree-session-executor");
    let project = project(&env);
    let mut leader = agent("leader");
    leader.role = "leader".into();
    leader.tool_bindings = vec!["files".into()];
    let spec = json!({"leader_id":"leader", "agents":[leader]});
    let catalog = serde_json::from_value(support::test_catalog()).unwrap();
    // Initialize through the production Chat runner without making a model call.
    let initial = open_session(OpenOptions {
        cwd: Some(project.clone()),
        session_id: Some("s1".into()),
        initial_spec: Some(spec),
        catalog: Some(catalog),
        ..Default::default()
    })
    .unwrap();
    let member = sessions::session_paths("s1").base.join("members/leader");
    assert!(member.join("work/.git").is_file());
    initial.close();
    drop(initial);
    std::fs::write(project.join("README.md"), "user changes\n").unwrap();

    let resumed = open_session(OpenOptions {
        cwd: Some(project.clone()),
        session_id: Some("s1".into()),
        catalog: Some(serde_json::from_value(support::test_catalog()).unwrap()),
        scripts: Some(HashMap::from([(
            "leader".to_string(),
            vec![Step::Call("write_file".into(), json!({"path":"result.txt", "content":"member result\n"})), Step::End],
        )])),
        ..Default::default()
    })
    .unwrap();
    resumed.runtime.start();
    resumed.runtime.user_message("continue", false).unwrap();
    let settled = resumed.runtime.settle(5);
    resumed.close();
    assert!(settled);
    assert_eq!(std::fs::read_to_string(member.join("work/result.txt")).unwrap(), "member result\n");
    assert!(!project.join("result.txt").exists());
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "user changes\n");
}
