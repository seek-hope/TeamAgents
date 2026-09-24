pub mod control;
pub mod kernel;
pub mod models;
mod references;
pub mod server;
pub mod storage;
pub mod v2;
pub mod views;

/// The core crate's version (the doctor reports the linked core, not a client).
pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
