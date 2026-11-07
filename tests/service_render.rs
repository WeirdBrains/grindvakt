use grindvakt::config::{AppConfig, Schedule};
use grindvakt::service::render_units_with;
use std::path::PathBuf;

#[test]
fn render_timer_interval() {
    let cfg = AppConfig {
        account_id: None,
        zone: "example.com".into(),
        name: "@".into(),
        ttl: 300,
        sandbox: true,
        api_base_url: None,
        schedule: Schedule {
            calendar: None,
            interval_secs: Some(300),
        },
    };
    let exe = PathBuf::from("/usr/local/bin/grindvakt");
    let data_dir = PathBuf::from("/home/user/.grindvakt");
    let rendered = render_units_with(&cfg, &exe, &data_dir).unwrap();
    assert!(rendered.timer_unit.contains("OnUnitActiveSec=300s"));
    assert!(
        rendered
            .service_unit
            .contains("ExecStart=/usr/local/bin/grindvakt sync --data-dir=/home/user/.grindvakt")
    );
    assert!(
        rendered
            .service_unit
            .contains("LoadCredentialEncrypted=grindvakt.dek:")
    );
}

#[test]
fn render_timer_calendar() {
    let cfg = AppConfig {
        account_id: None,
        zone: "example.com".into(),
        name: "www".into(),
        ttl: 300,
        sandbox: false,
        api_base_url: None,
        schedule: Schedule {
            calendar: Some("hourly".into()),
            interval_secs: None,
        },
    };
    let exe = PathBuf::from("/opt/grindvakt");
    let data_dir = PathBuf::from("/home/user/.grindvakt");
    let rendered = render_units_with(&cfg, &exe, &data_dir).unwrap();
    assert!(rendered.timer_unit.contains("OnCalendar=hourly"));
    // No accidental double slash after colon in LoadCredentialEncrypted
    assert!(
        !rendered
            .service_unit
            .contains("LoadCredentialEncrypted=grindvakt.dek://")
    );
}
