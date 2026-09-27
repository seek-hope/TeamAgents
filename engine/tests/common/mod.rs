//! Shared test scratch: a root that removes itself (D-226).
//!
//! Every suite used to `create_dir_all` its own `<TMPDIR>/teamagents-<name>-<uuid>` and leave it there. Measured
//! 2026-09-27 on one developer machine: **46,016 directories** (`v2-driver` 16,221, `v2-daemon` 9,003,
//! `v2-supervisor` 5,550, `rebuild` 5,838, `jobs` 5,076, `v2-mcp` 4,328 — two days of runs), about **9 GiB** by
//! `df` across their removal (`/tmp` 11 GiB → 2 GiB), and invisible to `review/leak_guard.py` because its stray
//! rule watched the `ta-` prefix only.
//! The tests' own state is not evidence — a *failing probe*'s is (D-140/D-148) — so a root goes when its test
//! does, on every exit path. `engine/tests/cli.rs`'s `Scratch` has had this shape since D-171; this is the same
//! guard for the suites that were missing it.

use std::path::{Path, PathBuf};

/// A fresh scratch directory that removes itself, and any command runner under it, when the test ends.
pub struct TempRoot {
    pub dir: PathBuf,
}

impl TempRoot {
    /// A fresh directory named `<TMPDIR>/teamagents-<name>-<uuid>` (the name the suites already used, so a leak
    /// is still recognisable by the leak guard's stray rule).
    pub fn new(name: &str) -> TempRoot {
        let dir = std::env::temp_dir().join(format!("teamagents-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create the scratch root");
        TempRoot { dir }
    }
}

impl std::ops::Deref for TempRoot {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.dir
    }
}

impl AsRef<Path> for TempRoot {
    fn as_ref(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        // A runner is a real process that outlives its daemon by design (§6.2), so a test that crashed a session
        // can leave one holding this root's job directory. Stop it by pid — found with `pgrep`, never `pkill -f`
        // (D-144's rule: the pattern ban is about the *kill*, not about finding a candidate) — and then take the
        // tree. `leak_guard.py` does the same for the probes (D-148).
        let pattern = format!("jobs-runner.*{}", self.dir.display());
        if let Ok(out) = std::process::Command::new("pgrep").args(["-f", &pattern]).output() {
            for pid in String::from_utf8_lossy(&out.stdout).lines() {
                let _ = std::process::Command::new("kill").args(["-KILL", pid]).output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
