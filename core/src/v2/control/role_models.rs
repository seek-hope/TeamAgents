//! The Worker policy is independent of the number of live Workers. The instance rows are its
//! materialized selections: updating them here also invalidates stale request preparation.
use super::{event, require_user, update_profile, Connection, Identity, Json, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Workers {
    Follow,
    Pinned { key: String },
    Unresolved,
}

fn policy(tx: &Connection) -> Result<Workers, String> {
    let raw: String = tx
        .query_row("SELECT value FROM meta WHERE key='worker_models'", [], |r| r.get(0))
        .map_err(|e| format!("read Worker model policy: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid Worker model policy: {e}"))
}

pub(super) fn leader(tx: &Connection) -> Result<Option<(String, Option<String>)>, String> {
    tx.query_row("SELECT id,json_extract(profile_json,'$.catalog_key') FROM instances WHERE is_leader=1", [], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .optional()
    .map_err(|e| format!("read Leader selection: {e}"))
}

pub(super) fn snapshot(tx: &Connection) -> Result<Json, String> {
    let (id, key) = leader(tx)?.unzip();
    Ok(json!({"leader_id":id,"leader":key.flatten(),"workers":policy(tx)?}))
}

const UNRESOLVED: &str =
    "select a shared Worker model or follow the Leader before creating Workers or starting new Worker requests";

pub(super) fn worker_profile(tx: &Connection, profile: &mut Json) -> Result<(), String> {
    if !profile.is_object()
        || ["model", "catalog_key", "options", "images", "context_window"].iter().any(|k| profile.get(*k).is_some())
    {
        return Err("Worker model overrides are not allowed; select a shared Worker model".into());
    }
    let key = match policy(tx)? {
        Workers::Pinned { key } => key,
        Workers::Unresolved => return Err(UNRESOLVED.into()),
        Workers::Follow => leader(tx)?
            .and_then(|(_, key)| key)
            .filter(|key| !key.trim().is_empty())
            .ok_or("select an exact Leader model before creating a following Worker")?,
    };
    profile["catalog_key"] = json!(key);
    profile["model"] = json!(key);
    Ok(())
}

pub(super) fn check_request(tx: &Connection, instance: &str) -> Result<(), String> {
    let blocked: bool = tx.query_row("SELECT is_leader=0 AND (SELECT json_extract(value,'$.mode') FROM meta WHERE key='worker_models')='unresolved' FROM instances WHERE id=?1", [instance], |r| r.get(0)).optional().map_err(|e|e.to_string())?.unwrap_or(false);
    if blocked {
        return Err(UNRESOLVED.into());
    }
    Ok(())
}

fn apply_workers(tx: &Connection, session: &str, model: &str) -> Result<(), String> {
    let mut stmt = tx
        .prepare("SELECT id FROM instances WHERE is_leader=0 AND lifecycle!='TERMINATED' AND session_id=?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map([session], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for id in ids {
        update_profile(tx, session, &id, &json!({"model":model}))?;
    }
    Ok(())
}

/// Stamp only the missing provenance after a unique boot-time match. Historical requests, options,
/// phase and revision stay intact; no execution or Worker policy is authorized by this operation.
pub(super) fn bind_legacy_leader(
    tx: &Connection,
    session: &str,
    params: &Json,
    identity: &Identity,
) -> Result<Json, String> {
    if *identity != Identity::System {
        return Err("legacy binding requires the runtime identity".into());
    }
    let key = params["catalog_key"].as_str().filter(|k| !k.trim().is_empty()).ok_or("missing catalogue key")?;
    let wire = params["wire_model"].as_str().ok_or("missing legacy model")?;
    let n = tx.execute("UPDATE instances SET profile_json=json_set(profile_json,'$.catalog_key',?1) WHERE session_id=?2 AND is_leader=1 AND lifecycle!='TERMINATED' AND json_extract(profile_json,'$.catalog_key') IS NULL AND json_extract(profile_json,'$.model')=?3", rusqlite::params![key, session, wire]).map_err(|e| e.to_string())?;
    if n != 1 {
        return Err("legacy Leader selection changed before binding".into());
    }
    Ok(json!({"catalog_key":key}))
}

pub(super) fn select_leader(tx: &Connection, session: &str, id: &str, params: &Json) -> Result<Json, String> {
    let (leader, old_key) = leader(tx)?.ok_or("session has no Leader")?;
    if id != leader {
        return Err("individual Worker profiles cannot be selected; select the workers role".into());
    }
    let result = update_profile(tx, session, id, params)?;
    if let Some(model) = params["model"].as_str() {
        if Some(model) != old_key.as_deref() && matches!(policy(tx)?, Workers::Follow) {
            apply_workers(tx, session, model)?;
        }
    }
    Ok(result)
}

pub(super) fn select(tx: &Connection, session: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    if params.get("instance_id").is_some() {
        return Err("role selection does not accept an instance_id".into());
    }
    let (id, leader_key) = leader(tx)?.ok_or("session has no Leader")?;
    if super::load_instance(tx, &id)?.0 != session {
        return Err("Leader does not belong to this session".into());
    }
    let model = params["model"].as_str().filter(|s| !s.trim().is_empty());
    if params.get("model").is_some() && model.is_none() {
        return Err("select an exact key from the model catalogue".into());
    }
    if params.get("effort").is_some_and(|v| v.as_str().is_none_or(|s| s.trim().is_empty())) {
        return Err("effort must be a nonempty string".into());
    }
    let follow = params.get("follow");
    match params["role"].as_str() {
        Some("leader") if follow.is_none() && (model.is_some() || params["effort"].is_string()) => {
            select_leader(tx, session, &id, params)?;
        }
        Some("workers") if params.get("effort").is_none() => {
            let (policy, key) = match (model, follow) {
                (Some(key), None) => (Workers::Pinned { key: key.into() }, key),
                (None, Some(Json::Bool(true))) if params.get("model").is_none() => (
                    Workers::Follow,
                    leader_key
                        .as_deref()
                        .filter(|k| !k.trim().is_empty())
                        .ok_or("select an exact Leader model before following it")?,
                ),
                _ => return Err("workers needs either an exact model key or follow=true".into()),
            };
            apply_workers(tx, session, key)?;
            tx.execute("UPDATE meta SET value=?1 WHERE key='worker_models'", [json!(policy).to_string()])
                .map_err(|e| e.to_string())?;
        }
        _ => return Err("select leader with model or effort, or workers with model or follow=true".into()),
    }
    let result = snapshot(tx)?;
    event(tx, session, "role_models_changed", &id, &result)?;
    Ok(result)
}
