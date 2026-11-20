//! The app's sync pass (config -> grindvakt-core provider -> plan/apply) against mock
//! DigitalOcean and DNSimple APIs. Provider details are tested in grindvakt-core; this checks
//! the app wiring and its log lines.
use grindvakt::config::{AppConfig, CnameConfig, Provider, RecordType, Schedule};
use grindvakt::sync::{SyncMode, sync_cname, sync_record};
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{body_json, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg(provider: Provider, name: &str, server: &MockServer) -> AppConfig {
    AppConfig {
        provider,
        account_id: Some(1234),
        zone: "example.com".into(),
        name: name.into(),
        record: RecordType::A,
        cname: None,
        ttl: 300,
        sandbox: false,
        api_base_url: Some(server.uri()),
        schedule: Schedule {
            calendar: None,
            interval_secs: Some(300),
        },
    }
}

fn token() -> SecretString {
    SecretString::from("test_token".to_string())
}

async fn do_list(server: &MockServer, records: serde_json::Value) {
    do_list_kind(server, "CNAME", json!([])).await;
    do_list_kind(server, "A", records).await;
}

async fn do_list_kind(server: &MockServer, kind: &str, records: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/v2/domains/example.com/records"))
        .and(query_param("type", kind))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"domain_records": records, "links": {}})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn digitalocean_dry_run_then_create() {
    let server = MockServer::start().await;
    do_list(&server, json!([])).await;
    Mock::given(method("POST"))
        .and(path("/v2/domains/example.com/records"))
        .and(body_json(
            json!({"type": "A", "name": "www", "data": "93.184.216.34", "ttl": 300}),
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"domain_record": {
            "id": 1, "type": "A", "name": "www", "data": "93.184.216.34", "ttl": 300}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = cfg(Provider::Digitalocean, "www", &server);
    let ip = "93.184.216.34".parse().unwrap();
    let dry = sync_record(&c, &token(), ip, SyncMode::DryRun)
        .await
        .unwrap();
    assert_eq!(
        dry.to_string(),
        "DRY RUN: would create A www.example.com = 93.184.216.34 (ttl=300)"
    );
    let done = sync_record(&c, &token(), ip, SyncMode::Apply)
        .await
        .unwrap();
    assert_eq!(
        done.to_string(),
        "Created A www.example.com = 93.184.216.34 (ttl=300)"
    );
}

#[tokio::test]
async fn digitalocean_refuses_a_record_where_a_cname_exists() {
    let server = MockServer::start().await;
    do_list_kind(
        &server,
        "CNAME",
        json!([{"id": 3, "type": "CNAME", "name": "www", "data": "home.example.net.", "ttl": 300}]),
    )
    .await;
    do_list_kind(&server, "A", json!([])).await;
    let c = cfg(Provider::Digitalocean, "www", &server);
    let e = sync_record(
        &c,
        &token(),
        "93.184.216.34".parse().unwrap(),
        SyncMode::Apply,
    )
    .await
    .unwrap_err();
    assert!(
        e.to_string().contains("is a CNAME to home.example.net"),
        "{e:#}"
    );
}

#[tokio::test]
async fn digitalocean_update_and_noop() {
    let server = MockServer::start().await;
    do_list(
        &server,
        json!([{"id": 9, "type": "A", "name": "@", "data": "1.1.1.1", "ttl": 300}]),
    )
    .await;
    Mock::given(method("PATCH"))
        .and(path("/v2/domains/example.com/records/9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"domain_record": {
            "id": 9, "type": "A", "name": "@", "data": "93.184.216.34", "ttl": 300}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = cfg(Provider::Digitalocean, "@", &server);
    let out = sync_record(
        &c,
        &token(),
        "93.184.216.34".parse().unwrap(),
        SyncMode::Apply,
    )
    .await
    .unwrap();
    assert_eq!(
        out.to_string(),
        "Updated A example.com: 1.1.1.1 -> 93.184.216.34 (ttl=300)"
    );
    let same = sync_record(&c, &token(), "1.1.1.1".parse().unwrap(), SyncMode::Apply)
        .await
        .unwrap();
    assert_eq!(same.to_string(), "No change needed: example.com -> 1.1.1.1");
}

fn dnsimple_rec(id: u64, name: &str, content: &str) -> serde_json::Value {
    json!({
        "id": id, "zone_id": "example.com", "parent_id": null, "name": name,
        "content": content, "ttl": 300, "priority": null, "type": "A",
        "regions": null, "system_record": false,
        "created_at": "2020-01-01T00:00:00Z", "updated_at": "2020-01-01T00:00:00Z"
    })
}

#[tokio::test]
async fn dnsimple_refuses_ambiguous_records() {
    let server = MockServer::start().await;
    let data = json!([
        dnsimple_rec(1, "home", "1.1.1.1"),
        dnsimple_rec(2, "home", "1.1.1.2")
    ]);
    Mock::given(method("GET"))
        .and(path_regex(".*/1234/zones/example.com/records$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-RateLimit-Limit", "4000")
                .insert_header("X-RateLimit-Remaining", "3999")
                .insert_header("X-RateLimit-Reset", "1700000000")
                .set_body_json(json!({"data": data, "pagination": {"current_page": 1,
                    "per_page": 100, "total_entries": 2, "total_pages": 1}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let c = cfg(Provider::Dnsimple, "home", &server);
    let err = sync_record(
        &c,
        &token(),
        "93.184.216.34".parse().unwrap(),
        SyncMode::Apply,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        err.starts_with("Refusing to modify records due to ambiguity"),
        "{err}"
    );
}

fn cname_cfg(server: &MockServer, content: CnameConfig) -> AppConfig {
    AppConfig {
        record: RecordType::Cname,
        cname: Some(content),
        ..cfg(Provider::Digitalocean, "www", server)
    }
}

async fn do_list_cname_zone(server: &MockServer, cname: serde_json::Value) {
    do_list_kind(server, "CNAME", cname.clone()).await;
    do_list_kind(server, "A", json!([])).await;
    do_list_kind(server, "AAAA", json!([])).await;
    do_list_kind(server, "TXT", json!([])).await;
    // The planner also asks for every type at the name, without a `type` filter.
    Mock::given(method("GET"))
        .and(path("/v2/domains/example.com/records"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"domain_records": cname, "links": {}})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn cname_from_a_weirdnet_link_is_created_with_the_links_name() {
    let server = MockServer::start().await;
    do_list_cname_zone(&server, json!([])).await;
    Mock::given(method("POST"))
        .and(path("/v2/domains/example.com/records"))
        .and(body_json(
            json!({"type": "CNAME", "name": "www", "data": "home.example.net.", "ttl": 300}),
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"domain_record": {
            "id": 1, "type": "CNAME", "name": "www", "data": "home.example.net.", "ttl": 300}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = cname_cfg(
        &server,
        CnameConfig {
            target: None,
            weirdnet_link: Some("weirdnet-morph://Home.Example.net/site".into()),
        },
    );
    let dry = sync_cname(&c, &token(), SyncMode::DryRun).await.unwrap();
    assert_eq!(
        dry.to_string(),
        "DRY RUN: would create CNAME www.example.com -> home.example.net (ttl=300)"
    );
    let done = sync_cname(&c, &token(), SyncMode::Apply).await.unwrap();
    assert_eq!(
        done.to_string(),
        "Created CNAME www.example.com -> home.example.net (ttl=300)"
    );
}

#[tokio::test]
async fn cname_noop_and_retarget_with_a_static_target() {
    let server = MockServer::start().await;
    do_list_cname_zone(
        &server,
        json!([{"id": 4, "type": "CNAME", "name": "www", "data": "old.example.net.", "ttl": 300}]),
    )
    .await;
    Mock::given(method("PATCH"))
        .and(path("/v2/domains/example.com/records/4"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"domain_record": {
            "id": 4, "type": "CNAME", "name": "www", "data": "home.example.net.", "ttl": 300}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = cname_cfg(
        &server,
        CnameConfig {
            target: Some("home.example.net".into()),
            weirdnet_link: None,
        },
    );
    let out = sync_cname(&c, &token(), SyncMode::Apply).await.unwrap();
    assert_eq!(
        out.to_string(),
        "Updated CNAME www.example.com: old.example.net -> home.example.net (ttl=300) [record_id=4]"
    );
    let same = sync_cname(
        &cname_cfg(
            &server,
            CnameConfig {
                target: Some("old.example.net".into()),
                weirdnet_link: None,
            },
        ),
        &token(),
        SyncMode::Apply,
    )
    .await
    .unwrap();
    assert_eq!(
        same.to_string(),
        "No change: CNAME www.example.com -> old.example.net"
    );
}

#[tokio::test]
async fn cname_refuses_an_invitation_link_before_touching_dns() {
    let server = MockServer::start().await;
    let c = cname_cfg(
        &server,
        CnameConfig {
            target: None,
            weirdnet_link: Some("weirdnet-morph://join/SECRETCODE".into()),
        },
    );
    let e = sync_cname(&c, &token(), SyncMode::Apply).await.unwrap_err();
    let text = format!("{e:#}");
    assert!(text.contains("invitation"), "{text}");
    assert!(!text.contains("SECRETCODE"), "{text}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn config_file_with_a_weirdnet_link_loads_and_default_stays_a() {
    let c = AppConfig::from_toml_str(
        "provider = \"digitalocean\"\nzone = \"example.com\"\nname = \"www\"\nrecord = \"cname\"\n\n[cname]\nweirdnet_link = \"weirdnet-morph://home.example.net/site\"\n\n[schedule]\ninterval_secs = 300\n",
    )
    .unwrap();
    assert_eq!(c.record, RecordType::Cname);
    let a = AppConfig::from_toml_str(
        "zone = \"example.com\"\nname = \"www\"\n[schedule]\ninterval_secs = 300\n",
    )
    .unwrap();
    assert_eq!(a.record, RecordType::A);
}
