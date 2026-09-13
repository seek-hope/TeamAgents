//! MCP tool bindings against the real stdio server used by the Python suite
//! (tests/mcp_echo_server.py, test_p3_tools_and_session.py parity).

mod support;

use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use support::*;
use teamagents_core::models::{ToolBinding, UserConfig};
use teamagents_engine::bound::BoundTools;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// The MCP server needs the Python env; skip (not fail) when it is absent.
fn python_with_mcp() -> Option<(String, PathBuf)> {
    let root = repo_root();
    let candidates = [root.join(".venv/bin/python"), PathBuf::from("python3")];
    for interpreter in candidates {
        let ok = Command::new(&interpreter)
            .args(["-c", "import mcp"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let server = root.join("tests/mcp_echo_server.py");
            assert!(server.exists(), "missing {server:?}");
            return Some((interpreter.to_string_lossy().into_owned(), server));
        }
    }
    None
}

fn catalog(command: &str, server: &PathBuf) -> UserConfig {
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "echo_service".into(),
        serde_json::from_value::<ToolBinding>(json!({
            "kind": "mcp", "mcp_server": "echo", "mcp_transport": "stdio",
            "command": command, "args": [server.to_string_lossy()], "tool_names": ["echo"],
        }))
        .unwrap(),
    );
    catalog
}

#[test]
fn mcp_binding_loads_and_calls_a_real_stdio_server() {
    let Some((python, server)) = python_with_mcp() else {
        eprintln!("skip: no python with the `mcp` package available");
        return;
    };
    let tools = BoundTools::load(&catalog(&python, &server), &["echo_service".to_string()]).unwrap();
    assert_eq!(tools.tools.len(), 1);
    // same-named tools get the service prefix (langchain-mcp tool_name_prefix)
    assert_eq!(tools.tools[0].name, "echo_echo");
    assert!(tools.names().contains("echo_echo"));
    let output = tools.call("echo_echo", &json!({"text": "ping", "times": 2})).expect("bound tool").unwrap();
    assert_eq!(output, json!("ping ping"));
    assert!(tools.call("send_message", &json!({})).is_none(), "team tools are not bound tools");
    tools.close();
}

#[test]
fn unbound_service_tools_are_not_advertised() {
    let Some((python, server)) = python_with_mcp() else { return };
    let mut config = catalog(&python, &server);
    config.tools.get_mut("echo_service").unwrap().tool_names = vec!["other".into()];
    let tools = BoundTools::load(&config, &["echo_service".to_string()]).unwrap();
    assert!(tools.tools.is_empty(), "tool_names filters the exposed set");
    tools.close();
}

#[test]
fn member_start_fails_on_unknown_binding() {
    isolated_state_home("mcp-binding");
    let catalog = UserConfig::default();
    let err = BoundTools::load(&catalog, &["ghost_service".to_string()]).err();
    assert!(err.unwrap().contains("unknown tool binding"));
}
