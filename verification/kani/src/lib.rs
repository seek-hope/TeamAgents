//! Kani 验证入口：`#[path]` 直接编译仓库里的 `core/src/kernel/types.rs`，
//! 只补一个 `models::now` 垫片（被证明的函数都不读它），因此证明的是**发布的那份代码**。
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
