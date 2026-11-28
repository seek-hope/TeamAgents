use super::*;
use crate::tools::{execute, execute_with_mode};
use serde_json::json;
use std::os::unix::{fs::symlink, net::UnixListener};

fn shell(root: &Path, cmd: &str) -> Result<String> {
    execute_with_mode(root, &json!({"op":"shell","command":cmd}), true)
}
fn shell_direct(root: &Path, cmd: &str) -> Result<String> {
    execute(root, &json!({"op":"shell","command":cmd}))
}
fn read(root: &Path, name: &str) -> Result<String> {
    execute(root, &json!({"op":"read_file","path":name}))
}
fn create(root: &Path, name: &str) -> Result<String> {
    execute(root, &json!({"op":"edit_file","path":name,"old":"","new":"new"}))
}

#[test]
fn native_and_staged_tools_protect_checkout_worktree_and_aliases() {
    let t = Scratch::new().unwrap();
    let ws = t.0.join("checkout");
    let git = ws.join(".git");
    fs::create_dir_all(git.join("hooks")).unwrap();
    fs::write(git.join("config"), "fixture").unwrap();
    let nested = ws.join("module");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join(".git"), "gitdir: ../.git/modules/module\n").unwrap();
    fs::create_dir_all(ws.join("separate-admin/objects")).unwrap();
    fs::write(ws.join("separate-admin/HEAD"), "ref: refs/heads/main\n").unwrap();
    for name in [".git/config", "module/.git", "separate-admin/HEAD", ".GiT/config"] {
        assert!(read(&ws, name).is_err());
        assert!(create(&ws, name).is_err());
    }
    let stage = Stage::new(&ws).unwrap();
    assert!(!stage.scratch.0.join(".git").exists());
    assert!(!stage.scratch.0.join("module/.git").exists());
    assert!(!stage.scratch.0.join("separate-admin").exists());
    fs::write(stage.scratch.0.join("module/.git"), "fixture replacement").unwrap();
    fs::write(stage.scratch.0.join("ordinary"), "published").unwrap();
    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(ws.join("ordinary")).unwrap(), "published");
    assert_eq!(fs::read_to_string(nested.join(".git")).unwrap(), "gitdir: ../.git/modules/module\n");
    assert_eq!(fs::read_to_string(git.join("config")).unwrap(), "fixture");

    let stage = Stage::new(&ws).unwrap();
    fs::remove_dir(stage.scratch.0.join("module")).unwrap();
    assert!(stage.publish().is_err());
    assert!(nested.join(".git").is_file());

    let outside = t.0.join("private");
    fs::write(&outside, "fixture only").unwrap();
    fs::hard_link(&outside, ws.join("ambiguous")).unwrap();
    assert!(read(&ws, "ambiguous").unwrap_err().contains("single-link"));
    assert!(Stage::new(&ws).is_err());
    fs::remove_file(ws.join("ambiguous")).unwrap();
    symlink(&outside, ws.join("ambiguous")).unwrap();
    assert!(Stage::new(&ws).is_err());
    fs::remove_file(ws.join("ambiguous")).unwrap();
    let _socket = UnixListener::bind(ws.join("socket")).unwrap();
    assert!(Stage::new(&ws).is_err());
    assert!(read(&ws, "socket").is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "fixture only");
}

#[test]
fn auth_directory_is_excluded_from_file_tools_and_shell_publication() {
    let ws = Scratch::new().unwrap();
    let name = ".teamagents-auth/profile.json";
    let auth = ws.0.join(".teamagents-auth");
    fs::create_dir(&auth).unwrap();
    fs::write(ws.0.join(name), "fixture").unwrap();
    assert!(read(&ws.0, name).is_err());
    assert!(create(&ws.0, name).is_err());
    assert!(Stage::new(&auth).is_err());
    let stage = Stage::new(&ws.0).unwrap();
    assert!(!stage.scratch.0.join(".teamagents-auth").exists());
    fs::create_dir(stage.scratch.0.join(".teamagents-auth")).unwrap();
    fs::write(stage.scratch.0.join(name), "replacement").unwrap();
    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(ws.0.join(name)).unwrap(), "fixture");
}

#[test]
fn staging_handles_disjoint_changes_and_refuses_stale_publication() {
    let ws = Scratch::new().unwrap();
    fs::write(ws.0.join("a"), "one").unwrap();
    let first = Stage::new(&ws.0).unwrap();
    let second = Stage::new(&ws.0).unwrap();
    fs::write(first.scratch.0.join("a"), "first").unwrap();
    fs::write(second.scratch.0.join("a"), "other").unwrap();
    fs::write(second.scratch.0.join("b"), "not published").unwrap();
    create(&ws.0, "unrelated").unwrap();
    first.publish().unwrap();
    assert!(second.publish().unwrap_err().contains("concurrently"));
    assert_eq!(read(&ws.0, "a").unwrap(), "first");
    assert_eq!(read(&ws.0, "unrelated").unwrap(), "new");
    assert!(read(&ws.0, "b").is_err());
}

#[test]
fn staged_publication_replaces_file_with_populated_directory() {
    let ws = Scratch::new().unwrap();
    fs::write(ws.0.join("item"), "old file").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    let item = stage.scratch.0.join("item");
    fs::remove_file(&item).unwrap();
    fs::create_dir_all(item.join("nested")).unwrap();
    fs::write(item.join("nested/data"), "new contents").unwrap();
    fs::set_permissions(&item, fs::Permissions::from_mode(0o550)).unwrap();

    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(ws.0.join("item/nested/data")).unwrap(), "new contents");
    assert_eq!(fs::metadata(ws.0.join("item")).unwrap().mode() & 0o777, 0o550);
}

#[test]
fn staged_publication_applies_directory_mode_after_child_changes() {
    let ws = Scratch::new().unwrap();
    let host = ws.0.join("existing");
    fs::create_dir(&host).unwrap();
    fs::write(host.join("old"), "old contents").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    let dir = stage.scratch.0.join("existing");
    fs::remove_file(dir.join("old")).unwrap();
    fs::write(dir.join("new"), "new contents").unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();

    stage.publish().unwrap();
    assert_eq!(fs::metadata(&host).unwrap().mode() & 0o777, 0o500);
    assert_eq!(fs::read_to_string(host.join("new")).unwrap(), "new contents");
    assert!(!host.join("old").exists());
}

#[test]
fn staged_publication_directory_recheck_requires_same_identity_and_mode() {
    let ws = Scratch::new().unwrap();
    let dir = ws.0.join("dir");
    let other = ws.0.join("other");
    for path in [&dir, &other] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let entry = |p: &Path| Node::of(&fs::metadata(p).unwrap());
    let expected = entry(&dir);
    assert!(!expected.matches_host(&entry(&other)));
    fs::write(dir.join("child"), "data").unwrap();
    assert!(expected.matches_host(&entry(&dir)));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(!expected.matches_host(&entry(&dir)));
}

#[test]
fn staged_publication_replaces_populated_directory_with_file() {
    let ws = Scratch::new().unwrap();
    fs::create_dir_all(ws.0.join("item/nested")).unwrap();
    fs::write(ws.0.join("item/nested/data"), "old contents").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    let item = stage.scratch.0.join("item");
    fs::remove_dir_all(&item).unwrap();
    fs::write(item, "new file").unwrap();

    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(ws.0.join("item")).unwrap(), "new file");
}

#[test]
fn staged_publication_rejects_replacement_with_stale_descendant() {
    let ws = Scratch::new().unwrap();
    fs::create_dir(ws.0.join("item")).unwrap();
    fs::write(ws.0.join("item/data"), "original").unwrap();
    fs::write(ws.0.join("removed"), "keep me").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    fs::remove_dir_all(stage.scratch.0.join("item")).unwrap();
    fs::write(stage.scratch.0.join("item"), "replacement").unwrap();
    fs::remove_file(stage.scratch.0.join("removed")).unwrap();
    fs::write(ws.0.join("item/data"), "concurrent host edit").unwrap();

    assert!(stage.publish().unwrap_err().contains("concurrently"));
    assert_eq!(fs::read_to_string(ws.0.join("item/data")).unwrap(), "concurrent host edit");
    assert_eq!(fs::read_to_string(ws.0.join("removed")).unwrap(), "keep me");
}

#[test]
fn staged_publication_preserves_host_tree_under_new_cache_marker() {
    let ws = Scratch::new().unwrap();
    let host = ws.0.join("crate/target");
    fs::create_dir_all(host.join("nested")).unwrap();
    fs::write(host.join("nested/data"), "host contents").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    let target = stage.scratch.0.join("crate/target");
    fs::remove_file(target.join("nested/data")).unwrap();
    fs::write(target.join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
    fs::write(target.join("artifact"), "build output").unwrap();
    fs::write(stage.scratch.0.join("crate/output"), "publish me").unwrap();

    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(ws.0.join("crate/output")).unwrap(), "publish me");
    assert_eq!(fs::read_to_string(host.join("nested/data")).unwrap(), "host contents");
    assert!(!host.join("CACHEDIR.TAG").exists());
    assert!(!host.join("artifact").exists());
}

#[test]
fn staged_publication_skips_rebuilt_cache_and_rejects_ordinary_shadow() {
    let ws = Scratch::new().unwrap();
    let host = ws.0.join("target");
    fs::create_dir(&host).unwrap();
    fs::write(host.join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
    fs::write(host.join("data"), "host cache").unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    let target = stage.scratch.0.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
    fs::write(target.join("data"), "rebuilt cache").unwrap();
    fs::write(stage.scratch.0.join("output"), "published").unwrap();
    stage.publish().unwrap();
    assert_eq!(fs::read_to_string(host.join("data")).unwrap(), "host cache");
    assert_eq!(fs::read_to_string(ws.0.join("output")).unwrap(), "published");

    let stage = Stage::new(&ws.0).unwrap();
    fs::write(stage.scratch.0.join("target"), "ordinary file").unwrap();
    fs::remove_file(stage.scratch.0.join("output")).unwrap();
    assert!(stage.publish().unwrap_err().contains("protected"));
    assert_eq!(fs::read_to_string(host.join("data")).unwrap(), "host cache");
    assert_eq!(fs::read_to_string(ws.0.join("output")).unwrap(), "published");
}

#[test]
fn cargo_build_cache_is_omitted_before_scanning_children() {
    let ws = Scratch::new().unwrap();
    fs::create_dir(ws.0.join("target")).unwrap();
    fs::write(ws.0.join("target/CACHEDIR.TAG"), CACHE_TAG).unwrap();
    symlink("unused-fixture", ws.0.join("target/unused")).unwrap();
    let stage = Stage::new(&ws.0).unwrap();
    assert!(!stage.scratch.0.join("target").exists());
    assert!(read(&ws.0, "target/CACHEDIR.TAG").is_err());
    fs::write(stage.scratch.0.join("target"), "fixture").unwrap();
    assert!(stage.publish().is_err());
    assert!(ws.0.join("target/CACHEDIR.TAG").exists());
}

#[test]
fn shell_uses_private_home_process_and_filesystem_views_and_publishes_changes() {
    let private = Scratch::new().unwrap();
    let ws = Scratch::new().unwrap();
    let _socket = UnixListener::bind(private.0.join("control")).unwrap();
    fs::write(private.0.join("private"), "test data").unwrap();
    fs::write(ws.0.join("remove"), "old").unwrap();
    let proc_ns = fs::read_link("/proc/self/ns/pid").unwrap();
    let script = format!("test ! -e {0}/private && test ! -S {0}/control && test ! -e /run/user && test \"$HOME\" = /home/agent && test \"$(readlink /proc/self/ns/pid)\" != '{1}' && printf scratch > \"$HOME/private\" && mkdir out && mv remove out/done && chmod 640 out/done", private.0.display(), proc_ns.display());
    shell(&ws.0, &script).unwrap();
    assert_eq!(read(&ws.0, "out/done").unwrap(), "old");
    assert_eq!(fs::metadata(ws.0.join("out/done")).unwrap().mode() & 0o777, 0o640);
    assert!(!ws.0.join("remove").exists());
    shell(&ws.0, "test ! -e \"$HOME/private\"").unwrap();
}

#[test]
fn rust_builds_from_workspace_sources_without_borrowing_host_cargo_home() {
    let ws = Scratch::new().unwrap();
    fs::create_dir(ws.0.join("src")).unwrap();
    fs::write(ws.0.join("src/main.rs"), "fn main() { println!(\"ok\"); }\n").unwrap();
    fs::write(ws.0.join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n").unwrap();
    // Exercise the installed project channel, not Cargo's mutable host configuration or package cache.
    fs::write(ws.0.join("rust-toolchain.toml"), include_bytes!("../rust-toolchain.toml")).unwrap();
    let output = shell(&ws.0, "test ! -e \"$CARGO_HOME/credentials.toml\" && cargo run --offline --quiet").unwrap();
    assert!(output.contains("stdout:\nok\n"), "{output}");
    assert!(ws.0.join("Cargo.lock").is_file());
    assert!(!ws.0.join("target").exists());
}

#[test]
fn direct_mode_works_beyond_staging_limits_while_protecting_git_and_auth() {
    let private = Scratch::new().unwrap();
    let ws = Scratch::new().unwrap();
    fs::create_dir_all(ws.0.join(".git/hooks")).unwrap();
    fs::write(ws.0.join(".git/config"), "git-fixture").unwrap();
    fs::create_dir_all(ws.0.join(".teamagents-auth")).unwrap();
    fs::write(ws.0.join(".teamagents-auth/profile.json"), "secret-token").unwrap();
    fs::create_dir_all(ws.0.join("admin_repo/objects")).unwrap();
    fs::write(ws.0.join("admin_repo/HEAD"), "ref: refs/heads/main\n").unwrap();
    // Create symlinks inside workspace to verify Direct mode does not fail like Stage copy.
    symlink("target-missing", ws.0.join("sym")).unwrap();

    // Direct shell reads git config, writes normal files, hides .teamagents-auth, and blocks .git/admin writes.
    let out = shell_direct(
        &ws.0,
        "cat .git/config && test ! -e .teamagents-auth/profile.json && echo direct-ok > output.txt && test \"$HOME\" = /home/agent",
    )
    .unwrap();
    assert!(out.contains("git-fixture"), "{out}");
    assert_eq!(read(&ws.0, "output.txt").unwrap(), "direct-ok\n");
    assert_eq!(fs::read_to_string(ws.0.join(".teamagents-auth/profile.json")).unwrap(), "secret-token");
    assert!(shell_direct(&ws.0, "touch .git/hooks/evil").is_err());
    assert!(shell_direct(&ws.0, "touch admin_repo/evil").is_err());
    assert!(!ws.0.join(".git/hooks/evil").exists());
    assert!(!ws.0.join("admin_repo/evil").exists());
    drop(private);
}
