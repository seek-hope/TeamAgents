//! Durable memory across sessions (D-405).
//!
//! A session's context dies with its goal and its database is per session; what survives is this: one
//! append-only note store at the state root (`<state root>/memory.json`), which any session under that root can
//! recall from. It is the smallest thing that answers "the agent learned this last week", and the three rules that
//! matter are the module's whole design:
//!
//! * **append-only** — a note is added, never rewritten or evicted; `MAX_NOTES` refuses rather than forgets, so a
//!   recall that once answered keeps answering the same way (`V2Memory.tla`'s `NotesAreAppendOnly`);
//! * **bounded** — a note's text is capped at `NOTE_MAX_CHARS` and a recall returns at most `RECALL_MAX` notes
//!   (`RecallIsBounded`), because a memory that can grow without limit is a context leak waiting to happen;
//! * **provenanced** — every note records the session, the instance and the time it came from, so a reader can
//!   tell who remembered it and when, and a wrong memory is traceable rather than anonymous.
//!
//! Recall is a **substring and tag match, newest first** — deliberately not a semantic ranking. The ceiling is
//! stated rather than hidden: a store this size is searched by the model reading the results, which is what the
//! `memory` tool's description says.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

const FILE: &str = "memory.json";
const VERSION: i64 = 1;

/// One note's text cap. A memory is a pointer — "the parser rejects tabs, see D-12" — not a document store.
pub const NOTE_MAX_CHARS: usize = 2_000;
/// How many notes the store keeps. Past this a `remember` is refused, because evicting an older note would change
/// what an earlier recall answered.
pub const MAX_NOTES: usize = 500;
/// The most notes one recall may return, whatever the caller asks for.
pub const RECALL_MAX: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub tags: Vec<String>,
    pub session: String,
    pub instance: String,
    pub created: f64,
}

pub struct Memory {
    home: PathBuf,
    pub notes: Vec<Note>,
}

impl Memory {
    /// Read the store, or start an empty one. A missing file is not an error (the first `remember` writes it); a
    /// *corrupt* one is, so a damaged store is visible instead of silently replaced.
    pub fn load(home: &Path) -> Result<Memory, String> {
        let file = home.join(FILE);
        let mut notes = Vec::new();
        if file.exists() {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
            let body: Json = serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", file.display()))?;
            let version = body["version"].as_i64().unwrap_or(0);
            if version != VERSION {
                return Err(format!("{}: version {version} is not the supported {VERSION}", file.display()));
            }
            for raw in body["notes"].as_array().cloned().unwrap_or_default() {
                notes.push(Note {
                    id: raw["id"].as_str().unwrap_or_default().to_string(),
                    text: raw["text"].as_str().unwrap_or_default().to_string(),
                    tags: raw["tags"]
                        .as_array()
                        .map(|tags| tags.iter().filter_map(|tag| tag.as_str().map(str::to_string)).collect())
                        .unwrap_or_default(),
                    session: raw["session"].as_str().unwrap_or_default().to_string(),
                    instance: raw["instance"].as_str().unwrap_or_default().to_string(),
                    created: raw["created"].as_f64().unwrap_or(0.0),
                });
            }
        }
        Ok(Memory { home: home.to_path_buf(), notes })
    }

    /// Add one note and persist it. Everything that can go wrong here is refused *before* the file changes.
    pub fn remember(
        &mut self,
        text: &str,
        tags: &[String],
        session: &str,
        instance: &str,
        now: f64,
    ) -> Result<Note, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("memory: the note text must not be empty".into());
        }
        if text.chars().count() > NOTE_MAX_CHARS {
            return Err(format!(
                "memory: the note is {} characters; the cap is {NOTE_MAX_CHARS} (store a pointer to the artifact or file instead)",
                text.chars().count()
            ));
        }
        if self.notes.len() >= MAX_NOTES {
            return Err(format!(
                "memory: the store already holds {MAX_NOTES} notes and never evicts one; keep the store useful by hand"
            ));
        }
        let note = Note {
            id: format!("m-{}", uuid::Uuid::new_v4().simple()),
            text: text.to_string(),
            tags: tags.iter().map(|tag| tag.trim().to_lowercase()).filter(|tag| !tag.is_empty()).collect(),
            session: session.to_string(),
            instance: instance.to_string(),
            created: now,
        };
        self.notes.push(note.clone());
        self.save()?;
        Ok(note)
    }

    /// The newest notes matching a query, at most [`RECALL_MAX`]. An empty query means "the newest ones", which is
    /// how a fresh session asks what the last one left behind. Matching is case-insensitive substring on the text
    /// or an exact tag.
    pub fn recall(&self, query: &str, limit: usize) -> Vec<&Note> {
        let limit = limit.clamp(1, RECALL_MAX);
        let needle = query.trim().to_lowercase();
        self.notes
            .iter()
            .rev()
            .filter(|note| {
                needle.is_empty() || note.text.to_lowercase().contains(&needle) || note.tags.contains(&needle)
            })
            .take(limit)
            .collect()
    }

    fn save(&self) -> Result<(), String> {
        let file = self.home.join(FILE);
        std::fs::create_dir_all(&self.home).map_err(|e| format!("create {}: {e}", self.home.display()))?;
        let body = json!({
            "version": VERSION,
            "notes": self.notes.iter().map(Note::to_json).collect::<Vec<_>>(),
        });
        let tmp = self.home.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::File::open(&tmp)
            .and_then(|file| file.sync_all())
            .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &file).map_err(|e| format!("rename {}: {e}", file.display()))?;
        Ok(())
    }
}

impl Note {
    fn to_json(&self) -> Json {
        json!({"id": self.id, "text": self.text, "tags": self.tags, "session": self.session,
               "instance": self.instance, "created": self.created})
    }

    /// The line a recall prints: the text, then who remembered it and when — provenance is part of the answer, not
    /// metadata a reader has to go looking for.
    pub fn render(&self) -> String {
        let tags = if self.tags.is_empty() { String::new() } else { format!(" [{}]", self.tags.join(", ")) };
        format!("{} ({}, {}, {}){tags}", self.text, self.session, self.instance, format_time(self.created))
    }
}

/// `created` is Unix seconds; the store's own clock is what a reader sees, not a locale.
fn format_time(seconds: f64) -> String {
    format!("t={}", seconds as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ta-memory-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn notes_survive_a_reload_with_their_provenance() {
        let dir = home("reload");
        let mut memory = Memory::load(&dir).expect("empty store");
        memory
            .remember("the parser rejects tabs (D-12)", &["parser".into()], "s-one", "i-leader", 100.0)
            .expect("remember");
        let reloaded = Memory::load(&dir).expect("reload");
        assert_eq!(reloaded.notes.len(), 1);
        let note = &reloaded.notes[0];
        assert_eq!(note.session, "s-one");
        assert_eq!(note.instance, "i-leader");
        assert_eq!(note.tags, vec!["parser".to_string()]);
        assert!(note.id.starts_with("m-"), "{}", note.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recall_filters_by_text_and_tag_newest_first_and_is_bounded() {
        let dir = home("recall");
        let mut memory = Memory::load(&dir).unwrap();
        for index in 0..25 {
            memory.remember(&format!("note {index} about the parser"), &[], "s", "i", index as f64).expect("remember");
        }
        memory.remember("the answer is 42", &["answer".into()], "s", "i", 999.0).expect("remember");
        // a substring match, newest first
        let hits = memory.recall("parser", 50);
        assert_eq!(hits.len(), RECALL_MAX, "the cap wins over the caller's ask");
        assert_eq!(hits[0].text, "note 24 about the parser");
        // a tag is an exact match
        assert_eq!(memory.recall("answer", 5).len(), 1);
        assert_eq!(memory.recall("ANSWER", 5).len(), 1, "matching is case-insensitive");
        // an empty query is "the newest", which is what a fresh session asks
        assert_eq!(memory.recall("", 3)[0].text, "the answer is 42");
        assert!(memory.recall("nothing matches this", 5).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_note_is_capped_and_the_store_never_evicts() {
        let dir = home("caps");
        let mut memory = Memory::load(&dir).unwrap();
        let long = "x".repeat(NOTE_MAX_CHARS + 1);
        assert!(memory.remember(&long, &[], "s", "i", 0.0).unwrap_err().contains("the cap is"));
        assert!(memory.remember("   ", &[], "s", "i", 0.0).unwrap_err().contains("must not be empty"));
        // fill the store: the last one is refused, and the first is still there
        for index in 0..MAX_NOTES {
            memory.remember(&format!("note {index}"), &[], "s", "i", index as f64).expect("remember");
        }
        let error = memory.remember("one too many", &[], "s", "i", 0.0).unwrap_err();
        assert!(error.contains("never evicts"), "{error}");
        assert_eq!(memory.notes.len(), MAX_NOTES);
        assert_eq!(memory.notes[0].text, "note 0", "the oldest note is still the first");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
