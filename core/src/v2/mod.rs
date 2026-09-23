//! R2 v2 persistence core: per-session single SQLite, the trusted Control
//! transaction entry, and the §4.1 data contract. Replaces the dual-store
//! coordination of the legacy implementation once the runtime lands (P2).

pub mod control;
pub mod models;
pub mod store;

pub use control::{Command, Control, Identity};
