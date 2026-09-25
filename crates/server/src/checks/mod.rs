//! Running uptime checks on a schedule.
//!
//! One task per enabled check. Each first run is spread out at random over
//! the check's interval when GhostDock starts, so a restart does not fire
//! every check at once; a check just made or changed runs within a couple
//! of seconds. At most 16 probes are in flight at a time, whatever the
//! number of checks. Changing a check restarts its task.
//!
//! Checks run from GhostDock's own network. A container check needs no
//! probe: it reads the container's running and health state from Docker,
//! and runs again as soon as Docker reports a change to that container.

pub mod routes;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use domain::checks::{Probe, Tracker};
use shared::checks::{Check, CheckKind, CheckState, CheckStatus};
use shared::event::ServerEvent;
use store::Store;
use store::metrics::MetricsStore;
use tokio::sync::Semaphore;

use crate::alerts::Alerts;
use crate::runner::Runner;

/// Probes in flight at once, across every check.
const CONCURRENT: usize = 16;
/// A check just made or changed runs within this.
const SOON: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Checks {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    metrics: Option<MetricsStore>,
    docker: Option<docker::Client>,
    runner: Runner,
    alerts: Alerts,
    client: probe::Client,
    permits: Semaphore,
    /// Set by [`Checks::start`]; until then nothing runs by itself.
    started: AtomicBool,
    tasks: Mutex<HashMap<i64, tokio::task::AbortHandle>>,
    live: Mutex<HashMap<i64, Live>>,
}

/// A check's state in this run of GhostDock.
struct Live {
    tracker: Tracker,
    status: CheckStatus,
}

impl std::fmt::Debug for Checks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checks").finish_non_exhaustive()
    }
}

fn pending(check_id: i64, state: CheckState) -> CheckStatus {
    CheckStatus {
        check_id,
        state,
        since: Some(Utc::now()),
        last_at: None,
        latency_ms: None,
        message: None,
        tls_days_left: None,
    }
}

impl Checks {
    #[must_use]
    pub fn new(
        store: Store,
        metrics: Option<MetricsStore>,
        docker: Option<docker::Client>,
        runner: Runner,
        alerts: Alerts,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                metrics,
                docker,
                runner,
                alerts,
                client: probe::Client::new(),
                permits: Semaphore::new(CONCURRENT),
                started: AtomicBool::new(false),
                tasks: Mutex::default(),
                live: Mutex::default(),
            }),
        }
    }

    fn tasks(&self) -> std::sync::MutexGuard<'_, HashMap<i64, tokio::task::AbortHandle>> {
        self.inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Live>> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts every enabled check, each first run at a random point in its
    /// interval.
    pub async fn start(&self) {
        self.inner.started.store(true, Ordering::SeqCst);
        let checks = match self.inner.store.checks_all().await {
            Ok(checks) => checks,
            Err(e) => {
                tracing::error!(error = %e, "could not read the uptime checks");
                return;
            }
        };
        for check in checks {
            let first = jitter(Duration::from_secs(u64::from(check.interval_s)));
            self.begin(check, first).await;
        }
    }

    /// Picks up a check just made or changed: whatever was running for it
    /// stops, and it starts again from pending, or paused if turned off.
    pub async fn restart(&self, id: i64) {
        self.forget(id);
        match self.inner.store.check_by_id(id).await {
            Ok(Some(check)) => {
                self.begin(check, SOON).await;
                if let Some(status) = self.status(id) {
                    self.inner
                        .runner
                        .publish(ServerEvent::CheckChanged { check: status });
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!(error = %e, check = id, "could not read a changed check"),
        }
    }

    /// Stops a check and forgets how it was doing.
    pub fn forget(&self, id: i64) {
        if let Some(task) = self.tasks().remove(&id) {
            task.abort();
        }
        self.live().remove(&id);
    }

    /// How `id` is doing, if it is known.
    #[must_use]
    pub fn status(&self, id: i64) -> Option<CheckStatus> {
        self.live().get(&id).map(|l| l.status.clone())
    }

    /// Records the check's starting state, and runs it if it is enabled
    /// and checks have been started.
    async fn begin(&self, check: Check, first: Duration) {
        let open = self
            .inner
            .store
            .incident_is_open(check.id)
            .await
            .unwrap_or(false);
        let state = if check.enabled {
            CheckState::Pending
        } else {
            CheckState::Paused
        };
        self.live().insert(
            check.id,
            Live {
                tracker: Tracker::new(open),
                status: pending(check.id, state),
            },
        );
        if !check.enabled || !self.inner.started.load(Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let id = check.id;
        let task = tokio::spawn(async move { me.run_forever(check, first).await });
        if let Some(old) = self.tasks().insert(id, task.abort_handle()) {
            old.abort();
        }
    }

    async fn run_forever(self, check: Check, first: Duration) {
        let interval = Duration::from_secs(u64::from(check.interval_s));
        // A container check also runs when Docker says its container changed.
        let mut changes =
            (check.kind == CheckKind::Container).then(|| self.inner.runner.subscribe());
        tokio::time::sleep(first).await;
        loop {
            self.run(&check).await;
            match changes.as_mut() {
                Some(events) => {
                    tokio::select! {
                        () = tokio::time::sleep(interval) => {}
                        () = changed(events, &check.target) => {}
                    }
                }
                None => tokio::time::sleep(interval).await,
            }
        }
    }

    /// Runs `id` once, now. For tests, which cannot wait out an interval.
    pub async fn run_now(&self, id: i64) -> Option<CheckStatus> {
        let check = self.inner.store.check_by_id(id).await.ok()??;
        let known = self.live().contains_key(&id);
        if !known {
            let open = self.inner.store.incident_is_open(id).await.unwrap_or(false);
            self.live().insert(
                id,
                Live {
                    tracker: Tracker::new(open),
                    status: pending(id, CheckState::Pending),
                },
            );
        }
        self.run(&check).await;
        self.status(id)
    }

    /// One run: probe, record, move the state, and tell whoever needs to
    /// know.
    async fn run(&self, check: &Check) {
        let (probe, latency_ms, tls_days_left) = {
            let _permit = self.inner.permits.acquire().await;
            self.probe(check).await
        };
        let now = Utc::now();
        if let Some(metrics) = &self.inner.metrics
            && let Err(e) = metrics
                .record_check(
                    check.id,
                    now.timestamp(),
                    !matches!(probe, Probe::Failed(_)),
                    latency_ms,
                )
                .await
        {
            tracing::warn!(error = %e, check = check.id, "could not record a check run");
        }

        let (step, status) = {
            let mut live = self.live();
            // Removed while it ran.
            let Some(entry) = live.get_mut(&check.id) else {
                return;
            };
            let step = entry.tracker.observe(&probe, check.retries);
            let status = &mut entry.status;
            status.state = entry.tracker.state;
            status.last_at = Some(now);
            status.latency_ms = latency_ms;
            status.tls_days_left = tls_days_left;
            status.message = match &probe {
                Probe::Ok => None,
                Probe::Degraded(why) | Probe::Failed(why) => Some(why.clone()),
            };
            if step.changed.is_some() {
                status.since = Some(now);
            }
            (step, status.clone())
        };

        let store = &self.inner.store;
        if let Some(cause) = &step.open
            && let Err(e) = store.incident_open(check.id, cause, now.timestamp()).await
        {
            tracing::error!(error = %e, check = check.id, "could not open an incident");
        }
        if step.close
            && let Err(e) = store.incident_close(check.id, now.timestamp()).await
        {
            tracing::error!(error = %e, check = check.id, "could not close an incident");
        }
        if step.changed.is_some() {
            self.inner.runner.publish(ServerEvent::CheckChanged {
                check: status.clone(),
            });
        }
        if step.alert && check.notify {
            self.inner
                .alerts
                .send(domain::alerts::check_alert(check, &status, now));
        }
    }

    /// What one run found, how long it took, and the certificate's days.
    async fn probe(&self, check: &Check) -> (Probe, Option<u32>, Option<i64>) {
        let timeout = Duration::from_secs(u64::from(check.timeout_s));
        let ms = |d: Duration| u32::try_from(d.as_millis()).unwrap_or(u32::MAX);
        match check.kind {
            CheckKind::Http => {
                match self
                    .inner
                    .client
                    .http(&check.target, timeout, check.keyword.as_deref())
                    .await
                {
                    Ok(answer) => {
                        let latency = ms(answer.latency);
                        let probe = match domain::checks::judge_http(
                            answer.status,
                            (check.expect_status_min, check.expect_status_max),
                            answer.keyword_found,
                        ) {
                            Ok(()) => domain::checks::assess(
                                answer.tls_days_left,
                                Some(latency),
                                check.latency_warn_ms,
                            ),
                            Err(why) => Probe::Failed(why),
                        };
                        (probe, Some(latency), answer.tls_days_left)
                    }
                    Err(why) => (Probe::Failed(why), None, None),
                }
            }
            CheckKind::Tcp => match probe::tcp(&check.target, timeout).await {
                Ok(took) => {
                    let latency = ms(took);
                    (
                        domain::checks::assess(None, Some(latency), check.latency_warn_ms),
                        Some(latency),
                        None,
                    )
                }
                Err(why) => (Probe::Failed(why), None, None),
            },
            CheckKind::Container => {
                let Some(docker) = &self.inner.docker else {
                    return (
                        Probe::Failed("the Docker daemon is not reachable".to_owned()),
                        None,
                        None,
                    );
                };
                match tokio::time::timeout(timeout, docker.list_containers()).await {
                    Ok(Ok(containers)) => {
                        let found = containers
                            .iter()
                            .find(|c| c.name == check.target)
                            .map(|c| (c.state, c.health));
                        (domain::checks::judge_container(found), None, None)
                    }
                    _ => (
                        Probe::Failed("the Docker daemon did not answer".to_owned()),
                        None,
                        None,
                    ),
                }
            }
        }
    }
}

/// Returns when Docker reports a change to the container named `name`, or
/// when events were missed, which may have included one.
async fn changed(events: &mut tokio::sync::broadcast::Receiver<ServerEvent>, name: &str) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match events.recv().await {
            Ok(ServerEvent::ContainerChanged { change })
                if change.name.as_deref() == Some(name) =>
            {
                return;
            }
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return,
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// A point in `[0, span)`, different each call. Spreading first runs needs
/// no more randomness than the clock's nanoseconds.
fn jitter(span: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let span_ms = u64::try_from(span.as_millis()).unwrap_or(u64::MAX).max(1);
    // Spread by a large odd multiplier, so neighbours in time land apart.
    Duration::from_millis(u64::from(nanos).wrapping_mul(2_654_435_761) % span_ms)
}
