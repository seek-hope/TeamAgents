//! Scratch-root ownership, shared by every guarded temp-root constructor in this workspace (D-528).
//!
//! A *process* carries its owning gate in its environment; a directory cannot, and a `ta-*` / `teamagents-*` name is
//! a convention that a concurrent session of the same user reproduces exactly. So the constructor that creates a
//! root records it in the gate's registry (a plain file in `TMPDIR`, never inside the root), and
//! `tools/leak_guard.py`'s audit reads that registry to tell this gate's leftovers from a peer's — or reports the
//! directory as *unattributed* when it is in nobody's registry, which is the honest answer and never a guess.
//!
//! Included with `#[path]` from each crate's own test code (`core`, `engine`, `tui` and the integration suites), so
//! one rule serves every constructor instead of drifting copies. Nothing here reads project state or product data:
//! it is a diagnostic file next to a test's own scratch root, and it is only ever written at *creation*.

// Each including test binary uses a subset of this helper (a CLI suite marks roots it made, a unit-test
// module uses only the record write), so an item unused by one binary is not dead code. Test-only file.
#![allow(dead_code)]

/// The registry a gate's roots are recorded in: `$TMPDIR/.ta-leak-owner-<marker>`.
///
/// A *file* in the gate's `TMPDIR`, deliberately never inside the root it describes: a test's scratch root is
/// functional fixture state (an artifact directory the product scans, a database directory it budgets, a directory
/// a test makes read-only), and an extra file in it changed those fixtures — measured as 20 failing tests and two
/// leaked directories before this shape replaced it. A plain file is also outside the stray rule, which counts
/// directories only (D-143).
pub fn registry_path(marker: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(".ta-leak-owner-{marker}"))
}

/// Record that this gate created `dir`.
///
/// A gate that exported a marker asked to be able to attribute its roots to itself, so a failure to record **fails
/// visibly**: an unrecorded root is indistinguishable from a peer's, and silently accepting that would turn "this
/// gate could not attribute its own roots" into a clean audit. Appends are short single lines, so parallel test
/// threads cannot interleave them.
pub fn record_ownership(dir: &std::path::Path) {
    use std::io::Write;
    let Ok(marker) = std::env::var("TEAMAGENTS_LEAK_GUARD_RUN") else { return };
    if marker.is_empty() {
        return;
    }
    let registry = registry_path(&marker);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&registry)?;
        writeln!(file, "{}", dir.display())
    };
    write().unwrap_or_else(|error| {
        panic!(
            "the gate marker is set but {} could not be recorded in {} ({error}): this root would be unattributable",
            dir.display(),
            registry.display()
        )
    });
}

/// Create a fresh guarded scratch root and record it, in one step.
///
/// Creation only: never call this onto a root a test then makes read-only, and never write into the root itself.
pub fn owned_temp_dir(prefix: &str, suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
    std::fs::create_dir_all(&dir).expect("create the scratch root");
    record_ownership(&dir);
    dir
}
