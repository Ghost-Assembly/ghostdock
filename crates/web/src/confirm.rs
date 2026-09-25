//! A destructive action that takes two taps.
//!
//! A modal would be heavier than the decision warrants, and a native
//! `confirm()` is unreliable in installed web apps. Arming in place keeps
//! the second tap exactly where the thumb already is.
//!
//! The button that arms is the button that confirms: only its words change,
//! so keyboard focus stays on it, and a screen reader is told what the next
//! press will do. Left armed, it disarms itself after a few seconds, so a
//! tap much later is a first tap again.

use std::time::Duration;

use leptos::html::Button;
use leptos::prelude::*;

use crate::screen::Screen;
use crate::ui::Icon;

/// How long an armed button waits for the second tap.
const ARMED_FOR: Duration = Duration::from_secs(5);

#[component]
pub fn Confirm(
    /// What the button says before it is armed.
    #[prop(into)]
    label: Signal<String>,
    /// What it says once armed; the tap that follows does the thing.
    confirm: &'static str,
    /// What it acts on, where the words alone do not say: a list of rows
    /// each with a "Remove" needs each one named to a screen reader.
    #[prop(optional, into)]
    subject: Option<String>,
    /// While true, neither tap does anything: set it while the action is
    /// in flight, so a second pair of taps cannot send it twice.
    #[prop(optional)]
    disabled: Option<Signal<bool>>,
    /// The small form that sits at the end of a row in a list.
    #[prop(optional)]
    row: bool,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let screen = Screen::new();
    let armed = RwSignal::new(false);
    // Counts armings, so a timer left from an earlier one does nothing.
    let armings = StoredValue::new(0_u32);
    let button = NodeRef::<Button>::new();
    let disabled = move || disabled.is_some_and(|d| d.get());

    let press = move |_| {
        if disabled() {
            return;
        }
        if armed.get_untracked() {
            armed.set(false);
            on_confirm.run(());
            return;
        }
        armed.set(true);
        let mine = armings.get_value().wrapping_add(1);
        armings.set_value(mine);
        screen.after(ARMED_FOR, move || {
            if armings.get_value() == mine {
                armed.set(false);
            }
        });
    };
    let cancel = move || {
        armed.set(false);
        if let Some(el) = button.get_untracked() {
            let _ = el.focus();
        }
    };

    // Signals rather than closures: every closure in a view is compiled
    // into code of its own, and this component appears on many screens.
    let words = Signal::derive(move || {
        if armed.get() {
            confirm.to_owned()
        } else {
            label.get()
        }
    });
    let name = Signal::derive(move || subject.as_ref().map(|s| format!("{}: {s}", words.get())));
    let marked = Signal::derive(move || armed.get().then_some("true"));
    let said = Signal::derive(move || {
        armed
            .get()
            .then(|| format!("{confirm}? Press again to confirm."))
    });
    let (wrap, main, quiet) = if row {
        ("row-confirm", "row-action", "row-action")
    } else {
        ("confirm", "button button-danger", "button button-quiet")
    };

    view! {
        <span class=wrap>
            <button
                class=main
                type="button"
                node_ref=button
                data-armed=marked
                aria-label=name
                disabled=disabled
                on:click=press
            >
                {row.then(|| view! { <Icon name="trash" /> })}
                {words}
            </button>
            <Show when=move || armed.get()>
                <button class=quiet type="button" on:click=move |_| cancel()>
                    "Cancel"
                </button>
            </Show>
            <span class="visually-hidden" role="status">{said}</span>
        </span>
    }
}
