//! Checking the path contract.
//!
//! GhostDock runs `docker compose` inside its own container, while the daemon
//! that acts on the result is on the host. A relative bind mount in a
//! compose file is resolved to an absolute path inside GhostDock's container
//! and then interpreted by the daemon *on the host*. The two agree only if
//! GhostDock's data directory is at the same path in both places.
//!
//! When they do not agree the failure is silent: the deploy succeeds, the
//! daemon creates an empty directory at the path it was given, and the
//! application quietly reads nothing. Verified against Docker 29.7.2. This
//! check exists so that mistake is reported at startup instead of being
//! discovered through a broken application.

use std::path::{Component, Path, PathBuf};

/// One mount on GhostDock's own container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Where it comes from on the host.
    pub source: String,
    /// Where it appears inside the container.
    pub destination: String,
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathContract {
    /// The data directory is at the same path on the host.
    Satisfied,
    /// It is not. Relative bind mounts in compose files will be wrong.
    Violated {
        container_path: String,
        host_path: String,
    },
    /// The data directory is not on any mount, so it lives in the container's
    /// own filesystem and does not exist on the host at all.
    Unmounted { container_path: String },
    /// Could not tell -- not running in a container, or it could not be
    /// inspected. Not a problem to report.
    Unknown,
}

impl PathContract {
    /// A sentence for a person, when there is something to say.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Satisfied | Self::Unknown => None,
            Self::Violated {
                container_path,
                host_path,
            } => Some(format!(
                "The data directory is mounted at {container_path} inside the container but comes \
                 from {host_path} on the host. Relative paths in compose files will point at \
                 empty directories. Mount it at the same path on both sides: \
                 -v {container_path}:{container_path}"
            )),
            Self::Unmounted { container_path } => Some(format!(
                "The data directory {container_path} is not mounted from the host, so stacks and \
                 settings will be lost when the container is replaced, and relative paths in \
                 compose files will not work. Mount it: -v {container_path}:{container_path}"
            )),
        }
    }
}

/// Checks whether `data_dir` is at the same path on the host.
///
/// The governing mount is the one with the longest destination that
/// contains `data_dir`, since the data directory may sit inside a larger
/// mount -- `/srv:/srv` with data in `/srv/ghostdock` is perfectly correct, and
/// a check that only looked for an exact match would call it broken.
#[must_use]
pub fn check(data_dir: &str, mounts: Option<&[Mount]>) -> PathContract {
    let Some(mounts) = mounts else {
        return PathContract::Unknown;
    };
    let data = normalise(Path::new(data_dir));

    let governing = mounts
        .iter()
        .filter(|m| data.starts_with(normalise(Path::new(&m.destination))))
        .max_by_key(|m| normalise(Path::new(&m.destination)).components().count());

    let Some(mount) = governing else {
        return PathContract::Unmounted {
            container_path: data_dir.to_owned(),
        };
    };

    // Where the data directory is on the host: the mount's source, plus
    // whatever lies beneath the mount point.
    let destination = normalise(Path::new(&mount.destination));
    let beneath = data.strip_prefix(&destination).unwrap_or(Path::new(""));
    let host = normalise(&Path::new(&mount.source).join(beneath));

    if host == data {
        PathContract::Satisfied
    } else {
        PathContract::Violated {
            container_path: data_dir.to_owned(),
            host_path: host.display().to_string(),
        }
    }
}

/// Resolves `.` and trailing slashes lexically, without touching the disk.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
