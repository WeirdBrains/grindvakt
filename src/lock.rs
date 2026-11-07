use anyhow::{Context, Result};
use directories::BaseDirs;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct SingleInstanceLock {
    file: File,
}

impl SingleInstanceLock {
    pub fn acquire(name: &str) -> Result<Self> {
        let lock_path = lock_file_path(name)?;
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
            let md = fs::metadata(parent)?;
            let mut perms = md.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(parent, perms).ok();
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        // tighten perms
        let md = file.metadata()?;
        let mut perms = md.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms).ok();

        file.try_lock_exclusive()
            .with_context(|| format!("lock {}", lock_path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for SingleInstanceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_file_path(name: &str) -> Result<PathBuf> {
    // Use XDG_RUNTIME_DIR if available; fall back to ~/.cache/grindvakt/run
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(Path::new(&dir)
            .join("grindvakt")
            .join(format!("{}.lock", sanitize_name(name))))
    } else {
        let base = BaseDirs::new().ok_or_else(|| anyhow::anyhow!("cannot resolve base dirs"))?;
        Ok(base
            .cache_dir()
            .join("grindvakt")
            .join("run")
            .join(format!("{}.lock", sanitize_name(name))))
    }
}

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
