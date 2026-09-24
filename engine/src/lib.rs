//! teamagents engine: everything the product needs beyond the authoritative
//! core (runtime loop, member backends, tools, sessions, CLI, worker protocol).
//!
//! Architecture (D-15/D-17): the Rust core owns every authoritative state
//! change; this crate drives it in-process through `teamagents_core::server`,
//! so the former TypeScript orchestration layer and its stdio hop are gone.

/// Default v2 session state root (plan §14: a separate `v2/` root while the
/// legacy layout is still identifiable, §4.4).
pub fn v2_root() -> std::path::PathBuf {
    config::state_dir().join("v2")
}

pub mod bound;
pub mod chat;
pub mod cli;
pub mod codex;
pub mod config;
pub mod core_client;
pub mod gateway;
pub mod history;
pub mod hooks;
pub mod jobs;
pub mod mcp;
pub mod observability;
pub mod providers;
pub mod reference;
pub mod review;
pub mod runtime;
pub mod scripted;
pub mod session;
pub mod sessions;
mod stream;
pub mod tools;
pub mod v2;
pub mod worker;
pub mod workspace;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tests that repoint XDG_* environment variables must hold this lock: the
/// process environment is shared by every test thread in the binary.
#[cfg(test)]
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
