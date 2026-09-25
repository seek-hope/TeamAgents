//! Library surface for tests; the binary is a thin shell over these modules.

pub mod daemon_client;
pub mod text;
pub mod theme;
pub mod v2app;
pub mod v2ui;
pub mod wrap;

/// Epoch seconds, the unit the daemon reports deadlines in. A small module-level
/// helper so the status line and its test agree on what "now" means.
pub fn now_epoch() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|since| since.as_secs_f64()).unwrap_or(0.0)
}
