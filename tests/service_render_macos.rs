#[cfg(target_os = "macos")]
mod macos_render_tests {
    use grindvakt::config::{AppConfig, Provider, Schedule};
    use std::path::PathBuf;

    #[test]
    fn render_launchd_interval() {
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
                interval_secs: Some(600),
            },
        };
        let exe = PathBuf::from("/usr/local/bin/grindvakt");
        let data_dir = PathBuf::from("/Users/test/.grindvakt");
        let rendered = grindvakt::service::render_launchd_with(&cfg, &exe, &data_dir).unwrap();
        assert!(
            rendered
                .plist
                .contains("<key>StartInterval</key><integer>600</integer>")
        );
        assert!(
            rendered
                .plist
                .contains("<string>/usr/local/bin/grindvakt</string>")
        );
        assert!(rendered.plist.contains("<string>sync</string>"));
        assert!(
            rendered
                .plist
                .contains("<string>--data-dir=/Users/test/.grindvakt</string>")
        );
        assert!(rendered.plist.contains("<key>StandardOutPath</key>"));
        assert!(rendered.plist.contains("<key>StandardErrorPath</key>"));
    }

    #[test]
    fn render_launchd_calendar_hourly_calendar_interval() {
        let cfg = AppConfig {
            provider: Provider::Digitalocean,
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
        let data_dir = PathBuf::from("/Users/test/.grindvakt");
        let rendered = grindvakt::service::render_launchd_with(&cfg, &exe, &data_dir).unwrap();
        assert!(rendered.plist.contains(
            "<key>StartCalendarInterval</key><dict><key>Minute</key><integer>0</integer></dict>"
        ));
    }
}

#[cfg(target_os = "macos")]
mod macos_calendar_array_tests {
    use grindvakt::config::{AppConfig, Provider, Schedule};
    use std::path::PathBuf;

    #[test]
    fn render_launchd_calendar_every_ten_minutes_array() {
        let cfg = AppConfig {
            provider: Provider::Dnsimple,
            account_id: None,
            zone: "example.com".into(),
            name: "@".into(),
            ttl: 300,
            sandbox: false,
            api_base_url: None,
            schedule: Schedule {
                calendar: Some("*:0/10".into()),
                interval_secs: None,
            },
        };
        let exe = PathBuf::from("/usr/local/bin/grindvakt");
        let data_dir = PathBuf::from("/Users/test/.grindvakt");
        let rendered = grindvakt::service::render_launchd_with(&cfg, &exe, &data_dir).unwrap();
        // Expect 6 dict entries with Minute 0,10,20,30,40,50
        for m in [0, 10, 20, 30, 40, 50] {
            let needle = format!("<dict><key>Minute</key><integer>{}</integer></dict>", m);
            assert!(rendered.plist.contains(&needle), "missing minute {}", m);
        }
        assert!(rendered.plist.contains("<array>") && rendered.plist.contains("</array>"));
    }
}
