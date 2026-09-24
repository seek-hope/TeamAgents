//! R2 v2 runtime (plan §3/§6/§9): the persistent phase machine over the v2
//! control plane (`driver`), the multi-instance coordinator (`supervisor`), the
//! bounded storage worker between async I/O and SQLite (`storage`), and the
//! session daemon plus its headless client (`daemon`/`exec`).

pub mod daemon;
pub mod driver;
pub mod exec;
pub mod storage;
pub mod supervisor;
