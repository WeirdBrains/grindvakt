use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use tempfile::NamedTempFile;

pub fn ensure_dir_0700(path: &Path) -> Result<()> {
    // Refuse to follow symlinks
    if path.exists() {
        let lmd =
            fs::symlink_metadata(path).with_context(|| format!("lstat {}", path.display()))?;
        if lmd.file_type().is_symlink() {
            anyhow::bail!("refusing to operate on symlinked path {}", path.display());
        }
        let md = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        if !md.is_dir() {
            anyhow::bail!("{} exists and is not a directory", path.display());
        }
        // ownership check
        let uid = md.uid();
        if uid != unsafe { libc::geteuid() } as u32 {
            anyhow::bail!("{} not owned by current user", path.display());
        }
        // tighten perms if needed
        let mut perms = md.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    } else {
        fs::create_dir_all(path).with_context(|| format!("mkdir -p {}", path.display()))?;
        let md = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let mut perms = md.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }
    Ok(())
}

pub fn atomic_write_0600(dest: &Path, data: &[u8]) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent for {}", dest.display()))?;
    ensure_dir_0700(parent)?;
    // Refuse to write through symlink destination
    if dest.exists() {
        let lmd =
            fs::symlink_metadata(dest).with_context(|| format!("lstat {}", dest.display()))?;
        if lmd.file_type().is_symlink() {
            anyhow::bail!("refusing to overwrite symlink {}", dest.display());
        }
    }
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("tempfile in {}", parent.display()))?;

    // ensure 0600 perms
    let md = tmp.as_file().metadata()?;
    let mut perms = md.permissions();
    perms.set_mode(0o600);
    tmp.as_file().set_permissions(perms)?;

    tmp.as_file_mut().write_all(data)?;
    tmp.as_file_mut().flush()?;
    tmp.as_file_mut().sync_all()?;

    // Persist atomically
    let tmp_path = tmp.path().to_path_buf();
    tmp.persist(dest)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), dest.display()))?;

    // fsync parent directory to ensure rename durability
    let dir_file = OpenOptions::new()
        .read(true)
        .open(parent)
        .with_context(|| format!("open dir {}", parent.display()))?;
    dir_file.sync_all().ok();
    Ok(())
}
