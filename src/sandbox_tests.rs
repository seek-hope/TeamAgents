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
    fs::write(stage.scratch.0.join("ordinary"), "not published").unwrap();
    assert!(stage.publish().unwrap_err().contains("reserved"));
    assert!(!ws.join("ordinary").exists());
    assert_eq!(fs::read_to_string(nested.join(".git")).unwrap(), "gitdir: ../.git/modules/module\n");
    assert_eq!(fs::read_to_string(git.join("config")).unwrap(), "fixture");

    let stage = Stage::new(&ws).unwrap();
    fs::remove_dir(stage.scratch.0.join("module")).unwrap();
    assert!(stage.publish().unwrap_err().contains("protected"));
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
    assert!(stage.publish().unwrap_err().contains("reserved"));
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
