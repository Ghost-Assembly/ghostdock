//! GhostDock server binary.

use std::time::Duration;

use anyhow::Context;
use server::app;
use server::session_store::SqliteSessionStore;
use server::state::AppState;
use store::Store;
use tower_sessions::session_store::ExpiredDeletion;

/// How often expired sessions are swept from the database.
const SESSION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

fn main() -> anyhow::Result<()> {
    // The container's healthcheck. Handled before starting a runtime so it
    // stays cheap: it runs every thirty seconds for as long as GhostDock does.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthcheck() { 0 } else { 1 });
    }
    serve()
}

/// Asks the running server whether it is up.
///
/// A few lines of std rather than an HTTP client: the image deliberately
/// carries no curl, and this is one request to a known local address.
fn healthcheck() -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let bind = env_or("GHOSTDOCK_BIND", "0.0.0.0:8080");
    let port = bind.rsplit_once(':').map_or("8080", |(_, port)| port);
    let Ok(addr) = format!("127.0.0.1:{port}").parse::<SocketAddr>() else {
        return false;
    };

    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));

    if stream
        .write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return false;
    }

    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200")
}

#[tokio::main]
async fn serve() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ghostdock=debug,server=debug".into()),
        )
        .init();

    let data_dir = env_or("GHOSTDOCK_DATA_DIR", "/var/lib/ghostdock");
    let bind = env_or("GHOSTDOCK_BIND", "0.0.0.0:8080");
    let secure_cookies = env_flag("GHOSTDOCK_COOKIE_SECURE");
    let ui_dir = env_or("GHOSTDOCK_UI_DIR", "/usr/share/ghostdock/web");

    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("creating data directory {data_dir}"))?;

    // Must be reachable at the same absolute path as on the Docker host: the
    // compose CLI resolves relative paths in a stack's file client-side, while
    // the daemon interprets the result.
    let stacks_root = std::path::PathBuf::from(format!("{data_dir}/stacks"));
    tokio::fs::create_dir_all(&stacks_root)
        .await
        .with_context(|| format!("creating {}", stacks_root.display()))?;

    // Reading the key is the server's job, not the store's: configuration
    // does not belong in the persistence layer.
    let cipher = store::secrets::Cipher::load_or_create(
        std::path::Path::new(&data_dir),
        std::env::var(store::secrets::KEY_ENV).ok().as_deref(),
    )
    .context("loading the encryption key")?;

    let db_path = format!("{data_dir}/ghostdock.db");
    let store = Store::open(&db_path, cipher)
        .await
        .with_context(|| format!("opening database {db_path}"))?;
    tracing::info!(db = %db_path, "database ready");

    let metrics_path = format!("{data_dir}/metrics.db");
    let metrics = match store::metrics::MetricsStore::open(&metrics_path).await {
        Ok(m) => Some(m),
        // Locked, unreadable, out of space: the file may be fine, and
        // replacing it would throw a year of history away over a hiccup.
        // Run without history until the next start instead.
        Err(e) if !e.is_corrupt() => {
            tracing::error!(error = %e, "metrics.db could not be opened; running without history");
            None
        }
        Err(e) => {
            // History is expendable; GhostDock is not. Set the damaged file aside
            // and start a fresh one.
            tracing::warn!(error = %e, "metrics.db is damaged; setting it aside and starting a new one");
            let aside = format!(
                "{metrics_path}.broken-{}",
                server::metrics::aligned(std::time::SystemTime::now(), 1)
            );
            let _ = tokio::fs::rename(&metrics_path, &aside).await;
            store::metrics::MetricsStore::open(&metrics_path).await.ok()
        }
    };

    // A management tool must start even when the thing it manages is down,
    // so an unreachable daemon is reported, not fatal.
    let docker = match docker::Client::connect() {
        Ok(client) => {
            match client.version().await {
                Ok(version) => tracing::info!(version = ?version, "connected to Docker daemon"),
                Err(e) => tracing::warn!(error = %e, "Docker daemon did not answer"),
            }
            Some(client)
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not connect to Docker; container views will be unavailable");
            None
        }
    };

    if !secure_cookies {
        tracing::warn!(
            "session cookies are not marked Secure; set GHOSTDOCK_COOKIE_SECURE=true when serving over HTTPS"
        );
    }

    // A deployment still marked running belongs to a command whose outcome
    // nobody will ever learn, so it is closed out rather than left spinning.
    match store.deployments_reap_stale().await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(count = n, "marked interrupted deployments as failed"),
        Err(e) => tracing::error!(error = %e, "could not reap interrupted deployments"),
    }

    if store.user_count().await? == 0 {
        tracing::info!("no accounts yet — open the UI to create the first administrator");
    }

    spawn_session_sweeper(store.clone());

    let allowed_origins =
        server::origin::parse_allowlist(std::env::var("GHOSTDOCK_ALLOWED_ORIGINS").ok().as_deref());
    if !allowed_origins.is_empty() {
        tracing::info!(?allowed_origins, "additional WebSocket origins allowed");
    }

    let problems = path_contract_problems(docker.as_ref(), &data_dir).await;
    let state = AppState::with_origins(store, docker, &stacks_root, allowed_origins)
        .with_problems(problems);
    let state = match metrics {
        Some(m) => state.with_metrics(m),
        None => state,
    };
    state
        .checker
        .clone()
        .spawn(server::checker::DEFAULT_INTERVAL);
    state
        .sampler
        .clone()
        .spawn(state.docker.clone(), state.runner.clone());
    if let Some(docker) = state.docker.clone() {
        server::watch::spawn(docker, state.runner.clone());
    }
    state.checks.start().await;
    state.alerts.clone().spawn_rules(&state.sampler);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "GhostDock listening");

    let ui_path = std::path::Path::new(&ui_dir);
    let ui = if ui_path.join("index.html").is_file() {
        tracing::info!(dir = %ui_dir, "serving web client");
        Some(ui_path)
    } else {
        tracing::warn!(dir = %ui_dir, "no web client found; serving the API only");
        None
    };

    // With each connection's address, which sign-in attempts are counted by.
    axum::serve(
        listener,
        app::build(state, secure_cookies, ui)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("server error")?;

    Ok(())
}

/// Checks that the data directory is at the same path on the host.
///
/// When it is not, relative bind mounts in compose files silently point at
/// empty directories: the deploy succeeds and the application reads
/// nothing. Reported here, loudly, rather than discovered through a broken
/// application.
async fn path_contract_problems(docker: Option<&docker::Client>, data_dir: &str) -> Vec<String> {
    let Some(docker) = docker else {
        return Vec::new();
    };
    // Docker names a container's host after its own short id unless told
    // otherwise, so this finds GhostDock's container when it is in one and
    // finds nothing when it is not.
    let Ok(me) = std::env::var("HOSTNAME") else {
        return Vec::new();
    };
    let mounts = docker.mounts_of(&me).await.ok().flatten();

    let result = domain::contract::check(data_dir, mounts.as_deref());
    match result.problem() {
        Some(problem) => {
            tracing::error!("{problem}");
            vec![problem]
        }
        None => {
            if result == domain::contract::PathContract::Satisfied {
                tracing::info!(dir = %data_dir, "data directory is at the same path on the host");
            }
            Vec::new()
        }
    }
}

/// Deletes expired sessions periodically.
///
/// Expired sessions are already unusable — `session_load` filters them — so
/// this only reclaims space, and a failure must never take the server down.
fn spawn_session_sweeper(store: Store) {
    let sessions = SqliteSessionStore::new(store);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SESSION_SWEEP_INTERVAL);
        loop {
            ticker.tick().await;
            if let Err(e) = sessions.delete_expired().await {
                tracing::warn!(error = %e, "sweeping expired sessions failed");
            }
        }
    });
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.ok() };
    let terminate = async {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        sig.recv().await
    };
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn env_flag(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}
