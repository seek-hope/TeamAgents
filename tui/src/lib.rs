//! Library surface for tests; the binary is a thin shell over these modules.

pub mod app;
pub mod daemon_client;
pub mod history;
pub mod i18n;
pub mod md;
pub mod model_picker;
pub mod review;
pub mod text;
pub mod theme;
pub mod ui;
pub mod worker;

pub use app::{cancel_task_feedback, decide_feedback, App, Effect, Focus, OpResult, Severity};
