//! Resource figures: sampling, the live hour, and what is kept.

pub mod host;
pub mod routes;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use domain::metrics::{Accumulator, sum_at};
use shared::metrics::{
    Current, LIVE_STEP, MAX_POINTS, Now, Reading, StackNow, SubjectKind, Target,
};
use store::metrics::MetricsStore;

pub use host::HostPaths;

/// 5 s points in the live hour.
const RING: usize = 720;
/// A subject silent this long has stopped: out of the snapshot.
const SILENT_SECS: i64 = 15;
/// The live hour, in seconds.
const HOUR: i64 = 3_600;

#[derive(Debug, Clone)]
pub struct MinuteRow {
    pub kind: SubjectKind,
    pub key: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub reading: Reading,
}

#[derive(Default)]
struct Live {
    project: Option<String>,
    service: Option<String>,
    pending: Accumulator,
    minute: Accumulator,
    ring: VecDeque<Reading>,
    last_seen: i64,
    latest: Option<Reading>,
}

#[derive(Default)]
struct Inner {
    live: HashMap<(SubjectKind, String), Live>,
    now: Now,
    unavailable: bool,
    host_memory: Option<u64>,
    host_cpus: Option<u32>,
}

#[derive(Clone)]
pub struct Sampler {
    inner: Arc<Mutex<Inner>>,
    store: Option<MetricsStore>,
    paths: HostPaths,
    /// Sizing advice and when it was worked out. An async lock, held while
    /// working it out, so requests arriving together share one pass.
    sizing: Arc<tokio::sync::Mutex<Option<Advice>>>,
}

/// Sizing advice and the minute it was worked out.
type Advice = (i64, Vec<shared::metrics::Recommendation>);

/// How long sizing advice stands. It rests on 30 days of history; working
/// it out costs tens of milliseconds a container.
const SIZING_FRESH_SECS: i64 = 900;

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

impl Sampler {
    #[must_use]
    pub fn new(store: Option<MetricsStore>, paths: HostPaths) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            store,
            paths,
            sizing: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn store(&self) -> Option<&MetricsStore> {
        self.store.as_ref()
    }

    pub fn observe(
        &self,
        kind: SubjectKind,
        key: &str,
        project: Option<&str>,
        service: Option<&str>,
        reading: &Reading,
    ) {
        let mut inner = self.lock();
        let live = inner.live.entry((kind, key.to_owned())).or_default();
        if project.is_some() {
            live.project = project.map(str::to_owned);
        }
        if service.is_some() {
            live.service = service.map(str::to_owned);
        }
        live.pending.add(reading);
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        self.lock().unavailable = unavailable;
    }

    pub fn set_host(&self, memory: u64, cpus: u32) {
        let mut inner = self.lock();
        inner.host_memory = Some(memory);
        inner.host_cpus = Some(cpus);
    }

    #[must_use]
    pub fn host_memory(&self) -> Option<u64> {
        self.lock().host_memory
    }

    /// Closes the 5 s point at `t` for every subject that reported, and
    /// rebuilds the snapshot.
    pub fn tick(&self, t: i64) -> Now {
        let mut inner = self.lock();
        for live in inner.live.values_mut() {
            if let Some(point) = live.pending.finish(t) {
                live.minute.add(&point);
                if live.ring.len() == RING {
                    live.ring.pop_front();
                }
                live.ring.push_back(point);
                live.latest = Some(point);
                live.last_seen = t;
            }
        }
        let fresh = |l: &Live| t - l.last_seen <= SILENT_SECS && l.latest.is_some();
        let current = |kind: SubjectKind| {
            let mut list: Vec<Current> = inner
                .live
                .iter()
                .filter(|((k, _), l)| *k == kind && fresh(l))
                .filter_map(|((_, key), l)| {
                    Some(Current {
                        key: key.clone(),
                        project: l.project.clone(),
                        reading: l.latest?,
                    })
                })
                .collect();
            list.sort_by(|a, b| a.key.cmp(&b.key));
            list
        };
        let containers = current(SubjectKind::Container);
        let mut projects: Vec<String> = containers
            .iter()
            .filter_map(|c| c.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        let stacks = projects
            .into_iter()
            .map(|project| {
                let readings: Vec<Reading> = containers
                    .iter()
                    .filter(|c| c.project.as_deref() == Some(&project))
                    .map(|c| c.reading)
                    .collect();
                let cpu_hour = stack_cpu_hour(&inner, &project, t);
                StackNow {
                    reading: sum_at(t, &readings),
                    project,
                    cpu_hour,
                }
            })
            .collect();
        let host = inner
            .live
            .get(&(SubjectKind::Host, "host".to_owned()))
            .filter(|l| fresh(l))
            .and_then(|l| l.latest);
        let now = Now {
            at: t,
            host,
            host_cpus: inner.host_cpus,
            containers,
            stacks,
            disks: current(SubjectKind::Disk),
            networks: current(SubjectKind::Network),
            containers_unavailable: inner.unavailable,
        };
        inner.now = now.clone();
        now
    }

    #[must_use]
    pub fn now(&self) -> Now {
        self.lock().now.clone()
    }

    /// Every subject's folded minute, stamped `t`, and a fresh minute.
    /// Subjects silent for the whole live hour are forgotten: one-off
    /// containers would otherwise pile up for the life of the process.
    pub fn take_minute(&self, t: i64) -> Vec<MinuteRow> {
        let mut inner = self.lock();
        let rows = inner
            .live
            .iter_mut()
            .filter_map(|((kind, key), live)| {
                Some(MinuteRow {
                    kind: *kind,
                    key: key.clone(),
                    project: live.project.clone(),
                    service: live.service.clone(),
                    reading: live.minute.finish(t)?,
                })
            })
            .collect();
        inner
            .live
            .retain(|_, live| !live.pending.is_empty() || t - live.last_seen <= HOUR);
        rows
    }

    /// The live hour for a subject, or a stack's containers summed per tick.
    /// Only the hour before the latest tick: a container that stopped long
    /// ago has no last hour.
    #[must_use]
    pub fn ring(&self, target: &Target) -> Vec<Reading> {
        let inner = self.lock();
        let since = inner.now.at - HOUR;
        let recent = move |r: &&Reading| r.t > since;
        match target {
            Target::Subject(kind, key) => inner
                .live
                .get(&(*kind, key.clone()))
                .map(|l| l.ring.iter().filter(recent).copied().collect())
                .unwrap_or_default(),
            Target::Stack(project) => {
                let mut by_t: std::collections::BTreeMap<i64, Vec<Reading>> =
                    std::collections::BTreeMap::new();
                for ((kind, _), live) in &inner.live {
                    if *kind == SubjectKind::Container && live.project.as_deref() == Some(project) {
                        for r in live.ring.iter().filter(recent) {
                            by_t.entry(r.t).or_default().push(*r);
                        }
                    }
                }
                by_t.into_iter().map(|(t, rs)| sum_at(t, &rs)).collect()
            }
        }
    }
}

/// A stack's CPU over the last hour, one point a minute, oldest first; a
/// minute with nothing running is `None`.
fn stack_cpu_hour(inner: &Inner, project: &str, t: i64) -> Vec<Option<f64>> {
    let start = t - 3_600 + 60;
    let mut minutes = vec![(0.0_f64, 0_u32); 60];
    for ((kind, _), live) in &inner.live {
        if *kind != SubjectKind::Container || live.project.as_deref() != Some(project) {
            continue;
        }
        // Per container, average its points in each minute; then add
        // containers together.
        let mut own = vec![(0.0_f64, 0_u32); 60];
        for r in &live.ring {
            let idx = (r.t - start).div_euclid(60);
            if let (Ok(i), Some(cpu)) = (usize::try_from(idx), r.cpu)
                && let Some(slot) = own.get_mut(i)
            {
                slot.0 += cpu;
                slot.1 += 1;
            }
        }
        for (slot, (sum, n)) in minutes.iter_mut().zip(own) {
            if n > 0 {
                slot.0 += sum / f64::from(n);
                slot.1 += 1;
            }
        }
    }
    minutes
        .into_iter()
        .map(|(sum, n)| (n > 0).then_some(sum))
        .collect()
}

/// Unix seconds, rounded down to a multiple of `step`.
#[must_use]
pub fn aligned(now: std::time::SystemTime, step: i64) -> i64 {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    secs - secs.rem_euclid(step)
}

pub const TICK: i64 = LIVE_STEP;

/// A series for `target` over `range`: the live hour from memory, longer
/// ranges from the store, already folded there to at most `MAX_POINTS`.
/// The live hour is 720 points; the caller thins it.
pub async fn read_series(
    sampler: &Sampler,
    target: &Target,
    range: shared::metrics::Range,
) -> Result<Vec<Reading>, crate::error::ApiError> {
    use shared::metrics::Resolution;
    let resolution = range.resolution();
    if resolution == Resolution::Live {
        return Ok(sampler.ring(target));
    }
    let Some(store) = sampler.store() else {
        return Ok(Vec::new());
    };
    let to = aligned(std::time::SystemTime::now(), 60) + 60;
    let from = to - range.seconds();
    Ok(match target {
        Target::Stack(project) => {
            store
                .read_stack_series(project, resolution, from, to, MAX_POINTS)
                .await?
        }
        Target::Subject(kind, key) => match store.find(*kind, key).await? {
            Some(row) => {
                store
                    .read_series(row.id, resolution, from, to, MAX_POINTS)
                    .await?
            }
            None => Vec::new(),
        },
    })
}

/// Each of a stack's containers over `range`: from the live hour, or from
/// the store for longer ranges.
pub async fn container_figures(
    sampler: &Sampler,
    project: &str,
    range: shared::metrics::Range,
) -> Result<Vec<shared::metrics::ContainerFigures>, crate::error::ApiError> {
    let resolution = range.resolution();
    if resolution == shared::metrics::Resolution::Live {
        return Ok(sampler.ring_figures(project));
    }
    let Some(store) = sampler.store() else {
        return Ok(Vec::new());
    };
    let to = aligned(std::time::SystemTime::now(), 60) + 60;
    Ok(store
        .container_figures(project, resolution, to - range.seconds(), to)
        .await?)
}

impl Sampler {
    /// Each of a stack's containers over the live hour.
    #[must_use]
    pub fn ring_figures(&self, project: &str) -> Vec<shared::metrics::ContainerFigures> {
        let inner = self.lock();
        let since = inner.now.at - HOUR;
        let mut list: Vec<_> = inner
            .live
            .iter()
            .filter(|((kind, _), live)| {
                *kind == SubjectKind::Container && live.project.as_deref() == Some(project)
            })
            .filter_map(|((_, key), live)| {
                let points: Vec<Reading> =
                    live.ring.iter().filter(|r| r.t > since).copied().collect();
                (!points.is_empty())
                    .then(|| domain::metrics::figures(key, live.service.as_deref(), &points))
            })
            .collect();
        list.sort_by(|a, b| a.key.cmp(&b.key));
        list
    }

    /// Starts sampling: host figures and container streams every 5 s, a
    /// row per subject per minute, quarter-hour rollups, a nightly prune.
    pub fn spawn(self, docker: Option<docker::Client>, runner: crate::runner::Runner) {
        self.set_unavailable(docker.is_none());
        let ticks = self.clone();
        let publisher = runner.clone();
        tokio::spawn(async move { ticks.run_ticks(publisher).await });
        let minutes = self.clone();
        tokio::spawn(async move { minutes.run_minutes().await });
        if let Some(docker) = docker {
            let streams = self.clone();
            tokio::spawn(async move { streams.run_streams(docker, runner).await });
        }
    }

    async fn run_ticks(self, runner: crate::runner::Runner) {
        let mut prev_host: Option<(domain::metrics::HostCounters, std::time::Instant)> = None;
        let mut prev_nets: HashMap<String, ((u64, u64), std::time::Instant)> = HashMap::new();
        let mut every = tokio::time::interval(std::time::Duration::from_secs(5));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Kept across ticks, so a read stuck on a hung mount is never
        // started again: that would pile up blocked threads.
        let mut reader = host::HostReader::new(self.paths.clone(), host::space).await;
        loop {
            every.tick().await;
            let snap = reader.read().await;
            let at = std::time::Instant::now();
            if let Some(c) = snap.counters {
                self.set_host(c.mem_total, c.cpus);
                if let Some((p, _)) = prev_host
                    && let Some(r) = domain::metrics::host_rate(&p, &c)
                {
                    self.observe(SubjectKind::Host, "host", None, None, &r);
                }
                prev_host = Some((c, at));
            }
            let seen: Vec<String> = snap.nets.iter().map(|n| n.0.clone()).collect();
            for (name, rx, tx) in snap.nets {
                if let Some((p, then)) = prev_nets.get(&name)
                    && let Some(r) =
                        domain::metrics::net_rate(*p, (rx, tx), at.duration_since(*then))
                {
                    self.observe(SubjectKind::Network, &name, None, None, &r);
                }
                prev_nets.insert(name, ((rx, tx), at));
            }
            // Interfaces that went away, remembered only while /proc answers.
            if snap.counters.is_some() {
                prev_nets.retain(|name, _| seen.contains(name));
            }
            for (key, used, capacity) in snap.disks {
                let r = Reading {
                    mem: Some(used),
                    mem_max: Some(used),
                    mem_limit: Some(capacity),
                    ..Reading::default()
                };
                self.observe(SubjectKind::Disk, &key, None, None, &r);
            }
            let now = self.tick(aligned(std::time::SystemTime::now(), TICK));
            runner.publish(shared::event::ServerEvent::Metrics { now: Box::new(now) });
        }
    }

    async fn run_minutes(self) {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        every.tick().await;
        let mut last_prune = 0_i64;
        let mut rolled_to: Option<i64> = None;
        loop {
            every.tick().await;
            let minute = aligned(std::time::SystemTime::now(), 60) - 60;
            let rows = self.take_minute(minute);
            let Some(store) = self.store.clone() else {
                continue;
            };
            let mut batch = Vec::with_capacity(rows.len());
            for row in rows {
                match store
                    .subject(
                        row.kind,
                        &row.key,
                        row.project.as_deref(),
                        row.service.as_deref(),
                        minute,
                    )
                    .await
                {
                    Ok(id) => batch.push((id, row.reading)),
                    Err(e) => tracing::warn!(error = %e, "could not record a subject"),
                }
            }
            if let Err(e) = store.write_minute(&batch).await {
                tracing::warn!(error = %e, "could not write a minute of figures");
            }
            if let Some((from, to)) = domain::metrics::quarter_due(minute + 60, rolled_to) {
                match store.rollup(from, to).await {
                    Ok(_) => rolled_to = Some(to),
                    Err(e) => tracing::warn!(error = %e, "could not roll up a quarter hour"),
                }
            }
            if minute - last_prune >= 86_400 {
                last_prune = minute;
                if let Err(e) = store.prune(minute).await {
                    tracing::warn!(error = %e, "could not prune figures");
                }
            }
        }
    }

    async fn run_streams(self, docker: docker::Client, runner: crate::runner::Runner) {
        use futures::StreamExt as _;
        let streaming: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
        let mut events = runner.subscribe();
        let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = reconcile.tick() => {}
                event = events.recv() => {
                    if let Ok(shared::event::ServerEvent::ContainerChanged { change }) = event {
                        self.note_event(&change).await;
                        if change.action != "start" { continue; }
                    } else {
                        continue;
                    }
                }
            }
            let containers = match docker.list_containers().await {
                Ok(list) => {
                    self.set_unavailable(false);
                    list
                }
                Err(_) => {
                    self.set_unavailable(true);
                    continue;
                }
            };
            for c in containers.into_iter().filter(|c| c.state.is_running()) {
                // By id: a container recreated under the same name is a new
                // container, and its stream must not be mistaken for the old
                // one's, which is about to end.
                let fresh = streaming
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(c.id.clone());
                if !fresh {
                    continue;
                }
                let me = self.clone();
                let client = docker.clone();
                let set = Arc::clone(&streaming);
                let project = c.compose.as_ref().map(|m| m.project.clone());
                let service = c.compose.as_ref().map(|m| m.service.clone());
                tokio::spawn(async move {
                    let mut stream = Box::pin(client.stats(&c.id));
                    let mut prev: Option<(domain::metrics::Counters, std::time::Instant)> = None;
                    while let Some(Ok(counters)) = stream.next().await {
                        let at = std::time::Instant::now();
                        if let Some((p, then)) = prev
                            && let Some(r) = domain::metrics::rate(
                                &p,
                                &counters,
                                at.duration_since(then),
                                me.host_memory(),
                            )
                        {
                            me.observe(
                                SubjectKind::Container,
                                &c.name,
                                project.as_deref(),
                                service.as_deref(),
                                &r,
                            );
                        }
                        prev = Some((counters, at));
                    }
                    set.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&c.id);
                });
            }
        }
    }

    async fn note_event(&self, change: &shared::event::ContainerChange) {
        let kind = match change.action.as_str() {
            "oom" => "oom",
            "restart" => "restart",
            _ => return,
        };
        let (Some(store), Some(name)) = (self.store.as_ref(), change.name.as_deref()) else {
            return;
        };
        let t = aligned(std::time::SystemTime::now(), 1);
        match store
            .subject(
                SubjectKind::Container,
                name,
                change.project.as_deref(),
                None,
                t,
            )
            .await
        {
            Ok(id) => {
                if let Err(e) = store.record_event(id, t, kind).await {
                    tracing::warn!(error = %e, "could not record a container event");
                }
                // An OOM kill changes the advice now, not in 15 minutes.
                if kind == "oom" {
                    *self.sizing.lock().await = None;
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not record a subject"),
        }
    }
}

impl Sampler {
    /// Sizing advice for every container seen in the last 30 days, flagged
    /// ones first. Worked out at most every 15 minutes.
    pub async fn sizing(
        &self,
    ) -> Result<Vec<shared::metrics::Recommendation>, crate::error::ApiError> {
        let Some(store) = self.store.clone() else {
            return Ok(Vec::new());
        };
        let now = aligned(std::time::SystemTime::now(), 60);
        let mut cached = Arc::clone(&self.sizing).lock_owned().await;
        if let Some((at, list)) = cached.as_ref()
            && now - at < SIZING_FRESH_SECS
        {
            return Ok(list.clone());
        }
        // Its own task, holding the lock until the advice is kept: a request
        // that gives up (a browser's deadline, on slow storage) must not take
        // the pass with it, or the advice would never appear.
        let pass = tokio::spawn(async move {
            let out = work_out_sizing(&store, now).await?;
            *cached = Some((now, out.clone()));
            Ok::<_, store::Error>(out)
        });
        match pass.await {
            Ok(done) => Ok(done?),
            Err(e) => Err(crate::error::ApiError::Internal(anyhow::Error::new(e))),
        }
    }
}

async fn work_out_sizing(
    store: &MetricsStore,
    now: i64,
) -> Result<Vec<shared::metrics::Recommendation>, store::Error> {
    use shared::metrics::Severity;
    let since = now - store::metrics::MINUTE_DAYS * 86_400;
    let mut out = Vec::new();
    for s in store.containers_seen_since(since).await? {
        let summary = store.sizing_summary(s.id, since, now + 60).await?;
        let ooms = store.count_events(s.id, "oom", since).await?;
        out.push(domain::sizing::advise(
            &s.key,
            s.project.as_deref(),
            s.service.as_deref(),
            &summary,
            ooms,
        ));
    }
    out.sort_by_key(|r| {
        (
            r.flags
                .iter()
                .map(|f| f.severity)
                .min()
                .unwrap_or(Severity::Info),
            r.flags.is_empty(),
            r.container.clone(),
        )
    });
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use shared::event::ContainerChange;
    use shared::metrics::Severity;

    #[tokio::test]
    async fn an_oom_kill_changes_the_advice_at_once() {
        let store = MetricsStore::open_in_memory().await.unwrap();
        let sampler = Sampler::new(Some(store.clone()), HostPaths::default());
        let now = aligned(std::time::SystemTime::now(), 60);
        let id = store
            .subject(SubjectKind::Container, "x-1", Some("x"), None, now)
            .await
            .unwrap();
        let rows: Vec<_> = (0..4 * 1440)
            .map(|i| {
                let r = Reading {
                    t: now - 4 * 86_400 + i * 60,
                    cpu: Some(0.1),
                    mem: Some(100 << 20),
                    mem_limit: Some(128 << 20),
                    ..Reading::default()
                };
                (id, r)
            })
            .collect();
        store.write_minute(&rows).await.unwrap();
        let bad = |list: &[shared::metrics::Recommendation]| {
            list[0].flags.iter().any(|f| f.severity == Severity::Bad)
        };
        assert!(!bad(&sampler.sizing().await.unwrap()));

        sampler
            .note_event(&ContainerChange {
                container_id: "abc".to_owned(),
                name: Some("x-1".to_owned()),
                project: Some("x".to_owned()),
                action: "oom".to_owned(),
            })
            .await;
        assert!(
            bad(&sampler.sizing().await.unwrap()),
            "not 15 minutes later"
        );
    }

    #[tokio::test]
    async fn a_sizing_pass_finishes_when_its_request_goes() {
        // On slow storage a pass can outlast the browser's deadline; if the
        // dropped request took the pass with it, advice would never appear.
        let store = MetricsStore::open_in_memory().await.unwrap();
        let sampler = Sampler::new(Some(store.clone()), HostPaths::default());
        let now = aligned(std::time::SystemTime::now(), 60);
        let id = store
            .subject(SubjectKind::Container, "x-1", Some("x"), None, now)
            .await
            .unwrap();
        let row = Reading {
            t: now - 60,
            cpu: Some(0.1),
            mem: Some(1 << 20),
            ..Reading::default()
        };
        store.write_minute(&[(id, row)]).await.unwrap();

        // The request gives up at its first wait.
        let _ = tokio::time::timeout(std::time::Duration::ZERO, sampler.sizing()).await;
        let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while sampler.sizing.lock().await.is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(finished.is_ok(), "the pass went with its request");
    }
}

#[cfg(test)]
mod forgetting {
    use super::*;

    fn cpu(v: f64) -> Reading {
        Reading {
            cpu: Some(v),
            ..Reading::default()
        }
    }

    #[test]
    fn a_subject_silent_for_an_hour_is_forgotten() {
        // Names from one-off runs would otherwise pile up for the life of
        // the process, each with an hour of points walked every tick.
        let s = Sampler::new(None, HostPaths::default());
        s.observe(SubjectKind::Container, "job-1", None, None, &cpu(0.5));
        s.tick(0);
        for i in 1..=740 {
            s.observe(SubjectKind::Host, "host", None, None, &cpu(0.1));
            s.tick(i * 5);
            if i % 12 == 0 {
                s.take_minute(i * 5);
            }
        }
        let inner = s.lock();
        assert!(
            !inner
                .live
                .contains_key(&(SubjectKind::Container, "job-1".to_owned()))
        );
        assert!(
            inner
                .live
                .contains_key(&(SubjectKind::Host, "host".to_owned()))
        );
    }
}
