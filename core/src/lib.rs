pub mod kernel;
pub mod models;
pub mod v2;

/// The core crate's version (the doctor reports the linked core, not a client).
pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
