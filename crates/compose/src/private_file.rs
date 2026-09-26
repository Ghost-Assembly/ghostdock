//! Writing a file only its owner can read.
//!
//! Shared by two crates that may not depend on each other: `compose` writes
//! a stack's `.env` with it, and `store` the key that seals secrets at rest.
//! `store` includes this file with `#[path]` rather than gaining an edge to
//! `compose`, and `shared`, which both already use, is kept to serde and
//! chrono. Standard library only, so it compiles unchanged in either crate.

use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Writes `bytes` to `path`, readable and writable by its owner only, and
/// on disk before it returns.
///
/// The mode is given at creation, so a new file is never briefly readable
/// by anyone else, and set again once the file is open, because a mode at
/// creation does nothing to a file that already exists. Both happen before
/// a byte is written. With `replace` false, an existing file is an error
/// rather than overwritten.
///
/// The directory is flushed too: a file whose contents reached the disk
/// but whose name did not is lost in a crash all the same.
pub(crate) fn write_private(path: &Path, bytes: &[u8], replace: bool) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).mode(0o600);
    if replace {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.set_permissions(Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}
