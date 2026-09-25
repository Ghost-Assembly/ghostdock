//! What is waiting to be applied.
//!
//! The point of this screen is the word "why". A tool that redeploys on a
//! timer can tell you it did something; this says what is about to change
//! before you agree to it.

use leptos::prelude::*;
use shared::update::StackUpdate;

use crate::api;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{Icon, Row, Topbar};

#[component]
pub fn Updates() -> impl IntoView {
    let load = RwSignal::new(Load::<Vec<StackUpdate>>::Loading);
    // How far a check of everything has got, while one runs: (done, of).
    let progress = RwSignal::new(None::<(usize, usize)>);
    let failures = RwSignal::new(Vec::<String>::new());

    let screen = Screen::new();
    let refresh = move || {
        screen.load(async move {
            load.set(Load::from(api::updates().await));
        });
    };
    Effect::new(move |_| refresh());

    // Checking every stack at once is the action a person actually wants
    // here; the background sweep is hourly, which is far too slow for
    // someone who has just pushed a commit.
    let check_all = move |_| {
        if progress.get_untracked().is_some() {
            return;
        }
        let stacks: Vec<(i64, String)> = load.with_untracked(|l| {
            l.ready()
                .map(|list| {
                    list.iter()
                        .map(|u| (u.stack.id, u.stack.name.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
        let total = stacks.len();
        progress.set(Some((0, total)));
        failures.set(Vec::new());
        // Every check runs, whether or not anyone stays on this screen. One
        // at a time, counted as each ends, so a long list shows it is
        // getting somewhere. A signal whose screen has gone
        // ignores the write, so counting is safe after leaving.
        screen.act(
            async move {
                let mut failed = Vec::new();
                for (done, (id, name)) in stacks.into_iter().enumerate() {
                    if let Err(e) = api::check_stack(id).await {
                        failed.push(format!("{name}: {}", e.message));
                    }
                    let _ = progress.try_set(Some((done + 1, total)));
                }
                failed
            },
            move |failed| {
                failures.set(failed);
                progress.set(None);
                refresh();
            },
        );
    };

    view! {
        <Topbar title="Updates">
            <button
                class="topbar-link"
                type="button"
                on:click=check_all
                disabled=move || progress.get().is_some() || load.with(|l| l.ready().is_none())
            >
                <Icon name="refresh-cw" />
                {move || match progress.get() {
                    Some((done, total)) => format!("Checked {done} of {total}"),
                    None => "Check now".to_owned(),
                }}
            </button>
        </Topbar>

        <Show when=move || failures.with(|f| !f.is_empty())>
            <p class="notice" role="alert">
                {move || format!("Some checks could not run. {}", failures.get().join(" "))}
            </p>
        </Show>

        {move || match load.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read update status."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) if list.is_empty() => view! {
                <div class="state-note">
                    <p>"No stacks to check."</p>
                    <p>"Register one and GhostDock will watch it for changes."</p>
                </div>
            }
            .into_any(),
            Load::Ready(list) => view! { <Board updates=list /> }.into_any(),
        }}
    }
}

#[component]
fn Board(updates: Vec<StackUpdate>) -> impl IntoView {
    let waiting: Vec<StackUpdate> = updates
        .iter()
        .filter(|u| u.reason.is_some())
        .cloned()
        .collect();
    // A stack whose check failed is neither waiting nor current, and saying
    // it is current would be a lie of omission.
    let unknown: Vec<StackUpdate> = updates
        .iter()
        .filter(|u| {
            u.reason.is_none() && (u.status.error.is_some() || u.status.checked_at.is_none())
        })
        .cloned()
        .collect();
    let current: Vec<StackUpdate> = updates
        .iter()
        .filter(|u| u.reason.is_none() && u.status.error.is_none() && u.status.checked_at.is_some())
        .cloned()
        .collect();

    // "Current" is only said of what has been checked. Nothing checked yet
    // and nothing found are different answers, and only one is reassuring.
    let (verdict, tone) = if waiting.is_empty() && current.is_empty() {
        ("Not checked yet".to_owned(), "quiet")
    } else if waiting.is_empty() && !unknown.is_empty() {
        ("No updates found".to_owned(), "quiet")
    } else if waiting.is_empty() {
        ("Everything is current".to_owned(), "quiet")
    } else if waiting.len() == 1 {
        ("1 stack has an update".to_owned(), "degraded")
    } else {
        (format!("{} stacks have updates", waiting.len()), "degraded")
    };

    let has_waiting = !waiting.is_empty();
    let has_unknown = !unknown.is_empty();
    let has_current = !current.is_empty();

    view! {
        <section class="verdict">
            <p class="verdict-line" data-tone=tone>{verdict}</p>
            <p class="verdict-count">
                {format!(
                    "{} {} watched{}",
                    updates.len(),
                    if updates.len() == 1 { "stack" } else { "stacks" },
                    if unknown.is_empty() {
                        String::new()
                    } else {
                        format!(", {} not checked or failed", unknown.len())
                    },
                )}
            </p>
        </section>

        <Show when=move || has_waiting>
            <h2 class="group-heading">"Waiting"</h2>
            <UpdateRows updates=waiting.clone() state="degraded" />
        </Show>

        <Show when=move || has_unknown>
            <h2 class="group-heading">"Not checked"</h2>
            <UpdateRows updates=unknown.clone() state="stopped" />
        </Show>

        <Show when=move || has_current>
            <h2 class="group-heading">"Up to date"</h2>
            <UpdateRows updates=current.clone() state="running" />
        </Show>
    }
}

#[component]
fn UpdateRows(updates: Vec<StackUpdate>, state: &'static str) -> impl IntoView {
    view! {
        <ul class="rows">
            {updates
                .into_iter()
                .map(|update| {
                    let detail = update
                        .reason
                        .clone()
                        .or_else(|| update.status.error.clone().map(|e| shorten(&e)))
                        .unwrap_or_else(|| {
                            if update.status.checked_at.is_some() {
                                "up to date".to_owned()
                            } else {
                                "not checked yet".to_owned()
                            }
                        });
                    let href = format!("/stacks/{}", update.stack.id);
                    let name = update.stack.name;
                    if update.auto_apply {
                        view! { <Row state href name detail count="auto" count_icon="repeat" /> }
                            .into_any()
                    } else {
                        view! { <Row state href name detail /> }.into_any()
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// Keeps a row readable when a check failed with a long message.
///
/// The whole text is on the stack's own screen; this is a summary line.
fn shorten(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim();
    if first.chars().count() > 70 {
        format!("{}…", first.chars().take(69).collect::<String>())
    } else {
        first.to_owned()
    }
}
