//! Attachment boundary tests use owned files and real SQLite transactions.
use super::*;
use teamagents_core::{kernel::EntryKind, v2::Control};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let root = Self(std::env::temp_dir().join(format!("teamagents-attachments-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&root.0).unwrap();
        root
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn attachment_budgets_paths_and_types_fail_closed() {
    let root = Root::new();
    let p = |name: &str| vec![name.to_string()];
    let label = "space \u{03bb}.txt";
    std::fs::write(root.0.join(label), "before").unwrap();
    assert_eq!(prepare(&root.0, &p(label), false).unwrap()[0].bytes, b"before");
    for invalid in ["../escape", "/etc/passwd", "https://example.test/a", "a/../../b", "bad\nname"] {
        assert!(prepare(&root.0, &p(invalid), true).is_err(), "{invalid:?}");
    }
    std::fs::write(root.0.join("binary"), [0xff, 0x00]).unwrap();
    assert!(prepare(&root.0, &p("binary"), true).is_err());
    std::fs::write(root.0.join("text"), vec![b'x'; MAX_TEXT_BYTES]).unwrap();
    assert!(prepare(&root.0, &p("text"), false).is_ok());
    std::fs::write(root.0.join("text"), vec![b'x'; MAX_TEXT_BYTES + 1]).unwrap();
    assert!(prepare(&root.0, &p("text"), false).is_err());
    let mut png = vec![0; MAX_IMAGE_BYTES];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    std::fs::write(root.0.join("image"), &png).unwrap();
    assert!(prepare(&root.0, &p("image"), true).is_ok());
    assert!(prepare(&root.0, &p("image"), false).is_err());
    std::fs::write(root.0.join("text"), vec![b'x'; MAX_TEXT_BYTES]).unwrap();
    assert!(prepare(&root.0, &["image", "image", "image", "text"].map(String::from), true).is_ok());
    assert!(prepare(&root.0, &["image", "image", "image", "text", "text"].map(String::from), true).is_err());
    png.push(0);
    std::fs::write(root.0.join("image"), png).unwrap();
    assert!(prepare(&root.0, &p("image"), true).is_err());
    assert!(prepare(&root.0, &vec![label.into(); 8], false).is_ok());
    assert!(prepare(&root.0, &vec![label.into(); 9], false).is_err());
    std::os::unix::fs::symlink(root.0.join(label), root.0.join("link")).unwrap();
    std::fs::create_dir(root.0.join("directory")).unwrap();
    std::os::unix::fs::symlink(root.0.join("directory"), root.0.join("linkdir")).unwrap();
    std::fs::write(root.0.join("directory/file"), "inside").unwrap();
    assert!(prepare(&root.0, &p("link"), true).is_err());
    assert!(prepare(&root.0, &p("linkdir/file"), true).is_err());
    assert!(prepare(&root.0, &p("directory"), true).is_err());
    let fifo = std::ffi::CString::new(root.0.join("fifo").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(prepare(&root.0, &p("fifo"), true).is_err());
    let paths = candidates(&root.0, "spa").unwrap();
    assert_eq!(paths["paths"], json!([label]));
    assert!(candidates(&root.0, "linkdir/").is_err());
}

#[tokio::test]
async fn immutable_snapshot_retry_revision_gc_and_corruption() {
    let root = Root::new();
    let label = "report.txt";
    std::fs::write(root.0.join(label), "before selection").unwrap();
    std::fs::write(root.0.join(label), "at submission").unwrap();
    let storage = Storage::open(&root.0.join("session.sqlite"), "s1", true, 32).unwrap();
    let workspace = root.0.clone();
    storage
        .call(move |c| {
            c.submit(
                Command {
                    command_id: "create".into(),
                    method: "create_instance".into(),
                    params: json!({"id":"i-leader","workspace_ref":workspace,"profile":{"model":"text"}}),
                },
                Identity::User,
            )
            .unwrap();
            c.submit(
                Command {
                    command_id: "begin".into(),
                    method: "begin_request".into(),
                    params: json!({"instance_id":"i-leader","request_id":"r","revision":0}),
                },
                Identity::User,
            )
            .unwrap();
        })
        .await
        .unwrap();
    let command = Command {
        command_id: "input-a".into(),
        method: "submit_input".into(),
        params: json!({"instance_id":"i-leader","envelope_id":"a","text":"read this","attachments":[label]}),
    };
    let reply = submit(&storage, &root.0, &UserConfig::default(), command.clone()).await.unwrap();
    assert_eq!(reply["queued"], true);
    std::fs::remove_file(root.0.join(label)).unwrap();
    assert_eq!(submit(&storage, &root.0, &UserConfig::default(), command.clone()).await.unwrap(), reply);
    let mut conflict = command;
    conflict.params["text"] = json!("different");
    assert!(submit(&storage, &root.0, &UserConfig::default(), conflict).await.is_err());
    let payload = storage
        .call(|c| {
            c.submit(
                Command {
                    command_id: "edit".into(),
                    method: "edit_input".into(),
                    params: json!({"instance_id":"i-leader","envelope_id":"a","revision":0,"text":"edited"}),
                },
                Identity::User,
            )
            .unwrap();
            let raw: String =
                c.connection().query_row("SELECT payload_json FROM envelopes WHERE id='a'", [], |r| r.get(0)).unwrap();
            assert!(c
                .submit(
                    Command { command_id: "gc".into(), method: "artifact_gc_claim".into(), params: json!({}) },
                    Identity::System
                )
                .unwrap()["claimed"]
                .as_array()
                .unwrap()
                .is_empty());
            c.submit(
                Command {
                    command_id: "drain".into(),
                    method: "drain_inbox".into(),
                    params: json!({"instance_id":"i-leader"}),
                },
                Identity::User,
            )
            .unwrap();
            serde_json::from_str::<Json>(&raw).unwrap()
        })
        .await
        .unwrap();
    let message = json!({"role":"user","content":payload["text"],"attachments":payload["attachments"]});
    let mut entries = vec![ContextEntry::new("e", EntryKind::User, message.clone())];
    expand(&root.0, &mut entries, false, "chat_completions").unwrap();
    let content = entries[0].message["content"].as_str().unwrap();
    assert!(content.contains("edited") && content.contains("> at submission"));
    assert!(!content.contains("before selection"));
    assert!(entries[0].message.get("attachments").is_none());
    let id = payload["attachments"][0]["artifact_id"].as_str().unwrap();
    std::fs::write(root.0.join("artifacts").join(format!("{id}.blob")), "corrupt").unwrap();
    let mut entries = vec![ContextEntry::new("e", EntryKind::User, message)];
    assert!(expand(&root.0, &mut entries, false, "chat_completions").unwrap_err().contains("corrupt"));
    drop(storage);
    let ctl = Control::open(&root.0.join("session.sqlite"), "s1", false).unwrap();
    let count: i64 = ctl
        .connection()
        .query_row("SELECT COUNT(*) FROM input_revisions WHERE envelope_id='a'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}
