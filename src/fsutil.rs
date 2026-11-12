//! Private directories and files, through `grindvakt_core::fs`.
//!
//! On Linux and macOS everything is owner-only (`0700` directories, `0600` files) and symlinks
//! are refused. On Windows the data directory is read by the Grindvakt service, which runs as its
//! own virtual account `NT SERVICE\Grindvakt`, so files get a protected DACL for SYSTEM,
//! Administrators and that account only, and a directory owned by an ordinary user is refused.

use anyhow::Result;
use std::path::Path;

pub use grindvakt_core::fs::Privacy;

/// The Windows service name; the service runs as the virtual account `NT SERVICE\Grindvakt`.
pub const SERVICE_NAME: &str = "Grindvakt";

/// The privacy policy for everything Grindvakt writes.
#[cfg(windows)]
pub const PRIVACY: Privacy = Privacy::ServiceAccount(SERVICE_NAME);
/// The privacy policy for everything Grindvakt writes.
#[cfg(not(windows))]
pub const PRIVACY: Privacy = Privacy::CurrentUser;

/// Creates `path` (and parents) private under [`PRIVACY`], or checks an existing one.
pub fn ensure_dir_0700(path: &Path) -> Result<()> {
    grindvakt_core::fs::ensure_private_dir_with(path, PRIVACY)?;
    Ok(())
}

/// Atomically replaces `dest` with `data`, private under [`PRIVACY`].
pub fn atomic_write_0600(dest: &Path, data: &[u8]) -> Result<()> {
    grindvakt_core::fs::atomic_write_private_with(dest, data, PRIVACY)?;
    Ok(())
}

/// Re-applies [`PRIVACY`] to an existing data directory and everything in it (Windows upgrades
/// from 1.1.x, whose files allowed LocalService). Returns the number of entries; a missing
/// directory is 0.
pub fn repair_tree(path: &Path) -> Result<usize> {
    Ok(grindvakt_core::fs::repair_private_tree(path, PRIVACY)?)
}
