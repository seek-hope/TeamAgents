//! Session-owned restrictions on already configured MCP bindings. Policy never defines a service.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpPolicy {
    pub revision: i64,
    pub services: BTreeMap<String, ServicePolicy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServicePolicy {
    pub running: bool,
    pub exposed: bool,
    pub generation: u64,
}

impl Default for ServicePolicy {
    fn default() -> Self {
        Self { running: true, exposed: true, generation: 0 }
    }
}

pub fn read(conn: &Connection) -> Result<McpPolicy, String> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='mcp_policy'", [], |r| r.get(0))
        .optional()
        .map_err(|e| format!("read MCP policy: {e}"))?;
    raw.map(|s| serde_json::from_str(&s))
        .transpose()
        .map(|p| p.unwrap_or_default())
        .map_err(|e| format!("invalid stored MCP policy: {e}"))
}

pub(super) fn change(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    let key = params["service"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .ok_or("select an exact configured service")?;
    let mut policy = read(tx)?;
    let mut rule = policy.services.get(key).copied().unwrap_or_default();
    let previous = rule;
    match params["action"].as_str() {
        Some("start") => rule.running = true,
        Some("stop") => rule.running = false,
        Some("hide") => rule.exposed = false,
        Some("expose") => rule.exposed = true,
        Some("reconnect") if rule.running => {
            rule.generation = rule.generation.checked_add(1).ok_or("MCP generation exhausted")?
        }
        Some("reconnect") => return Err("service is stopped; start it first".into()),
        _ => return Err("MCP action must be start, stop, hide, expose or reconnect".into()),
    }
    if previous != rule {
        // A change must not invalidate a frozen call (including an approval, nested call, or
        // lost reply). Refuse rather than cancelling, draining, or claiming remote effects stopped.
        let busy: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM model_requests WHERE status='PENDING') OR EXISTS(
            SELECT 1 FROM operations WHERE status IN ('PREPARED','DISPATCH_COMMITTED','RUNNING','OUTCOME_UNKNOWN'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("MCP boundary: {e}"))?;
        if busy {
            return Err("MCP policy unchanged: pending request, operation or unknown outcome in this session".into());
        }
        if !previous.running && rule.running {
            rule.generation = rule.generation.checked_add(1).ok_or("MCP generation exhausted")?;
        }
        policy.revision = policy.revision.checked_add(1).ok_or("MCP revision exhausted")?;
        policy.services.insert(key.to_owned(), rule);
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('mcp_policy',?1)",
            [serde_json::to_string(&policy).map_err(|e| e.to_string())?],
        )
        .map_err(|e| format!("store MCP policy: {e}"))?;
        tx.execute("UPDATE instances SET revision=revision+1 WHERE lifecycle<>'TERMINATED'", [])
            .map_err(|e| format!("fence MCP surface: {e}"))?;
        event(
            tx,
            session,
            "mcp_policy_changed",
            "session",
            &json!({"service":key,"rule":rule,"revision":policy.revision}),
        )?;
    }
    Ok(json!({"service":key,"desired":rule,"revision":policy.revision,"changed":previous!=rule}))
}
