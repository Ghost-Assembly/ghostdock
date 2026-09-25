//! Where alerts go, the resource rules that raise them, and what was sent.
//!
//! A channel's URL and token are typed once and never shown again: the
//! server keeps them sealed and says only which host the URL points at.
//!
//! Every list here is drawn by one function from plain rows, which keeps
//! the screen's share of the bundle small.

use leptos::prelude::*;
use shared::alerts::{
    AlertChannel, AlertDelivery, AlertRule, ChannelKind, Metric, NewAlertChannel, NewAlertRule,
};

use crate::api;
use crate::confirm::Confirm;
use crate::screen::Screen;
use crate::ui::{ErrorNotice, Pick, Row, TextField, Topbar, options};

/// Deliveries shown; the server keeps 200.
const SHOWN: usize = 50;

/// One row of this screen, as data.
struct Line {
    state: &'static str,
    name: String,
    detail: String,
    /// What removing it calls: `channels` or `rules`, and the id.
    remove: Option<(&'static str, i64)>,
    /// A channel to send a test to.
    test: Option<i64>,
}

/// A rule in words: "Blog memory above 80% for 10 min".
fn rule_words(rule: &AlertRule, stacks: &[(String, String)]) -> String {
    let subject = match rule.subject.split_once(':') {
        Some(("stack", _)) => stacks
            .iter()
            .find(|(value, _)| *value == rule.subject)
            .map_or_else(|| rule.subject.clone(), |(_, name)| name.clone()),
        Some((_, name)) => name.to_owned(),
        None => "Host".to_owned(),
    };
    format!(
        "{subject} {} above {}% for {} min",
        rule.metric.label(),
        shared::metrics::trimmed(rule.above_pct, 3),
        rule.for_min
    )
}

/// A rule's bar and word: firing, clear, or off.
fn rule_state(rule: &AlertRule) -> (&'static str, &'static str) {
    match (rule.enabled, rule.firing) {
        (false, _) => ("stopped", "off"),
        (true, true) => ("unhealthy", "firing now"),
        (true, false) => ("running", "clear"),
    }
}

fn channel_line(c: &AlertChannel) -> Line {
    Line {
        state: "none",
        name: c.name.clone(),
        detail: format!("{} to {}", c.kind.as_str(), c.host),
        remove: Some(("channels", c.id)),
        test: Some(c.id),
    }
}

fn rule_line(r: &AlertRule, stacks: &[(String, String)]) -> Line {
    let (state, word) = rule_state(r);
    Line {
        state,
        name: rule_words(r, stacks),
        detail: word.to_owned(),
        remove: Some(("rules", r.id)),
        test: None,
    }
}

fn delivery_line(d: &AlertDelivery) -> Line {
    let when = crate::time::local(d.at, "%d %b %H:%M");
    Line {
        state: if d.ok { "running" } else { "unhealthy" },
        name: format!("{} {}", d.subject, d.state),
        detail: match &d.error {
            None => format!("sent to {} at {when}", d.channel),
            Some(error) => format!(
                "not sent to {} at {when}, after {} tries: {error}",
                d.channel, d.attempts
            ),
        },
        remove: None,
        test: None,
    }
}

#[component]
pub fn AlertSettings() -> impl IntoView {
    let screen = Screen::new();
    let channels = RwSignal::new(Vec::<AlertChannel>::new());
    let rules = RwSignal::new(Vec::<AlertRule>::new());
    let deliveries = RwSignal::new(Vec::<AlertDelivery>::new());
    // Each registered stack as a rule's subject, and its name.
    let stacks = RwSignal::new(Vec::<(String, String)>::new());
    let error = RwSignal::new(None::<String>);
    // What a test found, said where the person tapped.
    let said = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    let refresh = move || {
        screen.load(async move {
            let read = async {
                channels.set(api::channels().await?);
                rules.set(api::rules().await?);
                deliveries.set(api::deliveries().await?);
                Ok::<_, api::Error>(())
            };
            if let Err(e) = read.await {
                error.set(Some(e.message));
            }
        });
    };
    refresh();
    screen.load(async move {
        if let Ok(list) = api::updates().await {
            stacks.set(
                list.into_iter()
                    .map(|u| (format!("stack:{}", u.stack.id), u.stack.name))
                    .collect(),
            );
        }
    });

    let remove = move |(what, id): (&'static str, i64)| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        screen.act(api::remove_alert(what, id), move |result| {
            if let Err(e) = result {
                error.set(Some(e.message));
            }
            busy.set(false);
            refresh();
        });
    };
    let test = move |id: i64| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        said.set("Sending a test".to_owned());
        screen.act(api::test_channel(id), move |result| {
            said.set(match result {
                Ok(d) if d.ok => format!("Test delivered to {}.", d.channel),
                Ok(d) => format!("Test not delivered: {}", delivery_line(&d).detail),
                Err(e) => e.message,
            });
            busy.set(false);
            refresh();
        });
    };

    let draw = move |lines: Vec<Line>, empty: &'static str| {
        if lines.is_empty() {
            return view! { <p class="entry-note">{empty}</p> }.into_any();
        }
        view! {
            <ul class="rows">
                {lines.into_iter().map(|l| {
                    let subject = l.name.clone();
                    view! {
                        <Row state=l.state name=l.name detail=l.detail>
                            {(l.remove.is_some() || l.test.is_some()).then(|| view! {
                                <span class="row-confirm">
                                    {l.test.map(|id| view! {
                                        <button class="row-action row-action-safe" type="button"
                                            aria-label=format!("Send test: {subject}")
                                            disabled=move || busy.get()
                                            on:click=move |_| test(id)>
                                            "Send test"
                                        </button>
                                    })}
                                    {l.remove.map(|target| view! {
                                        <Confirm label="Remove" confirm="Remove it" subject=subject.clone() row=true
                                            disabled=Signal::derive(move || busy.get())
                                            on_confirm=Callback::new(move |()| remove(target)) />
                                    })}
                                </span>
                            })}
                        </Row>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    };

    view! {
        <Topbar title="Alerts" back="/settings" />
        <p class="entry-note">
            "Every channel gets every alert: an uptime check going down, coming back or \
             degrading, and a resource rule firing or clearing."
        </p>
        <ErrorNotice error />

        <h2 class="group-heading">"Channels"</h2>
        <p class="entry-note" role="status">{move || said.get()}</p>
        {move || draw(
            channels.with(|list| list.iter().map(channel_line).collect()),
            "No channels yet, so alerts go nowhere.",
        )}
        <NewChannel on_added=refresh />

        <h2 class="group-heading">"Resource rules"</h2>
        {move || draw(
            rules.with(|list| stacks.with(|s| list.iter().map(|r| rule_line(r, s)).collect())),
            "No rules yet.",
        )}
        <NewRule stacks on_added=refresh />

        <h2 class="group-heading">"Sent"</h2>
        {move || draw(
            deliveries.with(|list| list.iter().take(SHOWN).map(delivery_line).collect()),
            "Nothing sent yet.",
        )}
    }
}

#[component]
fn NewChannel(on_added: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let screen = Screen::new();
    let name = RwSignal::new(String::new());
    let kind = RwSignal::new("webhook".to_owned());
    let url = RwSignal::new(String::new());
    let token = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let new = NewAlertChannel {
            name: name.get_untracked(),
            kind: ChannelKind::parse(&kind.get_untracked()).unwrap_or(ChannelKind::Webhook),
            url: url.get_untracked(),
            token: Some(token.get_untracked()).filter(|t| !t.trim().is_empty()),
        };
        screen.act(async move { api::add_channel(&new).await }, move |result| {
            match result {
                Ok(_) => {
                    name.set(String::new());
                    // Not coming back from the server, so not left on screen.
                    url.set(String::new());
                    token.set(String::new());
                    on_added();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <form on:submit=submit>
            <ErrorNotice error />
            <TextField label="Name" value=name placeholder="phone" />
            <Pick label="Kind" value=kind options=options(&[
                ("webhook", "Webhook: a JSON POST"),
                ("ntfy", "ntfy: a push to a topic"),
            ]) />
            <TextField label="URL" value=url secret=true />
            <TextField label="Token (optional)" value=token secret=true />
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Adding" } else { "Add channel" }}
            </button>
            <p class="entry-note">
                "The URL and token are stored encrypted. From now on only the URL's host is shown."
            </p>
        </form>
    }
}

#[component]
fn NewRule(
    stacks: RwSignal<Vec<(String, String)>>,
    on_added: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let screen = Screen::new();
    // `host`, `stack:<id>`, or empty for the container named below.
    let subject = RwSignal::new("host".to_owned());
    let container = RwSignal::new(String::new());
    let metric = RwSignal::new("cpu".to_owned());
    let above = RwSignal::new("90".to_owned());
    let minutes = RwSignal::new("5".to_owned());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        // Whole numbers: parsing a fraction would cost the bundle more than
        // a tenth of a percent is worth.
        let (Ok(above_pct), Ok(for_min)) = (
            above.get_untracked().trim().parse::<u32>(),
            minutes.get_untracked().trim().parse::<u32>(),
        ) else {
            error.set(Some(
                "The share and the minutes must be whole numbers.".to_owned(),
            ));
            return;
        };
        let above_pct = f64::from(above_pct);
        let chosen = subject.get_untracked();
        let new = NewAlertRule {
            subject: if chosen.is_empty() {
                format!("container:{}", container.get_untracked().trim())
            } else {
                chosen
            },
            metric: Metric::parse(&metric.get_untracked()).unwrap_or(Metric::Cpu),
            above_pct,
            for_min,
            enabled: true,
        };
        busy.set(true);
        error.set(None);
        screen.act(async move { api::add_rule(&new).await }, move |result| {
            match result {
                Ok(_) => on_added(),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let subjects = Signal::derive(move || {
        let mut all = options(&[("host", "The host")]);
        all.extend(
            stacks
                .get()
                .into_iter()
                .map(|(v, name)| (v, format!("Stack {name}"))),
        );
        all.push((String::new(), "A container".to_owned()));
        all
    });

    view! {
        <form on:submit=submit>
            <ErrorNotice error />
            <Pick label="Watch" value=subject options=subjects />
            <Show when=move || subject.with(String::is_empty)>
                <TextField label="Container name" value=container placeholder="blog-web-1" />
            </Show>
            <Pick label="Measure" value=metric options=options(&[
                ("cpu", "CPU, of the host's cores"),
                ("memory", "Memory, of its limit or the host's"),
                ("disk", "Disk, the host's fullest"),
            ]) />
            <TextField label="Above (%)" value=above />
            <TextField label="For (minutes)" value=minutes />
            <button class="button button-quiet" type="submit" disabled=move || busy.get()>
                {move || if busy.get() { "Adding" } else { "Add rule" }}
            </button>
            <p class="entry-note">"Alerts once when it starts, and once when it clears."</p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(subject: &str, enabled: bool, firing: bool) -> AlertRule {
        AlertRule {
            id: 1,
            subject: subject.to_owned(),
            metric: Metric::Memory,
            above_pct: 80.0,
            for_min: 10,
            enabled,
            firing,
        }
    }

    #[test]
    fn a_rule_reads_as_a_sentence() {
        let stacks = vec![("stack:3".to_owned(), "Blog".to_owned())];
        let words = |subject| rule_words(&rule(subject, true, false), &stacks);
        assert_eq!(words("stack:3"), "Blog memory above 80% for 10 min");
        assert_eq!(words("stack:9"), "stack:9 memory above 80% for 10 min");
        assert_eq!(
            words("container:web-1"),
            "web-1 memory above 80% for 10 min"
        );
        assert_eq!(words("host"), "Host memory above 80% for 10 min");
    }

    #[test]
    fn a_rule_state_is_shown_in_words_too() {
        assert_eq!(
            rule_state(&rule("host", true, true)),
            ("unhealthy", "firing now")
        );
        assert_eq!(rule_state(&rule("host", true, false)), ("running", "clear"));
        assert_eq!(rule_state(&rule("host", false, true)), ("stopped", "off"));
    }

    #[test]
    fn a_failed_delivery_says_why_and_how_often_it_was_tried() {
        let line = delivery_line(&AlertDelivery {
            id: 1,
            channel: "ops".to_owned(),
            subject: "Blog".to_owned(),
            state: "down".to_owned(),
            at: chrono::Utc::now(),
            ok: false,
            attempts: 4,
            error: Some("could not connect".to_owned()),
        });
        assert_eq!(line.state, "unhealthy");
        assert!(
            line.detail.contains("after 4 tries: could not connect"),
            "{}",
            line.detail
        );
    }
}
