//! Mapping bollard's daemon types onto GhostDock's wire types.
//!
//! Kept separate and pure so the awkward parts — optional everything, the
//! leading slash on names, Compose labels — are testable without a daemon.

use bollard::models::{
    ContainerSummary, ContainerSummaryHealthStatusEnum, ContainerSummaryStateEnum, PortSummary,
};
use chrono::DateTime;
use shared::container::{ComposeMembership, Container, ContainerState, Health, Port};

/// Compose writes these onto every container it creates.
pub const LABEL_PROJECT: &str = "com.docker.compose.project";
pub const LABEL_SERVICE: &str = "com.docker.compose.service";

/// Converts one daemon container summary into the wire type.
#[must_use]
pub fn to_container(summary: ContainerSummary) -> Container {
    let id = summary.id.unwrap_or_default();
    let status = summary.status.unwrap_or_default();

    Container {
        name: primary_name(summary.names.as_deref(), &id),
        image: summary.image.unwrap_or_default(),
        state: map_state(summary.state.as_ref()),
        health: map_health(
            summary.health.as_ref().and_then(|h| h.status.as_ref()),
            &status,
        ),
        created: summary.created.and_then(|s| DateTime::from_timestamp(s, 0)),
        ports: map_ports(summary.ports.unwrap_or_default()),
        compose: map_compose(summary.labels.as_ref()),
        status,
        id,
    }
}

/// The daemon prefixes names with `/`; users never see that form.
pub(crate) fn primary_name(names: Option<&[String]>, id: &str) -> String {
    names
        .and_then(<[String]>::first)
        .map(|n| n.trim_start_matches('/').to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| id.to_owned())
}

fn map_state(state: Option<&ContainerSummaryStateEnum>) -> ContainerState {
    use ContainerSummaryStateEnum as S;
    match state {
        Some(S::CREATED) => ContainerState::Created,
        Some(S::RUNNING) => ContainerState::Running,
        Some(S::PAUSED) => ContainerState::Paused,
        Some(S::RESTARTING) => ContainerState::Restarting,
        Some(S::REMOVING) => ContainerState::Removing,
        Some(S::EXITED) => ContainerState::Exited,
        Some(S::DEAD) => ContainerState::Dead,
        _ => ContainerState::Unknown,
    }
}

/// Prefers the structured field, falling back to the status string.
///
/// Older daemons omit the field entirely, and losing health silently would
/// hide exactly the condition a user most needs to see.
fn map_health(
    status: Option<&ContainerSummaryHealthStatusEnum>,
    status_text: &str,
) -> Option<Health> {
    use ContainerSummaryHealthStatusEnum as H;
    match status {
        Some(H::STARTING) => Some(Health::Starting),
        Some(H::HEALTHY) => Some(Health::Healthy),
        Some(H::UNHEALTHY) => Some(Health::Unhealthy),
        // NONE means the image defines no healthcheck — genuinely absent.
        Some(H::NONE) => None,
        _ => domain::health::from_status(status_text),
    }
}

/// Normalises the daemon's port list for display.
///
/// A published port on a dual-stack host is reported once per address family
/// (`0.0.0.0` and `::`), which is one binding, not two. Collapsing them keeps
/// the UI from showing the same mapping twice; the IPv4 address is kept
/// because that is the one a user can paste into a browser. The result is
/// sorted so rows do not reorder between polls.
fn map_ports(ports: Vec<PortSummary>) -> Vec<Port> {
    let mut mapped: Vec<Port> = ports.into_iter().map(map_port).collect();

    mapped.sort_by(|a, b| {
        (a.container_port, &a.protocol, a.host_port)
            .cmp(&(b.container_port, &b.protocol, b.host_port))
            // Within one binding, prefer a non-IPv6-wildcard address.
            .then_with(|| is_v6_wildcard(a).cmp(&is_v6_wildcard(b)))
    });
    mapped.dedup_by(|a, b| {
        a.container_port == b.container_port
            && a.protocol == b.protocol
            && a.host_port == b.host_port
    });
    mapped
}

fn is_v6_wildcard(port: &Port) -> bool {
    port.host_ip.as_deref() == Some("::")
}

fn map_port(port: PortSummary) -> Port {
    Port {
        host_ip: port.ip,
        host_port: port.public_port,
        container_port: port.private_port,
        protocol: port.typ.map_or_else(
            || "tcp".to_owned(),
            |t| {
                let s = t.to_string();
                if s.is_empty() { "tcp".to_owned() } else { s }
            },
        ),
    }
}

/// Both labels are required: a project without a service is not addressable.
fn map_compose(
    labels: Option<&std::collections::HashMap<String, String>>,
) -> Option<ComposeMembership> {
    let labels = labels?;
    Some(ComposeMembership {
        project: labels.get(LABEL_PROJECT)?.clone(),
        service: labels.get(LABEL_SERVICE)?.clone(),
    })
}

/// Picks the registry digest matching `image` from a list of repo digests.
///
/// An image pulled under several names carries several entries, and taking
/// the first would compare one repository's digest against another's. Only
/// the entry whose repository matches the reference is meaningful.
#[must_use]
pub fn repo_digest(repo_digests: Option<&[String]>, image: &str) -> Option<String> {
    let digests = repo_digests?;
    let repository =
        image.rsplit_once(':').map_or(
            image,
            |(name, tag)| {
                if tag.contains('/') { image } else { name }
            },
        );

    // Exact repository match first.
    for entry in digests {
        if let Some((entry_repo, digest)) = entry.split_once('@')
            && entry_repo == repository
        {
            return Some(digest.to_owned());
        }
    }

    // A single entry is unambiguous even if the name is written differently
    // (`nginx` against `docker.io/library/nginx`), which is the common case.
    if digests.len() == 1 {
        return digests[0].split_once('@').map(|(_, d)| d.to_owned());
    }
    None
}
