//! Many containers' output in one place, followed live.
//!
//! The latest lines of what is chosen, merged by time, then new lines over
//! one WebSocket as they are written. Each line is labeled with its
//! container in a column of its own: color means state here, so it cannot
//! also tell containers apart.

use std::sync::Arc;

use leptos::html::Pre;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use shared::logs::{LiveLog, Resume, Stream, TaggedLine};
use shared::stack::Stack;
use web_sys::{CloseEvent, MessageEvent};

use crate::load::Load;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Field, Topbar, pinned, to_bottom};
use crate::{api, socket};

/// Most lines kept. Following a chatty host for an hour would otherwise
/// grow the page until the phone gives up.
const MAX_LINES: usize = 5_000;

/// Most lines in the page at once; the rest stay searchable in memory and
/// in the download. Half the single-container view's: each line here also
/// carries its container's name and its search marks, and clearing a
/// search lays every one of them out again at once.
const ON_SCREEN: usize = 500;

/// Most matches in the page while searching. Every match is redrawn with
/// its marks when the search changes, and a thousand of them took a slow
/// phone past its frame budget on one keystroke.
const MATCHES_ON_SCREEN: usize = 200;

/// How long typing must pause before a search runs, so each keystroke only
/// echoes and the redraw happens once.
const SEARCH_AFTER: std::time::Duration = std::time::Duration::from_millis(150);

/// Most lines added in one frame. A burst, or the lines held while paused,
/// arrive over several frames instead of stalling one.
const BATCH: usize = 100;

/// Most lines held while paused; past it the oldest are let go.
const MAX_HELD: usize = 5_000;

/// What is being read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    /// Every running container.
    All,
    /// A registered stack, including containers that start later.
    Stack(i64),
    /// These containers, by name.
    Containers(Vec<String>),
}

impl Choice {
    /// From the page's own address: `?stack=<id>` or `?containers=<a,b>`.
    fn from_query(stack: Option<String>, containers: Option<String>) -> Self {
        if let Some(id) = stack.and_then(|s| s.parse::<i64>().ok()) {
            return Self::Stack(id);
        }
        match containers {
            Some(list) => Self::Containers(
                list.split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            None => Self::All,
        }
    }

    /// The API's selection for this, or none when nothing is chosen.
    fn query(&self) -> Option<String> {
        match self {
            Self::All => Some("all".to_owned()),
            Self::Stack(id) => Some(format!("stack={id}")),
            Self::Containers(names) if names.is_empty() => None,
            Self::Containers(names) => Some(format!(
                "containers={}",
                names
                    .iter()
                    .map(|n| api::component(n))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
        }
    }

    /// The containers it reads now, by name.
    fn names(&self, stacks: &[Stack]) -> Vec<String> {
        match self {
            Self::All => stacks
                .iter()
                .flat_map(|s| &s.containers)
                .filter(|c| c.state.is_running())
                .map(|c| c.name.clone())
                .collect(),
            Self::Stack(id) => stacks
                .iter()
                .filter(|s| s.managed.as_ref().is_some_and(|m| m.id == *id))
                .flat_map(|s| &s.containers)
                .map(|c| c.name.clone())
                .collect(),
            Self::Containers(names) => names.clone(),
        }
    }

    fn has_stack(&self, stack: &Stack) -> bool {
        match self {
            Self::All => true,
            Self::Stack(id) => stack.managed.as_ref().is_some_and(|m| m.id == *id),
            Self::Containers(names) => {
                !stack.containers.is_empty()
                    && stack.containers.iter().all(|c| names.contains(&c.name))
            }
        }
    }

    /// With one container added or taken away.
    fn toggle_container(&self, stacks: &[Stack], name: &str) -> Self {
        let mut names = self.names(stacks);
        crate::ui::toggle(&mut names, name.to_owned());
        Self::Containers(names)
    }

    /// With a whole stack added or taken away. A registered stack chosen on
    /// its own is followed as a stack, so containers it starts later appear.
    fn toggle_stack(&self, stacks: &[Stack], project: &str) -> Self {
        let Some(stack) = stacks.iter().find(|s| s.project == project) else {
            return self.clone();
        };
        let members: Vec<String> = stack.containers.iter().map(|c| c.name.clone()).collect();
        let mut names = self.names(stacks);
        if self.has_stack(stack) {
            names.retain(|n| !members.contains(n));
            return Self::Containers(names);
        }
        if names.is_empty()
            && let Some(managed) = &stack.managed
        {
            return Self::Stack(managed.id);
        }
        for member in members {
            if !names.contains(&member) {
                names.push(member);
            }
        }
        Self::Containers(names)
    }

    /// What it reads, in a few words.
    fn describe(&self, stacks: &[Stack]) -> String {
        match self {
            Self::All => "All running containers".to_owned(),
            Self::Stack(id) => stacks
                .iter()
                .find(|s| s.managed.as_ref().is_some_and(|m| m.id == *id))
                .map_or_else(
                    || "One stack".to_owned(),
                    |s| format!("Stack {}", s.project),
                ),
            Self::Containers(names) => match names.as_slice() {
                [] => "Nothing chosen".to_owned(),
                [one] => one.clone(),
                many => format!("{} containers", many.len()),
            },
        }
    }
}

/// `text` cut into runs that do and do not match `needle`, which is lower
/// case; `lower` is `text` lower-cased. Where lower-casing changed the
/// text's length, the positions do not carry over, and the line is shown
/// whole, unmarked: it is still shown because it matches.
fn segments(text: &str, lower: &str, needle: &str) -> Vec<(String, bool)> {
    let whole = || vec![(text.to_owned(), false)];
    if needle.is_empty() || lower.len() != text.len() {
        return whole();
    }
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = lower.get(at..).and_then(|rest| rest.find(needle)) {
        let start = at + found;
        let end = start + needle.len();
        let (Some(before), Some(hit)) = (text.get(at..start), text.get(start..end)) else {
            return whole();
        };
        if !before.is_empty() {
            out.push((before.to_owned(), false));
        }
        out.push((hit.to_owned(), true));
        at = end;
    }
    if let Some(rest) = text.get(at..).filter(|rest| !rest.is_empty()) {
        out.push((rest.to_owned(), false));
    }
    out
}

/// The name a line is labeled with: without its stack's name in front,
/// which Compose puts on every container it makes. On a phone the label
/// column is narrow, and `blog-web-1` beside `blog-db-1` would both show
/// as `blog-…`.
fn label(line: &TaggedLine) -> &str {
    line.stack
        .as_deref()
        .and_then(|stack| line.container.strip_prefix(stack))
        .and_then(|rest| rest.strip_prefix('-'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(&line.container)
}

/// A line as held here: shared rather than copied on every update, and
/// lower-cased once on arrival rather than on every keystroke of a search.
#[derive(Clone)]
struct Row {
    seq: u64,
    line: Arc<TaggedLine>,
    lower: Arc<str>,
}

/// Lines on their way to the screen, a frame's worth at a time.
#[derive(Clone, Copy)]
struct Feed {
    rows: RwSignal<Vec<Row>>,
    next_seq: StoredValue<u64>,
    /// Arrived and not yet shown.
    pending: StoredValue<Vec<TaggedLine>>,
    scheduled: StoredValue<bool>,
    paused: RwSignal<bool>,
    /// Arrived while paused.
    held: StoredValue<Vec<TaggedLine>>,
    held_count: RwSignal<usize>,
    /// Whether the reader is following the newest line. Set from the
    /// pane's own scroll events, not measured before each batch: lines
    /// laid out between one scroll to the bottom and the next batch made
    /// a measurement say the reader had scrolled away when they had not.
    follow: StoredValue<bool>,
    pane: NodeRef<Pre>,
    screen: Screen,
}

impl Feed {
    fn add(self, lines: impl IntoIterator<Item = TaggedLine>) {
        self.pending.update_value(|p| p.extend(lines));
        self.schedule();
    }

    fn schedule(self) {
        if self.scheduled.get_value() {
            return;
        }
        self.scheduled.set_value(true);
        self.screen.next_frame(move || self.flush());
    }

    fn flush(self) {
        self.scheduled.set_value(false);
        let batch: Vec<TaggedLine> = self
            .pending
            .try_update_value(|p| p.drain(..p.len().min(BATCH)).collect())
            .unwrap_or_default();
        if self.paused.get_untracked() {
            let mut count = 0;
            self.held.update_value(|held| {
                held.extend(batch);
                if held.len() > MAX_HELD {
                    held.drain(..held.len() - MAX_HELD);
                }
                count = held.len();
            });
            self.held_count.set(count);
        } else if !batch.is_empty() {
            let at_bottom = self.follow.get_value();
            self.push(batch);
            if at_bottom {
                let pane = self.pane;
                self.screen.next_frame(move || to_bottom(pane));
            }
        }
        if self.pending.with_value(|p| !p.is_empty()) {
            self.schedule();
        }
    }

    /// The reader moved the pane themselves: follow only if they are at
    /// the bottom once the scroll has happened.
    fn reader_scrolled(self) {
        let pane = self.pane;
        let follow = self.follow;
        self.screen
            .next_frame(move || follow.set_value(pinned(pane)));
    }

    fn push(self, incoming: Vec<TaggedLine>) {
        let mut seq = self.next_seq.get_value();
        self.rows.update(|all| {
            for line in incoming {
                let lower: Arc<str> = line.text.to_lowercase().into();
                all.push(Row {
                    seq,
                    line: Arc::new(line),
                    lower,
                });
                seq += 1;
            }
            if all.len() > MAX_LINES {
                all.drain(..all.len() - MAX_LINES);
            }
        });
        self.next_seq.set_value(seq);
    }

    /// Shows what was held, ahead of anything newer, and goes to the
    /// newest: resuming is asking to follow again.
    fn resume(self) {
        self.paused.set(false);
        self.follow.set_value(true);
        let pane = self.pane;
        self.screen.next_frame(move || to_bottom(pane));
        let held = std::mem::take(&mut *self.held.write_value());
        self.held_count.set(0);
        self.pending.update_value(|p| {
            let newer = std::mem::replace(p, held);
            p.extend(newer);
        });
        self.schedule();
    }

    fn clear(self) {
        self.pending.set_value(Vec::new());
        self.held.set_value(Vec::new());
        self.held_count.set(0);
        self.rows.set(Vec::new());
    }
}

#[component]
pub fn LogsAcross() -> impl IntoView {
    let screen = Screen::new();
    let query = use_query_map();
    let choice = RwSignal::new(Choice::All);
    // Follows the address, so a link here with another selection applies.
    Effect::new(move |_| {
        let chosen = query.with(|q| Choice::from_query(q.get("stack"), q.get("containers")));
        choice.set(chosen);
    });

    let stacks = RwSignal::new(Load::<Vec<Stack>>::Loading);
    screen.load(async move { stacks.set(api::stacks().await.into()) });
    let listed = move || stacks.with(|s| s.ready().cloned().unwrap_or_default());

    let feed = Feed {
        rows: RwSignal::new(Vec::new()),
        next_seq: StoredValue::new(0),
        pending: StoredValue::new(Vec::new()),
        scheduled: StoredValue::new(false),
        paused: RwSignal::new(false),
        held: StoredValue::new(Vec::new()),
        held_count: RwSignal::new(0),
        follow: StoredValue::new(true),
        pane: NodeRef::new(),
        screen,
    };
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let note = RwSignal::new(None::<&'static str>);
    let skipped = RwSignal::new(0_u64);
    // What is typed, and the search it becomes once typing pauses.
    let typed = RwSignal::new(String::new());
    let filter = RwSignal::new(String::new());
    let errors_only = RwSignal::new(false);
    let following = RwSignal::new(false);
    let follower = StoredValue::new_local(None::<socket::Owned>);
    // Bumped to start again after the socket closed.
    let attempt = RwSignal::new(0_u32);

    let follow = move |selection: String, resume: Resume| {
        let since = resume
            .since()
            .map_or_else(String::new, |t| format!("&since={t}"));
        let url = socket::url(&format!(
            "/api/v1/hosts/{}/logs/socket?{selection}{since}",
            api::HOST
        ));
        let Ok(opened) = socket::Owned::connect(&url) else {
            note.set(Some("Could not start following."));
            return;
        };
        let opened = opened
            .on_message(move |ev: MessageEvent| {
                let Some(text) = ev.data().as_string() else {
                    return;
                };
                match serde_json::from_str::<LiveLog>(&text) {
                    Ok(LiveLog::Line(line)) if resume.is_new(&line.untagged()) => {
                        feed.add([line]);
                    }
                    Ok(LiveLog::Skipped { skipped: n }) => skipped.update(|s| *s += n),
                    _ => {}
                }
            })
            // Not reconnected by itself: a reconnect would replay lines.
            .on_close(move |ev: CloseEvent| {
                follower.set_value(None);
                following.set(false);
                note.set(Some(match ev.reason().as_str() {
                    "stopped" => "Every container chosen has stopped.",
                    "revoked" => "Access to these logs was withdrawn.",
                    _ => "Following stopped.",
                }));
            });
        follower.set_value(Some(opened));
        following.set(true);
    };

    // Only the newest read may land: choosing again leaves the previous
    // one's in flight.
    let latest = StoredValue::new(0_u64);
    Effect::new(move |_| {
        let selection = choice.with(Choice::query);
        attempt.track();
        follower.set_value(None);
        following.set(false);
        feed.clear();
        feed.paused.set(false);
        loaded.set(false);
        error.set(None);
        note.set(None);
        skipped.set(0);
        let mine = latest.get_value() + 1;
        latest.set_value(mine);
        let Some(selection) = selection else {
            loaded.set(true);
            return;
        };
        screen.load(async move {
            let answer = api::logs_across(&selection).await;
            if latest.try_get_value() != Some(mine) {
                return;
            }
            match answer {
                Ok(merged) => {
                    // Continue from what the snapshot showed; see Resume
                    // for why the overlap is matched line by line.
                    let shown: Vec<_> = merged.lines.iter().map(TaggedLine::untagged).collect();
                    let resume = Resume::after(&shown);
                    feed.add(merged.lines);
                    follow(selection, resume);
                }
                Err(e) => error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    });

    on_cleanup(move || follower.set_value(None));

    let needle = Memo::new(move |_| filter.get().to_lowercase());
    // Which lines match, by sequence number: cheap to compute from shared
    // rows, and cheap to compare, so nothing redraws when nothing changed.
    let matching = Memo::new(move |_| {
        let only_errors = errors_only.get();
        needle.with(|needle| {
            feed.rows.with(|all| {
                all.iter()
                    .filter(|row| !only_errors || row.line.stream == Stream::Stderr)
                    .filter(|row| needle.is_empty() || row.lower.contains(needle.as_str()))
                    .map(|row| row.seq)
                    .collect::<Vec<_>>()
            })
        })
    });
    let on_screen = move || {
        if needle.with(String::is_empty) {
            ON_SCREEN
        } else {
            MATCHES_ON_SCREEN
        }
    };
    // A new search, or none, starts from the newest line: what was on
    // screen before is gone, and a pane left scrolled to the top of the
    // new lines would stop following them.
    Effect::new(move |_| {
        needle.track();
        errors_only.track();
        screen.next_frame(move || to_bottom(feed.pane));
    });
    // Bumped by each search, so its lines are drawn afresh with their
    // marks. A line then never redraws itself: re-marking every line on the
    // page as a search changed was what took a slow phone past its frame
    // budget.
    let generation = Memo::new(move |previous: Option<&u32>| {
        needle.track();
        previous.map_or(0, |g| g.wrapping_add(1))
    });
    let shown = Memo::new(move |_| {
        let limit = on_screen();
        let generation = generation.get();
        matching.with(|all| {
            let skip = all.len().saturating_sub(limit);
            all.iter()
                .skip(skip)
                .map(|seq| (*seq, generation))
                .collect::<Vec<_>>()
        })
    });
    let row_for = move |seq: u64| {
        feed.rows.with_untracked(|all| {
            let first = all.first().map_or(0, |row| row.seq);
            usize::try_from(seq.saturating_sub(first))
                .ok()
                .and_then(|i| all.get(i))
                .cloned()
        })
    };

    let pick_all = move |_| {
        choice.update(|c| {
            *c = if *c == Choice::All {
                Choice::Containers(Vec::new())
            } else {
                Choice::All
            }
        })
    };

    view! {
        <Topbar title="Logs" />

        <details class="mlog-picker">
            <summary class="mlog-summary">
                {move || choice.with(|c| c.describe(&listed()))}
            </summary>
            <fieldset class="checks">
                <legend class="checks-area">"Read from"</legend>
                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=move || choice.with(|c| *c == Choice::All)
                        on:change=pick_all
                    />
                    <span class="check-name">"All running containers"</span>
                </label>
            </fieldset>
            {move || match stacks.get() {
                Load::Loading => view! { <p class="entry-note">"Reading stacks"</p> }.into_any(),
                Load::Failed(message) => view! {
                    <p class="entry-note">{format!("Could not read the stacks. {message}")}</p>
                }
                .into_any(),
                Load::Ready(list) => list
                    .into_iter()
                    .map(|stack| {
                        let project = stack.project.clone();
                        let for_check = stack.clone();
                        view! {
                            <fieldset class="checks mlog-group">
                                <label class="check">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || choice.with(|c| c.has_stack(&for_check))
                                        on:change=move |_| {
                                            let all = listed();
                                            choice.update(|c| *c = c.toggle_stack(&all, &project));
                                        }
                                    />
                                    <span class="check-name mlog-stack">{stack.project.clone()}</span>
                                </label>
                                {stack
                                    .containers
                                    .into_iter()
                                    .map(|container| {
                                        let name = container.name.clone();
                                        let toggled = container.name.clone();
                                        view! {
                                            <label class="check mlog-member">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=move || {
                                                        choice.with(|c| c.names(&listed()).contains(&name))
                                                    }
                                                    on:change=move |_| {
                                                        let all = listed();
                                                        choice.update(|c| *c = c.toggle_container(&all, &toggled));
                                                    }
                                                />
                                                <span class="check-name">{container.name}</span>
                                                <span class="check-detail">{container.status}</span>
                                            </label>
                                        }
                                    })
                                    .collect_view()}
                            </fieldset>
                        }
                    })
                    .collect_view()
                    .into_any(),
            }}
        </details>

        <ErrorNotice error />

        <Field label="Search">
            <input
                class="field-input"
                type="search"
                autocapitalize="none"
                spellcheck="false"
                placeholder="error, timeout, 500"
                prop:value=move || typed.get()
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    typed.set(value.clone());
                    screen.after(SEARCH_AFTER, move || {
                        if typed.with_untracked(|now| *now == value) {
                            filter.set(value);
                        }
                    });
                }
            />
        </Field>

        <div class="actions actions-pair">
            <button
                class="button button-quiet"
                type="button"
                aria-pressed=move || feed.paused.get().to_string()
                disabled=move || !following.get()
                on:click=move |_| {
                    if feed.paused.get_untracked() { feed.resume() } else { feed.paused.set(true) }
                }
            >
                {move || if feed.paused.get() { "Resume" } else { "Pause" }}
            </button>
            <button
                class="button button-quiet"
                type="button"
                aria-pressed=move || errors_only.get().to_string()
                on:click=move |_| errors_only.update(|on| *on = !*on)
            >
                "Only stderr"
            </button>
        </div>
        {move || choice.with(Choice::query).map(|selection| view! {
            // A plain link, so the browser does the saving.
            <a
                class="button button-quiet"
                href=format!("/api/v1/hosts/{}/logs.txt?{selection}", api::HOST)
                download="containers.log"
            >
                "Download"
            </a>
        })}

        <Show when=move || feed.paused.get()>
            <p class="entry-note" role="status">
                {move || match feed.held_count.get() {
                    0 => "Paused. New lines wait here until you resume.".to_owned(),
                    1 => "Paused. 1 new line is waiting.".to_owned(),
                    n => format!("Paused. {n} new lines are waiting."),
                }}
            </p>
        </Show>
        <Show when=move || { skipped.get() > 0 }>
            <p class="entry-note">
                {move || format!("Skipped {} lines that came faster than they could be sent.", skipped.get())}
            </p>
        </Show>
        <Show when=move || note.get().is_some()>
            <p class="entry-note">{move || note.get().unwrap_or_default()}</p>
            <button class="button button-quiet" type="button" on:click=move |_| attempt.update(|n| *n += 1)>
                "Follow again"
            </button>
        </Show>

        {move || {
            if choice.with(|c| c.query().is_none()) {
                return view! { <p class="state-note">"Choose containers to read."</p> }.into_any();
            }
            if !loaded.get() {
                return view! { <p class="state-note">"Reading output"</p> }.into_any();
            }
            let count = matching.with(Vec::len);
            let lines_word = if count == 1 { "line" } else { "lines" };
            let limit = on_screen();
            let summary = if count > limit && limit == MATCHES_ON_SCREEN {
                format!("The latest {limit} of {count} matching {lines_word}.")
            } else if count > limit {
                format!("The latest {limit} of {count} {lines_word}. Search looks through all of them.")
            } else {
                format!("{count} {lines_word}")
            };
            view! { <p class="verdict-count">{summary}</p> }.into_any()
        }}

        <Show when=move || loaded.get() && (following.get() || matching.with(|m| !m.is_empty()))>
            <pre
                class="log log-tall mlog"
                node_ref=feed.pane
                tabindex="0"
                aria-label="Log lines"
                // Reaching the bottom, however it happened, follows again;
                // only the reader's own scrolling stops it. A scroll the page
                // caused, as lines were laid out or trimmed, once read as the
                // reader leaving and stranded the pane at the top.
                on:scroll=move |_| {
                    if pinned(feed.pane) {
                        feed.follow.set_value(true);
                    }
                }
                on:wheel=move |_| feed.reader_scrolled()
                on:touchmove=move |_| feed.reader_scrolled()
                on:keydown=move |_| feed.reader_scrolled()
                on:pointerup=move |_| feed.reader_scrolled()
            >
                <For each=move || shown.get() key=|key| *key let:key>
                    {row_for(key.0).map(|row| view! { <Line row needle=needle.get_untracked() /> })}
                </For>
            </pre>
        </Show>
    }
}

/// One line: its container's name in a column, then what it said, with
/// what the search matched marked.
#[component]
fn Line(row: Row, needle: String) -> impl IntoView {
    let class = if row.line.stream == Stream::Stderr {
        "log-line mlog-line log-stderr"
    } else {
        "log-line mlog-line"
    };
    let name = label(&row.line).to_owned();
    let title = row.line.container.clone();
    let text = segments(&row.line.text, &row.lower, &needle)
        .into_iter()
        .map(|(text, hit)| {
            if hit {
                view! { <mark class="mlog-match">{text}</mark> }.into_any()
            } else {
                text.into_any()
            }
        })
        .collect_view();
    view! {
        <div class=class>
            <span class="mlog-name" title=title>{name}</span>
            <span class="mlog-text">{text}</span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::container::{Container, ContainerState};
    use shared::deployment::SourceKind;
    use shared::stack::{Managed, StackState};

    fn container(name: &str, running: bool) -> Container {
        Container {
            id: format!("id-{name}"),
            name: name.to_owned(),
            image: "alpine".to_owned(),
            state: if running {
                ContainerState::Running
            } else {
                ContainerState::Exited
            },
            status: String::new(),
            health: None,
            created: None,
            ports: Vec::new(),
            compose: None,
        }
    }

    fn stack(project: &str, id: Option<i64>, containers: Vec<Container>) -> Stack {
        Stack {
            icon: None,
            project: project.to_owned(),
            state: StackState::Running,
            running_count: 0,
            total_count: containers.len(),
            containers,
            managed: id.map(|id| Managed {
                id,
                name: project.to_owned(),
                source_kind: SourceKind::Inline,
                busy: false,
            }),
        }
    }

    fn board() -> Vec<Stack> {
        vec![
            stack(
                "blog",
                Some(1),
                vec![container("blog-web-1", true), container("blog-db-1", true)],
            ),
            stack("other", None, vec![container("other-x-1", false)]),
            stack("empty", Some(3), Vec::new()),
        ]
    }

    #[test]
    fn the_address_chooses_a_stack_or_containers_or_everything() {
        assert_eq!(Choice::from_query(Some("4".into()), None), Choice::Stack(4));
        assert_eq!(
            Choice::from_query(None, Some("a, b".into())),
            Choice::Containers(vec!["a".into(), "b".into()])
        );
        assert_eq!(Choice::from_query(Some("x".into()), None), Choice::All);
        assert_eq!(Choice::from_query(None, None), Choice::All);
    }

    #[test]
    fn the_query_names_what_is_chosen_and_nothing_is_none() {
        assert_eq!(Choice::All.query().as_deref(), Some("all"));
        assert_eq!(Choice::Stack(2).query().as_deref(), Some("stack=2"));
        assert_eq!(
            Choice::Containers(vec!["a-1".into(), "b.c".into()])
                .query()
                .as_deref(),
            Some("containers=a-1,b.c")
        );
        assert_eq!(Choice::Containers(Vec::new()).query(), None);
    }

    #[test]
    fn a_registered_stack_chosen_alone_is_followed_as_a_stack() {
        let all = board();
        let none = Choice::Containers(Vec::new());
        assert_eq!(none.toggle_stack(&all, "blog"), Choice::Stack(1));
        // Even with nothing running yet: what starts is followed.
        assert_eq!(none.toggle_stack(&all, "empty"), Choice::Stack(3));
        // An unregistered one has no id, so its containers are named.
        assert_eq!(
            none.toggle_stack(&all, "other"),
            Choice::Containers(vec!["other-x-1".into()])
        );
    }

    #[test]
    fn toggling_moves_between_whole_stacks_and_named_containers() {
        let all = board();
        let blog = Choice::Stack(1);
        assert!(blog.has_stack(&all[0]));
        assert_eq!(
            blog.toggle_container(&all, "blog-db-1"),
            Choice::Containers(vec!["blog-web-1".into()])
        );
        assert_eq!(
            blog.toggle_stack(&all, "blog"),
            Choice::Containers(Vec::new())
        );
        assert_eq!(
            blog.toggle_stack(&all, "other"),
            Choice::Containers(vec![
                "blog-web-1".into(),
                "blog-db-1".into(),
                "other-x-1".into()
            ])
        );
        // Everything running, less one.
        assert_eq!(
            Choice::All.toggle_container(&all, "blog-web-1"),
            Choice::Containers(vec!["blog-db-1".into()])
        );
    }

    #[test]
    fn it_says_what_it_reads() {
        let all = board();
        assert_eq!(Choice::All.describe(&all), "All running containers");
        assert_eq!(Choice::Stack(1).describe(&all), "Stack blog");
        assert_eq!(
            Choice::Containers(vec!["a".into(), "b".into()]).describe(&all),
            "2 containers"
        );
    }

    #[test]
    fn a_label_leaves_out_the_stack_compose_named_it_after() {
        let line = |container: &str, stack: Option<&str>| TaggedLine {
            container: container.to_owned(),
            stack: stack.map(str::to_owned),
            stream: Stream::Stdout,
            at: None,
            text: String::new(),
        };
        assert_eq!(label(&line("blog-web-1", Some("blog"))), "web-1");
        assert_eq!(label(&line("blogger-1", Some("blog"))), "blogger-1");
        assert_eq!(label(&line("blog-", Some("blog"))), "blog-");
        assert_eq!(label(&line("by-hand", None)), "by-hand");
    }

    #[test]
    fn matches_are_marked_without_losing_the_original_case() {
        let text = "Error: an ERROR, then error";
        let marked = segments(text, &text.to_lowercase(), "error");
        let hits: Vec<&str> = marked
            .iter()
            .filter(|(_, hit)| *hit)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(hits, ["Error", "ERROR", "error"]);
        let joined: String = marked.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn text_whose_length_changes_when_lowered_is_shown_whole() {
        // The Kelvin sign lowers to a one-byte k.
        let text = "\u{212A}elvin";
        assert_eq!(
            segments(text, &text.to_lowercase(), "kelvin"),
            [(text.to_owned(), false)]
        );
        assert_eq!(
            segments("plain", "plain", ""),
            [("plain".to_owned(), false)]
        );
    }
}
