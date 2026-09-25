//! Shared application state.

use std::path::Path;
use std::sync::Arc;

use compose::Compose;
use docker::Client;
use store::Store;

use crate::checker::Checker;
use crate::revocation::Revocations;
use crate::runner::Runner;

/// Everything a handler needs. Cheap to clone; every field shares a handle.
#[derive(Debug, Clone)]
pub struct AppState {
    pub store: Store,
    /// `None` when the daemon could not be reached at startup.
    ///
    /// GhostDock still serves the UI in that case so it can explain the problem.
    /// A management tool that refuses to boot because the thing it manages is
    /// down is useless exactly when it is needed.
    pub docker: Option<Client>,
    pub runner: Runner,
    pub checker: Checker,
    /// Origins allowed to open a WebSocket besides the server's own host.
    ///
    /// For a reverse proxy that does not preserve the original Host header.
    pub allowed_origins: Arc<Vec<String>>,
    /// Problems with GhostDock's own deployment, found at startup.
    pub problems: Arc<Vec<String>>,
    /// Tells long-lived connections when their credential is withdrawn.
    pub revocations: Revocations,
    /// Resource figures: the live hour, and history when a store is given.
    pub sampler: crate::metrics::Sampler,
    /// Failed sign-ins, so passwords cannot be guessed at full speed.
    pub login_limiter: crate::limiter::LoginLimiter,
    /// Sends alerts, and evaluates the resource rules that raise them.
    pub alerts: crate::alerts::Alerts,
    /// Runs the uptime checks.
    pub checks: crate::checks::Checks,
}

impl AppState {
    /// `stacks_root` holds one materialised project directory per stack.
    pub fn new(store: Store, docker: Option<Client>, stacks_root: &Path) -> Self {
        Self::with_origins(store, docker, stacks_root, Vec::new())
    }

    /// As [`AppState::new`], plus origins accepted for WebSocket handshakes.
    pub fn with_origins(
        store: Store,
        docker: Option<Client>,
        stacks_root: &Path,
        allowed_origins: Vec<String>,
    ) -> Self {
        let runner = Runner::new(Compose::new(stacks_root), store.clone());
        let checker = Checker::new(
            store.clone(),
            docker.clone(),
            runner.clone(),
            store::hosts::LOCAL_HOST_ID,
        );
        let alerts = crate::alerts::Alerts::new(store.clone());
        let checks = crate::checks::Checks::new(
            store.clone(),
            None,
            docker.clone(),
            runner.clone(),
            alerts.clone(),
        );
        Self {
            store,
            docker,
            runner,
            checker,
            allowed_origins: Arc::new(allowed_origins),
            problems: Arc::new(Vec::new()),
            revocations: Revocations::new(),
            sampler: crate::metrics::Sampler::new(None, crate::metrics::HostPaths::default()),
            login_limiter: crate::limiter::LoginLimiter::default(),
            alerts,
            checks,
        }
    }

    /// Checks built again around the current alerts and history, before
    /// anything has started them.
    fn rebuild_checks(&mut self) {
        self.checks = crate::checks::Checks::new(
            self.store.clone(),
            self.sampler.store().cloned(),
            self.docker.clone(),
            self.runner.clone(),
            self.alerts.clone(),
        );
    }

    /// Retries alert deliveries after `backoff` rather than seconds: for
    /// tests.
    #[must_use]
    pub fn with_alert_backoff(mut self, backoff: std::time::Duration) -> Self {
        self.alerts = self.alerts.with_backoff(backoff);
        self.rebuild_checks();
        self
    }
}

impl AppState {
    /// Records problems found while starting up.
    #[must_use]
    pub fn with_problems(mut self, problems: Vec<String>) -> Self {
        self.problems = Arc::new(problems);
        self
    }

    /// Keeps history in `store`. Without it, only the live hour exists.
    #[must_use]
    pub fn with_metrics(mut self, store: store::metrics::MetricsStore) -> Self {
        self.sampler =
            crate::metrics::Sampler::new(Some(store), crate::metrics::HostPaths::default());
        self.rebuild_checks();
        self
    }
}
