//! R2 v2 runtime (plan §3/§6): the persistent single-instance driver over the
//! v2 control plane, with the bounded storage worker between async I/O and
//! SQLite. Multi-instance scheduling arrives with P3; the phase machine,
//! recovery matrix and job protocol here are the ones P3 reuses.

pub mod driver;
pub mod storage;
