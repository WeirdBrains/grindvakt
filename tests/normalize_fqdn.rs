use grindvakt::config::normalize_fqdn;

#[test]
fn normalize_cases() {
    assert_eq!(normalize_fqdn("example.com", "@"), "example.com");
    assert_eq!(normalize_fqdn("example.com.", "www"), "www.example.com");
    assert_eq!(
        normalize_fqdn("example.com", "api.example.com."),
        "api.example.com"
    );
    assert_eq!(normalize_fqdn("example.com", ""), "example.com");
}
