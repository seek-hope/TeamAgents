pub mod kernel;
pub mod models;
pub mod v2;

/// The core crate's version (the doctor reports the linked core, not a client).
pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Guarded test scratch roots (D-528): a `ta-*` name is a convention, so each root records the gate that made it.
#[cfg(test)]
#[path = "../../test_scratch_owner.rs"]
pub(crate) mod scratch_owner;
