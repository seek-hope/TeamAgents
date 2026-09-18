//! Process environment isolation shared by integration tests in one binary.

use std::ffi::{OsStr, OsString};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Keep this guard alive until all workers and child processes have stopped.
#[must_use = "hold the environment guard for the entire test"]
pub struct TestEnv {
    root: PathBuf,
    previous: Vec<(OsString, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl TestEnv {
    pub fn new(tag: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!("ta-engine-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("config")).expect("create test environment");
        let mut env = Self { root, previous: Vec::new(), _lock: lock };
        env.set("XDG_STATE_HOME", env.root.clone());
        env.set("XDG_CONFIG_HOME", env.root.join("config"));
        env
    }

    /// Record the original value once, including non-Unicode values and absence.
    pub fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let key = key.as_ref();
        if !self.previous.iter().any(|(saved, _)| saved == key) {
            self.previous.push((key.to_owned(), std::env::var_os(key)));
        }
        std::env::set_var(key, value);
    }
}

impl Deref for TestEnv {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.iter().rev() {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
