//! The host's own figures: `/proc` for CPU, memory and load (not
//! namespaced, so visible from inside GhostDock's container), and when mounted,
//! the host's network from `/host/proc` and disks under `/host/disks`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::task::JoinHandle;

use domain::metrics::{HostCounters, parse_loadavg, parse_meminfo, parse_net_dev, parse_proc_stat};

#[derive(Debug, Clone)]
pub struct HostPaths {
    pub proc: PathBuf,
    pub host_proc: PathBuf,
    pub disks: PathBuf,
    pub root: PathBuf,
}

impl Default for HostPaths {
    fn default() -> Self {
        Self {
            proc: "/proc".into(),
            host_proc: "/host/proc".into(),
            disks: "/host/disks".into(),
            root: "/".into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostSnapshot {
    pub counters: Option<HostCounters>,
    pub nets: Vec<(String, u64, u64)>,
    /// (key, used bytes, capacity bytes).
    pub disks: Vec<(String, u64, u64)>,
}

/// How long reading the host may take. A stale network share can hang
/// `statvfs`; the tick must not hang with it.
pub const DEADLINE: Duration = Duration::from_secs(2);

/// Disk space as (used, capacity) bytes, or `None` when a disk cannot say.
pub type Space = fn(&Path) -> Option<(u64, u64)>;

/// Reads the host once. For a single read; the sampler keeps one
/// [`HostReader`] so a stuck read is never started twice.
pub async fn read_host(paths: HostPaths) -> HostSnapshot {
    HostReader::new(paths, space).await.read().await
}

/// Reads the host's figures, each source on its own: a disk that hangs
/// costs its own figure and one blocked thread, never the host's CPU and
/// memory or another disk's space.
pub struct HostReader {
    paths: HostPaths,
    disks: Vec<(String, PathBuf)>,
    space: Space,
    /// Reads still running from an earlier pass. A stuck one is left to
    /// finish: not waited for again, not started again.
    proc_pending: Option<JoinHandle<ProcFigures>>,
    disk_pending: HashMap<String, JoinHandle<Option<(u64, u64)>>>,
}

type ProcFigures = (Option<HostCounters>, Vec<(String, u64, u64)>);

impl HostReader {
    /// Finds the disks to watch, once: mounts are fixed for the life of
    /// the container.
    pub async fn new(paths: HostPaths, space: Space) -> Self {
        let listing = paths.clone();
        let disks = tokio::task::spawn_blocking(move || find_disks(&listing))
            .await
            .unwrap_or_default();
        Self {
            paths,
            disks,
            space,
            proc_pending: None,
            disk_pending: HashMap::new(),
        }
    }

    /// Whatever the host can tell within [`DEADLINE`].
    pub async fn read(&mut self) -> HostSnapshot {
        let deadline = tokio::time::Instant::now() + DEADLINE;

        // Start every read that is not already stuck, then wait for them
        // together. A stuck read that has since finished is stale: dropped,
        // and a fresh one started.
        let proc = match self.proc_pending.take() {
            Some(stuck) if !stuck.is_finished() => {
                self.proc_pending = Some(stuck);
                None
            }
            _ => {
                let paths = self.paths.clone();
                Some(tokio::task::spawn_blocking(move || read_proc(&paths)))
            }
        };
        let mut started = Vec::new();
        for (key, path) in &self.disks {
            if self.disk_pending.get(key).is_some_and(|h| !h.is_finished()) {
                continue;
            }
            let (space, path) = (self.space, path.clone());
            self.disk_pending.insert(
                key.clone(),
                tokio::task::spawn_blocking(move || space(&path)),
            );
            started.push(key.clone());
        }

        let (counters, nets) = match proc {
            Some(mut handle) => match tokio::time::timeout_at(deadline, &mut handle).await {
                Ok(done) => done.unwrap_or_default(),
                Err(_) => {
                    tracing::warn!("reading /proc is taking too long; other figures continue");
                    self.proc_pending = Some(handle);
                    (None, Vec::new())
                }
            },
            None => (None, Vec::new()),
        };
        let mut disks = Vec::new();
        for key in started {
            let Some(handle) = self.disk_pending.get_mut(&key) else {
                continue;
            };
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(done) => {
                    self.disk_pending.remove(&key);
                    if let Ok(Some((used, capacity))) = done {
                        disks.push((key, used, capacity));
                    }
                }
                Err(_) => {
                    tracing::warn!(disk = %key, "a disk is not answering; its figure is left out")
                }
            }
        }
        HostSnapshot {
            counters,
            nets,
            disks,
        }
    }
}

/// CPU, memory and load, and the host's network when mounted.
fn read_proc(paths: &HostPaths) -> ProcFigures {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok();
    let counters = (|| {
        let (busy, total, cpus) = parse_proc_stat(&read(paths.proc.join("stat"))?)?;
        let (mem_total, mem_available) = parse_meminfo(&read(paths.proc.join("meminfo"))?)?;
        let load1 = read(paths.proc.join("loadavg")).and_then(|t| parse_loadavg(&t));
        Some(HostCounters {
            cpu_busy: busy,
            cpu_total: total,
            cpus,
            mem_total,
            mem_available,
            load1,
        })
    })();
    let nets = read(paths.host_proc.join("1/net/dev"))
        .map(|t| parse_net_dev(&t))
        .unwrap_or_default();
    (counters, nets)
}

/// The disk Docker uses (`/` inside GhostDock's container), and each directory
/// under `disks`. Directory entries only: their types come from the listing
/// itself, so a dead share mounted there cannot hang it.
fn find_disks(paths: &HostPaths) -> Vec<(String, PathBuf)> {
    let mut disks = vec![("/".to_owned(), paths.root.clone())];
    if let Ok(entries) = std::fs::read_dir(&paths.disks) {
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        found.sort();
        disks.extend(found.into_iter().map(|p| (p.display().to_string(), p)));
    }
    disks
}

/// Used and total bytes of the filesystem holding `path`.
#[must_use]
pub fn space(path: &Path) -> Option<(u64, u64)> {
    let s = rustix::fs::statvfs(path).ok()?;
    let block = s.f_frsize.max(1);
    let capacity = s.f_blocks.saturating_mul(block);
    let free = s.f_bavail.saturating_mul(block);
    (capacity > 0).then(|| (capacity.saturating_sub(free), capacity))
}
