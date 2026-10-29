//! D-451: complete local exports from one SQLite read snapshot, with bounded streaming and no replacement.
use super::{session_browser, sessions::Registry};
use serde_json::{json, Value as Json};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RECORD: usize = 16 * 1024 * 1024;
const PAGE: i64 = 256;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Markdown,
}
impl Format {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "json" => Ok(Self::Json),
            "markdown" | "md" => Ok(Self::Markdown),
            _ => Err("export format must be json or markdown".into()),
        }
    }
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
struct Bounded<'a, W> {
    inner: W,
    bytes: u64,
    limit: u64,
    cancel: &'a AtomicBool,
}
impl<W: Write> Write for Bounded<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("export cancelled"));
        }
        if bytes.len() as u64 > self.limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("export output exceeds byte budget"));
        }
        let count = self.inner.write(bytes)?;
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn export(
    registry: &Registry,
    id: &str,
    output: &Path,
    format: Format,
    cancel: &AtomicBool,
) -> Result<Json, String> {
    export_limit(registry, id, output, format, cancel, MAX_BYTES)
}
fn export_limit(
    registry: &Registry,
    id: &str,
    output: &Path,
    format: Format,
    cancel: &AtomicBool,
    limit: u64,
) -> Result<Json, String> {
    let root = session_browser::directory(registry, id)?;
    if std::fs::symlink_metadata(output).is_ok() {
        return Err("export refuses an existing destination".into());
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| format!("export directory: {e}"))?;
    let name = output.file_name().ok_or("export needs a destination filename")?;
    let output = parent.join(name);
    let temporary = Temporary(parent.join(format!(".teamagents-export-{}.tmp", uuid::Uuid::new_v4())));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary.0)
        .map_err(|e| format!("export temporary: {e}"))?;
    let mut writer = Bounded { inner: file, bytes: 0, limit, cancel };
    let db = root.join("session.sqlite");
    let conn = if db.exists() { Some(teamagents_core::v2::store::open_read_only(&db)?) } else { None };
    if let Some(conn) = &conn {
        conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    }
    let report = stream(conn.as_ref(), id, &root, format, &mut writer)?;
    if let Some(conn) = &conn {
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    }
    writer.flush().and_then(|_| writer.inner.sync_all()).map_err(|e| format!("export sync: {e}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("export cancelled".into());
    }
    // hard_link is atomic and never replaces a file created after the initial existence check.
    std::fs::hard_link(&temporary.0, &output).map_err(|e| format!("export publish (no replacement): {e}"))?;
    File::open(&parent).and_then(|f| f.sync_all()).map_err(|e| format!("export directory sync: {e}"))?;
    Ok(json!({"output":output,"bytes":writer.bytes,"snapshot":report,"complete":true}))
}

fn emit(writer: &mut impl Write, format: Format, value: &Json) -> Result<(), String> {
    if format == Format::Json {
        serde_json::to_writer(writer, value).map_err(|e| e.to_string())
    } else {
        // Indented literal blocks contain ALL supplied text, including labels/HTML/fence delimiters.
        // JSON escaping retains literal controls without sending terminal escape sequences.
        for line in serde_json::to_string_pretty(value).map_err(|e| e.to_string())?.lines() {
            writeln!(writer, "    {line}").map_err(|e| e.to_string())?;
        }
        writeln!(writer).map_err(|e| e.to_string())
    }
}
fn write_text(writer: &mut impl Write, text: &str) -> Result<(), String> {
    writer.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn stream(
    conn: Option<&rusqlite::Connection>,
    id: &str,
    root: &Path,
    format: Format,
    writer: &mut impl Write,
) -> Result<Json, String> {
    let scalar = |sql: &str| -> Result<i64, String> {
        conn.map(|c| c.query_row(sql, [], |r| r.get(0)).map_err(|e| e.to_string())).unwrap_or(Ok(0))
    };
    // This first SELECT establishes the read snapshot before any output or subsequent table reads.
    let watermark = scalar("SELECT COALESCE(MAX(sequence),0) FROM events")?;
    let high = scalar("SELECT COALESCE(MAX(rowid),0) FROM context_entries")?;
    let metadata = json!({"format_version":1,"session_id":id,"event_watermark":watermark,"entry_high_watermark":high,
        "media":"References preserved. External/media bytes are not embedded or fetched; their availability is not checked.",
        "attachments":"Input snapshot references include catalog state and local presence; bytes are not embedded.",
        "history":"All stored rows, including covered summaries, members and epochs. Pruned/deleted rows cannot be recovered."});
    if format == Format::Json {
        write_text(writer, "{\"snapshot\":")?;
        emit(writer, format, &metadata)?;
    } else {
        write_text(writer, "# Session export\n\n## Snapshot\n\n")?;
        emit(writer, format, &metadata)?;
    }
    for (table, title, sql) in [
        ("instances","Members","SELECT rowid, json_object('id',id,'session_id',session_id,'workspace',workspace_ref,'lifecycle',lifecycle,'phase',phase) FROM instances WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"),
        ("context_entries","History","SELECT rowid, json_object('id',id,'instance_id',instance_id,'epoch',epoch,'idx',idx,'kind',kind,'message_raw',message_json,'payload_ref',payload_ref,'envelope_id',envelope_id,'refs_raw',refs_json,'created',created,'compressed_by',compressed_by) FROM context_entries WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"),
        ("envelopes","Inputs","SELECT rowid, json_object('id',id,'session_id',session_id,'sender',sender,'recipient',recipient,'epoch',epoch,'sequence',sequence,'state',state,'payload_raw',payload_json,'payload_ref',payload_ref) FROM envelopes WHERE rowid > ?1 AND kind='user_input' ORDER BY rowid LIMIT ?2"),
        ("goals","Work goals","SELECT rowid,json_object('id',id,'status',status,'deadline',deadline,'original_request_ref',original_request_ref,'limits_raw',limits_json,'usage_raw',known_usage_json,'unknown_usage',unknown_usage) FROM goals WHERE rowid>?1 ORDER BY rowid LIMIT ?2"),
        ("work_states","Durable work state","SELECT rowid,json_object('goal_id',NULLIF(goal_id,''),'instance_id',instance_id,'epoch',epoch,'task_id',task_id,'state',state,'checkpoint_raw',checkpoint_json,'checkpoint_provenance','model_claim','policy_raw',policy_json,'pending_reason',pending_reason,'repair_turns',empty_turns,'revision',revision,'reason',reason) FROM work_states WHERE rowid>?1 ORDER BY rowid LIMIT ?2"),
        ("work_inputs","Work input attribution","SELECT rowid,json_object('goal_id',NULLIF(goal_id,''),'instance_id',instance_id,'epoch',epoch,'envelope_id',envelope_id,'state',state) FROM work_inputs WHERE rowid>?1 ORDER BY rowid LIMIT ?2"),
        ("work_requests","Frozen work context","SELECT rowid,json_object('request_id',request_id,'projection_raw',context_json,'billed_goal_id',billed_goal_id,'unknown_usage',unknown_usage) FROM work_requests WHERE rowid>?1 ORDER BY rowid LIMIT ?2"),
        ("input_revisions","Input revisions","SELECT rowid, json_object('envelope_id',envelope_id,'revision',revision,'action',action,'payload_raw',payload_json,'created',created) FROM input_revisions WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"),
        ("input_attachments","Attachment references","SELECT x.rowid, json_object('envelope_id',x.envelope_id,'ordinal',x.ordinal,'artifact_id',x.artifact_id,'digest',a.digest,'size',a.size,'completeness',a.completeness) FROM input_attachments x LEFT JOIN artifacts a ON a.id=x.artifact_id WHERE x.rowid > ?1 ORDER BY x.rowid LIMIT ?2"),
    ] {
        if format == Format::Json { write_text(writer,&format!(",\"{table}\":["))?; }
        else { write_text(writer,&format!("## {title}\n\n"))?; }
        let mut first=true;
        let exists = conn.map(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())).transpose()?.unwrap_or(false);
        if exists {
            let conn=conn.unwrap();
            let mut after=0i64;
            loop {
                let mut stmt=conn.prepare(sql).map_err(|e|format!("export {table}: {e}"))?;
                let mut rows=stmt.query(rusqlite::params![after,PAGE]).map_err(|e|e.to_string())?;
                let mut count=0;
                while let Some(row)=rows.next().map_err(|e|e.to_string())? {
                    after=row.get(0).map_err(|e|e.to_string())?;
                    let raw=row.get_ref(1).map_err(|e|e.to_string())?.as_str().map_err(|e|e.to_string())?;
                    if raw.len()>MAX_RECORD { return Err(format!("export {table} record {after} exceeds 16 MiB")); }
                    let mut value:Json=serde_json::from_str(raw).map_err(|e|e.to_string())?;
                    value["cursor"]=json!(after);
                    for (raw, parsed) in [("message_raw","message"),("payload_raw","payload"),("refs_raw","refs"),
                        ("limits_raw","limits"),("usage_raw","known_usage"),("checkpoint_raw","checkpoint"),("policy_raw","policy"),("projection_raw","projection")] {
                        if let Some(source)=value.get(raw).cloned() {
                            match source.as_str().map(serde_json::from_str::<Json>) {
                                Some(Ok(data)) => {value.as_object_mut().unwrap().remove(raw);value[parsed]=data;}
                                _ => {value["unavailable"]=json!("Stored structured content missing or malformed; raw/reference retained.");}
                            }
                        }
                    }
                    if table=="input_attachments" {
                        let id=value["artifact_id"].as_str().unwrap_or("");
                        let safe=id.starts_with("input-") && id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-');
                        // Metadata only, constrained to the engine-owned directory; no arbitrary payload_ref reads.
                        let directory=root.join("artifacts");
                        let present=safe && std::fs::symlink_metadata(&directory).is_ok_and(|m|m.is_dir()&&!m.file_type().is_symlink())
                            && std::fs::symlink_metadata(directory.join(format!("{id}.blob"))).is_ok_and(|m|m.is_file()&&!m.file_type().is_symlink());
                        value["availability"]=json!(if present {"reference only; local snapshot present (digest not checked by export)"} else {"snapshot unavailable; reference retained"});
                    }
                    if format==Format::Json && !first { write_text(writer,",")?; }
                    emit(writer,format,&value)?; first=false; count+=1;
                }
                if count<PAGE { break; }
            }
        }
        if format==Format::Json { write_text(writer,"]")?; }
        else if first { write_text(writer,"No stored records.\n\n")?; }
    }
    if format == Format::Json {
        write_text(writer, "}\n")?;
    }
    Ok(metadata)
}

#[cfg(test)]
#[path = "session_export_tests.rs"]
mod tests;
