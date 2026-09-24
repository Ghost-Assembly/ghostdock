//! Project slugs.
//!
//! A slug becomes both a directory name on disk and a Compose project name,
//! so it is a security boundary: a stack called `../../etc` must not be able
//! to reach outside the stacks directory, and a name Compose rejects must be
//! caught before we shell out.

/// Longest slug accepted. Compose itself has no hard limit, but container
/// names are derived from it and Docker's own limit is 63 characters.
pub const MAX_LEN: usize = 48;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SlugError {
    #[error("name must not be empty")]
    Empty,
    #[error("name must be {MAX_LEN} characters or fewer")]
    TooLong,
    #[error("name must start with a letter or digit")]
    BadStart,
    #[error("name may contain only lowercase letters, digits, dashes and underscores")]
    BadCharacter,
}

/// Validates a slug for use as a directory name and Compose project name.
///
/// Deliberately a whitelist. A blacklist that tries to reject `..` and `/`
/// invites the next encoding that means the same thing.
pub fn validate(slug: &str) -> Result<(), SlugError> {
    if slug.is_empty() {
        return Err(SlugError::Empty);
    }
    if slug.len() > MAX_LEN {
        return Err(SlugError::TooLong);
    }
    let first = slug.as_bytes()[0];
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(SlugError::BadStart);
    }
    if !slug
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(SlugError::BadCharacter);
    }
    Ok(())
}

/// Best-effort conversion of a human name into a valid slug.
///
/// Returns `None` when nothing usable survives, rather than inventing a name
/// the user did not choose.
#[must_use]
pub fn from_name(name: &str) -> Option<String> {
    let mut out = String::new();
    for ch in name.trim().chars() {
        match ch {
            'a'..='z' | '0'..='9' => out.push(ch),
            'A'..='Z' => out.push(ch.to_ascii_lowercase()),
            '-' | '_' => out.push(ch),
            ' ' | '.' | '/' | ':' => out.push('-'),
            _ => {}
        }
    }
    // Leading punctuation would fail validation, and a leading dash is also
    // how an argument gets mistaken for a flag.
    let trimmed = out
        .trim_start_matches(['-', '_'])
        .trim_end_matches(['-', '_']);
    let slug: String = trimmed.chars().take(MAX_LEN).collect();
    let slug = slug.trim_end_matches(['-', '_']).to_owned();

    validate(&slug).ok().map(|()| slug)
}
