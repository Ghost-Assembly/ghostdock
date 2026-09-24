//! Parsing image references.
//!
//! Pure, and tested heavily, because the defaults are not obvious and
//! getting them wrong means asking the wrong registry about the wrong
//! repository and quietly reporting "no update" forever.
//!
//! The rules Docker actually applies:
//!   - `nginx`                  -> registry-1.docker.io, library/nginx, :latest
//!   - `user/app`               -> registry-1.docker.io, user/app, :latest
//!   - `ghcr.io/user/app:1.2`   -> ghcr.io, user/app, :1.2
//!   - `localhost:5000/app`     -> localhost:5000, app, :latest
//!
//! The first component is a registry only if it contains a dot or a colon,
//! or is exactly `localhost`. That is the rule Docker uses, and it is why
//! `myteam/app` is Docker Hub rather than a host called `myteam`.

/// Where an unqualified image comes from.
pub const DEFAULT_REGISTRY: &str = "registry-1.docker.io";

/// Docker Hub's implicit namespace for single-word images.
pub const DEFAULT_NAMESPACE: &str = "library";

/// Applied when a reference names no tag and no digest.
pub const DEFAULT_TAG: &str = "latest";

/// A parsed image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// Registry host, including a port when one was given.
    pub registry: String,
    /// Repository path, with Docker Hub's `library/` filled in.
    pub repository: String,
    /// Tag, absent when the reference pins a digest.
    pub tag: Option<String>,
    /// Digest, when the reference pins one.
    pub digest: Option<String>,
}

impl Reference {
    /// True when the reference already pins an exact image.
    ///
    /// Such an image cannot drift, so polling a registry about it is wasted
    /// effort and a wasted rate-limit token.
    #[must_use]
    pub fn is_pinned(&self) -> bool {
        self.digest.is_some()
    }

    /// What to ask the registry about.
    #[must_use]
    pub fn manifest_target(&self) -> &str {
        self.digest
            .as_deref()
            .or(self.tag.as_deref())
            .unwrap_or(DEFAULT_TAG)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("image reference is empty")]
    Empty,
    #[error("image reference {0:?} is not something GhostDock can parse")]
    Malformed(String),
}

/// Parses an image reference the way Docker does.
pub fn parse(image: &str) -> Result<Reference, ParseError> {
    let image = image.trim();
    if image.is_empty() {
        return Err(ParseError::Empty);
    }

    // A digest, if present, always comes last and is unambiguous.
    let (remainder, digest) = match image.split_once('@') {
        Some((rest, digest)) if !digest.is_empty() => (rest, Some(digest.to_owned())),
        Some(_) => return Err(ParseError::Malformed(image.to_owned())),
        None => (image, None),
    };

    let (registry, path) = split_registry(remainder);

    // A colon after the last slash is a tag; one before it is a port, which
    // `split_registry` has already taken.
    let (repository, tag) = match path.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') && !tag.is_empty() => {
            (repo.to_owned(), Some(tag.to_owned()))
        }
        _ => (path.to_owned(), None),
    };

    if repository.is_empty() {
        return Err(ParseError::Malformed(image.to_owned()));
    }

    // Docker Hub keeps single-word images under `library/`.
    let repository = if registry == DEFAULT_REGISTRY && !repository.contains('/') {
        format!("{DEFAULT_NAMESPACE}/{repository}")
    } else {
        repository
    };

    Ok(Reference {
        registry,
        repository,
        // An explicit digest replaces the implied `latest` rather than
        // joining it; asking for both would be contradictory.
        tag: if digest.is_some() {
            tag
        } else {
            Some(tag.unwrap_or_else(|| DEFAULT_TAG.to_owned()))
        },
        digest,
    })
}

/// Splits a leading registry host from the repository path.
fn split_registry(remainder: &str) -> (String, &str) {
    match remainder.split_once('/') {
        Some((first, rest)) if is_registry_host(first) => (first.to_owned(), rest),
        _ => (DEFAULT_REGISTRY.to_owned(), remainder),
    }
}

/// Docker's rule: a leading component is a host only if it looks like one.
fn is_registry_host(component: &str) -> bool {
    component == "localhost" || component.contains('.') || component.contains(':')
}
