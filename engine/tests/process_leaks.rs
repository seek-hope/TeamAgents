//! P2-4 / P2-5 regressions: a failed initialize handshake must not leak the
//! server process. The reader threads hold Arcs, so Drop alone never runs;
//! the start/connect error path has to kill+wait the child itself.
//! Fakes read the initialize request first: an unsolicited early response can
//! race pending-request registration and turn this test into a spurious timeout.

use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ta-leak-{tag}-{}-{nonce}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Linux-only liveness probe (zombies count as alive: they still occupy a pid
/// until someone calls wait()).
fn proc_gone(pid: &str) -> Option<bool> {
    if !Path::new("/proc").exists() {
        return None;
    }
    Some(!Path::new(&format!("/proc/{pid}")).exists())
}

fn assert_reaped(pidfile: &Path) {
    let pid = std::fs::read_to_string(pidfile).expect("server wrote its pid");
    let pid = pid.trim();
    if let Some(gone) = proc_gone(pid) {
        assert!(gone, "server pid {pid} survived the failed handshake");
    }
}

/// P2-4: codex app-server. The fake replies an error to initialize, then
/// sleeps: without close() on the error path the process (and the two reader
/// threads) would leak.
#[test]
fn codex_failed_initialize_reaps_the_child() {
    use teamagents_engine::codex::{AppServerOptions, CodexAppServer};

    let dir = scratch("codex");
    let pidfile = dir.join("pid");
    // CodexAppServer supplies "app-server" as argv[1]. Have a stable shell
    // read that fixture, so a just-written script never becomes an executable
    // image (which can race parallel child creation with ETXTBSY).
    let script = dir.join("app-server");
    std::fs::write(
        &script,
        "echo $$ > \"$TA_TEST_PID_FILE\"\nIFS= read -r request\necho '{\"id\":1,\"error\":{\"code\":-1,\"message\":\"nope\"}}'\nexec sleep 60\n",
    )
    .unwrap();

    let server = CodexAppServer::new(
        &dir,
        AppServerOptions {
            codex_bin: Some("/bin/sh".into()),
            env: vec![("TA_TEST_PID_FILE".into(), pidfile.to_string_lossy().into_owned())],
            ..Default::default()
        },
    );
    let err = server.start().expect_err("initialize must fail");
    assert!(err.contains("nope"), "{err}");
    assert_reaped(&pidfile);
    server.close(); // close stays idempotent after a failed start
}

/// P2-5: MCP stdio server, same failure shape.
#[test]
fn mcp_failed_initialize_reaps_the_child() {
    use teamagents_engine::mcp::McpClient;

    let dir = scratch("mcp");
    let pidfile = dir.join("pid");
    let script = format!(
        "echo $$ > {}; IFS= read -r request; echo '{{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{{\"code\":-1,\"message\":\"nope\"}}}}'; exec sleep 60",
        pidfile.display()
    );
    // Host PID is needed for /proc reaping assertions; sandbox PIDs differ.
    let result = McpClient::connect_stdio_in("sh", &["-c".into(), script], &[], &dir, "host", false, 2, 2);
    let err = match result {
        Ok(client) => {
            client.close();
            panic!("initialize must fail");
        }
        Err(e) => e,
    };
    assert!(err.contains("nope"), "{err}");
    assert_reaped(&pidfile);
}
