//! Async work, tied to the screen that started it or deliberately not.
//!
//! Leptos panics when anything reads a signal that has been disposed, and a
//! release build aborts on panic: the page stays on screen and stops
//! responding. Leaving a screen disposes its signals, so any task still
//! running for it (a response arriving late, a timer firing) must not touch
//! them. Every task in this client starts here, and clippy refuses the raw
//! `spawn_local`, `set_timeout` and `request_animation_frame` elsewhere.
//!
//! Two kinds of work, treated differently:
//!
//! - Reading, to show something: [`Screen::load`]. Cancelled when the
//!   screen goes, since nobody is looking at the answer.
//! - Doing, on the server: [`Screen::act`]. Never cancelled. A deploy, a
//!   removal, a save runs to the end whether or not anyone stays to watch;
//!   only what happens afterwards on screen is skipped if it has gone.
//!
//! The screen is the owner of the component that makes the `Screen`, not
//! whatever reactive block happens to be current when a task starts. A block
//! re-renders as data changes; tying a task to it would cancel the task on
//! an ordinary update and leave a button stuck saying "Working".

use std::future::Future;
use std::time::Duration;

use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct Screen {
    owner: StoredValue<Owner>,
}

impl Screen {
    /// Call at the top of a component, before any reactive block.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner: StoredValue::new(Owner::current().unwrap_or_default()),
        }
    }

    fn owner(self) -> Option<Owner> {
        self.owner.try_get_value()
    }

    /// Runs `fut` while the screen exists; drops it when the screen goes.
    #[allow(clippy::disallowed_methods)] // The one place tasks are spawned.
    pub fn load(self, fut: impl Future<Output = ()> + 'static) {
        if let Some(owner) = self.owner() {
            owner.with(|| leptos::task::spawn_local_scoped_with_cancellation(fut));
        }
    }

    /// Sends `request` and always lets it finish. `then` runs with the
    /// result only if the screen is still there, under its owner.
    #[allow(clippy::disallowed_methods)] // The one place tasks are spawned.
    pub fn act<T: 'static>(
        self,
        request: impl Future<Output = T> + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        leptos::task::spawn_local(async move {
            let result = request.await;
            if let Some(owner) = self.owner() {
                owner.with(|| then(result));
            }
        });
    }

    /// Runs `f` after `delay`, if the screen still exists then.
    #[allow(clippy::disallowed_methods)] // The one place timers are set.
    pub fn after(self, delay: Duration, f: impl FnOnce() + 'static) {
        set_timeout(
            move || {
                if let Some(owner) = self.owner() {
                    owner.with(f);
                }
            },
            delay,
        );
    }

    /// Runs `f` before the next repaint, if the screen still exists then.
    #[allow(clippy::disallowed_methods)] // The one place frames are requested.
    pub fn next_frame(self, f: impl FnOnce() + 'static) {
        request_animation_frame(move || {
            if let Some(owner) = self.owner() {
                owner.with(f);
            }
        });
    }

    /// Wraps `action` so a burst of calls runs it once, `delay` after the
    /// first. A single deploy produces a dozen container events in a second;
    /// reloading on each would fire a dozen overlapping requests for the
    /// same answer.
    pub fn coalesce(
        self,
        delay: Duration,
        action: impl Fn() + Copy + Send + Sync + 'static,
    ) -> impl Fn() + Copy + Send + Sync + 'static {
        let pending = StoredValue::new(false);
        move || {
            if pending.try_get_value().unwrap_or(true) {
                return;
            }
            pending.try_set_value(true);
            self.after(delay, move || {
                pending.try_set_value(false);
                action();
            });
        }
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}
