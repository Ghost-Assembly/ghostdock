//! A destructive action that takes two taps.
//!
//! A modal would be heavier than the decision warrants, and a native
//! `confirm()` is unreliable in installed web apps. Arming in place keeps
//! the second tap exactly where the thumb already is.

use leptos::prelude::*;

#[component]
pub fn Confirm(
    /// What the button says before it is armed.
    label: &'static str,
    /// What it says once armed; the tap that follows does the thing.
    confirm: &'static str,
    #[prop(optional)] disabled: Option<Signal<bool>>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let armed = RwSignal::new(false);
    let disabled = move || disabled.is_some_and(|d| d.get());

    view! {
        <Show
            when=move || armed.get()
            fallback=move || view! {
                <button class="button button-danger" type="button" disabled=disabled
                    on:click=move |_| armed.set(true)>
                    {label}
                </button>
            }
        >
            <div class="actions actions-pair">
                <button class="button button-danger" type="button" disabled=disabled
                    on:click=move |_| {
                        armed.set(false);
                        on_confirm.run(());
                    }>
                    {confirm}
                </button>
                <button class="button button-quiet" type="button"
                    on:click=move |_| armed.set(false)>
                    "Cancel"
                </button>
            </div>
        </Show>
    }
}
