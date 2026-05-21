use std::collections::BTreeMap;
use std::fs;

use camino::Utf8PathBuf;
use simit::cargo::Package;
use simit::config::{HomebrewOverrides, ProjectConfig};
use tempfile::TempDir;

fn load_toml(toml: &str) -> anyhow::Result<ProjectConfig> {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("simit.toml"), toml).unwrap();
    ProjectConfig::load(temp.path())
}

fn package() -> Package {
    Package {
        id: "path+file:///demo#0.1.0".to_owned(),
        name: "metadata-name".to_owned(),
        version: "0.1.0".to_owned(),
        license: Some("MIT".to_owned()),
        description: Some("metadata description".to_owned()),
        homepage: Some("https://metadata.example.com".to_owned()),
        rust_version: None,
        features: BTreeMap::new(),
        manifest_path: Utf8PathBuf::from("/demo/Cargo.toml"),
    }
}

#[test]
fn loading_without_simit_toml_returns_default_config() {
    let temp = TempDir::new().unwrap();

    let cfg = ProjectConfig::load(temp.path()).unwrap();

    assert!(cfg.homebrew.is_none());
}

#[test]
fn valid_homebrew_section_loads_and_validates() {
    let cfg = load_toml(
        r#"[homebrew]
name = "mythos"
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
binaries = ["mythos", "mythos-ui"]
description = "Mythos command line"
homepage = "https://codeberg.org/caniko/mythos"
license = "MIT"
"#,
    )
    .unwrap();

    cfg.validate_homebrew().unwrap();
    let homebrew = cfg.homebrew.unwrap();
    assert_eq!(homebrew.name.as_deref(), Some("mythos"));
    assert_eq!(homebrew.binaries, ["mythos", "mythos-ui"]);
    assert_eq!(
        homebrew.archive_pattern,
        "{name}-{version}-{arch}-{os}.tar.gz"
    );
    assert!(homebrew.platforms.darwin_arm);
}

#[test]
fn unknown_homebrew_field_is_rejected() {
    let err = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
typo = true
"#,
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("parsing"));
    assert!(message.contains("typo") || message.contains("unknown field"));
}

#[test]
fn missing_required_homebrew_field_is_a_parse_error() {
    let err = load_toml(
        r#"[homebrew]
download_repo = "caniko/mythos"
"#,
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("tap_url"));
}

#[test]
fn description_longer_than_eighty_characters_is_invalid() {
    let long_description = "x".repeat(81);
    let cfg = load_toml(&format!(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
description = "{long_description}"
"#
    ))
    .unwrap();

    let err = cfg.validate_homebrew().unwrap_err();

    assert!(format!("{err:#}").contains("description"));
}

#[test]
fn non_https_homepage_is_invalid() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
homepage = "http://example.com"
"#,
    )
    .unwrap();

    let err = cfg.validate_homebrew().unwrap_err();

    assert!(format!("{err:#}").contains("homepage"));
}

#[test]
fn all_disabled_platforms_are_invalid() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"

[homebrew.platforms]
darwin_arm = false
darwin_intel = false
linux_arm = false
linux_intel = false
"#,
    )
    .unwrap();

    let err = cfg.validate_homebrew().unwrap_err();

    assert!(format!("{err:#}").contains("all platforms disabled"));
}

#[test]
fn platforms_accept_inline_table_form() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
platforms = { linux_arm = false }
"#,
    )
    .unwrap();

    cfg.validate_homebrew().unwrap();
    let platforms = cfg.homebrew.unwrap().platforms;
    assert!(platforms.darwin_arm);
    assert!(platforms.darwin_intel);
    assert!(!platforms.linux_arm);
    assert!(platforms.linux_intel);
}

#[test]
fn resolve_homebrew_uses_cli_then_config_then_metadata_precedence() {
    let cfg = load_toml(
        r#"[homebrew]
name = "config-name"
tap_url = "https://config.example.com/homebrew-demo.git"
download_repo = "config/demo"
description = "config description"
homepage = "https://config.example.com"
license = "Apache-2.0"
archive_pattern = "config-{name}-{version}-{arch}-{os}.tar.gz"
binaries = ["config-bin"]
"#,
    )
    .unwrap();
    let cli_binaries = vec!["cli-bin".to_owned()];

    let resolved = cfg
        .resolve_homebrew(
            HomebrewOverrides {
                name: Some("cli-name"),
                binaries: Some(&cli_binaries),
                tap_url: Some("https://cli.example.com/homebrew-demo.git"),
                description: Some("cli description"),
                homepage: Some("https://cli.example.com"),
                license: Some("BSD-2-Clause"),
                download_repo: Some("cli/demo"),
                archive_pattern: Some("cli-{name}-{version}-{arch}-{os}.tar.gz"),
                disabled_platforms: &[],
            },
            &package(),
        )
        .unwrap();

    assert_eq!(resolved.name, "cli-name");
    assert_eq!(resolved.binaries, ["cli-bin"]);
    assert_eq!(
        resolved.tap_url,
        "https://cli.example.com/homebrew-demo.git"
    );
    assert_eq!(resolved.description, "cli description");
    assert_eq!(resolved.homepage, "https://cli.example.com");
    assert_eq!(resolved.license, "BSD-2-Clause");
    assert_eq!(resolved.download_repo, "cli/demo");
    assert_eq!(
        resolved.archive_pattern,
        "cli-{name}-{version}-{arch}-{os}.tar.gz"
    );

    let config_wins = cfg
        .resolve_homebrew(HomebrewOverrides::default(), &package())
        .unwrap();
    assert_eq!(config_wins.name, "config-name");
    assert_eq!(config_wins.description, "config description");
    assert_eq!(config_wins.homepage, "https://config.example.com");
    assert_eq!(config_wins.license, "Apache-2.0");
}

#[test]
fn resolve_homebrew_uses_metadata_when_config_omits_optional_fields() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_homebrew(HomebrewOverrides::default(), &package())
        .unwrap();

    assert_eq!(resolved.name, "metadata-name");
    assert_eq!(resolved.description, "metadata description");
    assert_eq!(resolved.homepage, "https://metadata.example.com");
    assert_eq!(resolved.license, "MIT");
}

#[test]
fn resolve_homebrew_errors_when_required_value_is_missing_everywhere() {
    let cfg = ProjectConfig::default();

    let err = cfg
        .resolve_homebrew(HomebrewOverrides::default(), &package())
        .unwrap_err();

    assert!(format!("{err:#}").contains("homebrew.tap_url not set"));
}

#[test]
fn resolve_homebrew_derives_name_from_package_metadata() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_homebrew(HomebrewOverrides::default(), &package())
        .unwrap();

    assert_eq!(resolved.name, "metadata-name");
}

#[test]
fn resolve_homebrew_derives_binaries_from_resolved_name() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_homebrew(
            HomebrewOverrides {
                name: Some("cli-name"),
                ..HomebrewOverrides::default()
            },
            &package(),
        )
        .unwrap();

    assert_eq!(resolved.binaries, ["cli-name"]);
}
