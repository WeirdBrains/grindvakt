#![cfg(target_os = "windows")]
use tempfile::TempDir;

#[test]
fn insecure_acl_rejected() {
    let t = TempDir::new().unwrap();
    let p = t.path().join("bad");
    std::fs::create_dir_all(&p).unwrap();
    // Give the directory to the current user explicitly: an elevated run would otherwise create
    // it owned by Administrators, which the service policy accepts (and tightens).
    let user = std::process::Command::new("whoami").output().unwrap();
    let user = String::from_utf8_lossy(&user.stdout).trim().to_string();
    let set = std::process::Command::new("icacls")
        .arg(&p)
        .args(["/setowner", &user])
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    // A directory an ordinary user owns is refused: the service must not trust it.
    let bad = grindvakt::fsutil::ensure_dir_0700(&p);
    assert!(bad.is_err(), "{bad:?}");
}
