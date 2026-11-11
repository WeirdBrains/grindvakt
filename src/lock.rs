use anyhow::{Context, Result};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use directories::BaseDirs;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut p = md.permissions();
                p.set_mode(0o700);
                fs::set_permissions(parent, p).ok();
            }
            #[cfg(not(unix))]
            {
                let p = md.permissions();
                fs::set_permissions(parent, p).ok();
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // tighten perms
            let md = file.metadata()?;
            let mut perms = md.permissions();
            perms.set_mode(0o600);
            file.set_permissions(perms).ok();
        }

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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // Use XDG_RUNTIME_DIR if available; fall back to ~/.cache/grindvakt/run
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            return Ok(Path::new(&dir)
                .join("grindvakt")
                .join(format!("{}.lock", sanitize_name(name))));
        }
        let base = BaseDirs::new().ok_or_else(|| anyhow::anyhow!("cannot resolve base dirs"))?;
        Ok(base
            .cache_dir()
            .join("grindvakt")
            .join("run")
            .join(format!("{}.lock", sanitize_name(name))))
    }
    #[cfg(target_os = "windows")]
    {
        // Shared lock under ProgramData\Grindvakt\run to ensure cross-identity single instance
        let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        Ok(Path::new(&pd)
            .join("Grindvakt")
            .join("run")
            .join(format!("{}.lock", sanitize_name(name))))
    }
}

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
