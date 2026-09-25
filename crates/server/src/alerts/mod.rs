//! Sending alerts, and the resource rules that raise them.
//!
//! Every channel gets every alert. A delivery is tried up to four times,
//! waiting longer each time, and how it went is recorded; the last 200 are
//! kept. A channel's URL and token are opened only here, only to send, and
//! nothing about them but the channel's name is ever logged or recorded.

pub mod routes;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use domain::alerts::{RuleChange, RuleTracker, Sample, Scope, Subject};
use shared::alerts::{Alert, AlertDelivery, ChannelKind};
use store::Store;
use store::alerts::ChannelSecret;

use crate::metrics::Minute;

/// Tries per delivery: the first and three more.
const ATTEMPTS: u32 = 4;
/// The wait before the first retry; each after waits twice as long.
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
/// How long one attempt may take.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Alerts {
    store: Store,
    client: probe::Client,
    backoff: Duration,
    /// Each rule's streak and whether it has fired, by rule id.
    rules: Arc<Mutex<HashMap<i64, RuleTracker>>>,
}

impl std::fmt::Debug for Alerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alerts").finish_non_exhaustive()
    }
}

impl Alerts {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            client: probe::Client::new(),
            backoff: FIRST_BACKOFF,
            rules: Arc::default(),
        }
    }

    /// Waits `backoff` before the first retry instead: for tests.
    #[must_use]
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Sends `alert` to every channel, in the background.
    pub fn send(&self, alert: Alert) {
        let me = self.clone();
        tokio::spawn(async move { me.deliver_all(&alert).await });
    }

    /// Sends `alert` to every channel and waits until each is done.
    pub async fn deliver_all(&self, alert: &Alert) -> Vec<AlertDelivery> {
        let channels = match self.store.channels_to_send().await {
            Ok(channels) => channels,
            Err(e) => {
                tracing::error!(error = %e, "could not read the alert channels");
                return Vec::new();
            }
        };
        let sends = channels
            .iter()
            .map(|channel| self.deliver(channel, alert, ATTEMPTS));
        futures::future::join_all(sends).await
    }

    /// Sends a test alert to one channel, once. `None` if there is no such
    /// channel.
    pub async fn test(&self, channel_id: i64) -> Result<Option<AlertDelivery>, store::Error> {
        let channels = self.store.channels_to_send().await?;
        let Some(channel) = channels.iter().find(|c| c.id == channel_id) else {
            return Ok(None);
        };
        let alert = Alert {
            kind: shared::alerts::AlertKind::Test,
            subject: channel.name.clone(),
            state: "test".to_owned(),
            message: format!("A test from GhostDock to {}. It works.", channel.name),
            at: chrono::Utc::now(),
            url: None,
        };
        Ok(Some(self.deliver(channel, &alert, 1).await))
    }

    /// Tries up to `attempts` times, and records how it went.
    async fn deliver(
        &self,
        channel: &ChannelSecret,
        alert: &Alert,
        attempts: u32,
    ) -> AlertDelivery {
        let (body, mut headers) = match channel.kind {
            ChannelKind::Webhook => (
                serde_json::to_vec(alert).unwrap_or_default(),
                vec![("Content-Type", "application/json".to_owned())],
            ),
            ChannelKind::Ntfy => {
                let n = domain::alerts::ntfy(alert);
                (
                    n.body.into_bytes(),
                    vec![
                        ("Title", n.title),
                        ("Priority", n.priority.to_owned()),
                        ("Tags", n.tags.to_owned()),
                    ],
                )
            }
        };
        if let Some(token) = &channel.token {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let mut tried = 0;
        let mut wait = self.backoff;
        let outcome = loop {
            tried += 1;
            match self
                .client
                .post(&channel.url, &headers, body.clone(), SEND_TIMEOUT)
                .await
            {
                Ok(_) => break Ok(()),
                Err(e) if tried >= attempts => break Err(e),
                Err(_) => {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
            }
        };
        if let Err(e) = &outcome {
            // The channel's name only: its URL is the secret.
            tracing::warn!(channel = %channel.name, error = %e, "an alert was not delivered");
        }
        let error = outcome.err();
        match self
            .store
            .delivery_record(
                &channel.name,
                &alert.subject,
                &alert.state,
                error.is_none(),
                tried,
                error.as_deref(),
            )
            .await
        {
            Ok(recorded) => recorded,
            Err(e) => {
                tracing::error!(error = %e, "could not record an alert delivery");
                AlertDelivery {
                    id: 0,
                    channel: channel.name.clone(),
                    subject: alert.subject.clone(),
                    state: alert.state.clone(),
                    at: alert.at,
                    ok: error.is_none(),
                    attempts: tried,
                    error,
                }
            }
        }
    }

    // ---- resource rules -------------------------------------------------

    /// Whether rule `id` is firing now.
    #[must_use]
    pub fn firing(&self, id: i64) -> bool {
        self.lock().get(&id).is_some_and(|t| t.firing)
    }

    /// Starts rule `id` afresh, as after it was changed or removed.
    pub fn reset(&self, id: i64) {
        self.lock().remove(&id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<i64, RuleTracker>> {
        self.rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Evaluates the rules against each minute's figures as sampling
    /// produces them, for as long as the process runs.
    pub fn spawn_rules(self, sampler: &crate::metrics::Sampler) {
        let mut minutes = sampler.minutes();
        tokio::spawn(async move {
            while minutes.changed().await.is_ok() {
                let minute = minutes.borrow_and_update().clone();
                if let Some(minute) = minute {
                    self.evaluate(&minute).await;
                }
            }
        });
    }

    /// Folds one minute into every enabled rule, and alerts on each that
    /// starts or stops being breached.
    pub async fn evaluate(&self, minute: &Minute) {
        let rules = match self.store.rules_list().await {
            Ok(rules) => rules,
            Err(e) => {
                tracing::warn!(error = %e, "could not read the alert rules");
                return;
            }
        };
        let samples: Vec<Sample<'_>> = minute
            .rows
            .iter()
            .map(|row| Sample {
                kind: row.kind,
                key: &row.key,
                project: row.project.as_deref(),
                reading: &row.reading,
            })
            .collect();
        let at = chrono::DateTime::from_timestamp(minute.t, 0).unwrap_or_default();
        for rule in rules.into_iter().filter(|r| r.enabled) {
            let Some(subject) = Subject::parse(&rule.subject) else {
                continue;
            };
            // A stack's containers are filed under its project name.
            let (label, project) = match &subject {
                Subject::Host => ("host".to_owned(), None),
                Subject::Container(name) => (name.clone(), None),
                Subject::Stack(id) => match self.store.stack_by_id(*id).await {
                    Ok(Some(stack)) => (stack.name, Some(stack.slug)),
                    // Forgotten: nothing left to watch.
                    _ => continue,
                },
            };
            let scope = match (&subject, &project) {
                (Subject::Container(name), _) => Scope::Container(name),
                (_, Some(project)) => Scope::Project(project),
                _ => Scope::Host,
            };
            let pct =
                domain::alerts::usage(scope, rule.metric, &samples, minute.cpus, minute.memory);
            let change =
                self.lock()
                    .entry(rule.id)
                    .or_default()
                    .observe(pct, rule.above_pct, rule.for_min);
            if let Some(change) = change {
                self.send(domain::alerts::rule_alert(
                    &label,
                    rule.metric,
                    rule.above_pct,
                    rule.for_min,
                    pct,
                    change,
                    at,
                ));
                if change == RuleChange::Fired {
                    tracing::info!(rule = rule.id, subject = %label, "a resource rule fired");
                }
            }
        }
    }
}
