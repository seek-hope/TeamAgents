use super::*;
use teamagents_core::v2::store;
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let r = Self(std::env::temp_dir().join(format!("teamagents-export-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&r.0).unwrap();
        r
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn insert(conn: &rusqlite::Connection, idx: i64) {
    let message = if idx == 2 {
        json!({"role":"assistant","content":"<script>bad()</script>\n```\n\u{1b}[2J", "tool_calls":[{"id":"call","function":{"name":"shell","arguments":"{}"}}]})
    } else if idx == 3 {
        json!({"role":"tool","tool_call_id":"call","content":"full result"})
    } else {
        json!({"role":"user","content":format!("record {idx} \u{03bb}")})
    };
    conn.execute("INSERT INTO context_entries (instance_id,epoch,idx,id,kind,message_json,created) VALUES ('i-leader',?1,?2,?3,?4,?5,0)",
        rusqlite::params![idx/300,idx,format!("e-{idx}"),if idx==2 {"assistant"}else if idx==3 {"tool_result"}else {"user"},message.to_string()]).unwrap();
}
#[test]
fn full_exports_keep_associations_and_refuse_overwrite_limits_and_cancellation() {
    let root = Root::new();
    let registry = Registry::load(&root.0).unwrap();
    let conn = store::open(&root.0.join("session.sqlite"), true).unwrap();
    for idx in 1..=600 {
        insert(&conn, idx);
    }
    conn.execute("INSERT INTO input_attachments VALUES ('pending',0,'input-missing')", []).unwrap();
    conn.execute("INSERT INTO input_revisions VALUES ('pending',0,'submitted','{\"text\":\"original\"}',0)", [])
        .unwrap();
    let cancel = AtomicBool::new(false);
    let path = root.0.join("export.json");
    let report = export(&registry, "default", &path, Format::Json, &cancel).unwrap();
    assert_eq!(report["complete"], true);
    let data: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let entries = data["context_entries"].as_array().unwrap();
    assert_eq!(entries.len(), 600);
    assert_eq!(entries[1]["message"]["tool_calls"][0]["id"], entries[2]["message"]["tool_call_id"]);
    assert_eq!(entries[300]["epoch"], 1);
    assert_eq!(data["input_attachments"][0]["availability"], "snapshot unavailable; reference retained");
    assert_eq!(data["input_revisions"][0]["payload"]["text"], "original");
    let before = std::fs::read(&path).unwrap();
    assert!(export(&registry, "default", &path, Format::Json, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let md = root.0.join("export.md");
    export(&registry, "default", &md, Format::Markdown, &cancel).unwrap();
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("record 600"));
    for line in text.lines().filter(|l| l.contains("<script>") || l.contains("```")) {
        assert!(line.starts_with("    "));
    }
    let failed = root.0.join("failed");
    assert!(export_limit(&registry, "default", &failed, Format::Json, &cancel, 100)
        .unwrap_err()
        .contains("byte budget"));
    assert!(!failed.exists());
    cancel.store(true, Ordering::Relaxed);
    assert!(export(&registry, "default", &failed, Format::Json, &cancel).unwrap_err().contains("cancelled"));
    assert!(!failed.exists());
    assert!(!std::fs::read_dir(&root.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    std::os::unix::fs::symlink(&path, &failed).unwrap();
    assert!(export(&registry, "default", &failed, Format::Json, &AtomicBool::new(false)).is_err());
}
#[test]
fn snapshot_is_consistent_during_concurrent_wal_append_and_empty_archive_is_exportable() {
    let root = Root::new();
    let db = root.0.join("session.sqlite");
    let conn = store::open(&db, true).unwrap();
    for idx in 1..=600 {
        insert(&conn, idx);
    }
    struct Append {
        data: Vec<u8>,
        db: PathBuf,
        did_append: bool,
    }
    impl Write for Append {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if !self.did_append {
                let conn = store::open(&self.db, false).unwrap();
                insert(&conn, 601);
                self.did_append = true;
            }
            self.data.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let read = store::open_read_only(&db).unwrap();
    read.execute_batch("BEGIN").unwrap();
    let mut writer = Append { data: vec![], db: db.clone(), did_append: false };
    stream(Some(&read), "default", &root.0, Format::Json, &mut writer).unwrap();
    let data: Json = serde_json::from_slice(&writer.data).unwrap();
    assert_eq!(data["context_entries"].as_array().unwrap().len(), 600);
    assert_eq!(data["snapshot"]["entry_high_watermark"], 600);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM context_entries", [], |r| r.get::<_, i64>(0)).unwrap(), 601);
    read.execute_batch("COMMIT").unwrap();
    let mut registry = Registry::load(&root.0).unwrap();
    let entry = registry.new_session("empty archived \u{03bb}").unwrap();
    registry.archive(&entry.id).unwrap();
    let path = root.0.join("empty.json");
    export(&registry, &entry.id, &path, Format::Json, &AtomicBool::new(false)).unwrap();
    let data: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(data["context_entries"].as_array().unwrap().is_empty());
    assert!(export(&registry, "nonexistent", &root.0.join("no"), Format::Json, &AtomicBool::new(false)).is_err());
}
