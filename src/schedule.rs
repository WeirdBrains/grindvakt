//! When Grindvakt runs. grindvakt-core only stores the schedule; the app interprets it:
//! systemd and launchd timers on Linux and macOS, and this module's wait logic in the
//! Windows service.

use crate::config::AppConfig;
use anyhow::{Result, bail};
use std::time::Duration;

/// The Windows service supports intervals of at least 60 s and the calendars `hourly`,
/// `daily` and `*:0/10`; anything else is refused (fail closed).
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
/// How long to wait before the next run: the interval (at least 60 s), or until the next
/// `hourly`, `daily` or `*:0/10` boundary in local time.
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

/// How long the Windows service pauses after a run before it computes the next wait: 60 s for
/// calendar schedules (so a run that ends inside the boundary minute cannot repeat it), none for
/// intervals, whose wait already starts after the run. Runs are `interval_secs` apart, not
/// `interval_secs + 60`.
pub fn post_run_guard(cfg: &AppConfig) -> Duration {
    if cfg.schedule.calendar.is_some() {
        Duration::from_secs(60)
    } else {
        Duration::ZERO
    }
}

/// Whether a systemd `OnCalendar` expression is acceptable: checked by `systemd-analyze` when it
/// is installed, otherwise by a conservative heuristic (no newlines, no `%`, at most 256 bytes).
pub fn validate_calendar(expr: &str) -> bool {
    if std::process::Command::new("systemd-analyze")
        .arg("--version")
        .output()
        .is_ok()
    {
        let out = std::process::Command::new("systemd-analyze")
            .arg("calendar")
            .arg(expr)
            .arg("--iterations=1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Ok(st) = out {
            return st.success();
        }
    }
    let t = expr.trim();
    !t.is_empty() && !t.contains('\n') && !t.contains('%') && t.len() <= 256
}
