//! Automations (D-367): user-defined schedules that start a goal on their own.
//!
//! An automation is a prompt plus a period, created by the user and stored beside the session's state
//! (`<state root>/automations.json`). The daemon owns the ticking: at each tick it asks which automations are
//! *due* — enabled and past their `next_at` — and for each one whose previous goal has settled, it opens a goal
//! attached to the Leader and submits the prompt as an ordinary input. The schedule is configuration, not
//! session execution state, so it stays a file; the goals it starts are ordinary goals with the session's own
//! limits, and nothing here can widen the authority a session already has.
//!
//! Three rules are the point of the module, and `V2Schedule.tla` states them: an automation never has two runs
//! in flight at once, a slot is never run twice (`next_at` advances in the same step that records a run), and a
//! disabled automation never runs. A missed slot is coalesced: a scheduled time that passed while the process
//! was down does not produce a backlog of runs, only the one owed now.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

const FILE: &str = "automations.json";
const VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// The period, in seconds. The CLI takes minutes; the smaller unit keeps tests honest.
    pub every_secs: u64,
    pub enabled: bool,
    /// Unix seconds at which the next run is due.
    pub next_at: f64,
    pub last_run: Option<f64>,
    /// The goal the last run opened, so the next tick can tell whether it is still running.
    pub last_goal: Option<String>,
}

pub struct Automations {
    pub home: PathBuf,
    pub items: Vec<Automation>,
}

impl Automations {
    /// Load the automations of a state root, or an empty set when the file does not exist.
    ///
    /// A file this build cannot read in full — an unknown version, a duplicate id, a zero period, an empty
    /// prompt — is refused rather than partially applied, the same rule the session registry uses.
    pub fn load(home: &Path) -> Result<Automations, String> {
        let file = home.join(FILE);
        if !file.exists() {
            return Ok(Automations { home: home.to_path_buf(), items: Vec::new() });
        }
        let text = std::fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
        let parsed: Json = serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", file.display()))?;
        let version = parsed["version"].as_i64().unwrap_or(-1);
        if version != VERSION {
            return Err(format!(
                "{} is version {version}, this build writes version {VERSION}: refusing to reinterpret it",
                file.display()
            ));
        }
        let mut items: Vec<Automation> = Vec::new();
        for raw in parsed["automations"].as_array().cloned().unwrap_or_default() {
            let id = raw["id"].as_str().unwrap_or("").to_string();
            let prompt = raw["prompt"].as_str().unwrap_or("").to_string();
            let every_secs = raw["every_secs"].as_u64().unwrap_or(0);
            if id.is_empty() || prompt.trim().is_empty() || every_secs == 0 {
                return Err(format!(
                    "{}: an automation is missing its id, prompt or period (id {id:?}, every_secs {every_secs})",
                    file.display()
                ));
            }
            if items.iter().any(|existing| existing.id == id) {
                return Err(format!("{}: two automations share the id {id:?}", file.display()));
            }
            items.push(Automation {
                id,
                name: raw["name"].as_str().unwrap_or("").to_string(),
                prompt,
                every_secs,
                enabled: raw["enabled"].as_bool().unwrap_or(true),
                next_at: raw["next_at"].as_f64().unwrap_or(0.0),
                last_run: raw["last_run"].as_f64(),
                last_goal: raw["last_goal"].as_str().map(str::to_string),
            });
        }
        Ok(Automations { home: home.to_path_buf(), items })
    }

    /// Write the schedule atomically (tmp + fsync + rename), so a crash never leaves a half file.
    pub fn save(&self) -> Result<(), String> {
        let file = self.home.join(FILE);
        std::fs::create_dir_all(&self.home).map_err(|e| format!("create {}: {e}", self.home.display()))?;
        let body = json!({
            "version": VERSION,
            "automations": self.items.iter().map(|item| json!({
                "id": item.id,
                "name": item.name,
                "prompt": item.prompt,
                "every_secs": item.every_secs,
                "enabled": item.enabled,
                "next_at": item.next_at,
                "last_run": item.last_run,
                "last_goal": item.last_goal,
            })).collect::<Vec<_>>(),
        });
        let tmp = self.home.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::File::open(&tmp)
            .and_then(|file| file.sync_all())
            .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &file).map_err(|e| format!("publish {}: {e}", file.display()))?;
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&Automation> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Create an automation; its first run is one period out.
    pub fn add(&mut self, name: &str, prompt: &str, every_secs: u64, now: f64) -> Result<Automation, String> {
        if every_secs == 0 {
            return Err("an automation needs a period larger than zero".into());
        }
        if prompt.trim().is_empty() {
            return Err("an automation needs a prompt".into());
        }
        let mut id = String::new();
        for _ in 0..32 {
            let candidate = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            if self.find(&candidate).is_none() {
                id = candidate;
                break;
            }
        }
        if id.is_empty() {
            return Err("could not find a free automation id after 32 tries".into());
        }
        let entry = Automation {
            id: id.clone(),
            name: if name.trim().is_empty() { id.clone() } else { name.trim().to_string() },
            prompt: prompt.to_string(),
            every_secs,
            enabled: true,
            next_at: now + every_secs as f64,
            last_run: None,
            last_goal: None,
        };
        self.items.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    pub fn remove(&mut self, id: &str) -> Result<Automation, String> {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Err(format!("no automation {id:?}; `teamagents automations` lists them"));
        };
        let removed = self.items.remove(index);
        self.save()?;
        Ok(removed)
    }

    /// Enable or disable an automation. Enabling re-arms the next run one period out, so resuming after a long
    /// pause does not immediately fire a stale schedule.
    pub fn set_enabled(&mut self, id: &str, enabled: bool, now: f64) -> Result<Automation, String> {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Err(format!("no automation {id:?}; `teamagents automations` lists them"));
        };
        self.items[index].enabled = enabled;
        if enabled {
            self.items[index].next_at = now + self.items[index].every_secs as f64;
        }
        let entry = self.items[index].clone();
        self.save()?;
        Ok(entry)
    }

    /// The enabled automations whose next run is due. A disabled one is never due, whatever its `next_at` says.
    pub fn due(&self, now: f64) -> Vec<Automation> {
        self.items.iter().filter(|item| item.enabled && item.next_at <= now).cloned().collect()
    }

    /// Record that a run started: advance `next_at` by one period from *now* (coalescing any missed slots into
    /// this single run) and remember the goal, so the next tick can tell whether it is still running.
    pub fn mark_run(&mut self, id: &str, now: f64, goal_id: &str) -> Result<(), String> {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Err(format!("no automation {id:?} to record a run for"));
        };
        self.items[index].last_run = Some(now);
        self.items[index].last_goal = Some(goal_id.to_string());
        self.items[index].next_at = now + self.items[index].every_secs as f64;
        self.save()
    }

    /// Filesystem-only status for `automations list`: never opens the database.
    pub fn rows(&self) -> Vec<Json> {
        self.items
            .iter()
            .map(|item| {
                json!({
                    "id": item.id,
                    "name": item.name,
                    "prompt": item.prompt,
                    "every_secs": item.every_secs,
                    "enabled": item.enabled,
                    "next_at": item.next_at,
                    "last_run": item.last_run,
                    "last_goal": item.last_goal,
                })
            })
            .collect()
    }
}

/// The path the schedule lives at, for callers that only need to name it.
pub fn file(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The daemon's tick, in milliseconds. A test shortens it (the same shape as `TEAMAGENTS_JOB_IDLE_TICK_MS`).
pub fn tick() -> std::time::Duration {
    std::time::Duration::from_millis(tick_ms())
}

fn tick_ms() -> u64 {
    std::env::var("TEAMAGENTS_AUTOMATION_TICK_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(30_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("ta-automations-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_automation_round_trips_and_is_neither_due_nor_runnable_while_disabled() {
        let scratch = Scratch::new("roundtrip");
        let mut schedule = Automations::load(&scratch.0).unwrap();
        let entry = schedule.add("hourly", "summarize the changelog", 3600, 1000.0).unwrap();
        assert_eq!(entry.next_at, 4600.0, "the first run is one period out");
        let reloaded = Automations::load(&scratch.0).unwrap();
        assert_eq!(reloaded.find(&entry.id), Some(&entry));
        // not due before its time, due after
        assert!(reloaded.due(4599.0).is_empty());
        assert_eq!(reloaded.due(4600.0).len(), 1);
        // disabled is never due, however overdue
        let mut paused = reloaded;
        paused.set_enabled(&entry.id, false, 5000.0).unwrap();
        assert!(paused.due(1_000_000.0).is_empty());
        // enabling re-arms one period out
        let resumed = paused.set_enabled(&entry.id, true, 5000.0).unwrap();
        assert_eq!(resumed.next_at, 8600.0);
    }

    #[test]
    fn a_run_advances_the_slot_so_a_missed_backlog_is_coalesced() {
        let scratch = Scratch::new("coalesce");
        let mut schedule = Automations::load(&scratch.0).unwrap();
        let entry = schedule.add("", "ping", 60, 0.0).unwrap();
        // the process was down for an hour: only one run is owed, and the next slot is one period from now
        assert_eq!(schedule.due(3600.0).len(), 1);
        schedule.mark_run(&entry.id, 3600.0, "goal-auto-1").unwrap();
        assert_eq!(schedule.find(&entry.id).unwrap().next_at, 3660.0);
        assert!(schedule.due(3600.0).is_empty(), "the same slot never runs twice");
        assert_eq!(schedule.find(&entry.id).unwrap().last_goal.as_deref(), Some("goal-auto-1"));
    }

    #[test]
    fn a_malformed_schedule_is_refused_rather_than_partially_applied() {
        let scratch = Scratch::new("validate");
        std::fs::write(file(&scratch.0), r#"{"version":99,"automations":[]}"#).unwrap();
        assert!(Automations::load(&scratch.0).is_err());
        std::fs::write(file(&scratch.0), r#"{"version":1,"automations":[{"id":"a","prompt":"x","every_secs":0}]}"#)
            .unwrap();
        assert!(Automations::load(&scratch.0).is_err());
    }
}
