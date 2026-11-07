use grindvakt::config::Provider;

#[test]
fn provider_display_is_lowercase() {
    assert_eq!(Provider::Dnsimple.to_string(), "dnsimple");
    assert_eq!(Provider::Digitalocean.to_string(), "digitalocean");
}
