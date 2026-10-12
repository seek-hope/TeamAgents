//! D-451: user-selected workspace files become immutable, catalogued input snapshots.
use super::storage::Storage;
use base64::Engine;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};
use teamagents_core::{
    kernel::ContextEntry,
    models::UserConfig,
    v2::{attachments::*, Command, Identity},
};

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;

struct Prepared {
    descriptor: InputAttachment,
    bytes: Vec<u8>,
}

fn relative(path: &str) -> Result<&Path, String> {
    let p = Path::new(path);
    if path.is_empty()
        || path.len() > 4096
        || path.contains("://")
        || path.chars().any(char::is_control)
        || p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(
            "attachments require workspace-relative paths without parent traversal, URLs, or control characters".into(),
        );
    }
    Ok(p)
}

/// Pin each component with O_NOFOLLOW, including symlinks that would stay inside the workspace.
fn open_confined(root: &Path, relative: &Path, directory: bool) -> Result<File, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|e| format!("attachment root: {e}"))?;
    let parts: Vec<_> =
        relative.components().filter_map(|c| if let Component::Normal(p) = c { Some(p) } else { None }).collect();
    if parts.is_empty() && !directory {
        return Err("attachment path must name a regular file".into());
    }
    for (i, part) in parts.iter().enumerate() {
        let is_dir = i + 1 < parts.len() || directory;
        let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())).join(part);
        file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | if is_dir { libc::O_DIRECTORY } else { 0 })
            .open(path)
            .map_err(|e| format!("attachment path (symlinks are refused): {e}"))?;
    }
    if !directory && !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("attachment is not a regular file".into());
    }
    Ok(file)
}

fn media_type(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Ok("image/webp");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "attachment is neither UTF-8 text nor PNG/JPEG/GIF/WebP")?;
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        return Err("text attachment contains binary/control bytes".into());
    }
    Ok("text/plain")
}

fn prepare(root: &Path, paths: &[String], images: bool) -> Result<Vec<Prepared>, String> {
    if paths.len() > MAX_ATTACHMENTS {
        return Err("at most eight attachments are allowed".into());
    }
    let mut prepared = Vec::new();
    let mut total = 0usize;
    for label in paths {
        let mut file = open_confined(root, relative(label)?, false)?;
        let before = file.metadata().map_err(|e| e.to_string())?;
        if before.len() > MAX_IMAGE_BYTES as u64 {
            return Err(format!("attachment {label:?} exceeds 5 MiB"));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("attachment {label:?}: {e}"))?;
        let after = file.metadata().map_err(|e| e.to_string())?;
        if (before.len(), before.mtime(), before.mtime_nsec(), before.ctime(), before.ctime_nsec())
            != (after.len(), after.mtime(), after.mtime_nsec(), after.ctime(), after.ctime_nsec())
            || before.len() != bytes.len() as u64
        {
            return Err(format!("attachment {label:?} changed during capture; submit again"));
        }
        let media_type = media_type(&bytes)?.to_string();
        if media_type.starts_with("image/") && !images {
            return Err(
                "the selected model/protocol does not declare image support; attachment was not submitted".into()
            );
        }
        let descriptor = InputAttachment {
            artifact_id: format!("input-{}", uuid::Uuid::new_v4()),
            label: label.clone(),
            media_type,
            bytes: bytes.len(),
            digest: format!("{:x}", Sha256::digest(&bytes)),
        };
        descriptor.validate()?;
        total += bytes.len();
        if total > MAX_TOTAL_BYTES {
            return Err("attachments exceed 16 MiB total".into());
        }
        prepared.push(Prepared { descriptor, bytes });
    }
    Ok(prepared)
}

fn publish(root: &Path, prepared: &[Prepared]) -> Result<(), String> {
    let dir = root.join("artifacts");
    std::fs::create_dir_all(&dir).map_err(|e| format!("attachment directory: {e}"))?;
    for item in prepared {
        let name = &item.descriptor.artifact_id;
        let temporary = dir.join(format!("{name}.tmp"));
        let destination = dir.join(format!("{name}.blob"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| format!("attachment staging file: {e}"))?;
        file.write_all(&item.bytes).and_then(|_| file.sync_all()).map_err(|e| format!("attachment write/sync: {e}"))?;
        // Hard-link publication refuses an existing destination instead of overwriting immutable data.
        std::fs::hard_link(&temporary, &destination).map_err(|e| format!("attachment publication: {e}"))?;
        std::fs::remove_file(&temporary).map_err(|e| format!("attachment staging cleanup: {e}"))?;
        File::open(&dir).and_then(|f| f.sync_all()).map_err(|e| format!("attachment directory sync: {e}"))?;
    }
    Ok(())
}

pub async fn submit(storage: &Storage, root: &Path, catalog: &UserConfig, command: Command) -> Result<Json, String> {
    if let Some(receipt) = storage
        .call({
            let command = command.clone();
            move |c| c.command_receipt(&command, &Identity::User)
        })
        .await??
    {
        return Ok(receipt);
    }
    let paths: Vec<String> = serde_json::from_value(command.params["attachments"].clone())
        .map_err(|_| "attachments must be an array of workspace-relative paths")?;
    if paths.len() > MAX_ATTACHMENTS {
        return Err("at most eight attachments are allowed".into());
    }
    let instance = command.params["instance_id"].as_str().ok_or("input.instance_id required")?.to_string();
    let (workspace, profile): (String, String) = storage.call(move |c| c.connection().query_row(
        "SELECT workspace_ref, profile_json FROM instances WHERE id = ?1 AND session_id = ?2 AND lifecycle != 'TERMINATED'",
        rusqlite::params![instance, c.session_id], |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|e| format!("attachment recipient: {e}"))).await??;
    let profile: Json = serde_json::from_str(&profile).map_err(|e| format!("attachment profile: {e}"))?;
    let images =
        crate::providers::resolve_model(catalog, profile["model"].as_str().unwrap_or("")).is_some_and(|(_, p)| {
            p.images
                && matches!(p.protocol.as_str(), "openai" | "chat/completions" | "deepseek" | "responses" | "anthropic")
        });
    let prepared = tokio::task::spawn_blocking(move || prepare(Path::new(&workspace), &paths, images))
        .await
        .map_err(|e| format!("attachment capture: {e}"))??;
    let descriptors: Vec<_> = prepared.iter().map(|p| p.descriptor.clone()).collect();
    let root_owned = root.to_path_buf();
    let cleanup = descriptors.clone();
    let outcome = async {
        for descriptor in &descriptors {
            let params = json!({"id":descriptor.artifact_id,"digest":descriptor.digest,"size":descriptor.bytes,
                "kind":"input_attachment","owner_scope":"input","owner_ref":command.command_id,
                "storage_ref":root.join("artifacts").join(format!("{}.blob",descriptor.artifact_id)).to_string_lossy()});
            storage
                .call(move |c| {
                    c.submit(
                        Command {
                            command_id: format!("stage-{}", uuid::Uuid::new_v4()),
                            method: "artifact_stage".into(),
                            params,
                        },
                        Identity::System,
                    )
                })
                .await??;
        }
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || publish(&root, &prepared))
            .await
            .map_err(|e| format!("attachment publication: {e}"))??;
        // The original client command is hashed, not a new read of its files on retry.
        storage.call(move |c| c.submit_input_snapshots(command, descriptors)).await?
    }
    .await;
    if outcome.is_err() {
        // Failure after staging must not leak STAGING rows or this round's blobs until a restart
        // (the boot-time recover() remains the backstop for a crash between operations).
        cleanup_failed_submission(storage, &root_owned, &cleanup).await;
    }
    outcome
}

/// Best-effort cleanup for a failed submission: abandon this round's staged artifacts and delete
/// only the blob/temp files named by this round's fresh artifact ids.
async fn cleanup_failed_submission(storage: &Storage, root: &Path, descriptors: &[InputAttachment]) {
    let dir = root.join("artifacts");
    for descriptor in descriptors {
        let id = descriptor.artifact_id.clone();
        let _ = storage
            .call(move |c| {
                c.submit(
                    Command {
                        command_id: format!("abandon-{id}"),
                        method: "artifact_abandon".into(),
                        params: json!({"id": id}),
                    },
                    Identity::System,
                )
            })
            .await;
        let _ = std::fs::remove_file(dir.join(format!("{}.blob", descriptor.artifact_id)));
        let _ = std::fs::remove_file(dir.join(format!("{}.tmp", descriptor.artifact_id)));
    }
}

/// At daemon boot no publication from the previous daemon remains in flight. The
/// atomic input transaction would have made referenced snapshots LIVE; only its
/// uncommitted staging rows can remain here. Keep abandoned bytes for diagnosis.
pub async fn recover(storage: &Storage) -> Result<(), String> {
    storage.call(|c| {
        let ids: Vec<String> = {
            let mut stmt = c.connection().prepare("SELECT id FROM artifacts WHERE owner_scope = 'input' AND completeness = 'STAGING' AND id NOT IN (SELECT artifact_id FROM input_attachments)").map_err(|e|e.to_string())?;
            let rows = stmt.query_map([],|r|r.get(0)).map_err(|e|e.to_string())?;
            rows.collect::<Result<_,_>>().map_err(|e|e.to_string())?
        };
        for id in ids {
            c.submit(Command { command_id:format!("abandon-{id}"),method:"artifact_abandon".into(),params:json!({"id":id}) },Identity::System)?;
        }
        Ok::<_, String>(())
    }).await?
}

/// Expand only immutable attachment references, never their original workspace paths.
pub fn expand(root: &Path, entries: &mut [ContextEntry], images: bool, protocol: &str) -> Result<(), String> {
    for entry in entries {
        if entry.kind != teamagents_core::kernel::EntryKind::User {
            continue;
        }
        let Some(value) = entry.message.get("attachments") else { continue };
        let attachments: Vec<InputAttachment> =
            serde_json::from_value(value.clone()).map_err(|e| format!("stored attachments: {e}"))?;
        if attachments.len() > MAX_ATTACHMENTS {
            return Err("stored input exceeds attachment count".into());
        }
        let mut parts = vec![json!({"type":"text","text":entry.message["content"].as_str().unwrap_or("")})];
        for descriptor in attachments {
            descriptor.validate()?;
            let file =
                open_confined(&root.join("artifacts"), Path::new(&format!("{}.blob", descriptor.artifact_id)), false)?;
            let mut bytes = Vec::new();
            file.take(descriptor.bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("attachment snapshot missing: {e}"))?;
            if bytes.len() != descriptor.bytes || format!("{:x}", Sha256::digest(&bytes)) != descriptor.digest {
                return Err(format!("attachment snapshot {} is corrupt", descriptor.artifact_id));
            }
            let heading = format!(
                "Attachment {:?} ({} bytes, SHA-256 {})",
                descriptor.label, descriptor.bytes, descriptor.digest
            );
            if descriptor.media_type == "text/plain" {
                let text = std::str::from_utf8(&bytes).map_err(|_| "stored text attachment is not UTF-8")?;
                let quoted = text.split('\n').map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n");
                parts.push(json!({"type":"text","text":format!("{heading}\nQuoted file content:\n{quoted}\nEnd of attachment.")}));
            } else {
                if !images || !matches!(protocol, "chat_completions" | "responses" | "anthropic") {
                    return Err(format!("image attachments unsupported by selected model/protocol {protocol}"));
                }
                parts.push(json!({"type":"text","text":heading}));
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                parts.push(json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{encoded}",descriptor.media_type)}}));
            }
        }
        // Text-only attachments remain strings for text-only protocols and compaction.
        if parts.iter().all(|p| p["type"] == "text") {
            entry.message["content"] =
                json!(parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n\n"));
        } else {
            entry.message["content"] = json!(parts);
        }
        entry.message.as_object_mut().ok_or("invalid stored message")?.remove("attachments");
    }
    Ok(())
}

/// Names only; completing a path neither reads file bytes nor submits input.
pub fn candidates(root: &Path, prefix: &str) -> Result<Json, String> {
    if !prefix.is_empty() {
        relative(prefix)?;
    }
    let (parent, leaf) = prefix.rsplit_once('/').map_or(("", prefix), |(a, b)| (a, b));
    let dir = open_confined(root, Path::new(parent), true)?;
    let mut candidates = Vec::new();
    let mut limited = false;
    for (index, entry) in
        std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd())).map_err(|e| e.to_string())?.enumerate()
    {
        if index >= 10_000 {
            limited = true;
            break;
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| n.starts_with(leaf) && !n.chars().any(char::is_control)) else {
            continue;
        };
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let path = format!(
            "{}{}{}{}",
            parent,
            if parent.is_empty() { "" } else { "/" },
            name,
            if kind.is_dir() { "/" } else { "" }
        );
        candidates.push(path);
    }
    candidates.sort();
    limited |= candidates.len() > 100;
    candidates.truncate(100);
    Ok(json!({"prefix":prefix,"paths":candidates,"limited":limited}))
}
