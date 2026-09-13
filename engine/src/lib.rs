//! teamagents engine: everything the product needs beyond the authoritative
//! core (runtime loop, member backends, tools, sessions, CLI, worker protocol).
//!
//! Architecture (D-15/D-17): the Rust core owns every authoritative state
//! change; this crate drives it in-process through `teamagents_core::server`,
//! so the former TypeScript orchestration layer and its stdio hop are gone.

pub mod chat;
pub mod cli;
pub mod codex;
pub mod config;
pub mod core_client;
pub mod gateway;
pub mod runtime;
pub mod scripted;
pub mod session;
pub mod sessions;
pub mod tools;
pub mod worker;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
