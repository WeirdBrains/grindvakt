#![cfg(target_os = "windows")]

use crate::config::{self, AppConfig, ConfigPaths};
use crate::ip::PublicIpConsensus;
use crate::provider::build_active_provider;
use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{error, info};
use widestring::U16CString;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::core::PCWSTR;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceErrorControl, ServiceInfo, ServiceStartType,
    ServiceState, ServiceType,
};
use windows_service::service_control_handler::ServiceControlHandlerResult;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_control_handler, service_dispatcher};

const SERVICE_NAME: &str = "Grindvakt";
const SERVICE_DISPLAY_NAME: &str = "Grindvakt DNS Updater";

pub fn install_service(exe_path: &Path, data_dir: &Path) -> Result<()> {
    // Create SCM connection and register a manual-start service running as LocalService.
    let mgr = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CREATE_SERVICE | ServiceManagerAccess::CONNECT,
    )
    .context("connect to Service Control Manager")?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::OnDemand,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe_path.to_path_buf(),
        launch_arguments: vec![
            OsString::from("service"),
            OsString::from("run"),
            OsString::from(format!("--data-dir={}", data_dir.display())),
        ],
        dependencies: vec![],
        account_name: Some(OsString::from(r"NT AUTHORITY\LocalService")),
        account_password: None,
    };
    match mgr.create_service(
        &info,
        ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP,
    ) {
        Ok(_svc) => {
            println!("Installed Windows service '{}'", SERVICE_NAME);
            println!("Start it later via: sc start {}", SERVICE_NAME);
            Ok(())
        }
        Err(e) => {
            bail!("create service failed: {}", e);
        }
    }
}

define_windows_service!(ffi_service_main, service_main);

pub fn run_service() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).context("start service dispatcher")
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = real_service_main() {
        let msg = U16CString::from_str(format!("Grindvakt service error: {e}")).unwrap();
        unsafe {
            OutputDebugStringW(PCWSTR::from_raw(msg.as_ptr()));
        }
    }
}

fn real_service_main() -> Result<()> {
    // Initialize file logging under ProgramData\Grindvakt
    if let Err(e) = init_service_logging() {
        let msg = U16CString::from_str(format!("Grindvakt logging init failed: {e}")).unwrap();
        unsafe {
            OutputDebugStringW(PCWSTR::from_raw(msg.as_ptr()));
        }
    }
    // Shared state for stop signal and current status
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_for_ctrl = Arc::clone(&pair);
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |control_event| match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let (lock, cv) = &*pair_for_ctrl;
                let mut f = lock.lock().unwrap_or_else(|e| e.into_inner());
                {
                    *f = true;
                    cv.notify_all();
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .context("register service ctrl handler")?;

    // Report start pending
    status_handle
        .set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::StartPending,
            controls_accepted: windows_service::service::ServiceControlAccept::STOP
                | windows_service::service::ServiceControlAccept::SHUTDOWN,
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(2),
            process_id: None,
        })
        .ok();

    // Worker thread: schedule loop using current config and a Tokio runtime
    let pair_worker = Arc::clone(&pair);
    let pair_notify = Arc::clone(&pair);
    use std::sync::atomic::{AtomicBool, Ordering};
    let worker_error = Arc::new(AtomicBool::new(false));
    let worker_error_w = Arc::clone(&worker_error);
    let worker = thread::spawn(move || {
        let outcome = std::panic::catch_unwind(|| worker_loop(pair_worker));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!("service worker error: {:#}", e);
                worker_error_w.store(true, Ordering::Relaxed);
                let (lock, cv) = &*pair_notify;
                let mut f = lock.lock().unwrap_or_else(|e| e.into_inner());
                *f = true;
                cv.notify_all();
            }
            Err(_) => {
                error!("service worker panicked");
                worker_error_w.store(true, Ordering::Relaxed);
                let (lock, cv) = &*pair_notify;
                let mut f = lock.lock().unwrap_or_else(|e| e.into_inner());
                *f = true;
                cv.notify_all();
            }
        }
    });

    // Report running
    status_handle
        .set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: windows_service::service::ServiceControlAccept::STOP
                | windows_service::service::ServiceControlAccept::SHUTDOWN,
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(0),
            process_id: None,
        })
        .ok();

    // Wait until stop is requested
    {
        let (lock, cv) = &*pair;
        let mut g = match lock.lock() {
            Ok(gg) => gg,
            Err(pe) => pe.into_inner(),
        };
        while !*g {
            g = match cv.wait(g) {
                Ok(gg) => gg,
                Err(pe) => pe.into_inner(),
            };
        }
    }
    // Signal received; now wait for worker to finish, reporting StopPending checkpoints
    let mut checkpoint: u32 = 1;
    while !worker.is_finished() {
        status_handle
            .set_service_status(windows_service::service::ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::StopPending,
                controls_accepted: windows_service::service::ServiceControlAccept::empty(),
                exit_code: windows_service::service::ServiceExitCode::Win32(0),
                checkpoint,
                wait_hint: Duration::from_secs(2),
                process_id: None,
            })
            .ok();
        checkpoint = checkpoint.saturating_add(1);
        thread::sleep(Duration::from_millis(200));
    }
    let _ = worker.join();

    // Report stop pending then stopped
    status_handle
        .set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::StopPending,
            controls_accepted: windows_service::service::ServiceControlAccept::empty(),
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })
        .ok();
    status_handle
        .set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: windows_service::service::ServiceControlAccept::empty(),
            exit_code: windows_service::service::ServiceExitCode::Win32(
                if worker_error.load(Ordering::Relaxed) {
                    1
                } else {
                    0
                },
            ),
            checkpoint: 0,
            wait_hint: Duration::from_secs(0),
            process_id: None,
        })
        .ok();
    Ok(())
}

fn worker_loop(pair: Arc<(Mutex<bool>, Condvar)>) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    // Resolve config/data paths; we inherit --data-dir passed by ServiceInstall arguments.
    let paths = ConfigPaths::resolve(
        std::env::args_os()
            .find_map(|a| {
                let s = a.to_string_lossy();
                if s.starts_with("--data-dir=") {
                    Some(PathBuf::from(s.trim_start_matches("--data-dir=")))
                } else {
                    None
                }
            })
            .as_deref(),
    )
    .context("resolve data dir")?;

    loop {
        if match pair.0.lock() {
            Ok(g) => *g,
            Err(pe) => *pe.into_inner(),
        } {
            break;
        }
        // Load config fresh each iteration to pick up changes
        let cfg = match config::load_or_init_config(&paths) {
            Ok(c) => c,
            Err(e) => {
                error!("load config failed: {}", e);
                sleep_minimum(&pair, Duration::from_secs(60))?;
                continue;
            }
        };
        // Validate schedule on Windows (fail closed)
        validate_schedule_windows(&cfg)?;
        match compute_wait(std::time::SystemTime::now(), &cfg) {
            Ok(wait_dur) => {
                if sleep_minimum(&pair, wait_dur)? {
                    break;
                }
            }
            Err(e) => {
                error!("{}", e);
                // Do not run; sleep a bounded backoff
                if sleep_minimum(&pair, Duration::from_secs(300))? {
                    break;
                }
                continue;
            }
        }
        // Perform one sync pass with unattended-only unlock
        if let Err(e) = do_sync_once_unattended(&rt, &paths, &cfg) {
            error!("sync failed: {}", e);
        }
        // Guard against rapid loop; enforce at least 60s between runs
        if sleep_minimum(&pair, Duration::from_secs(60))? {
            break;
        }
    }
    Ok(())
}

pub fn validate_schedule_windows(cfg: &AppConfig) -> Result<()> {
    // Enforce mutual exclusion and raw calendar validation first.
    let cal_opt = cfg.schedule.calendar.as_ref();
    let int_opt = cfg.schedule.interval_secs;
    if cal_opt.is_some() && int_opt.is_some() {
        bail!("schedule must not set both calendar and interval_secs");
    }
    // Reject raw calendar containing newline/'%' or overly long
    if let Some(raw) = cal_opt {
        if raw.contains('\n') || raw.contains('%') || raw.len() > 256 {
            bail!("invalid calendar expression");
        }
        let t = raw.trim().to_lowercase();
        match t.as_str() {
            "daily" | "hourly" | "*:0/10" => Ok(()),
            other => bail!("unsupported calendar expression: {}", other),
        }
    } else if let Some(secs) = int_opt {
        if secs < 60 {
            bail!("interval_secs must be >= 60");
        }
        Ok(())
    } else {
        bail!("schedule must set either calendar or interval_secs");
    }
}
pub fn compute_wait(now: std::time::SystemTime, cfg: &AppConfig) -> Result<Duration> {
    if let Some(secs) = cfg.schedule.interval_secs {
        return Ok(Duration::from_secs(secs.max(60)));
    }
    if let Some(cal) = &cfg.schedule.calendar {
        let t = cal.trim().to_lowercase();
        use chrono::{Local, LocalResult, TimeZone, Timelike};
        let now_dt: chrono::DateTime<Local> = now.into();
        return Ok(match t.as_str() {
            "hourly" => {
                let next = now_dt
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap()
                    .with_nanosecond(0)
                    .unwrap()
                    + chrono::Duration::hours(1);
                (next - now_dt)
                    .to_std()
                    .unwrap_or(Duration::from_secs(60 * 60))
            }
            "daily" => {
                let next_date = now_dt.date_naive().succ_opt().unwrap();
                let naive_midnight = next_date.and_hms_opt(0, 0, 0).unwrap();
                let next_local = match Local.from_local_datetime(&naive_midnight) {
                    LocalResult::Single(dt) => dt,
                    LocalResult::Ambiguous(a, b) => a.min(b),
                    LocalResult::None => now_dt + chrono::Duration::hours(24),
                };
                (next_local - now_dt)
                    .to_std()
                    .unwrap_or(Duration::from_secs(24 * 60 * 60))
            }
            "*:0/10" => {
                let m = now_dt.minute();
                let add = 10 - (m % 10);
                let next = now_dt.with_second(0).unwrap().with_nanosecond(0).unwrap()
                    + chrono::Duration::minutes(add as i64);
                (next - now_dt).to_std().unwrap_or(Duration::from_secs(600))
            }
            other => {
                // Reject unsupported calendars explicitly
                bail!("unsupported calendar expression: {}", other);
            }
        });
    }
    Ok(Duration::from_secs(0))
}

fn sleep_minimum(pair: &Arc<(Mutex<bool>, Condvar)>, dur: Duration) -> Result<bool> {
    let step = Duration::from_millis(200);
    let mut waited = Duration::from_millis(0);
    while waited < dur {
        if match pair.0.lock() {
            Ok(g) => *g,
            Err(pe) => *pe.into_inner(),
        } {
            return Ok(true);
        }
        thread::sleep(step);
        waited += step;
    }
    Ok(false)
}

fn do_sync_once_unattended(
    rt: &tokio::runtime::Runtime,
    paths: &ConfigPaths,
    cfg: &AppConfig,
) -> Result<()> {
    // Only attempt unattended DPAPI path; no interactive fallback.
    let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    let p = Path::new(&pd)
        .join("Grindvakt")
        .join("credentials")
        .join("grindvakt.dek.dpapi");
    if !p.exists() {
        bail!(
            "unattended credential not enrolled ({} missing)",
            p.display()
        );
    }
    // Bound each step to avoid indefinite blocking
    let read_res = rt
        .block_on(async move {
            tokio::time::timeout(Duration::from_secs(10), async move {
                tokio::task::spawn_blocking({
                    let p2 = p.clone();
                    move || std::fs::read(&p2)
                })
                .await
            })
            .await
        })
        .context("read dpapi blob timeout")?;
    let blob = read_res.context("join read dpapi blob")??;
    let unprotect_res = rt
        .block_on(async move {
            tokio::time::timeout(Duration::from_secs(5), async move {
                tokio::task::spawn_blocking(move || crate::winutil::dpapi_unprotect(&blob)).await
            })
            .await
        })
        .context("dpapi unprotect timeout")?;
    let dek = unprotect_res.context("join dpapi unprotect")??;
    if dek.len() != 32 {
        bail!("unexpected DEK length");
    }
    let token = crate::vault::unlock_provider_with_dek(paths, &dek, cfg.provider.clone())
        .context("unlock token")?;
    let provider = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), build_active_provider(cfg, &token)).await
        })
        .context("provider build timeout")??;
    // Resolve public IP and perform sync
    let consensus = PublicIpConsensus::default();
    let ip = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), consensus.resolve()).await
        })
        .context("public ip resolve timeout")??;
    let action = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), provider.plan_sync_action(cfg, ip)).await
        })
        .context("plan action timeout")??;
    match action {
        crate::dnsclient::DnsAction::Noop(cur) => {
            info!("No change needed: {} -> {}", cfg.record_fqdn(), cur);
        }
        crate::dnsclient::DnsAction::Create { name, ttl } => {
            rt.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(15),
                    provider.create_a_record(cfg, name, ip, ttl),
                )
                .await
            })
            .context("create record timeout")??;
            info!("Created A {} = {} (ttl={})", cfg.record_fqdn(), ip, ttl);
        }
        crate::dnsclient::DnsAction::Update {
            record_id,
            ttl,
            prev,
        } => {
            rt.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(15),
                    provider.update_a_record(cfg, record_id, ip, ttl),
                )
                .await
            })
            .context("update record timeout")??;
            info!(
                "Updated A {}: {} -> {} (ttl={})",
                cfg.record_fqdn(),
                prev,
                ip,
                ttl
            );
        }
        crate::dnsclient::DnsAction::Ambiguous { reason } => {
            bail!("Refusing to modify records due to ambiguity: {reason}");
        }
    }
    Ok(())
}

fn init_service_logging() -> Result<()> {
    let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    let log_dir = Path::new(&pd).join("Grindvakt");
    let log_path = log_dir.join("service.log");
    crate::fsutil::ensure_dir_0700(&log_dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(file)
        .try_init();
    Ok(())
}
