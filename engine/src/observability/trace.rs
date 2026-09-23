//! Append-only JSONL trace: every request, attempt, tool dispatch, receipt,
//! retry and completion carries a stable id and timestamps. Previews are
//! separate from authoritative records; credentials and auth headers are
//! never written (plan §9).

use serde_json::{json, Value as Json};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Trace {
    file: std::fs::File,
    pub path: PathBuf,
    seq: u64,
}

impl Trace {
    pub fn create(dir: &Path, run_id: &str) -> Result<Trace, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("trace dir: {e}"))?;
        let path = dir.join(format!("{run_id}.trace.jsonl"));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("trace file {}: {e}", path.display()))?;
        Ok(Trace { file, path, seq: 0 })
    }

    pub fn record(&mut self, kind: &str, mut fields: Json) -> Result<(), String> {
        self.seq += 1;
        let object = fields.as_object_mut().ok_or("trace fields must be an object")?;
        object.insert("seq".into(), json!(self.seq));
        object.insert("kind".into(), json!(kind));
        object.insert("ts".into(), json!(teamagents_core::models::now()));
        let mut line = fields.to_string();
        line.push('\n');
        self.file.write_all(line.as_bytes()).map_err(|e| format!("trace write: {e}"))
    }
}
