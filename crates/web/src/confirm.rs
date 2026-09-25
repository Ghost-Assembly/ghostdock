//! A destructive action that takes two taps.
//!
//! A modal would be heavier than the decision warrants, and a native
//! `confirm()` is unreliable in installed web apps. Arming in place keeps
//! the second tap exactly where the thumb already is.

use leptos::prelude::*;

#[component]
pub fn Confirm(
    /// What the button says before it is armed.
    #[prop(into)]
    label: Signal<String>,
    /// What it says once armed; the tap that follows does the thing.
    confirm: &'static str,
    /// While true, neither tap does anything: set it while the action is
    /// in flight, so a second pair of taps cannot send it twice.
    #[prop(optional)]
    disabled: Option<Signal<bool>>,
    /// The small form that sits at the end of a row in a list.
    #[prop(optional)]
    row: bool,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let armed = RwSignal::new(false);
    let disabled = move || disabled.is_some_and(|d| d.get());
    let go = move |_| {
        if disabled() {
            return;
        }
        armed.set(false);
        on_confirm.run(());
    };

    if row {
        return view! {
            <Show
                when=move || armed.get()
                fallback=move || view! {
                    <button class="row-action" type="button" disabled=disabled
                        on:click=move |_| armed.set(true)>
                        {move || label.get()}
                    </button>
                }
            >
                <span class="row-confirm">
                    <button class="row-action" data-armed="true" type="button" disabled=disabled
                        on:click=go>
                        {confirm}
                    </button>
                    <button class="row-action" type="button" on:click=move |_| armed.set(false)>
                        "Cancel"
                    </button>
                </span>
            </Show>
        }
        .into_any();
    }

    view! {
        <Show
            when=move || armed.get()
            fallback=move || view! {
                <button class="button button-danger" type="button" disabled=disabled
                    on:click=move |_| armed.set(true)>
                    {move || label.get()}
                </button>
            }
        >
            <div class="actions actions-pair">
                <button class="button button-danger" type="button" disabled=disabled
                    on:click=go>
                    {confirm}
                </button>
                <button class="button button-quiet" type="button"
                    on:click=move |_| armed.set(false)>
                    "Cancel"
                </button>
            </div>
        </Show>
    }
    .into_any()
}
