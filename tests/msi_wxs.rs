//! The MSI's service data dir must be Windows Installer's `CommonAppDataFolder` (`%PROGRAMDATA%`).
//! An unknown directory id such as `ProgramDataFolder` silently resolves to the root of the
//! system drive, which put 1.1.3's data dir at `C:\Grindvakt`.

const WXS: &str = include_str!("../scripts/package/windows/grindvakt.wxs");

#[test]
fn service_data_dir_is_under_programdata() {
    assert!(WXS.contains(r#"<Directory Id="CommonAppDataFolder">"#));
    assert!(WXS.contains(r#"--data-dir=&quot;[CommonAppDataFolder]Grindvakt&quot;"#));
    assert!(!WXS.contains("ProgramDataFolder"));
}

#[test]
fn install_folder_goes_on_the_system_path() {
    assert!(WXS.contains(r#"Name="PATH""#));
    assert!(WXS.contains(r#"Value="[INSTALLFOLDER]""#));
    assert!(WXS.contains(r#"System="yes""#));
}

#[test]
fn service_restarts_after_a_crash() {
    assert!(WXS.contains("<util:ServiceConfig"));
    assert_eq!(WXS.matches(r#"FailureActionType="restart""#).count(), 3);
    assert!(WXS.contains(r#"RestartServiceDelayInSeconds="60""#));
}
