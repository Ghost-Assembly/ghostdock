//! Core types, state machines, and business rules.
//!
//! ARCHITECTURAL INVARIANT: no I/O. No tokio, no sqlx, no HTTP, no
//! process spawning. Everything here is a pure function of its inputs
//! so it can be unit-tested without a Docker daemon or a database.

pub mod auth;
pub mod contract;
pub mod discovery;
pub mod glob;
pub mod health;
pub mod metrics;
pub mod sizing;
pub mod source;
pub mod stack;
pub mod update;
