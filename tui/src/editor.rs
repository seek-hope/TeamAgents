//! D-454: edit the composer draft in the user's external editor.
//!
//! The editor is chosen from `VISUAL`, then `EDITOR` (or the explicit argument
//! used by tests). A nonzero exit, a missing editor or a read failure leaves the
//! caller's draft untouched; only a successful edit returns new text.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;

pub fn edit_text(initial: &str, editor: Option<&str>) -> Result<String, String> {
    let editor = editor
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("VISUAL").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
        })
        .or_else(|| {
            std::env::var("EDITOR").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
        })
        .ok_or_else(|| "set VISUAL or EDITOR to use the external editor".to_string())?;
    let dir = std::env::temp_dir().join(format!("teamagents-edit-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|error| format!("cannot create the editor directory: {error}"))?;
    let path = dir.join("draft.md");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot create the draft file: {error}"))?;
        file.write_all(initial.as_bytes()).map_err(|error| format!("cannot write the draft file: {error}"))?;
        file.sync_all().ok();
        drop(file);
        let command = format!("{editor} {}", shell_quote(&path));
        match Command::new("sh").arg("-c").arg(&command).status() {
            Ok(status) if status.success() => {
                std::fs::read_to_string(&path).map_err(|error| format!("cannot read the edited draft: {error}"))
            }
            Ok(status) => Err(format!("the editor exited with {status}; the draft is unchanged")),
            Err(error) => Err(format!("cannot start the editor: {error}; the draft is unchanged")),
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("editor.sh");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn the_editor_can_replace_the_draft_and_a_cancel_preserves_it() {
        let dir = std::env::temp_dir().join(format!("ta-editor-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = script(&dir, "#!/bin/sh\nprintf 'edited text' > \"$1\"\n");
        assert_eq!(edit_text("original", Some(write.to_str().unwrap())).unwrap(), "edited text");
        let cancel = script(&dir, "#!/bin/sh\nexit 1\n");
        let error = edit_text("original", Some(cancel.to_str().unwrap())).unwrap_err();
        assert!(error.contains("draft is unchanged"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
