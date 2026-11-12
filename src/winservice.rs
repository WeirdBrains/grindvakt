#![cfg(target_os = "windows")]

use crate::config::{self, AppConfig, ConfigPaths};
use crate::ip::PublicIpConsensus;
use crate::sync::{SyncMode, sync_record};
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
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceErrorControl,
    ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceType,
};
use windows_service::service_control_handler::ServiceControlHandlerResult;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_control_handler, service_dispatcher};

const SERVICE_NAME: &str = "Grindvakt";
const SERVICE_DISPLAY_NAME: &str = "Grindvakt DNS Updater";
const SERVICE_DESCRIPTION: &str = "Updates DNS A record on a schedule";

/// Recovery after a crash, the same as the MSI's `ServiceConfigFailureActions`: restart after
/// 60 s every time, and forget earlier failures after a day.
fn recovery_actions() -> ServiceFailureActions {
    let restart = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(60),
    };
    ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    }
}

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
        ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::CHANGE_CONFIG,
    ) {
        Ok(svc) => {
            svc.set_description(SERVICE_DESCRIPTION)
                .context("set service description")?;
            svc.update_failure_actions(recovery_actions())
                .context("set service recovery actions")?;
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
        report_outside_log(&format!("Grindvakt service error: {e:#}"));
    }
}

/// Reports a problem that cannot go to `service.log`: to the debugger and as an error in the
/// Windows Application event log (source "Grindvakt"), so a service without a log is visible.
fn report_outside_log(msg: &str) {
    use windows::Win32::System::EventLog::{
        DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
    };
    let Ok(text) = U16CString::from_str(msg.replace('\0', " ")) else {
        return;
    };
    unsafe {
        OutputDebugStringW(PCWSTR::from_raw(text.as_ptr()));
        if let Ok(source) = RegisterEventSourceW(PCWSTR::null(), windows::core::w!("Grindvakt")) {
            let strings = [PCWSTR::from_raw(text.as_ptr())];
            let _ = ReportEventW(
                source,
                EVENTLOG_ERROR_TYPE,
                0,
                1,
                None,
                0,
                Some(&strings),
                None,
            );
            let _ = DeregisterEventSource(source);
        }
    }
}

fn real_service_main() -> Result<()> {
    // Initialize file logging under ProgramData\Grindvakt
    if let Err(e) = init_service_logging() {
        report_outside_log(&format!(
            "Grindvakt cannot write service.log, so it logs nothing else: {e:#}"
        ));
    }
    info!("service starting (grindvakt {})", env!("CARGO_PKG_VERSION"));
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
    info!("service stopping");
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
    info!("service stopped");

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
    info!("data dir {}", paths.data_dir.display());

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
            error!("sync failed: {:#}", e);
        }
        // Calendar schedules wait out the rest of the boundary minute so one boundary never
        // runs twice; interval schedules already wait at least 60 s.
        let guard = crate::schedule::post_run_guard(&cfg);
        if !guard.is_zero() && sleep_minimum(&pair, guard)? {
            break;
        }
    }
    Ok(())
}

pub use crate::schedule::{compute_wait, validate_schedule_windows};

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
    let token = crate::vault::unlock_provider_with_dek(paths, &dek, cfg.provider)
        .context("unlock token")?;
    // Resolve public IP and perform sync
    let consensus = PublicIpConsensus::default();
    let ip = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), consensus.resolve()).await
        })
        .context("public ip resolve timeout")??;
    let outcome = rt
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(25),
                sync_record(cfg, &token, ip, SyncMode::Apply),
            )
            .await
        })
        .context("sync timeout")??;
    info!("{outcome}");
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
        .with_ansi(false)
        .with_writer(file)
        .try_init();
    Ok(())
}
