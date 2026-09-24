//! Wire types shared between the server and the web client.
//!
//! ARCHITECTURAL INVARIANT: this crate must compile for BOTH
//! `x86_64-unknown-linux-gnu` and `wasm32-unknown-unknown`.
//! It may depend only on serialisation crates — never on Leptos,
//! tokio, or anything HTTP. See AGENTS.md.
//!
//! It also runs in the browser, where a panic stops the whole client, so
//! nothing here may panic.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]
// A failing test is meant to panic.
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

pub mod audit;
pub mod auth;
pub mod cleanup;
pub mod container;
pub mod deployment;
pub mod event;
pub mod host;
pub mod logs;
pub mod metrics;
pub mod source;
pub mod stack;
pub mod token;
pub mod update;

/// At most the first `max` characters of `text`, cut on a character boundary
/// so it cannot panic whatever the text holds: ids, hashes, commit shas.
#[must_use]
pub fn short(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(end, _)| text.get(..end).unwrap_or(text))
}

/// The body returned for every unsuccessful API call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    /// Stable, machine-readable discriminator (e.g. `not_authenticated`).
    pub code: String,
    /// Human-readable explanation. Safe to show a user.
    pub message: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_cuts_on_characters_not_bytes() {
        assert_eq!(super::short("abcdef", 3), "abc");
        assert_eq!(super::short("ab", 12), "ab");
        assert_eq!(super::short("", 12), "");
        // A byte slice at 2 would land inside the first character.
        assert_eq!(super::short("ééé", 1), "é");
    }
}
