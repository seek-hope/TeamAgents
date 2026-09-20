//! Workspace review tests use actual bytes and Git, not tool-result strings.

mod support;

use serde_json::{json, Value as Json};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{isolated_state_home, TestEnv};
use teamagents_engine::review;

fn fixture(tag: &str) -> (TestEnv, PathBuf, PathBuf) {
    let mut env = isolated_state_home(tag);
    env.set("GIT_CONFIG_GLOBAL", "/dev/null");
    env.set("GIT_CONFIG_NOSYSTEM", "1");
    let root = env.join("project");
    let session = env.join("sessions/s1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&session).unwrap();
    (env, root, session)
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn changes(report: &Json) -> Vec<&str> {
    report["changes"].as_array().unwrap().iter().map(|c| c["path"].as_str().unwrap()).collect()
}

fn lines(report: &Json) -> String {
    report["detail"]["lines"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>().join("\n")
}

#[test]
fn dirty_inputs_are_the_baseline_and_all_edit_batches_survive_reopen() {
    let (_env, root, session) = fixture("review-dirty");
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "committed\n").unwrap();
    std::fs::write(root.join("remove.txt"), "delete me\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    std::fs::write(root.join("a.txt"), "user dirty input\n").unwrap();
    std::fs::write(root.join("notes.txt"), "user untracked input\n").unwrap();
    review::ensure(&session, &root).unwrap();
    let before = review::report(&session, &root, None, 0).unwrap();
    assert!(changes(&before).is_empty());

    // Multiple write sources/batches: no tool receipt is involved.
    std::fs::write(root.join("a.txt"), "first edit\n").unwrap();
    let out = Command::new("sh")
        .current_dir(&root)
        .args(["-c", "printf 'final edit\n' > a.txt; printf 'shell output\n' > shell.txt"])
        .status()
        .unwrap();
    assert!(out.success());
    std::fs::remove_file(root.join("remove.txt")).unwrap();
    std::fs::rename(root.join("notes.txt"), root.join("renamed.txt")).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "external backend result"]);
    review::ensure(&session, &root).unwrap(); // Reopen must not recapture HEAD/current bytes.
    let report = review::report(&session, &root, Some("a.txt"), 0).unwrap();
    assert_eq!(report["baseline_at"], before["baseline_at"]);
    assert_eq!(changes(&report), ["a.txt", "notes.txt", "remove.txt", "renamed.txt", "shell.txt"]);
    assert!(lines(&report).contains("-user dirty input"));
    assert!(lines(&report).contains("+final edit"));
    assert!(!lines(&report).contains("committed"));
    assert_eq!(report["complete"], true);
}

#[test]
fn non_git_files_have_diffs_for_add_delete_mode_and_binary_changes() {
    let (_env, root, session) = fixture("review-nongit");
    std::fs::write(root.join("run.sh"), "echo hi\n").unwrap();
    std::fs::write(root.join("gone.txt"), "old\n").unwrap();
    std::fs::write(root.join("binary.bin"), [0, 1, 2]).unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(root.join("gone.txt")).unwrap();
    std::fs::write(root.join("new.txt"), "新文件\n").unwrap();
    std::fs::write(root.join("binary.bin"), [0, 1, 3]).unwrap();
    let report = review::report(&session, &root, Some("gone.txt"), 0).unwrap();
    assert_eq!(changes(&report), ["binary.bin", "gone.txt", "new.txt", "run.sh"]);
    assert!(lines(&report).contains("-old"));
    let report = review::report(&session, &root, Some("new.txt"), 0).unwrap();
    assert!(lines(&report).contains("+新文件"));
    let report = review::report(&session, &root, Some("binary.bin"), 0).unwrap();
    assert!(lines(&report).contains("二进制"));
    assert_ne!(report["changes"][0]["before"]["hash"], report["changes"][0]["after"]["hash"]);
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o1755)).unwrap();
    let report = review::report(&session, &root, Some("run.sh"), 0).unwrap();
    assert!(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "run.sh" && c["after"]["mode"] == 0o1755));
    assert!(lines(&report).is_empty());
}

#[test]
fn symlinks_and_replaced_directories_never_expose_external_file_contents() {
    let (env, root, session) = fixture("review-links");
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/secret.txt"), "inside\n").unwrap();
    let outside = env.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "EXTERNAL-PRIVATE-CONTENT\n").unwrap();
    symlink(outside.join("secret.txt"), root.join("link")).unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::remove_file(root.join("link")).unwrap();
    symlink("sub/secret.txt", root.join("link")).unwrap();
    let report = review::report(&session, &root, Some("link"), 0).unwrap();
    assert!(lines(&report).contains("+sub/secret.txt"));
    assert!(!report.to_string().contains("EXTERNAL-PRIVATE-CONTENT"));
    std::fs::rename(root.join("sub"), env.join("original-sub")).unwrap();
    symlink(&outside, root.join("sub")).unwrap();
    let report = review::report(&session, &root, Some("sub/secret.txt"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert!(!report.to_string().contains("EXTERNAL-PRIVATE-CONTENT"));
    assert!(review::report(&session, &root, Some("../outside/secret.txt"), 0).is_err());
    assert!(review::report(&session, &root, Some(outside.to_str().unwrap()), 0).is_err());
    use std::os::unix::ffi::OsStrExt;
    symlink(std::ffi::OsStr::from_bytes(b"invalid-\xff"), root.join("non-utf8-link")).unwrap();
    let report = review::report(&session, &root, Some("non-utf8-link"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert!(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "non-utf8-link" && c["after"]["problem"].as_str().unwrap().contains("UTF-8")));
}

#[test]
fn ignored_files_and_session_private_state_are_not_copied_or_disclosed() {
    let (_env, root, _) = fixture("review-scope");
    git(&root, &["init", "-q"]);
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(root.join("ignored.txt"), "not-in-review\n").unwrap();
    let session = root.join("session");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("private.txt"), "member private context\n").unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::write(root.join("ignored.txt"), "changed\n").unwrap();
    let report = review::report(&session, &root, None, 0).unwrap();
    assert!(changes(&report).is_empty());
    assert_eq!(report["scope"], "git_tracked_and_unignored");
    assert!(review::report(&session, &root, Some("ignored.txt"), 0).is_err());
    assert!(review::report(&session, &root, Some("session/private.txt"), 0).is_err());
}

#[test]
fn shared_members_reuse_one_baseline_and_sessions_stay_independent() {
    let (_env, root, session) = fixture("review-shared");
    let a = session.join("members/a");
    let b = session.join("members/b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(root.join("a"), "original\n").unwrap();
    review::register(&session, &a, &root).unwrap();
    std::fs::write(root.join("a"), "later\n").unwrap();
    review::register(&session, &b, &root).unwrap();
    assert_eq!(review::registered_root(&a).unwrap(), review::registered_root(&b).unwrap());
    assert_eq!(std::fs::read_dir(session.join("reviews")).unwrap().count(), 1);
    assert!(lines(&review::report(&session, &root, Some("a"), 0).unwrap()).contains("-original"));
    let second = session.with_file_name("s2");
    review::ensure(&second, &root).unwrap();
    assert!(changes(&review::report(&second, &root, None, 0).unwrap()).is_empty());
}

#[test]
fn large_special_and_long_line_outputs_report_incompleteness_without_blocking() {
    let (_env, root, session) = fixture("review-limits");
    std::fs::write(root.join("big"), vec![b'a'; 2 * 1024 * 1024 + 1]).unwrap();
    assert!(Command::new("mkfifo").arg(root.join("pipe")).status().unwrap().success());
    std::fs::write(root.join("long"), "before\n").unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::write(root.join("long"), "x".repeat(6000)).unwrap();
    let report = review::report(&session, &root, Some("long"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert_eq!(report["detail"]["truncated"], true);
    assert!(changes(&report).contains(&"big") && changes(&report).contains(&"pipe"));
    assert!(lines(&report).len() < 5000);
}

#[test]
fn paged_diffs_cover_the_whole_change_and_do_not_trust_diff_configuration() {
    let (mut env, root, session) = fixture("review-pages");
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("file.txt"), "old\n").unwrap();
    review::ensure(&session, &root).unwrap();
    let marker = root.join("external-was-run");
    git(&root, &["config", "diff.external", &format!("touch {}", marker.display())]);
    git(&root, &["config", "core.fsmonitor", &format!("touch {}", marker.display())]);
    env.set("GIT_EXTERNAL_DIFF", format!("touch {}", marker.display()));
    let content: String = (0..400).map(|n| format!("line-{n}\n")).collect();
    std::fs::write(root.join("file.txt"), &content).unwrap();
    let mut all = String::new();
    let mut offset = 0;
    loop {
        let report = review::report(&session, &root, Some("file.txt"), offset).unwrap();
        assert!(report["detail"]["lines"].as_array().unwrap().len() <= 120);
        all.push_str(&lines(&report));
        let Some(next) = report["detail"]["next_offset"].as_u64() else { break };
        assert!(next as usize > offset);
        offset = next as usize;
    }
    assert!(all.contains("+line-0") && all.contains("+line-399"));
    assert!(!marker.exists());
    assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), content);
}

#[test]
fn tampered_baseline_and_failed_initial_capture_never_become_clean_evidence() {
    let (_env, root, session) = fixture("review-corrupt");
    std::fs::write(root.join("file"), "old\n").unwrap();
    review::ensure(&session, &root).unwrap();
    let base = std::fs::read_dir(session.join("reviews")).unwrap().next().unwrap().unwrap().path();
    let blob = std::fs::read_dir(base.join("blobs")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::metadata(&blob).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(&blob, "wrong\n").unwrap();
    std::fs::write(root.join("file"), "new\n").unwrap();
    assert!(review::report(&session, &root, Some("file"), 0).unwrap_err().contains("哈希"));
    let manifest_path = base.join("manifest.json");
    let mut manifest: Json = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"]["file"]["hash"] = Json::Null;
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(review::report(&session, &root, None, 0).unwrap_err().contains("损坏"));
    std::fs::remove_file(base.join("manifest.json")).unwrap();
    assert!(review::ensure(&session, &root).unwrap_err().contains("不会"));
    assert!(review::report(&session, &root, None, 0).is_err());
}

#[test]
fn git_subdirectory_review_is_scoped_and_refresh_can_show_reverted_changes() {
    let (_env, root, session) = fixture("review-subdir");
    git(&root, &["init", "-q"]);
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("outside"), "out\n").unwrap();
    std::fs::write(root.join("sub/a"), "old\n").unwrap();
    review::ensure(&session, &root.join("sub")).unwrap();
    std::fs::write(root.join("outside"), "outside change\n").unwrap();
    std::fs::write(root.join("sub/a"), "new\n").unwrap();
    let report = review::report(&session, &root.join("sub"), Some("a"), 0).unwrap();
    assert_eq!(changes(&report), ["a"]);
    assert!(lines(&report).contains("+new"));
    std::fs::write(root.join("sub/a"), "old\n").unwrap();
    let reverted = review::report(&session, &root.join("sub"), Some("a"), 0).unwrap();
    assert!(changes(&reverted).is_empty());
    assert!(reverted["detail"]["lines"].as_array().unwrap().is_empty());
}

#[test]
fn production_session_captures_before_writes_and_resumes_the_same_review() {
    use teamagents_engine::session::{open_session, OpenOptions};
    let (_env, root, _) = fixture("review-session");
    std::fs::write(root.join("file"), "input\n").unwrap();
    let open = || {
        open_session(OpenOptions {
        cwd:Some(root.clone()), session_id:Some("s1".into()),
        catalog:Some(serde_json::from_value(support::test_catalog()).unwrap()),
        initial_spec:Some(json!({"leader_id":"leader","agents":[support::member("leader","leader"),support::member("dev","worker")]})),
        ..Default::default()
    }).unwrap()
    };
    let first = open();
    std::fs::write(root.join("file"), "changed\n").unwrap();
    let evidence = first.review("dev", Some("file"), 0).unwrap();
    assert_eq!(evidence["shared"], true);
    assert!(lines(&evidence).contains("-input"));
    assert!(first.review("../leader", None, 0).is_err());
    first.close();
    drop(first);
    let resumed = open();
    let after = resumed.review("leader", Some("file"), 0).unwrap();
    assert_eq!(after["baseline_at"], evidence["baseline_at"]);
    assert_eq!(lines(&after), lines(&evidence));
    resumed.close();
}

#[test]
fn isolated_and_worktree_roots_review_their_files_not_sibling_private_context() {
    use teamagents_core::models::AgentSpec;
    use teamagents_engine::workspace;
    let (_env, root, session) = fixture("review-isolated");
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    for (id, policy) in [("iso", "isolated"), ("dev", "git_worktree")] {
        let member = session.join("members").join(id);
        let agent: AgentSpec = serde_json::from_value(json!({
            "id":id,"name":id,"role":"worker","runtime_kind":"deepagents","model_profile":"m","workspace_policy":policy
        }))
        .unwrap();
        let work = workspace::prepare(&agent, &root, &member).unwrap();
        std::fs::write(member.join("chat_tree.json"), "private conversation").unwrap();
        review::register(&session, &member, &work.path).unwrap();
        std::fs::write(work.path.join("result.txt"), "member deliverable\n").unwrap();
        let report = review::report(&session, &work.path, Some("result.txt"), 0).unwrap();
        assert_eq!(changes(&report), ["result.txt"]);
        assert!(lines(&report).contains("+member deliverable"));
        assert!(!report.to_string().contains("private conversation"));
        assert!(review::report(&session, &work.path, Some("../chat_tree.json"), 0).is_err());
    }
}

#[test]
fn corrupt_git_index_and_file_count_limit_are_explicit_not_clean_reviews() {
    let (_env, root, session) = fixture("review-git-failure");
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("one"), "a").unwrap();
    git(&root, &["add", "."]);
    review::ensure(&session, &root).unwrap();
    std::fs::write(root.join(".git/index"), "invalid index").unwrap();
    assert!(review::report(&session, &root, None, 0).is_err());

    // A fresh independent root avoids the intentionally damaged repository.
    let plain = session.with_file_name("many-files");
    std::fs::create_dir_all(&plain).unwrap();
    for n in 0..5001 {
        std::fs::write(plain.join(format!("f{n:05}")), "").unwrap();
    }
    review::ensure(&session, &plain).unwrap();
    let report = review::report(&session, &plain, None, 0).unwrap();
    assert_eq!(report["complete"], false);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn pagination_rejects_changes_between_pages_until_explicit_refresh() {
    let (_env, root, session) = fixture("review-revision");
    std::fs::write(root.join("file"), "old\n").unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::write(root.join("file"), "changed\n".repeat(400)).unwrap();
    let first = review::report(&session, &root, Some("file"), 0).unwrap();
    let revision = first["revision"].as_str().unwrap();
    let second = review::report_checked(&session, &root, Some("file"), 120, Some(revision)).unwrap();
    assert_eq!(second["detail"]["offset"], 120);
    std::fs::write(root.join("other"), "also modified\n").unwrap();
    assert!(review::report_checked(&session, &root, Some("file"), 240, Some(revision))
        .unwrap_err()
        .contains("分页期间变化"));
    let refreshed = review::report_checked(&session, &root, Some("file"), 0, None).unwrap();
    assert_ne!(refreshed["revision"], revision);
}

#[test]
fn a_parent_symlink_cannot_redirect_baseline_paths_into_private_state() {
    let (_env, root, _) = fixture("review-private-alias");
    let session = root.join("session");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(root.join("sub/history"), "input\n").unwrap();
    std::fs::write(session.join("history"), "PRIVATE-CONTEXT\n").unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::rename(root.join("sub"), root.join("saved")).unwrap();
    symlink(&session, root.join("sub")).unwrap();
    let report = review::report(&session, &root, Some("sub/history"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert!(!report.to_string().contains("PRIVATE-CONTEXT"));
    assert!(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "sub/history" && c["status"] == "unreviewed"));
}

#[test]
fn unknown_baseline_paths_are_not_mislabeled_as_new_files() {
    let (_env, root, session) = fixture("review-unknown-origin");
    std::fs::write(root.join("observed"), "old\n").unwrap();
    review::ensure(&session, &root).unwrap();
    let base = std::fs::read_dir(session.join("reviews")).unwrap().next().unwrap().unwrap().path();
    let path = base.join("manifest.json");
    // Simulate a persisted timed-out first scan without waiting ten seconds.
    let mut manifest: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["warnings"] = json!(["基线读取超过 10 秒，后续文件未纳入基线"]);
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    std::fs::write(root.join("unobserved"), "not proven new\n").unwrap();
    let report = review::report(&session, &root, Some("unobserved"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert_eq!(report["changes"][0]["status"], "unreviewed");
    assert!(!lines(&report).contains("+not proven new"));
}

#[test]
fn parent_aliases_cannot_read_git_metadata_as_old_workspace_files() {
    let (_env, root, session) = fixture("review-git-alias");
    git(&root, &["init", "-q"]);
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/secret"), "original\n").unwrap();
    std::fs::write(root.join(".git/secret"), "GIT-PRIVATE-DATA\n").unwrap();
    review::ensure(&session, &root).unwrap();
    std::fs::rename(root.join("sub"), root.join("saved")).unwrap();
    symlink(root.join(".git"), root.join("sub")).unwrap();
    let report = review::report(&session, &root, Some("sub/secret"), 0).unwrap();
    assert_eq!(report["complete"], false);
    assert!(!report.to_string().contains("GIT-PRIVATE-DATA"));
    assert!(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "sub/secret" && c["status"] == "unreviewed"));
}
