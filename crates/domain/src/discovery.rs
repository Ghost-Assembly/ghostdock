//! Finding stacks in a repository.

/// What discovery looks for unless told otherwise: a conventionally named
/// compose file in any directory, or any YAML file directly inside a
/// top-level `compose/` directory (one file per stack).
pub const DEFAULT_PATTERN: &str = "**/{compose,docker-compose}.{yml,yaml}, compose/*.{yml,yaml}";

const GENERIC: &[&str] = &["compose", "docker-compose"];

/// The name a stack found at `path` should be registered under.
///
/// A file named for its stack (`compose/blog.yml`) gives that name. A
/// conventionally named one (`blog/compose.yaml`) takes its directory's.
/// One at the repository root has no name of its own and returns `None`,
/// for the caller to name after the repository.
#[must_use]
pub fn stack_name(path: &str) -> Option<String> {
    let (dir, file) = path.rsplit_once('/').map_or(("", path), |(d, f)| (d, f));
    let stem = file
        .strip_suffix(".yml")
        .or_else(|| file.strip_suffix(".yaml"))
        .unwrap_or(file);
    if GENERIC.contains(&stem) {
        let parent = dir.rsplit('/').next().filter(|p| !p.is_empty())?;
        Some(parent.to_owned())
    } else {
        Some(stem.to_owned())
    }
}
