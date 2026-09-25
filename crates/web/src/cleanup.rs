//! Reclaiming disk space: stopped containers nobody manages, then images
//! nothing uses.
//!
//! Nothing is removed without showing what and how much first. Reclaiming
//! space is never urgent, and an image deleted by mistake is only
//! recoverable by downloading it again. Containers come first on the page
//! because removing one can free the image beneath it.
//!
//! Sizes are in the binary units every other screen uses, so an image here
//! and the memory on the Host screen are measured the same way. Docker
//! prints powers of 1000, so its figures read a few percent larger.

use leptos::prelude::*;
use shared::cleanup::{CleanupPreview, CleanupResult, CleanupScope, StoppedContainer, UnusedImage};
use shared::metrics::format_bytes;

use crate::api;
use crate::confirm::Confirm;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Row, Topbar};

#[component]
pub fn Cleanup() -> impl IntoView {
    let preview = RwSignal::new(Load::<CleanupPreview>::Loading);
    let result = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let screen = Screen::new();
    let busy = RwSignal::new(false);

    let refresh = move || {
        screen.load(async move {
            preview.set(Load::from(api::cleanup_preview().await));
        });
    };
    Effect::new(move |_| refresh());

    // The label a removal button carries: what it will do, or that it is.
    let label = move |idle: String| {
        Signal::derive(move || {
            if busy.get() {
                "Removing".to_owned()
            } else {
                idle.clone()
            }
        })
    };
    let run = move |scope: CleanupScope| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        // Runs to the end on the server even if this screen is left.
        screen.act(api::run_cleanup(scope), move |outcome| {
            match outcome {
                Ok(done) => {
                    result.set(Some(result_line(scope, &done)));
                    refresh();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <Topbar title="Cleanup" back="/settings" />

        <ErrorNotice error />

        <Show when=move || result.with(Option::is_some)>
            <p class="entry-note">{move || result.get().unwrap_or_default()}</p>
        </Show>

        {move || match preview.get() {
            Load::Loading => view! { <p class="state-note">"Looking"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not look for anything to reclaim."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(found) if found.is_empty() => view! {
                <div class="state-note">
                    <p>"Nothing to reclaim."</p>
                    <p>"Every image is in use, and no stopped container is left over."</p>
                </div>
            }
            .into_any(),
            Load::Ready(found) => {
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
                                format!("{} can be reclaimed", format_bytes(dangling_bytes + unused_bytes))
                            } else {
                                "Stopped containers can be removed".to_owned()
                            }}
                        </p>
                        <p class="verdict-count">{counts.join(", ")}</p>
                    </section>

                    <Show when=move || has_leftover>
                        <h2 class="group-heading">"Left by stacks that are gone"</h2>
                        <ContainerRows containers=found.leftover.clone() />
                        <Confirm
                            label=label(format!("Remove {leftover_count} left over"))
                            confirm="Remove them"
                            disabled=Signal::derive(move || busy.get())
                            on_confirm=Callback::new(move |()| run(CleanupScope::Leftover))
                        />
                        <p class="entry-note">
                            "Stopped, from compose projects GhostDock does not manage and that have \
                             nothing running. Their volumes are kept."
                        </p>
                    </Show>

                    <Show when=move || has_standalone>
                        <h2 class="group-heading">"Stopped, not from Compose"</h2>
                        <ContainerRows containers=found.standalone.clone() />
                        <Confirm
                            label=label(format!("Remove {standalone_count} stopped"))
                            confirm="Remove them"
                            disabled=Signal::derive(move || busy.get())
                            on_confirm=Callback::new(move |()| run(CleanupScope::Standalone))
                        />
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
                            on:click=move |_| run(CleanupScope::Dangling)
                        >
                            {move || {
                                if busy.get() {
                                    "Removing".to_owned()
                                } else {
                                    format!("Remove untagged ({})", format_bytes(dangling_bytes))
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
                        <Confirm
                            label=label(format!(
                                "Remove everything unused ({})",
                                format_bytes(dangling_bytes + unused_bytes),
                            ))
                            confirm="Remove them"
                            disabled=Signal::derive(move || busy.get())
                            on_confirm=Callback::new(move |()| run(CleanupScope::AllUnused))
                        />
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

/// What a removal did, in a sentence.
fn result_line(scope: CleanupScope, done: &CleanupResult) -> String {
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
            format_bytes(done.reclaimed_bytes),
        ),
        CleanupScope::Leftover | CleanupScope::Standalone => format!(
            "Removed {n} {}. Any images they used are listed below \
             if nothing else needs them.{kept}",
            if n == 1 { "container" } else { "containers" },
        ),
    }
}

#[component]
fn ContainerRows(containers: Vec<StoppedContainer>) -> impl IntoView {
    view! {
        <ul class="rows">
            {containers
                .into_iter()
                .map(|c| view! {
                    <Row state="stopped" name=c.name ident=true detail=format!("{}, {}", c.image, c.status) />
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
                .map(|image| view! {
                    <Row state="none" name=image.label() ident=true count=format_bytes(image.size_bytes) />
                })
                .collect_view()}
        </ul>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_the_same_units_as_everywhere_else() {
        let done = CleanupResult {
            removed: vec!["sha256:1".to_owned()],
            reclaimed_bytes: 1536,
            kept: Vec::new(),
        };
        assert_eq!(
            result_line(CleanupScope::Dangling, &done),
            "Removed 1 image, reclaiming 1.5 KiB."
        );
        let done = CleanupResult {
            removed: vec!["a".to_owned(), "b".to_owned()],
            reclaimed_bytes: 0,
            kept: vec!["c".to_owned()],
        };
        assert_eq!(
            result_line(CleanupScope::Leftover, &done),
            "Removed 2 containers. Any images they used are listed below if nothing else \
             needs them. 1 could not be removed and were left alone."
        );
    }
}
