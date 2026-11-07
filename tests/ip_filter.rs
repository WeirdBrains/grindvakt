use std::net::Ipv4Addr;

#[test]
fn reserved_ranges_blocked() {
    fn is_global(ip: Ipv4Addr) -> bool {
        grindvakt::ip::is_global_ipv4(ip)
    }
    assert!(!is_global("10.0.0.1".parse().unwrap()));
    assert!(!is_global("192.168.0.1".parse().unwrap()));
    assert!(!is_global("127.0.0.1".parse().unwrap()));
    assert!(!is_global("169.254.1.10".parse().unwrap()));
    assert!(is_global("93.184.216.34".parse().unwrap())); // example.org
}
