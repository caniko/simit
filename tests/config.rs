use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::sync::{Mutex, OnceLock};

use camino::Utf8PathBuf;
use simit::cargo::Package;
use simit::config::{
    ChocolateyOverrides, FlakeMode, FlakeScope, HomebrewOverrides, ProjectConfig, ScoopOverrides,
};
use tempfile::TempDir;

fn load_toml(toml: &str) -> anyhow::Result<ProjectConfig> {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("simit.toml"), toml).unwrap();
    ProjectConfig::load(temp.path())
}

fn load_cargo_manifest(manifest: &str) -> anyhow::Result<ProjectConfig> {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("Cargo.toml"), manifest).unwrap();
    ProjectConfig::load(temp.path())
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_fake_nix<T>(json: &str, body: impl FnOnce(&TempDir) -> T) -> T {
    let _guard = env_lock().lock().unwrap();
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let nix = bin.join("nix");
    fs::write(
        &nix,
        format!(
            r#"#!/bin/sh
printf '%s\n' '{}'
"#,
            json
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let old_path = env::var_os("PATH");
    let new_path = match &old_path {
        Some(path) => {
            let mut paths = vec![bin];
            paths.extend(env::split_paths(path));
            env::join_paths(paths).unwrap()
        }
        None => temp.path().join("bin").into_os_string(),
    };
    unsafe {
        env::set_var("PATH", new_path);
    }
    let result = body(&temp);
    unsafe {
        match old_path {
            Some(path) => env::set_var("PATH", path),
            None => env::remove_var("PATH"),
        }
    }
    result
}

fn package() -> Package {
    Package {
        id: "path+file:///demo#0.1.0".to_owned(),
        name: "metadata-name".to_owned(),
        version: "0.1.0".to_owned(),
        edition: Some("2024".to_owned()),
        authors: vec!["Metadata Author".to_owned()],
        license: Some("MIT".to_owned()),
        description: Some("metadata description".to_owned()),
        homepage: Some("https://metadata.example.com".to_owned()),
        rust_version: None,
        publish: None,
        features: BTreeMap::new(),
        dependencies: Vec::new(),
        manifest_path: Utf8PathBuf::from("/demo/Cargo.toml"),
    }
}

#[test]
fn loading_without_simit_toml_returns_default_config() {
    let temp = TempDir::new().unwrap();

    let cfg = ProjectConfig::load(temp.path()).unwrap();

    assert!(cfg.homebrew.is_none());
    assert!(cfg.chocolatey.is_none());
    assert!(cfg.scoop.is_none());
}

#[test]
fn workspace_cargo_metadata_simit_config_loads() {
    let cfg = load_cargo_manifest(
        r#"[workspace]
members = []

[workspace.metadata.simit.homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_homebrew(HomebrewOverrides::default(), &package())
        .unwrap();

    assert_eq!(
        resolved.tap_url,
        "https://codeberg.org/caniko/homebrew-mythos.git"
    );
    assert_eq!(resolved.download_repo, "caniko/mythos");
}

#[test]
fn package_cargo_metadata_simit_config_loads() {
    let cfg = load_cargo_manifest(
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"

[package.metadata.simit.scoop]
bucket_url = "https://codeberg.org/caniko/scoop-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_scoop(ScoopOverrides::default(), &package())
        .unwrap();

    assert_eq!(
        resolved.bucket_url,
        "https://codeberg.org/caniko/scoop-mythos.git"
    );
    assert_eq!(resolved.download_repo, "caniko/mythos");
}

#[test]
fn release_signing_config_loads() {
    let cfg = load_toml(
        r#"[release.signing]
key = "818D507F1E62139F8A17EAA64623DEA06FDACFE1"
trust_root = "keys/release-maintainers.gpg"
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.release.signing.key.as_deref(),
        Some("818D507F1E62139F8A17EAA64623DEA06FDACFE1")
    );
    assert_eq!(
        cfg.release.signing.trust_root,
        "keys/release-maintainers.gpg"
    );
    assert!(cfg.release.signing.required);
}

#[test]
fn release_signing_config_defaults_trust_root() {
    let cfg = load_toml(
        r#"[release.signing]
key = "818D507F1E62139F8A17EAA64623DEA06FDACFE1"
"#,
    )
    .unwrap();

    assert_eq!(cfg.release.signing.trust_root, "keys/maintainers.gpg");
    assert!(cfg.release.signing.required);
}

#[test]
fn release_smoke_config_loads() {
    let cfg = load_toml(
        r#"[release.smoke]
command = "nix run .#release-smoke --"
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.release.smoke.command.as_deref(),
        Some("nix run .#release-smoke --")
    );
}

#[test]
fn flake_and_ci_config_load() {
    let cfg = load_toml(
        r#"[flake]
scope = "full"
mode = "custom"
backend = "py-harbor"
toolchain_binding = "toolchain.rustToolchain"
crane_lib_binding = "craneLib"
package_binding = "package"
formatter_output = true
formatting_check = true
pre_commit_shell_hook = true

[flake.expected_outputs]
packages = ["default", "docs", "site"]
apps = ["default"]
dev_shells = ["default", "docs"]
checks = ["default", "formatting", "hm-module"]
top_level = ["hmModules"]

[ci]
extra_setup = ["apt-get update && apt-get install -y --no-install-recommends postgresql-client"]
extra_env = { SKILLNET_TEST_PG_URL = "${{ secrets.SKILLNET_TEST_PG_URL }}" }
required_secrets = ["CRATES_IO_API_TOKEN"]

[ci.pages]
repo = "caniko/plinth"
"#,
    )
    .unwrap();

    assert_eq!(cfg.flake.scope, Some(FlakeScope::Full));
    assert_eq!(cfg.flake.mode, FlakeMode::Custom);
    assert_eq!(cfg.flake.backend, simit::config::FlakeBackend::PyHarbor);
    assert_eq!(cfg.flake.toolchain_binding, "toolchain.rustToolchain");
    assert_eq!(
        cfg.flake.expected_outputs.packages,
        ["default", "docs", "site"]
    );
    assert_eq!(
        cfg.flake.expected_outputs.checks,
        ["default", "formatting", "hm-module"]
    );
    assert_eq!(cfg.flake.expected_outputs.apps, ["default"]);
    assert_eq!(cfg.flake.expected_outputs.dev_shells, ["default", "docs"]);
    assert_eq!(cfg.flake.expected_outputs.top_level, ["hmModules"]);
    assert_eq!(cfg.ci.extra_setup.len(), 1);
    assert_eq!(
        cfg.ci
            .extra_env
            .get("SKILLNET_TEST_PG_URL")
            .map(String::as_str),
        Some("${{ secrets.SKILLNET_TEST_PG_URL }}")
    );
    assert_eq!(cfg.ci.required_secrets, ["CRATES_IO_API_TOKEN"]);
    let pages = cfg.resolve_codeberg_pages().unwrap().unwrap();
    assert_eq!(pages.repo, "caniko/plinth");
    assert_eq!(pages.owner, "caniko");
    assert_eq!(pages.token_secret, "codeberg_token");
    assert_eq!(pages.source_branch, "trunk");
    assert_eq!(pages.deploy_app, ".#deploy-pages");
}

#[test]
fn flake_and_ci_config_load_from_flake_output() {
    with_fake_nix(
        r#"{"flake":{"scope":"hooks-only","mode":"custom","backend":"py-harbor","toolchain_binding":"toolchain.rustToolchain","expected_outputs":{"apps":["default"],"dev_shells":["default"],"checks":["hm-module"],"top_level":["hmModules"]}},"ci":{"extra_setup":["echo setup"],"extra_env":{"PG_URL":"${{ secrets.PG_URL }}"}}}"#,
        |temp| {
            fs::write(
                temp.path().join("flake.nix"),
                r#"{ outputs = { self }: { simitConfig = {}; }; }"#,
            )
            .unwrap();

            let cfg = ProjectConfig::load(temp.path()).unwrap();
            assert_eq!(cfg.flake.scope, Some(FlakeScope::HooksOnly));
            assert_eq!(cfg.flake.mode, FlakeMode::Custom);
            assert_eq!(cfg.flake.backend, simit::config::FlakeBackend::PyHarbor);
            assert_eq!(cfg.flake.toolchain_binding, "toolchain.rustToolchain");
            assert_eq!(cfg.flake.expected_outputs.apps, ["default"]);
            assert_eq!(cfg.flake.expected_outputs.dev_shells, ["default"]);
            assert_eq!(cfg.flake.expected_outputs.checks, ["hm-module"]);
            assert_eq!(cfg.flake.expected_outputs.top_level, ["hmModules"]);
            assert_eq!(cfg.ci.extra_setup, ["echo setup"]);
            assert_eq!(
                cfg.ci.extra_env.get("PG_URL").map(String::as_str),
                Some("${{ secrets.PG_URL }}")
            );
        },
    );
}

#[test]
fn invalid_flake_mode_is_rejected() {
    load_toml(
        r#"[flake]
mode = "bespoke"
"#,
    )
    .unwrap_err();
}

#[test]
fn invalid_flake_scope_is_rejected() {
    load_toml(
        r#"[flake]
scope = "everything"
"#,
    )
    .unwrap_err();
}

#[test]
fn empty_expected_output_is_rejected() {
    let err = load_toml(
        r#"[flake]
mode = "custom"

[flake.expected_outputs]
checks = [""]
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("[flake.expected_outputs].checks must not contain empty values")
    );
}

#[test]
fn multiline_ci_env_is_rejected() {
    let err = load_toml(
        r#"[ci]
extra_env = { BAD = "one\ntwo" }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("[ci].extra_env must not contain multiline")
    );
}

#[test]
fn flake_simit_config_output_loads() {
    with_fake_nix(
        r#"{"homebrew":{"tap_url":"https://codeberg.org/caniko/homebrew-mythos.git","download_repo":"caniko/mythos"}}"#,
        |temp| {
            fs::write(
                temp.path().join("flake.nix"),
                r#"{ outputs = { self }: { simitConfig = {}; }; }"#,
            )
            .unwrap();

            let resolved = ProjectConfig::load(temp.path())
                .unwrap()
                .resolve_homebrew(HomebrewOverrides::default(), &package())
                .unwrap();
            assert_eq!(resolved.download_repo, "caniko/mythos");
        },
    );
}

#[test]
fn multiple_project_config_sources_are_rejected() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"

[package.metadata.simit.scoop]
bucket_url = "https://codeberg.org/caniko/scoop-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let err = ProjectConfig::load(temp.path()).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("multiple simit project config sources found"));
    assert!(message.contains("simit.toml"));
    assert!(message.contains("Cargo.toml [package.metadata.simit]"));
}

#[test]
fn workspace_and_package_cargo_metadata_sources_are_rejected() {
    let err = load_cargo_manifest(
        r#"[package]
name = "demo"
version = "0.1.0"

[package.metadata.simit.homebrew]
tap_url = "https://package.example.com/homebrew-demo.git"
download_repo = "package/demo"

[workspace.metadata.simit.scoop]
bucket_url = "https://workspace.example.com/scoop-demo.git"
download_repo = "workspace/demo"
"#,
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("Cargo.toml [workspace.metadata.simit]"));
    assert!(message.contains("Cargo.toml [package.metadata.simit]"));
}

#[test]
fn cargo_metadata_and_flake_sources_are_rejected() {
    with_fake_nix(
        r#"{"scoop":{"bucket_url":"https://codeberg.org/caniko/scoop-mythos.git","download_repo":"caniko/mythos"}}"#,
        |temp| {
            fs::write(
                temp.path().join("Cargo.toml"),
                r#"[workspace]
members = []

[workspace.metadata.simit.homebrew]
tap_url = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
            )
            .unwrap();
            fs::write(
                temp.path().join("flake.nix"),
                r#"{ outputs = { self }: { simitConfig = {}; }; }"#,
            )
            .unwrap();

            let err = ProjectConfig::load(temp.path()).unwrap_err();
            let message = format!("{err:#}");
            assert!(message.contains("Cargo.toml [workspace.metadata.simit]"));
            assert!(message.contains("flake outputs.simitConfig"));
        },
    );
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
fn tap_url_with_embedded_credentials_is_invalid() {
    let cfg = load_toml(
        r#"[homebrew]
tap_url = "https://user:token@codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let err = cfg.validate_homebrew().unwrap_err();

    assert!(format!("{err:#}").contains("embedded credentials"));
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

#[test]
fn valid_chocolatey_section_loads_and_validates() {
    let cfg = load_toml(
        r#"[chocolatey]
name = "mythos"
id = "mythos"
title = "Mythos"
authors = "Can"
description = "Mythos command line"
project_url = "https://codeberg.org/caniko/mythos"
license_url = "https://codeberg.org/caniko/mythos/src/branch/trunk/LICENSE"
tags = "mythos cli"
release_notes_url = "https://codeberg.org/caniko/mythos/releases"
download_repo = "caniko/mythos"

[chocolatey.push]
source = "https://push.chocolatey.org/"
"#,
    )
    .unwrap();

    cfg.validate_chocolatey().unwrap();
    let chocolatey = cfg.chocolatey.unwrap();
    assert_eq!(chocolatey.name.as_deref(), Some("mythos"));
    assert_eq!(
        chocolatey.archive_pattern,
        "{name}-{version}-{arch}-windows.zip"
    );
    assert_eq!(chocolatey.push.source, "https://push.chocolatey.org/");
}

#[test]
fn resolve_chocolatey_errors_when_required_value_is_missing_everywhere() {
    let cfg = ProjectConfig::default();

    let err = cfg
        .resolve_chocolatey(ChocolateyOverrides::default(), &package())
        .unwrap_err();

    assert!(format!("{err:#}").contains("chocolatey.download_repo not set"));
}

#[test]
fn resolve_chocolatey_uses_cli_then_config_then_metadata_precedence() {
    let cfg = load_toml(
        r#"[chocolatey]
name = "config-name"
id = "config-id"
title = "Config Title"
authors = "Config Authors"
description = "config description"
project_url = "https://config.example.com"
license_url = "https://config.example.com/license"
tags = "config tags"
release_notes_url = "https://config.example.com/releases"
download_repo = "config/demo"
archive_pattern = "config-{name}-{version}-{arch}.zip"

[chocolatey.push]
source = "https://config.example.com/choco"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_chocolatey(
            ChocolateyOverrides {
                name: Some("cli-name"),
                id: Some("cli-id"),
                title: Some("CLI Title"),
                authors: Some("CLI Authors"),
                description: Some("cli description"),
                project_url: Some("https://cli.example.com"),
                license_url: Some("https://cli.example.com/license"),
                tags: Some("cli tags"),
                release_notes_url: Some("https://cli.example.com/releases"),
                download_repo: Some("cli/demo"),
                archive_pattern: Some("cli-{name}-{version}-{arch}.zip"),
                push_source: Some("https://cli.example.com/choco"),
            },
            &package(),
        )
        .unwrap();

    assert_eq!(resolved.name, "cli-name");
    assert_eq!(resolved.id, "cli-id");
    assert_eq!(resolved.title, "CLI Title");
    assert_eq!(resolved.authors.as_deref(), Some("CLI Authors"));
    assert_eq!(resolved.description, "cli description");
    assert_eq!(resolved.project_url, "https://cli.example.com");
    assert_eq!(
        resolved.license_url.as_deref(),
        Some("https://cli.example.com/license")
    );
    assert_eq!(resolved.tags.as_deref(), Some("cli tags"));
    assert_eq!(
        resolved.release_notes_url.as_deref(),
        Some("https://cli.example.com/releases")
    );
    assert_eq!(resolved.download_repo, "cli/demo");
    assert_eq!(resolved.archive_pattern, "cli-{name}-{version}-{arch}.zip");
    assert_eq!(resolved.push.source, "https://cli.example.com/choco");

    let config_wins = cfg
        .resolve_chocolatey(ChocolateyOverrides::default(), &package())
        .unwrap();
    assert_eq!(config_wins.name, "config-name");
    assert_eq!(config_wins.description, "config description");
    assert_eq!(config_wins.project_url, "https://config.example.com");
}

#[test]
fn resolve_chocolatey_uses_metadata_when_config_omits_optional_fields() {
    let cfg = load_toml(
        r#"[chocolatey]
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_chocolatey(ChocolateyOverrides::default(), &package())
        .unwrap();

    assert_eq!(resolved.name, "metadata-name");
    assert_eq!(resolved.id, "metadata-name");
    assert_eq!(resolved.title, "metadata-name");
    assert_eq!(resolved.description, "metadata description");
    assert_eq!(resolved.project_url, "https://metadata.example.com");
}

#[test]
fn valid_scoop_section_loads_and_validates() {
    let cfg = load_toml(
        r#"[scoop]
name = "mythos"
bucket_url = "https://codeberg.org/caniko/scoop-mythos.git"
description = "Mythos command line"
homepage = "https://codeberg.org/caniko/mythos"
license = "MIT"
download_repo = "caniko/mythos"
binaries = ["mythos", "mythos-ui"]
"#,
    )
    .unwrap();

    cfg.validate_scoop().unwrap();
    let scoop = cfg.scoop.unwrap();
    assert_eq!(scoop.name.as_deref(), Some("mythos"));
    assert_eq!(scoop.binaries, ["mythos", "mythos-ui"]);
    assert_eq!(scoop.archive_pattern, "{name}-{version}-{arch}-windows.zip");
    assert!(scoop.architectures.x64);
    assert!(scoop.architectures.arm64);
}

#[test]
fn resolve_scoop_errors_when_required_value_is_missing_everywhere() {
    let cfg = ProjectConfig::default();

    let err = cfg
        .resolve_scoop(ScoopOverrides::default(), &package())
        .unwrap_err();

    assert!(format!("{err:#}").contains("scoop.bucket_url not set"));
}

#[test]
fn resolve_scoop_uses_cli_then_config_then_metadata_precedence() {
    let cfg = load_toml(
        r#"[scoop]
name = "config-name"
bucket_url = "https://config.example.com/scoop-demo.git"
description = "config description"
homepage = "https://config.example.com"
license = "Apache-2.0"
download_repo = "config/demo"
archive_pattern = "config-{name}-{version}-{arch}.zip"
binaries = ["config-bin"]
"#,
    )
    .unwrap();
    let cli_binaries = vec!["cli-bin".to_owned()];

    let resolved = cfg
        .resolve_scoop(
            ScoopOverrides {
                name: Some("cli-name"),
                bucket_url: Some("https://cli.example.com/scoop-demo.git"),
                description: Some("cli description"),
                homepage: Some("https://cli.example.com"),
                license: Some("BSD-2-Clause"),
                download_repo: Some("cli/demo"),
                archive_pattern: Some("cli-{name}-{version}-{arch}.zip"),
                binaries: Some(&cli_binaries),
                disabled_architectures: &["arm64".to_owned()],
            },
            &package(),
        )
        .unwrap();

    assert_eq!(resolved.name, "cli-name");
    assert_eq!(
        resolved.bucket_url,
        "https://cli.example.com/scoop-demo.git"
    );
    assert_eq!(resolved.description, "cli description");
    assert_eq!(resolved.homepage, "https://cli.example.com");
    assert_eq!(resolved.license, "BSD-2-Clause");
    assert_eq!(resolved.download_repo, "cli/demo");
    assert_eq!(resolved.archive_pattern, "cli-{name}-{version}-{arch}.zip");
    assert_eq!(resolved.binaries, ["cli-bin"]);
    assert!(resolved.architectures.x64);
    assert!(!resolved.architectures.arm64);

    let config_wins = cfg
        .resolve_scoop(ScoopOverrides::default(), &package())
        .unwrap();
    assert_eq!(config_wins.name, "config-name");
    assert_eq!(config_wins.description, "config description");
    assert_eq!(config_wins.homepage, "https://config.example.com");
    assert_eq!(config_wins.license, "Apache-2.0");
}

#[test]
fn resolve_scoop_uses_metadata_when_config_omits_optional_fields() {
    let cfg = load_toml(
        r#"[scoop]
bucket_url = "https://codeberg.org/caniko/scoop-mythos.git"
download_repo = "caniko/mythos"
"#,
    )
    .unwrap();

    let resolved = cfg
        .resolve_scoop(ScoopOverrides::default(), &package())
        .unwrap();

    assert_eq!(resolved.name, "metadata-name");
    assert_eq!(resolved.description, "metadata description");
    assert_eq!(resolved.homepage, "https://metadata.example.com");
    assert_eq!(resolved.license, "MIT");
    assert_eq!(resolved.binaries, ["metadata-name"]);
}

#[test]
fn all_disabled_scoop_architectures_are_invalid() {
    let cfg = load_toml(
        r#"[scoop]
bucket_url = "https://codeberg.org/caniko/scoop-mythos.git"
download_repo = "caniko/mythos"

[scoop.architectures]
x64 = false
arm64 = false
"#,
    )
    .unwrap();

    let err = cfg.validate_scoop().unwrap_err();

    assert!(format!("{err:#}").contains("all architectures disabled"));
}
