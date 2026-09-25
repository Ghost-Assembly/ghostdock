//! Slowing down password guessing.
//!
//! Failed sign-ins are counted per username and per client address over a
//! fixed window; past a limit, further attempts are refused until the window
//! ends, without checking the password at all. In memory only: a restart
//! forgets the counts, which costs an attacker a restart they cannot cause.
//!
//! The per-address limit is the higher one. Behind a reverse proxy every
//! client shares the proxy's address, and a limit tuned for one person would
//! lock everyone out.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How attempts are limited.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Failures allowed per username in one window.
    pub per_user: u32,
    /// Failures allowed per client address in one window.
    pub per_address: u32,
    pub window: Duration,
    /// Most usernames and addresses remembered at once, so a flood of
    /// made-up usernames cannot grow memory without bound.
    pub capacity: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            per_user: 10,
            per_address: 50,
            window: Duration::from_secs(15 * 60),
            capacity: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    User(String),
    Address(IpAddr),
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    failures: u32,
}

/// Counts failed sign-ins. Cheap to clone; clones share their counts.
#[derive(Debug, Clone, Default)]
pub struct LoginLimiter {
    limits: Limits,
    windows: Arc<Mutex<HashMap<Key, Window>>>,
}

impl LoginLimiter {
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            windows: Arc::default(),
        }
    }

    /// Whether another attempt may be made for `username` from `address`.
    #[must_use]
    pub fn allows(&self, username: &str, address: Option<IpAddr>, now: Instant) -> bool {
        let windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        self.keys(username, address).all(|(key, limit)| {
            windows.get(&key).is_none_or(|w| {
                now.duration_since(w.started) >= self.limits.window || w.failures < limit
            })
        })
    }

    /// Counts a failed attempt against the username and the address.
    pub fn failed(&self, username: &str, address: Option<IpAddr>, now: Instant) {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        for (key, _) in self.keys(username, address) {
            if !windows.contains_key(&key) && windows.len() >= self.limits.capacity {
                let window = self.limits.window;
                windows.retain(|_, w| now.duration_since(w.started) < window);
                if windows.len() >= self.limits.capacity
                    && let Some(oldest) = windows
                        .iter()
                        .min_by_key(|(_, w)| w.started)
                        .map(|(k, _)| k.clone())
                {
                    windows.remove(&oldest);
                }
            }
            let entry = windows.entry(key).or_insert(Window {
                started: now,
                failures: 0,
            });
            if now.duration_since(entry.started) >= self.limits.window {
                *entry = Window {
                    started: now,
                    failures: 0,
                };
            }
            entry.failures = entry.failures.saturating_add(1);
        }
    }

    /// Clears the username's count once its owner signs in. The address's
    /// stays: one success from a shared address says nothing about the rest.
    pub fn succeeded(&self, username: &str) {
        self.windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&Key::User(username.to_lowercase()));
    }

    /// How many usernames and addresses are remembered.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn keys(&self, username: &str, address: Option<IpAddr>) -> impl Iterator<Item = (Key, u32)> {
        // Usernames are matched case-insensitively at sign-in, so they are
        // counted that way too.
        std::iter::once((Key::User(username.to_lowercase()), self.limits.per_user))
            .chain(address.map(|a| (Key::Address(a), self.limits.per_address)))
    }
}
