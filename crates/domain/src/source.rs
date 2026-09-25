//! What may be handed to git as a repository URL or a ref.
//!
//! Both reach git's command line, and git reads more into them than an
//! address: a URL starting with `-` is an option (`--upload-pack=<command>`
//! runs the command), and `ext::<command>` is a transport that runs one.
//! Checked where they enter, so the person typing one is told, and again
//! in `gitsync`, which never lets either be read as an option.

/// Schemes GhostDock fetches over. Anything else is refused rather than
/// passed on to whatever git would make of it.
const SCHEMES: [&str; 5] = ["https", "http", "ssh", "git", "file"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceProblem {
    #[error("Give the repository a URL.")]
    Empty,
    #[error("A repository URL cannot start with -.")]
    LeadingDash,
    #[error("A repository URL cannot contain spaces or control characters.")]
    Whitespace,
    #[error("Use an https, http, ssh, git or file URL, or user@host:path.")]
    Scheme,
    #[error("The repository URL has no host.")]
    NoHost,
    #[error("Remove the password from the URL and add it under Credentials instead.")]
    PasswordInUrl,
    #[error("Name the branch or tag, for example refs/heads/main.")]
    RefEmpty,
    #[error("A branch or tag cannot start with -.")]
    RefLeadingDash,
    #[error("A branch or tag cannot contain spaces or control characters.")]
    RefWhitespace,
}

/// Checks a repository URL before it is stored or used.
///
/// Accepts `https`, `http`, `ssh`, `git` and `file` URLs and the scp-like
/// `user@host:path`. A plain local path is refused: `file://` says the same
/// thing without git having to guess.
pub fn check_repo_url(url: &str) -> Result<(), SourceProblem> {
    if url.is_empty() {
        return Err(SourceProblem::Empty);
    }
    if url.starts_with('-') {
        return Err(SourceProblem::LeadingDash);
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(SourceProblem::Whitespace);
    }
    // `<transport>::<address>` hands the address to a remote helper, and
    // `ext::` runs it as a command.
    if let Some((transport, _)) = url.split_once("::")
        && !transport.is_empty()
        && transport
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Err(SourceProblem::Scheme);
    }

    if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !SCHEMES.contains(&scheme.as_str()) {
            return Err(SourceProblem::Scheme);
        }
        let authority = rest.split('/').next().unwrap_or_default();
        let (userinfo, host) = match authority.rsplit_once('@') {
            Some((userinfo, host)) => (Some(userinfo), host),
            None => (None, authority),
        };
        if userinfo.is_some_and(|u| u.contains(':')) {
            return Err(SourceProblem::PasswordInUrl);
        }
        if scheme == "file" {
            return Ok(());
        }
        return check_host(host);
    }

    // scp-like: `[user@]host:path`, with the colon before any slash.
    match url.split_once(':') {
        Some((prefix, _)) if !prefix.contains('/') => {
            let host = prefix.rsplit_once('@').map_or(prefix, |(_, host)| host);
            check_host(host)
        }
        _ => Err(SourceProblem::Scheme),
    }
}

fn check_host(host: &str) -> Result<(), SourceProblem> {
    // The port, if any, is not part of what could be read as an option.
    if host.is_empty() || host.starts_with(':') {
        return Err(SourceProblem::NoHost);
    }
    // ssh would read a host starting with `-` as an option of its own.
    if host.starts_with('-') {
        return Err(SourceProblem::LeadingDash);
    }
    Ok(())
}

/// Checks a branch, tag or other ref before it is stored or used.
///
/// Deliberately loose about what a ref may be (git decides that), and
/// strict only about what would let it be read as something else.
pub fn check_git_ref(git_ref: &str) -> Result<(), SourceProblem> {
    if git_ref.is_empty() {
        return Err(SourceProblem::RefEmpty);
    }
    if git_ref.starts_with('-') {
        return Err(SourceProblem::RefLeadingDash);
    }
    if git_ref.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(SourceProblem::RefWhitespace);
    }
    Ok(())
}
