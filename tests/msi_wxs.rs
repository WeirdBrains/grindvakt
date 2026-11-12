//! Checks on the WiX source of the Windows MSI (the built MSI is checked by make-msi.ps1).

const WXS: &str = include_str!("../scripts/package/windows/grindvakt.wxs");

/// The service data dir must be Windows Installer's `CommonAppDataFolder` (`%PROGRAMDATA%`). An
/// unknown directory id such as `ProgramDataFolder` silently resolves to the root of the system
/// drive, which put 1.1.3's data dir at `C:\Grindvakt`.
#[test]
fn service_data_dir_is_under_programdata() {
    assert!(WXS.contains(r#"<StandardDirectory Id="CommonAppDataFolder">"#));
    assert!(WXS.contains(r#"--data-dir=&quot;[CommonAppDataFolder]Grindvakt&quot;"#));
    assert!(!WXS.contains("ProgramDataFolder"));
}

#[test]
fn is_a_wix_4_plus_package_that_upgrades_1_1_x() {
    assert!(WXS.contains(r#"xmlns="http://wixtoolset.org/schemas/v4/wxs""#));
    assert!(WXS.contains(r#"UpgradeCode="{6BB7E9C7-6F10-4C1D-8C7B-39E0D6C3B7C5}""#));
    assert!(WXS.contains(r#"<MajorUpgrade Schedule="afterInstallInitialize""#));
    assert!(WXS.contains(r#"Id="filLicense""#) && WXS.contains(r#"Name="LICENSE.txt""#));
    assert!(WXS.contains(r#"<ui:WixUI Id="WixUI_Minimal" />"#));
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

#[test]
fn service_runs_as_its_own_account_and_only_it_gets_the_data_dir() {
    assert!(WXS.contains(r#"Account="NT SERVICE\Grindvakt""#));
    assert!(!WXS.contains("LocalService") && !WXS.contains(";;;LS)"));
    let sid = grindvakt_core::fs::service_sid("Grindvakt");
    assert!(WXS.contains(&format!(
        r#"Sddl="O:SYG:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{sid})""#
    )));
}

#[test]
fn upgrades_restrict_old_files_and_restore_a_running_service() {
    for ca in ["GvRememberRunning", "GvRepairAcl", "GvRestoreRunning"] {
        assert!(WXS.contains(&format!(r#"<Custom Action="{ca}""#)), "{ca}");
    }
    assert!(WXS.contains(
        r#"ExeCommand="service repair-acl --data-dir=&quot;[CommonAppDataFolder]Grindvakt&quot;""#
    ));
    // Windows Installer refuses (error 2613) any deferred action ahead of
    // RemoveExistingProducts, so the running check is immediate and runs first.
    assert!(WXS.contains(r#"<Custom Action="GvRememberRunning" Before="InstallInitialize""#));
    assert!(!WXS.contains(r#"Before="RemoveExistingProducts""#));
    assert!(WXS.contains(r#"After="StartServices""#));
    assert_eq!(WXS.matches(r#"Impersonate="no""#).count(), 2);
    assert_eq!(WXS.matches(r#"Execute="immediate""#).count(), 1);
}
