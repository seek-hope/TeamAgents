//! MCP stdio client regressions: a chatty server must not deadlock the
//! handshake, and server processes must not inherit the engine's environment
//! (findings 3/4).

mod support;

use std::time::{Duration, Instant};
use teamagents_engine::mcp::McpClient;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-mcp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// finding 3: the server's stderr is not a pipe nobody drains. 70_000 bytes is
/// past the 64KiB pipe capacity for a stdio server that never got to its reply.
/// Host mode: this is about stderr draining, not about the bwrap workspace
/// (that path is covered by `mcp::tests::stdio_workspace_isolates_*`, which
/// needs a machine with bubblewrap).
#[test]
fn noisy_stderr_does_not_block_the_handshake() {
    let server = env!("CARGO_BIN_EXE_fake-mcp-server");
    let noisy = vec!["--noisy-stderr".to_string(), "70000".to_string()];
    let root = scratch("noisy");

    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let result =
            McpClient::connect_stdio_in(server, &noisy, &[], &root, "host", false, 60, 120).and_then(|client| {
                let tools = client.tools()?;
                client.close();
                Ok(tools.len())
            });
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(tools)) => {
            assert_eq!(tools, 1);
            assert!(started.elapsed() < Duration::from_secs(10), "handshake completes promptly");
        }
        Ok(Err(e)) => panic!("handshake failed: {e}"),
        Err(_) => panic!("handshake still blocked after 10s: the server's stderr is not drained"),
    }
}

/// finding 4: only the SDK's safe variables (plus binding.env) reach the child.
/// Host mode for the same reason as above: the whitelist is applied before the
/// execution mode is chosen, so this stays meaningful without bubblewrap.
#[test]
fn server_environment_is_whitelisted() {
    let dir = scratch("env");
    let dump = dir.join("env.txt");
    let mut env = support::isolated_state_home("mcp-env");
    env.set("TA_MCP_SENTINEL_KEY", "sk-should-never-leak");
    env.set("TA_MCP_SENTINEL_OTHER", "also-not");
    // a server that dumps its environment and exits (no MCP handshake needed)
    let _ = McpClient::connect_stdio_in(
        "sh",
        &["-c".into(), format!("env > {}", dump.display())],
        &[("TA_MCP_BINDING_VAR".into(), "from-binding".into())],
        &dir,
        "host",
        false,
        2,
        2,
    );
    let dumped = std::fs::read_to_string(&dump).expect("the child wrote its environment");
    assert!(!dumped.contains("TA_MCP_SENTINEL_KEY"), "model/API keys must not leak:\n{dumped}");
    assert!(!dumped.contains("sk-should-never-leak"), "{dumped}");
    assert!(!dumped.contains("TA_MCP_SENTINEL_OTHER"), "{dumped}");
    assert!(dumped.contains("TA_MCP_BINDING_VAR=from-binding"), "binding env is layered on top:\n{dumped}");
    assert!(dumped.contains("PATH=") && dumped.contains("HOME="), "safe variables survive:\n{dumped}");
    // nothing else: the whitelist is six names plus the binding's own, and the
    // shell adds PWD/SHLVL/_ itself
    let names: Vec<&str> = dumped.lines().filter_map(|line| line.split('=').next()).collect();
    for name in &names {
        assert!(
            matches!(
                *name,
                "HOME"
                    | "LOGNAME"
                    | "PATH"
                    | "SHELL"
                    | "TERM"
                    | "USER"
                    | "TA_MCP_BINDING_VAR"
                    | "_"
                    | "PWD"
                    | "SHLVL"
                    | "OLDPWD"
            ),
            "unexpected inherited variable {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
