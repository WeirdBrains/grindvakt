use crate::config::AppConfig;
use crate::fsutil::ensure_dir_0700;
use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SERVICE_NAME: &str = "grindvakt.service";
const TIMER_NAME: &str = "grindvakt.timer";
const CREDENTIAL_NAME: &str = "grindvakt.dek";

pub struct ServiceRender {
    pub service_unit: String,
    pub timer_unit: String,
}

pub fn render_units_with(
    cfg: &AppConfig,
    exe_path: &Path,
    data_dir: &Path,
) -> Result<ServiceRender> {
    validate_schedule(cfg)?;
    let mut service = String::new();
    let mut timer = String::new();
    let credential_encrypted_path =
        default_credstore_path()?.join(format!("{}.enc", CREDENTIAL_NAME));

    service.push_str("[Unit]\n");
    service.push_str("Description=Grindvakt DNS updater (oneshot)\n");
    service.push_str("After=network-online.target\n");
    service.push_str("\n[Service]\n");
    service.push_str("Type=oneshot\n");
    // Use absolute path and explicit data dir; no secrets in args.
    service.push_str(&format!(
        "ExecStart={} sync --data-dir={}\n",
        exe_path.display(),
        data_dir.display()
    ));
    service.push_str("NoNewPrivileges=yes\n");
    service.push_str("PrivateTmp=yes\n");
    service.push_str("PrivateDevices=yes\n");
    service.push_str("ProtectKernelTunables=yes\n");
    service.push_str("ProtectKernelModules=yes\n");
    service.push_str("ProtectControlGroups=yes\n");
    service.push_str("LockPersonality=yes\n");
    service.push_str("RestrictNamespaces=yes\n");
    service.push_str("SystemCallArchitectures=native\n");
    service.push_str("CapabilityBoundingSet=\n");
    service.push_str("AmbientCapabilities=\n");
    service.push_str("RestrictSUIDSGID=yes\n");
    service.push_str("UMask=0077\n");
    service.push_str(&format!(
        "LoadCredentialEncrypted={}:{}\n",
        CREDENTIAL_NAME,
        credential_encrypted_path.display()
    ));
    service.push_str("Environment=RUST_LOG=info\n");

    timer.push_str("[Unit]\n");
    timer.push_str("Description=Run grindvakt sync on schedule\n");
    timer.push_str("\n[Timer]\n");
    if let Some(cal) = &cfg.schedule.calendar {
        // Do not attempt to validate here; user can run `systemd-analyze calendar` themselves.
        timer.push_str(&format!("OnCalendar={}\n", cal.replace('\n', " ")));
    } else if let Some(secs) = cfg.schedule.interval_secs {
        timer.push_str(&format!("OnUnitActiveSec={}s\n", secs.max(60)));
        timer.push_str("OnBootSec=1min\n");
    } else {
        // Should not happen due to validation
        timer.push_str("OnUnitActiveSec=5min\n");
    }
    timer.push_str("Persistent=true\n");
    timer.push_str("\n[Install]\n");
    timer.push_str("WantedBy=timers.target\n");

    Ok(ServiceRender {
        service_unit: service,
        timer_unit: timer,
    })
}

fn validate_schedule(cfg: &AppConfig) -> Result<()> {
    // Forbid newlines and '%' specifiers; basic length check
    if let Some(cal) = &cfg.schedule.calendar
        && (cal.contains('\n') || cal.contains('%') || cal.len() > 256)
    {
        bail!("invalid calendar expression");
    }
    if let Some(secs) = cfg.schedule.interval_secs
        && secs < 60
    {
        bail!("interval_secs must be >= 60");
    }
    Ok(())
}

#[derive(Default)]
pub struct InstallUnits;
impl InstallUnits {
    pub fn new() -> Self {
        Self
    }
    pub async fn install(&self, cfg: &AppConfig, exe_path: &Path, data_dir: &Path) -> Result<()> {
        let dir = default_user_unit_dir()?;
        ensure_dir_0700(&dir)?;
        let rendered = render_units_with(cfg, exe_path, data_dir)?;
        write_unit_file(&dir.join(SERVICE_NAME), &rendered.service_unit)?;
        write_unit_file(&dir.join(TIMER_NAME), &rendered.timer_unit)?;
        println!("Installed units to {}", dir.display());
        println!("Enable with: systemctl --user enable --now {}", TIMER_NAME);
        println!("Note: user manager must be running; linger remains unchanged.");
        println!(
            "Unattended credential required: run `grindvakt service enroll-credential` before starting the unit."
        );
        Ok(())
    }
}

fn write_unit_file(path: &Path, content: &str) -> Result<()> {
    // Unit files are typically 0644
    let parent = path.parent().ok_or_else(|| anyhow::anyhow!("no parent"))?;
    ensure_dir_0700(parent)?;
    fs::write(path, content.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    let md = fs::metadata(path)?;
    let mut perms = md.permissions();
    perms.set_mode(0o644);
    fs::set_permissions(path, perms).ok();
    Ok(())
}

fn default_user_unit_dir() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config/systemd/user"))
}

fn default_credstore_path() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("systemd/user/credstore.encrypted"));
    }
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config/systemd/user/credstore.encrypted"))
}
