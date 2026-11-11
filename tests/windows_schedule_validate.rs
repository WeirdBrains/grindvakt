#![cfg(target_os = "windows")]
use grindvakt::config::{AppConfig, Schedule};
use grindvakt::winservice::validate_schedule_windows;

fn cfg_with(calendar: Option<&str>, interval: Option<u64>) -> AppConfig {
    AppConfig {
        schedule: Schedule {
            calendar: calendar.map(|s| s.to_string()),
            interval_secs: interval,
        },
        ..AppConfig::default()
    }
}

#[test]
fn valid_calendars() {
    for cal in ["hourly", "daily", "*:0/10"] {
        let cfg = cfg_with(Some(cal), None);
        assert!(validate_schedule_windows(&cfg).is_ok(), "{}", cal);
    }
}

#[test]
fn valid_interval_min() {
    let cfg = cfg_with(None, Some(60));
    assert!(validate_schedule_windows(&cfg).is_ok());
}

#[test]
fn invalid_interval_too_small() {
    let cfg = cfg_with(None, Some(59));
    assert!(validate_schedule_windows(&cfg).is_err());
}

#[test]
fn invalid_calendar_unsupported() {
    let cfg = cfg_with(Some("weekly"), None);
    assert!(validate_schedule_windows(&cfg).is_err());
}

#[test]
fn invalid_calendar_newline_percent_too_long() {
    assert!(validate_schedule_windows(&cfg_with(Some("hourly\n"), None)).is_err());
    assert!(validate_schedule_windows(&cfg_with(Some("%hourly"), None)).is_err());
    let long = "a".repeat(257);
    assert!(validate_schedule_windows(&cfg_with(Some(&long), None)).is_err());
}

#[test]
fn invalid_neither_set() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = None;
    cfg.schedule.interval_secs = None;
    assert!(validate_schedule_windows(&cfg).is_err());
}

#[test]
fn invalid_both_set() {
    let cfg = cfg_with(Some("hourly"), Some(300));
    assert!(validate_schedule_windows(&cfg).is_err());
}
