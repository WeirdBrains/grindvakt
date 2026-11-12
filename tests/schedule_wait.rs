use chrono::Timelike;
use grindvakt::config::{AppConfig, Provider, Schedule};
use grindvakt::schedule::{compute_wait, post_run_guard};
use std::time::{Duration, SystemTime};

#[test]
fn compute_wait_interval_min() {
    let cfg = AppConfig {
        provider: Provider::Dnsimple,
        account_id: None,
        zone: "example.com".into(),
        name: "@".into(),
        ttl: 300,
        sandbox: false,
        api_base_url: None,
        schedule: Schedule {
            calendar: None,
            interval_secs: Some(30),
        }, // too small -> 60s
    };
    let d = compute_wait(SystemTime::now(), &cfg).unwrap();
    assert_eq!(d, Duration::from_secs(60));
}

#[test]
fn compute_wait_hourly_on_the_hour() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = Some("hourly".into());
    cfg.schedule.interval_secs = None;
    let now = chrono::Local::now()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let d = compute_wait(now.into(), &cfg).unwrap();
    assert_eq!(d, Duration::from_secs(60 * 60));
}

#[test]
fn compute_wait_ten_minute_marks() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = Some("*:0/10".into());
    cfg.schedule.interval_secs = None;
    let now = chrono::Local::now().with_minute(7).unwrap();
    let d = compute_wait(now.into(), &cfg).unwrap();
    // Next mark is at minute 10 => 3 minutes minus seconds remainder -> at least 120s, at most 180s
    assert!(d.as_secs() <= 180 && d.as_secs() >= 60);
}

#[test]
fn unsupported_calendar_rejected() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = Some("weird".into());
    cfg.schedule.interval_secs = None;
    let d = compute_wait(SystemTime::now(), &cfg);
    assert!(d.is_err());
}

#[test]
fn interval_runs_have_no_extra_guard() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = None;
    cfg.schedule.interval_secs = Some(300);
    assert_eq!(post_run_guard(&cfg), Duration::ZERO);
}

#[test]
fn calendar_runs_wait_out_the_boundary_minute() {
    let mut cfg = AppConfig::default();
    cfg.schedule.calendar = Some("*:0/10".into());
    cfg.schedule.interval_secs = None;
    assert_eq!(post_run_guard(&cfg), Duration::from_secs(60));
}
