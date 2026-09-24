//! Saying what is waiting, in words.
//!
//! The reason matters as much as the fact. "An update is available" sends
//! someone to read a diff; "nginx:alpine moved" tells them what they are
//! about to change, which is the whole difference between this and a tool
//! that simply redeploys on a timer.

use shared::update::UpdateStatus;

/// A short, plain-language description of what is waiting.
///
/// Returns `None` when nothing is.
#[must_use]
pub fn reason(status: &UpdateStatus) -> Option<String> {
    let behind = status.is_behind_git();
    let moved = status.moved_images();

    match (behind, moved.len()) {
        (false, 0) => None,
        (true, 0) => Some("a new commit".to_owned()),
        (false, 1) => Some(format!("{} moved", short_image(&moved[0].image))),
        (false, n) => Some(format!("{n} images moved")),
        (true, 1) => Some(format!(
            "a new commit and {} moved",
            short_image(&moved[0].image)
        )),
        (true, n) => Some(format!("a new commit and {n} images moved")),
    }
}

/// Trims a reference to the part a person recognises.
///
/// `ghcr.io/team/service/app:1.2` reads as `app:1.2`: the registry and
/// namespace are the same for every image in a stack and crowd out the part
/// that differs.
#[must_use]
pub fn short_image(image: &str) -> String {
    let (path, tag) = match image.rsplit_once(':') {
        Some((path, tag)) if !tag.contains('/') => (path, Some(tag)),
        _ => (image, None),
    };
    let name = path.rsplit('/').next().unwrap_or(path);

    match tag {
        Some(tag) => format!("{name}:{tag}"),
        None => name.to_owned(),
    }
}
