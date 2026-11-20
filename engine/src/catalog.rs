//! Versioned metadata from user-bound Chat Completions directories. Discovery never calls inference.
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use teamagents_core::models::{ModelProfile, UserConfig};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ROWS: usize = 4096;
const HISTORY: i64 = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Metadata {
    window: u64,
    images: bool,
    missing: bool,
}
type Rows = BTreeMap<String, Metadata>;

fn endpoint(seed: &ModelProfile) -> Result<String, String> {
    if !matches!(seed.protocol.as_str(), "chat/completions" | "openai") {
        return Err("catalogue needs a Chat Completions seed".into());
    }
    let raw = seed.base_url.as_deref().ok_or("catalogue seed needs an explicit base_url")?;
    let url = url::Url::parse(raw).map_err(|_| "invalid catalogue API root")?;
    let local = matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if url.host().is_none()
        || !(url.scheme() == "https" || local && url.scheme() == "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("catalogue requires HTTPS or a loopback HTTP IP, without credentials, query or fragment".into());
    }
    Ok(format!("{}/models", url.as_str().trim_end_matches('/')))
}

pub(crate) fn validate_sources(cfg: &UserConfig) -> Result<(), String> {
    if cfg.catalog_sources.len() > 16 {
        return Err("at most 16 catalogue sources are supported".into());
    }
    for (ns, key) in &cfg.catalog_sources {
        if ns.is_empty() || ns.len() > 64 || !ns.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) {
            return Err("invalid catalogue namespace (use ASCII letters, digits, dash or underscore)".into());
        }
        endpoint(cfg.models.get(key).ok_or("unknown catalogue seed profile")?)?;
    }
    Ok(())
}

fn observations(bytes: &[u8]) -> Result<Rows, String> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|_| "invalid directory JSON")?;
    let rows = doc["data"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= MAX_ROWS)
        .ok_or("empty, oversized or missing model list")?;
    if !doc["links"]["next"].is_null() || doc.get("total_count").is_some_and(|n| n.as_u64() != Some(rows.len() as u64))
    {
        return Err("partial model directory".into());
    }
    let mut ids = HashSet::new();
    let mut out = Rows::new();
    for row in rows {
        let id = row["id"].as_str().ok_or("missing model id")?;
        if id.is_empty()
            || id.len() > 256
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-:/".contains(&b))
            || !ids.insert(id)
        {
            return Err("invalid or duplicate model id".into());
        }
        let has = |v: &Value, s: &str| v.as_array().is_some_and(|list| list.iter().any(|v| v.as_str() == Some(s)));
        let arch = &row["architecture"];
        if !has(&arch["input_modalities"], "text")
            || !has(&arch["output_modalities"], "text")
            || !has(&row["supported_parameters"], "tools")
        {
            continue;
        }
        let window = row["context_length"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= i64::MAX as u64)
            .ok_or("invalid context_length for a tool-capable text model")?;
        out.insert(id.into(), Metadata { window, images: has(&arch["input_modalities"], "image"), missing: false });
    }
    if out.is_empty() {
        return Err("directory contains no verified text/tool models".into());
    }
    Ok(out)
}

fn fetch(seed: &ModelProfile) -> Result<Rows, String> {
    let dest = endpoint(seed)?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(15)).build();
    let mut request = agent.get(&dest).set("accept", "application/json");
    if let Some(var) = &seed.api_key_env {
        let token = std::env::var(var).ok().filter(|s| !s.is_empty()).ok_or("catalogue credential is not available")?;
        if !token.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err("invalid catalogue credential".into());
        }
        request = request.set("authorization", &format!("Bearer {token}"));
    }
    let reply = request.call().map_err(|e| match e {
        ureq::Error::Status(n, _) => format!("catalogue HTTP {n}"),
        _ => "catalogue transport failed".into(),
    })?;
    if reply.status() != 200 {
        return Err(format!("catalogue HTTP {} (no redirects)", reply.status()));
    }
    let mut bytes = Vec::new();
    reply.into_reader().take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "cannot read directory response")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("directory exceeds 8 MiB".into());
    }
    observations(&bytes)
}

fn location() -> PathBuf {
    crate::config::xdg_state_home().join("teamagents-catalog")
}
fn identity(ns: &str, cfg: &UserConfig) -> String {
    let key = &cfg.catalog_sources[ns];
    let seed = &cfg.models[key];
    teamagents_core::kernel::args_hash(&json!([ns, key, seed.provider, seed.protocol, seed.base_url, seed.api_key_env]))
}
fn current(conn: &Connection, id: &str) -> Result<(i64, Rows), String> {
    let row: Option<(i64, String)> = conn
        .query_row("SELECT version,body FROM snapshots WHERE source=?1 ORDER BY version DESC LIMIT 1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| e.to_string())?;
    row.map(|(v, b)| {
        serde_json::from_str(&b).map(|rows| (v, rows)).map_err(|_| "invalid catalogue snapshot".to_owned())
    })
    .unwrap_or_else(|| Ok((0, Rows::new())))
}
fn reader(dir: &Path) -> Result<Option<Connection>, String> {
    let path = dir.join("catalog.sqlite");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map(Some)
        .map_err(|e| e.to_string())
}
fn writer(dir: &Path) -> Result<Connection, String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| e.to_string())?;
    let path = dir.join("catalog.sqlite");
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS snapshots(source TEXT NOT NULL,version INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(source,version));").map_err(|e|e.to_string())?;
    Ok(conn)
}

/// Project rows cannot shadow this namespace. A complete user profile at the same key is an explicit override.
fn apply(cfg: &mut UserConfig, ns: &str, rows: Rows) {
    let seed = cfg.models[&cfg.catalog_sources[ns]].clone();
    for (wire, row) in rows {
        cfg.models.entry(format!("{ns}::{wire}")).or_insert_with(|| ModelProfile {
            model: wire,
            images: row.images,
            context_window: Some(row.window),
            efforts: Vec::new(),
            generation_options: HashMap::new(),
            price: None,
            ..seed.clone()
        });
    }
}
pub(crate) fn overlay(cfg: &mut UserConfig) -> Result<(), String> {
    if cfg.catalog_sources.is_empty() {
        return Ok(());
    }
    let Some(db) = reader(&location())? else { return Ok(()) };
    for ns in cfg.catalog_sources.keys().cloned().collect::<Vec<_>>() {
        let (_, rows) = current(&db, &identity(&ns, cfg))?;
        apply(cfg, &ns, rows)
    }
    Ok(())
}

fn change(dir: &Path, cfg: &UserConfig, ns: &str, verb: &str) -> Result<Value, String> {
    let id = identity(ns, cfg);
    let seed = &cfg.models[&cfg.catalog_sources[ns]];
    if verb == "list" {
        let (version, rows) = match reader(dir)? {
            Some(db) => current(&db, &id)?,
            None => (0, Rows::new()),
        };
        return Ok(json!({"source":ns,"endpoint":endpoint(seed)?,"version":version,"models":rows}));
    }
    let mut db = writer(dir)?;
    let (base, _) = current(&db, &id)?;
    let observed = match verb {
        "refresh" => Some(fetch(seed)?),
        "rollback" => None,
        _ => return Err("unknown catalogue action".into()),
    };
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let (version, old) = current(&tx, &id)?;
    if version != base {
        return Err("catalogue changed concurrently; retry".into());
    }
    let updated = if let Some(mut new) = observed {
        for (key, row) in &old {
            new.entry(key.clone()).or_insert_with(|| Metadata { missing: true, ..row.clone() });
        }
        new
    } else {
        let body: String = tx
            .query_row("SELECT body FROM snapshots WHERE source=?1 AND version=?2", params![id, version - 1], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("no previous catalogue version")?;
        serde_json::from_str(&body).map_err(|_| "invalid previous catalogue snapshot")?
    };
    if updated.len() > MAX_ROWS {
        return Err("retained catalogue exceeds model limit".into());
    }
    let mut diff = BTreeMap::new();
    for key in old.keys().chain(updated.keys()) {
        if old.get(key) != updated.get(key) {
            diff.insert(key, json!({"before":old.get(key),"after":updated.get(key)}));
        }
    }
    let next = if diff.is_empty() { version } else { version + 1 };
    if next != version {
        tx.execute(
            "INSERT INTO snapshots VALUES(?1,?2,?3)",
            params![id, next, serde_json::to_string(&updated).map_err(|e| e.to_string())?],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM snapshots WHERE source=?1 AND version<=?2", params![id, next - HISTORY])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({"source":ns,"endpoint":endpoint(seed)?,"version":next,"changes":diff}))
}

/// Return independent source receipts, including failures, without logging remote bodies or credentials.
pub fn command(cfg: &UserConfig, verb: &str, source: Option<&str>) -> Result<(Value, bool), String> {
    validate_sources(cfg)?;
    if source.is_some_and(|s| !cfg.catalog_sources.contains_key(s)) {
        return Err("unknown catalogue source".into());
    }
    if verb == "rollback" && source.is_none() {
        return Err("catalog rollback requires --name SOURCE".into());
    }
    let mut output = BTreeMap::new();
    let mut ok = true;
    for ns in cfg.catalog_sources.keys().filter(|ns| source.is_none_or(|s| s == *ns)) {
        let value = change(&location(), cfg, ns, verb).unwrap_or_else(|error| {
            ok = false;
            json!({"error":error})
        });
        output.insert(ns, value);
    }
    Ok((json!({"sources":output}), ok))
}
