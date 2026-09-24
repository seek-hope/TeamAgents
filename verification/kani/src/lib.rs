//! Kani entry point: `#[path]` compiles the repository's own `core/src/kernel/types.rs`
//! and adds only a `models::now` shim (none of the proven functions reads it), so the
//! proofs apply to the published code.
//!
//! ```text
//! make verify-kani
//! ```
pub mod models {
    pub fn now() -> f64 {
        0.0
    }
}

#[path = "../../../core/src/kernel/types.rs"]
pub mod kernel_types;

mod harnesses;
