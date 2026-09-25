//! GhostDock server library.
//!
//! ARCHITECTURAL INVARIANT: this crate must never depend on Leptos.
//! No server functions, ever. The frontend talks to a plain JSON + SSE
//! API so that it remains a replaceable client. See the design spec.

pub mod accounts;
pub mod app;
pub mod audit;
pub mod auth;
pub mod checker;
pub mod detach;
pub mod discovery;
pub mod error;
pub mod events;
pub mod exec;
pub mod hosts;
pub mod limiter;
pub mod mcp;
pub mod metrics;
pub mod ops;
pub mod origin;
pub mod reference;
pub mod revocation;
pub mod runner;
pub mod session_store;
pub mod socket;
pub mod sources;
pub mod stacks;
pub mod state;
pub mod tokens;
pub mod updates;
pub mod watch;
