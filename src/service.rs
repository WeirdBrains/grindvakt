use crate::config::AppConfig;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::fsutil::ensure_dir_0700;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::bail;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "grindvakt.service";
#[cfg(target_os = "linux")]
const TIMER_NAME: &str = "grindvakt.timer";
#[cfg(target_os = "linux")]
const CREDENTIAL_NAME: &str = "grindvakt.dek";

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "com.weirdbrains.grindvakt";

#[cfg(target_os = "linux")]
pub struct ServiceRender {
    pub service_unit: String,
    pub timer_unit: String,
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
pub struct LaunchdRender {
    pub plist: String,
}

#[cfg(target_os = "macos")]
pub fn render_launchd_with(
    cfg: &AppConfig,
    exe_path: &Path,
    data_dir: &Path,
) -> Result<LaunchdRender> {
    validate_schedule(cfg)?;
    let mut plist = String::new();
    let logs_dir = default_logs_dir()?;
    let stdout_path = logs_dir.join("grindvakt.out.log");
    let stderr_path = logs_dir.join("grindvakt.err.log");
    let mut schedule = String::new();
    // Precedence: calendar if set; else interval
    if let Some(cal) = &cfg.schedule.calendar {
        let t = cal.trim().to_lowercase();
        match t.as_str() {
            "daily" => {
                schedule.push_str("<key>StartCalendarInterval</key><dict><key>Hour</key><integer>0</integer><key>Minute</key><integer>0</integer></dict>\n");
            }
            "hourly" => {
                schedule.push_str("<key>StartCalendarInterval</key><dict><key>Minute</key><integer>0</integer></dict>\n");
            }
            "*:0/10" => {
                // Render as StartCalendarInterval array of minute marks 0,10,20,30,40,50
                schedule.push_str("<key>StartCalendarInterval</key><array>\n");
                for m in [0, 10, 20, 30, 40, 50] {
                    schedule.push_str(&format!(
                        "<dict><key>Minute</key><integer>{}</integer></dict>\n",
                        m
                    ));
                }
                schedule.push_str("</array>\n");
            }
            other => {
                bail!("unsupported macOS calendar expression: {}", other);
            }
        }
    } else if let Some(secs) = cfg.schedule.interval_secs {
        schedule.push_str(&format!(
            "<key>StartInterval</key><integer>{}</integer>\n",
            secs.max(60)
        ));
    }
    plist.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    plist.push('\n');
    plist.push_str(r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#);
    plist.push('\n');
    plist.push_str(r#"<plist version="1.0">"#);
    plist.push('\n');
    plist.push_str("<dict>\n");
    plist.push_str(&format!(
        "<key>Label</key><string>{}</string>\n",
        LAUNCHD_LABEL
    ));
    plist.push_str("<key>ProgramArguments</key>\n<array>\n");
    plist.push_str(&format!(
        "<string>{}</string>\n",
        xml_escape(&exe_path.display().to_string())
    ));
    plist.push_str("<string>sync</string>\n");
    plist.push_str(&format!(
        "<string>--data-dir={}</string>\n",
        xml_escape(&data_dir.display().to_string())
    ));
    plist.push_str("</array>\n");
    plist.push_str(&schedule);
    plist.push_str("<key>RunAtLoad</key><false/>\n");
    plist.push_str("<key>StandardOutPath</key>\n");
    plist.push_str(&format!(
        "<string>{}</string>\n",
        xml_escape(&stdout_path.display().to_string())
    ));
    plist.push_str("<key>StandardErrorPath</key>\n");
    plist.push_str(&format!(
        "<string>{}</string>\n",
        xml_escape(&stderr_path.display().to_string())
    ));
    plist.push_str("</dict>\n</plist>\n");
    Ok(LaunchdRender { plist })
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
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
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "macos")]
    pub async fn install(&self, cfg: &AppConfig, exe_path: &Path, data_dir: &Path) -> Result<()> {
        let dir = default_launch_agents_dir()?;
        ensure_dir_0700(&dir)?;
        // Ensure logs dir as well
        let logs_dir = default_logs_dir()?;
        ensure_dir_0700(&logs_dir)?;
        let rendered = render_launchd_with(cfg, exe_path, data_dir)?;
        let path = dir.join(format!("{}.plist", LAUNCHD_LABEL));
        write_plist_file(&path, &rendered.plist)?;
        println!("Installed launch agent to {}", path.display());
        println!("Note: agent is not loaded automatically. Load later with:");
        println!("  launchctl load {}", path.display());
        println!(
            "Unattended credential (Keychain) recommended: run `grindvakt service enroll-credential`."
        );
        Ok(())
    }
    #[cfg(target_os = "windows")]
    pub async fn install(&self, cfg: &AppConfig, exe_path: &Path, data_dir: &Path) -> Result<()> {
        crate::winservice::validate_schedule_windows(cfg)?;
        crate::winservice::install_service(exe_path, data_dir)?;
        println!("Note: service is manual-start. Use the MSI installer for production deployment.");
        println!("Enroll unattended credential with: grindvakt service enroll-credential");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
fn write_plist_file(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow::anyhow!("no parent"))?;
    ensure_dir_0700(parent)?;
    // Reject symlink targets (including dangling)
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                bail!("refusing to overwrite symlink {}", path.display());
            }
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e).with_context(|| format!("stat {}", path.display()));
            }
        }
    }
    // Atomic 0600 write
    crate::fsutil::atomic_write_0600(path, content.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn default_user_unit_dir() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config/systemd/user"))
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
fn default_launch_agents_dir() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home.join("Library/LaunchAgents"))
}

#[cfg(target_os = "macos")]
fn default_logs_dir() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home.join("Library/Logs/Grindvakt"))
}
