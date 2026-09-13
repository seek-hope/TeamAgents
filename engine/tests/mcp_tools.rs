//! MCP tool bindings against the real stdio server (the `fake-mcp-server`
//! binary, formerly tests/mcp_echo_server.py).

mod support;

use serde_json::json;
use support::*;
use teamagents_core::models::{ToolBinding, UserConfig};
use teamagents_engine::bound::BoundTools;

/// The stdio MCP server shipped with the engine crate; no external env needed.
fn echo_server() -> String {
    env!("CARGO_BIN_EXE_fake-mcp-server").to_string()
}

fn catalog(command: &str) -> UserConfig {
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "echo_service".into(),
        serde_json::from_value::<ToolBinding>(json!({
            "kind": "mcp", "mcp_server": "echo", "mcp_transport": "stdio",
            "command": command, "tool_names": ["echo"],
        }))
        .unwrap(),
    );
    catalog
}

#[test]
fn mcp_binding_loads_and_calls_a_real_stdio_server() {
    let command = echo_server();
    let tools = BoundTools::load(&catalog(&command), &["echo_service".to_string()]).unwrap();
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
    let mut config = catalog(&echo_server());
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
