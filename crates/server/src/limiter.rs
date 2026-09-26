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
//!
//! Usernames and addresses are counted in separate tables, each bounded on
//! its own, so filling one cannot stop the other counting.

use std::collections::HashMap;
use std::hash::Hash;
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
    /// Most usernames, and separately most addresses, remembered at once,
    /// so a flood of made-up ones cannot grow memory without bound.
    ///
    /// A lockout is never forgotten to make room. When every entry in a
    /// table is a lockout, a username or address not already in it is
    /// refused rather than let through uncounted. The trade-off, accepted
    /// knowingly: someone who locks out about this many usernames (or
    /// addresses) keeps new ones of that kind out until the lockouts end,
    /// one window later.
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

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    failures: u32,
}

/// One kind of key's counts, with that kind's limit.
#[derive(Debug)]
struct Table<K> {
    windows: HashMap<K, Window>,
    limit: u32,
}

impl<K: Eq + Hash + Clone> Table<K> {
    fn new(limit: u32) -> Self {
        Self {
            windows: HashMap::new(),
            limit,
        }
    }

    fn locked(&self, w: &Window, now: Instant, window: Duration) -> bool {
        now.duration_since(w.started) < window && w.failures >= self.limit
    }

    /// Full, and every entry a lockout: nothing can make room.
    fn saturated(&self, now: Instant, limits: &Limits) -> bool {
        self.windows.len() >= limits.capacity
            && self
                .windows
                .values()
                .all(|w| self.locked(w, now, limits.window))
    }

    fn allows(&self, key: &K, now: Instant, limits: &Limits) -> bool {
        match self.windows.get(key) {
            Some(w) => now.duration_since(w.started) >= limits.window || w.failures < self.limit,
            // Fail closed: a key that could not be counted is not let in.
            None => !self.saturated(now, limits),
        }
    }

    fn failed(&mut self, key: K, now: Instant, limits: &Limits) {
        if !self.windows.contains_key(&key) && self.windows.len() >= limits.capacity {
            let window = limits.window;
            self.windows
                .retain(|_, w| now.duration_since(w.started) < window);
            if self.windows.len() >= limits.capacity {
                // Only an entry holding no one out may make room.
                // Evicting a lockout would let a burst of made-up keys buy
                // its owner a fresh allowance.
                let oldest = self
                    .windows
                    .iter()
                    .filter(|(_, w)| !self.locked(w, now, window))
                    .min_by_key(|(_, w)| w.started)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(oldest) => {
                        self.windows.remove(&oldest);
                    }
                    // Every entry is a lockout. Not counted, and `allows`
                    // refuses this key until one ends.
                    None => return,
                }
            }
        }
        let entry = self.windows.entry(key).or_insert(Window {
            started: now,
            failures: 0,
        });
        if now.duration_since(entry.started) >= limits.window {
            *entry = Window {
                started: now,
                failures: 0,
            };
        }
        entry.failures = entry.failures.saturating_add(1);
    }
}

#[derive(Debug)]
struct Tables {
    users: Table<String>,
    addresses: Table<IpAddr>,
}

/// Counts failed sign-ins. Cheap to clone; clones share their counts.
#[derive(Debug, Clone)]
pub struct LoginLimiter {
    limits: Limits,
    tables: Arc<Mutex<Tables>>,
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl LoginLimiter {
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            tables: Arc::new(Mutex::new(Tables {
                users: Table::new(limits.per_user),
                addresses: Table::new(limits.per_address),
            })),
        }
    }

    /// Whether another attempt may be made for `username` from `address`.
    #[must_use]
    pub fn allows(&self, username: &str, address: Option<IpAddr>, now: Instant) -> bool {
        let tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        tables.users.allows(&user_key(username), now, &self.limits)
            && address.is_none_or(|a| tables.addresses.allows(&a, now, &self.limits))
    }

    /// Counts a failed attempt against the username and the address.
    pub fn failed(&self, username: &str, address: Option<IpAddr>, now: Instant) {
        let mut tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        tables.users.failed(user_key(username), now, &self.limits);
        if let Some(address) = address {
            tables.addresses.failed(address, now, &self.limits);
        }
    }

    /// Clears the username's count once its owner signs in. The address's
    /// stays: one success from a shared address says nothing about the rest.
    pub fn succeeded(&self, username: &str) {
        self.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .users
            .windows
            .remove(&user_key(username));
    }

    /// How many usernames and addresses are remembered, together.
    #[must_use]
    pub fn remembered(&self) -> usize {
        let tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        tables.users.windows.len() + tables.addresses.windows.len()
    }
}

/// Usernames are matched case-insensitively at sign-in, so they are counted
/// that way too.
fn user_key(username: &str) -> String {
    username.to_lowercase()
}
