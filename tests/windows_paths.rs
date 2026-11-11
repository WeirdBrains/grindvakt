#![cfg(target_os = "windows")]
use grindvakt::config::ConfigPaths;

#[test]
fn default_paths_under_programdata() {
    let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    let paths = ConfigPaths::resolve(None).unwrap();
    assert!(
        paths
            .data_dir
            .starts_with(std::path::Path::new(&pd).join("Grindvakt"))
    );
    assert!(
        paths.credstore_dir.starts_with(
            std::path::Path::new(&pd)
                .join("Grindvakt")
                .join("credentials")
        )
    );
}
