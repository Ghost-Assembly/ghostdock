//! Building `docker compose` invocations.
//!
//! Kept pure and separate: argv is exactly where a mistake silently targets
//! the wrong project or drops a flag, and it can be tested without a daemon.

use std::path::Path;

/// How aggressively to refresh images on deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pull {
    /// Re-pull every image. What a redeploy means to most people.
    Always,
    /// Only fetch images that are missing locally.
    Missing,
    /// Never contact a registry.
    Never,
}

impl Pull {
    fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Missing => "missing",
            Self::Never => "never",
        }
    }
}

/// Arguments common to every invocation, identifying which project is meant.
#[derive(Debug, Clone)]
pub struct Project<'a> {
    /// Compose project name. Becomes the `com.docker.compose.project` label.
    pub name: &'a str,
    /// Directory holding the compose file, and the base for relative paths.
    pub dir: &'a Path,
    /// Compose file within that directory.
    pub file: &'a str,
    /// Absolute path to an env file, when the stack has variables.
    ///
    /// Absolute, and deliberately not required to sit beside the compose
    /// file: for a Git-backed stack the compose file lives in the checked-out
    /// repository, and writing an env file next to it would both clobber any
    /// `.env` the repository ships and be deleted by the next `git clean`.
    pub env_file: Option<&'a Path>,
}

impl Project<'_> {
    fn base(&self) -> Vec<String> {
        let mut argv = vec![
            "compose".to_owned(),
            "--project-name".to_owned(),
            self.name.to_owned(),
            "--project-directory".to_owned(),
            self.dir.display().to_string(),
            "--file".to_owned(),
            self.dir.join(self.file).display().to_string(),
        ];
        if let Some(env_file) = self.env_file {
            argv.push("--env-file".to_owned());
            argv.push(env_file.display().to_string());
        }
        argv
    }
}

/// Fully resolved configuration, as Compose itself understands the file.
///
/// This is what makes a second parser unnecessary: the UI reads the same
/// interpretation the deployer acts on.
#[must_use]
pub fn config(project: &Project) -> Vec<String> {
    let mut argv = project.base();
    argv.extend([
        "config".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ]);
    argv
}

/// Bring a project up and wait for it to actually be healthy.
///
/// `--wait` is the reason a deploy here can report real failure: it blocks
/// until services are running or healthy, so a stack that starts and
/// immediately crash-loops is a failed deploy rather than a successful one.
#[must_use]
pub fn up(project: &Project, pull: Pull, wait_timeout_secs: u64) -> Vec<String> {
    let mut argv = project.base();
    argv.extend([
        "up".to_owned(),
        "--detach".to_owned(),
        "--pull".to_owned(),
        pull.as_str().to_owned(),
        // Services deleted from the file leave containers behind otherwise,
        // and those keep holding ports and names.
        "--remove-orphans".to_owned(),
        "--wait".to_owned(),
        "--wait-timeout".to_owned(),
        wait_timeout_secs.to_string(),
    ]);
    argv
}

/// Stop and remove a project's containers, leaving named volumes alone.
///
/// Volumes are never removed here. Deleting data must be a separate, explicit
/// act — "stop this stack" must not quietly mean "destroy its database".
#[must_use]
pub fn down(project: &Project) -> Vec<String> {
    let mut argv = project.base();
    argv.extend(["down".to_owned(), "--remove-orphans".to_owned()]);
    argv
}

/// Stop a project's containers without removing them.
#[must_use]
pub fn stop(project: &Project) -> Vec<String> {
    let mut argv = project.base();
    argv.push("stop".to_owned());
    argv
}

/// Restart a project's containers in place.
#[must_use]
pub fn restart(project: &Project) -> Vec<String> {
    let mut argv = project.base();
    argv.push("restart".to_owned());
    argv
}

/// Identifies a project by name only, for when its file is not at hand.
///
/// Compose then finds the project's containers and networks by their
/// labels. It also looks for a default-named file in `dir` and every
/// directory above it and would use one it found, so a caller checks
/// [`crate::default_file_above`] first.
fn by_name(name: &str, dir: &Path) -> Vec<String> {
    vec![
        "compose".to_owned(),
        "--project-name".to_owned(),
        name.to_owned(),
        "--project-directory".to_owned(),
        dir.display().to_string(),
    ]
}

/// [`down`], for a project known only by name. Volumes stay, as always.
#[must_use]
pub fn down_by_name(name: &str, dir: &Path) -> Vec<String> {
    let mut argv = by_name(name, dir);
    argv.extend(["down".to_owned(), "--remove-orphans".to_owned()]);
    argv
}

/// [`stop`], for a project known only by name.
#[must_use]
pub fn stop_by_name(name: &str, dir: &Path) -> Vec<String> {
    let mut argv = by_name(name, dir);
    argv.push("stop".to_owned());
    argv
}

/// [`restart`], for a project known only by name.
#[must_use]
pub fn restart_by_name(name: &str, dir: &Path) -> Vec<String> {
    let mut argv = by_name(name, dir);
    argv.push("restart".to_owned());
    argv
}
