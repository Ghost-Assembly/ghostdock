//! Reclaiming disk space: stopped containers nobody manages, then images
//! nothing uses.
//!
//! Nothing is removed without showing what and how much first. Reclaiming
//! space is never urgent, and an image deleted by mistake is only
//! recoverable by downloading it again. Containers come first on the page
//! because removing one can free the image beneath it.

use leptos::prelude::*;
use shared::cleanup::{CleanupPreview, CleanupResult, CleanupScope, StoppedContainer, UnusedImage};

use crate::api;
use crate::screen::Screen;

#[component]
pub fn Cleanup() -> impl IntoView {
    let preview = RwSignal::new(None::<CleanupPreview>);
    let result = RwSignal::new(None::<(CleanupScope, CleanupResult)>);
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let busy = RwSignal::new(false);

    let refresh = move || {
        screen.load(async move {
            match api::cleanup_preview(1).await {
                Ok(found) => preview.set(Some(found)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    Effect::new(move |_| refresh());

    let run = move |scope: CleanupScope| {
        move |_| {
            if busy.get_untracked() {
                return;
            }
            busy.set(true);
            error.set(None);
            // Runs to the end on the server even if this screen is left.
            screen.act(api::run_cleanup(1, scope), move |outcome| {
                match outcome {
                    Ok(done) => {
                        result.set(Some((scope, done)));
                        refresh();
                    }
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            });
        }
    };

    view! {
        <header class="topbar">
            <h1 class="wordmark">"Cleanup"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        <Show when=move || error.get().is_some()>
            <p class="notice" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>

        <Show when=move || result.get().is_some()>
            <p class="entry-note">
                {move || {
                    result
                        .get()
                        .map(|(scope, done)| {
                            let kept = if done.kept.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    " {} could not be removed and were left alone.",
                                    done.kept.len(),
                                )
                            };
                            let n = done.removed.len();
                            match scope {
                                CleanupScope::Dangling | CleanupScope::AllUnused => format!(
                                    "Removed {n} {}, reclaiming {}.{kept}",
                                    if n == 1 { "image" } else { "images" },
                                    human_size(done.reclaimed_bytes),
                                ),
                                CleanupScope::Leftover | CleanupScope::Standalone => format!(
                                    "Removed {n} {}. Any images they used are listed below \
                                     if nothing else needs them.{kept}",
                                    if n == 1 { "container" } else { "containers" },
                                ),
                            }
                        })
                        .unwrap_or_default()
                }}
            </p>
        </Show>

        {move || match preview.get() {
            None => view! { <p class="state-note">"Looking"</p> }.into_any(),
            Some(found) if found.is_empty() => view! {
                <div class="state-note">
                    <p>"Nothing to reclaim."</p>
                    <p>"Every image is in use, and no stopped container is left over."</p>
                </div>
            }
            .into_any(),
            Some(found) => {
                let dangling_bytes = found.dangling_bytes();
                let unused_bytes = found.unused_bytes();
                let has_dangling = !found.dangling.is_empty();
                let has_unused = !found.unused.is_empty();
                let has_leftover = !found.leftover.is_empty();
                let has_standalone = !found.standalone.is_empty();
                let images = found.dangling.len() + found.unused.len();
                let containers = found.leftover.len() + found.standalone.len();
                let mut counts = Vec::new();
                if images > 0 {
                    counts.push(format!("{images} unused {}", if images == 1 { "image" } else { "images" }));
                }
                if containers > 0 {
                    counts.push(format!(
                        "{containers} stopped {}",
                        if containers == 1 { "container" } else { "containers" },
                    ));
                }
                let leftover_count = found.leftover.len();
                let standalone_count = found.standalone.len();
                view! {
                    <section class="verdict">
                        <p class="verdict-line">
                            {if dangling_bytes + unused_bytes > 0 {
                                format!("{} can be reclaimed", human_size(dangling_bytes + unused_bytes))
                            } else {
                                "Stopped containers can be removed".to_owned()
                            }}
                        </p>
                        <p class="verdict-count">{counts.join(", ")}</p>
                    </section>

                    <Show when=move || has_leftover>
                        <h2 class="group-heading">"Left by stacks that are gone"</h2>
                        <ContainerRows containers=found.leftover.clone() />
                        <button
                            class="button button-danger"
                            type="button"
                            disabled=move || busy.get()
                            on:click=run(CleanupScope::Leftover)
                        >
                            {move || if busy.get() {
                                "Removing".to_owned()
                            } else {
                                format!("Remove {leftover_count} left over")
                            }}
                        </button>
                        <p class="entry-note">
                            "Stopped, from compose projects GhostDock does not manage and that have \
                             nothing running. Their volumes are kept."
                        </p>
                    </Show>

                    <Show when=move || has_standalone>
                        <h2 class="group-heading">"Stopped, not from Compose"</h2>
                        <ContainerRows containers=found.standalone.clone() />
                        <button
                            class="button button-danger"
                            type="button"
                            disabled=move || busy.get()
                            on:click=run(CleanupScope::Standalone)
                        >
                            {move || if busy.get() {
                                "Removing".to_owned()
                            } else {
                                format!("Remove {standalone_count} stopped")
                            }}
                        </button>
                        <p class="entry-note">
                            "Started by hand, with docker run or another tool. Remove them only \
                             if you are done with them; their volumes are kept."
                        </p>
                    </Show>

                    <Show when=move || has_dangling>
                        <h2 class="group-heading">"Untagged"</h2>
                        <ImageRows images=found.dangling.clone() />
                        <button
                            class="button button-quiet"
                            type="button"
                            disabled=move || busy.get()
                            on:click=run(CleanupScope::Dangling)
                        >
                            {move || {
                                if busy.get() {
                                    "Removing".to_owned()
                                } else {
                                    format!("Remove untagged ({})", human_size(dangling_bytes))
                                }
                            }}
                        </button>
                        <p class="entry-note">
                            "Layers left behind by newer builds. Nothing refers to them."
                        </p>
                    </Show>

                    <Show when=move || has_unused>
                        <h2 class="group-heading">"Not used by any container"</h2>
                        <ImageRows images=found.unused.clone() />
                        <button
                            class="button button-danger"
                            type="button"
                            disabled=move || busy.get()
                            on:click=run(CleanupScope::AllUnused)
                        >
                            {move || {
                                if busy.get() {
                                    "Removing".to_owned()
                                } else {
                                    format!(
                                        "Remove everything unused ({})",
                                        human_size(dangling_bytes + unused_bytes),
                                    )
                                }
                            }}
                        </button>
                        <p class="entry-note">
                            "These still have names. Removing one means pulling it again \
                             the next time something needs it."
                        </p>
                    </Show>
                }
                .into_any()
            }
        }}
    }
}

#[component]
fn ContainerRows(containers: Vec<StoppedContainer>) -> impl IntoView {
    view! {
        <ul class="rows">
            {containers
                .into_iter()
                .map(|c| view! {
                    <li class="row">
                        <span class="row-link">
                            <span class="row-bar" data-state="stopped"></span>
                            <span class="row-name">{c.name}</span>
                            <span class="row-detail">{format!("{}, {}", c.image, c.status)}</span>
                        </span>
                    </li>
                })
                .collect_view()}
        </ul>
    }
}

#[component]
fn ImageRows(images: Vec<UnusedImage>) -> impl IntoView {
    view! {
        <ul class="rows">
            {images
                .into_iter()
                .map(|image| {
                    let size = human_size(image.size_bytes);
                    view! {
                        <li class="row">
                            <span class="row-link">
                                <span class="row-bar" data-state="stopped"></span>
                                <span class="row-name">{image.label()}</span>
                                <span class="row-count">{size}</span>
                            </span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// Sizes as a person reads them.
///
/// Powers of 1000, matching what Docker itself prints, so the two agree.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS.get(unit).copied().unwrap_or("TB"))
    }
}
