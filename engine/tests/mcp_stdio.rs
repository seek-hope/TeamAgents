//! MCP stdio client regressions: a chatty server must not deadlock the
//! handshake, and server processes must not inherit the engine's environment
//! (findings 3/4).

use std::path::PathBuf;
use std::time::{Duration, Instant};
use teamagents_engine::mcp::McpClient;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-mcp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn python3() -> Option<String> {
    std::process::Command::new("python3")
        .args(["-c", "print(1)"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| "python3".to_string())
}

/// finding 3: the server's stderr is not a pipe nobody drains. 70_000 bytes is
/// past the 64KiB pipe capacity for a stdio server that never got to its reply.
#[test]
fn noisy_stderr_does_not_block_the_handshake() {
    let Some(python) = python3() else {
        eprintln!("skip: python3 is not available");
        return;
    };
    let dir = scratch("noisy");
    let script = dir.join("noisy_server.py");
    std::fs::write(
        &script,
        r#"
import json, sys
sys.stderr.write("x" * 70000)
sys.stderr.flush()
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try: msg = json.loads(line)
    except Exception: continue
    if msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2025-06-18",
              "capabilities": {}, "serverInfo": {"name": "noisy", "version": "0"}}})
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{"name": "echo", "inputSchema": {"type": "object"}}]}})
"#,
    )
    .unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let result = McpClient::connect_stdio(&python, &[script.to_string_lossy().into_owned()], &[])
            .and_then(|client| {
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
    let _ = std::fs::remove_dir_all(&dir);
}

/// finding 4: only the SDK's safe variables (plus binding.env) reach the child.
#[test]
fn server_environment_is_whitelisted() {
    let Some(_python) = python3() else {
        eprintln!("skip: python3 is not available");
        return;
    };
    let dir = scratch("env");
    let dump = dir.join("env.txt");
    std::env::set_var("TA_MCP_SENTINEL_KEY", "sk-should-never-leak");
    std::env::set_var("TA_MCP_SENTINEL_OTHER", "also-not");
    // a server that dumps its environment and exits (no MCP handshake needed)
    let _ = McpClient::connect_stdio(
        "sh",
        &["-c".into(), format!("env > {}", dump.display())],
        &[("TA_MCP_BINDING_VAR".into(), "from-binding".into())],
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
                "HOME" | "LOGNAME" | "PATH" | "SHELL" | "TERM" | "USER" | "TA_MCP_BINDING_VAR" | "_" | "PWD" | "SHLVL"
                    | "OLDPWD"
            ),
            "unexpected inherited variable {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
