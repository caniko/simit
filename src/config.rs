//! Project-level simit configuration.
//!
//! Packager setting precedence is intentionally centralized here. For each
//! setting, the value comes from, in order:
//!
//! 1. The CLI flag, when provided.
//! 2. The corresponding simit project config field, when a project config
//!    source exists and the field is present.
//! 3. The Cargo package metadata fallback, where one exists.
//! 4. An error.
//!
//! Settings that have no Cargo fallback, such as `tap_url` and
//! `download_repo`, error if neither a CLI flag nor config value provides them.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::cli::{CiProvider, CrowWorkflowFormat, Platform, Runtime, WorkspaceStrategy};
use crate::user_config::validate_runner_label;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub prebuild: Option<PrebuildConfig>,
    #[serde(default)]
    pub release: ReleaseConfig,
    #[serde(default)]
    pub flake: FlakeConfig,
    #[serde(default)]
    pub ci: CiConfig,
    #[serde(default)]
    pub homebrew: Option<HomebrewConfig>,
    #[serde(default)]
    pub chocolatey: Option<ChocolateyConfig>,
    #[serde(default)]
    pub scoop: Option<ScoopConfig>,
    #[serde(default)]
    pub aur: Option<AurConfig>,
    #[serde(default)]
    pub copr: Option<CoprConfig>,
    #[serde(default)]
    pub apt: Option<AptConfig>,
    #[serde(default)]
    pub flatpak: Option<FlatpakConfig>,
    #[serde(default)]
    pub winget: Option<WingetConfig>,
    #[serde(default)]
    pub vscode: Option<VscodeConfig>,
    #[serde(default)]
    pub jetbrains: Option<JetbrainsConfig>,
}

/// `[vscode]` — publish a VS Code/Open VSX extension from Forgejo CI.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VscodeConfig {
    /// Extension package directory, relative to the repository root.
    #[serde(default = "default_vscode_extension_dir")]
    pub extension_dir: String,
    /// Optional runner override. Defaults to the normal release/CI runner.
    #[serde(default)]
    pub runner: Option<String>,
    /// Codeberg `<owner>/<repo>` receiving release assets.
    pub codeberg_repo: String,
    /// REST API base URL.
    #[serde(default = "default_codeberg_api_base")]
    pub codeberg_api_base: String,
    /// Actions secret exposed as `CODEBERG_TOKEN`.
    #[serde(default = "default_codeberg_token_secret")]
    pub codeberg_token_secret: String,
    /// Source used for Marketplace/Open VSX PATs.
    #[serde(default)]
    pub pat_source: VscodePatSource,
    /// Runner file-env variable pointing at the VS Code Marketplace PAT file.
    #[serde(default = "default_vscode_vsce_pat_file_env")]
    pub vsce_pat_file_env: String,
    /// Runner file-env variable pointing at the Open VSX PAT file.
    #[serde(default = "default_vscode_ovsx_pat_file_env")]
    pub ovsx_pat_file_env: String,
    /// Actions secret containing the VS Code Marketplace PAT.
    #[serde(default = "default_vscode_vsce_pat_secret")]
    pub vsce_pat_secret: String,
    /// Actions secret containing the Open VSX PAT.
    #[serde(default = "default_vscode_ovsx_pat_secret")]
    pub ovsx_pat_secret: String,
    /// Command that packages release assets. `$VERSION` contains the tag version.
    #[serde(default = "default_vscode_package_command")]
    pub package_command: String,
    /// Optional Cargo package whose version must match the release tag.
    #[serde(default)]
    pub cargo_package: Option<String>,
    /// Project-specific checks/build commands run before packaging/publishing.
    #[serde(default)]
    pub prepublish_commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VscodePatSource {
    FileEnv,
    ActionsSecret,
    #[default]
    Both,
}

fn default_vscode_extension_dir() -> String {
    "vscode".to_owned()
}

fn default_vscode_vsce_pat_file_env() -> String {
    "VSCE_PAT_FILE".to_owned()
}

fn default_vscode_ovsx_pat_file_env() -> String {
    "OVSX_PAT_FILE".to_owned()
}

fn default_vscode_vsce_pat_secret() -> String {
    "VSCE_PAT".to_owned()
}

fn default_vscode_ovsx_pat_secret() -> String {
    "OVSX_PAT".to_owned()
}

fn default_vscode_package_command() -> String {
    "nix run .#package-release-assets -- \"$VERSION\"".to_owned()
}

/// `[jetbrains]` — publish an IntelliJ Platform plugin from Forgejo CI.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JetbrainsConfig {
    #[serde(default = "default_jetbrains_plugin_dir")]
    pub plugin_dir: String,
    pub plugin_xml_id: String,
    #[serde(default = "default_jetbrains_package_installable")]
    pub package_installable: String,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub cargo_package: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub credential_source: JetbrainsCredentialSource,
    #[serde(default = "default_jetbrains_marketplace_token_file_env")]
    pub marketplace_token_file_env: String,
    #[serde(default = "default_jetbrains_certificate_chain_file_env")]
    pub certificate_chain_file_env: String,
    #[serde(default = "default_jetbrains_private_key_file_env")]
    pub private_key_file_env: String,
    #[serde(default = "default_jetbrains_private_key_password_file_env")]
    pub private_key_password_file_env: String,
    #[serde(default = "default_jetbrains_marketplace_token_secret")]
    pub marketplace_token_secret: String,
    #[serde(default = "default_jetbrains_certificate_chain_secret")]
    pub certificate_chain_secret: String,
    #[serde(default = "default_jetbrains_private_key_secret")]
    pub private_key_secret: String,
    #[serde(default = "default_jetbrains_private_key_password_secret")]
    pub private_key_password_secret: String,
    #[serde(default)]
    pub prepublish_commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JetbrainsCredentialSource {
    FileEnv,
    ActionsSecret,
    #[default]
    Both,
}

fn default_jetbrains_plugin_dir() -> String {
    "pkl-lsp-jetbrains".to_owned()
}
fn default_jetbrains_package_installable() -> String {
    ".#jetbrains-plugin".to_owned()
}
fn default_jetbrains_marketplace_token_file_env() -> String {
    "JETBRAINS_MARKETPLACE_TOKEN_FILE".to_owned()
}
fn default_jetbrains_certificate_chain_file_env() -> String {
    "JETBRAINS_CERTIFICATE_CHAIN_FILE".to_owned()
}
fn default_jetbrains_private_key_file_env() -> String {
    "JETBRAINS_PRIVATE_KEY_FILE".to_owned()
}
fn default_jetbrains_private_key_password_file_env() -> String {
    "JETBRAINS_PRIVATE_KEY_PASSWORD_FILE".to_owned()
}
fn default_jetbrains_marketplace_token_secret() -> String {
    "JETBRAINS_MARKETPLACE_TOKEN".to_owned()
}
fn default_jetbrains_certificate_chain_secret() -> String {
    "JETBRAINS_CERTIFICATE_CHAIN".to_owned()
}
fn default_jetbrains_private_key_secret() -> String {
    "JETBRAINS_PRIVATE_KEY".to_owned()
}
fn default_jetbrains_private_key_password_secret() -> String {
    "JETBRAINS_PRIVATE_KEY_PASSWORD".to_owned()
}

/// `[flatpak]` — open a Flathub manifest-update PR on stable releases.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlatpakConfig {
    /// GitHub `flathub/<app-id>` repository.
    pub repo: String,
    /// Flatpak application id.
    pub app_id: String,
    /// Manifest files copied into the Flathub PR.
    #[serde(default)]
    pub manifest_files: Vec<String>,
    /// Base branch the PR targets.
    #[serde(default = "default_flatpak_base_branch")]
    pub base_branch: String,
    /// CI secret holding the GitHub PAT.
    #[serde(default = "default_flathub_token_secret")]
    pub token_secret: String,
}

fn default_flatpak_base_branch() -> String {
    "master".to_owned()
}

fn default_flathub_token_secret() -> String {
    "FLATHUB_TOKEN".to_owned()
}

/// `[winget]` — submit a winget-pkgs manifest PR on stable releases.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WingetConfig {
    /// winget `PackageIdentifier`, e.g. `Caniko.Modde`.
    pub package_id: String,
    /// Codeberg/GitHub `<owner>/<repo>` for the installer download URL.
    pub download_repo: String,
    /// Windows zip file name with a literal `{version}` placeholder, used as the
    /// installer URL, e.g. `modde-{version}-x86_64-windows.zip`.
    pub zip_archive: String,
    /// CI secret holding the GitHub PAT.
    #[serde(default = "default_winget_token_secret")]
    pub token_secret: String,
}

fn default_winget_token_secret() -> String {
    "WINGET_PAT".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlakeConfig {
    #[serde(default)]
    pub scope: Option<FlakeScope>,
    #[serde(default)]
    pub mode: FlakeMode,
    #[serde(default)]
    pub backend: FlakeBackend,
    #[serde(default = "default_toolchain_binding")]
    pub toolchain_binding: String,
    #[serde(default = "default_crane_lib_binding")]
    pub crane_lib_binding: String,
    #[serde(default = "default_package_binding")]
    pub package_binding: String,
    #[serde(default = "default_true")]
    pub formatter_output: bool,
    #[serde(default = "default_true")]
    pub formatting_check: bool,
    #[serde(default = "default_true")]
    pub pre_commit_shell_hook: bool,
    #[serde(default)]
    pub expected_outputs: FlakeExpectedOutputs,
    /// Generated pre-commit components. An empty list keeps language-based
    /// auto-detection for backwards compatibility.
    #[serde(default)]
    pub components: Vec<FlakeComponent>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FlakeComponent {
    Treefmt,
    CargoFmt,
    CargoClippy,
    CargoMsrv,
    CargoAudit,
    CargoDeny,
    NixFlakeCheck,
    UvRuffFormat,
    UvMypy,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FlakeScope {
    HooksOnly,
    Full,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FlakeMode {
    #[default]
    Canonical,
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FlakeBackend {
    #[default]
    RustCrane,
    PyHarbor,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlakeExpectedOutputs {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub dev_shells: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub top_level: Vec<String>,
}

impl Default for FlakeConfig {
    fn default() -> Self {
        Self {
            scope: None,
            mode: FlakeMode::Canonical,
            backend: FlakeBackend::RustCrane,
            toolchain_binding: default_toolchain_binding(),
            crane_lib_binding: default_crane_lib_binding(),
            package_binding: default_package_binding(),
            formatter_output: true,
            formatting_check: true,
            pre_commit_shell_hook: true,
            expected_outputs: FlakeExpectedOutputs::default(),
            components: Vec::new(),
        }
    }
}

fn default_toolchain_binding() -> String {
    "rustToolchain".to_owned()
}

fn default_crane_lib_binding() -> String {
    "craneLib".to_owned()
}

fn default_package_binding() -> String {
    "package".to_owned()
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CiConfig {
    #[serde(default)]
    pub provider: Option<CiProvider>,
    #[serde(default)]
    pub platform: Option<Platform>,
    #[serde(default)]
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub runner: Option<String>,
    /// Native GitHub-hosted runner label for each Nix system in a flake-only
    /// workflow. When set, Nix-only GitHub CI is rendered as a native matrix
    /// instead of evaluating every system from one runner.
    #[serde(default)]
    pub nix_system_runners: BTreeMap<String, String>,
    #[serde(default)]
    pub windows_runner: Option<String>,
    #[serde(default)]
    pub workspace: bool,
    #[serde(default)]
    pub workspace_strategy: WorkspaceStrategy,
    #[serde(default)]
    pub packages: Vec<String>,
    /// Nix installables that must be built by generated hosted-runner jobs.
    /// Values are passed to `nix build --no-link` unchanged.
    #[serde(default)]
    pub nix_builds: Vec<String>,
    #[serde(default)]
    pub with_nextest: bool,
    #[serde(default)]
    pub with_msrv: bool,
    #[serde(default)]
    pub with_audit: bool,
    #[serde(default)]
    pub with_deny: bool,
    #[serde(default)]
    pub with_docs: bool,
    #[serde(default)]
    pub with_artifacts: bool,
    #[serde(default)]
    pub with_pypi_publish: bool,
    #[serde(default)]
    pub pypi_token_secret: Option<String>,
    #[serde(default)]
    pub pypi_trusted_publishing: bool,
    #[serde(default)]
    pub publish_crates: bool,
    #[serde(default)]
    pub extra_setup: Vec<String>,
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    #[serde(default)]
    pub required_secrets: Vec<String>,
    #[serde(default)]
    pub required_env: Vec<String>,
    #[serde(default)]
    pub om_ci: bool,
    #[serde(default)]
    pub om_ci_augment: bool,
    #[serde(default)]
    pub omnix_ref: Option<String>,
    #[serde(default)]
    pub pages: Option<CodebergPagesConfig>,
    #[serde(default)]
    pub step_runners: BTreeMap<String, String>,
    /// Workflow components. Currently used by Python/Nix CI; an empty list
    /// selects the complete project-appropriate default set.
    #[serde(default)]
    pub components: Vec<CiComponent>,
    #[serde(default)]
    pub crow: CrowCiConfig,
}

/// `[prebuild]` - native GitHub Nix builds shared by CI and releases.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrebuildConfig {
    #[serde(default)]
    pub release_archives: bool,
    #[serde(default)]
    pub publish_attic: bool,
    /// rs-harbor app used for Attic publication.
    #[serde(default)]
    pub attic_app: Option<String>,
    /// GitHub-hosted runner label for each native Nix system. This keeps the
    /// prebuild additive when the project's primary CI is Forgejo or Crow.
    #[serde(default)]
    pub system_runners: BTreeMap<String, String>,
}

impl PrebuildConfig {
    pub(crate) fn effective_system_runners<'a>(
        &'a self,
        ci: &'a CiConfig,
    ) -> &'a BTreeMap<String, String> {
        if self.system_runners.is_empty() {
            &ci.nix_system_runners
        } else {
            &self.system_runners
        }
    }
}

/// `[ci.crow]` — project-side Crow workflow rendering options.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CrowCiConfig {
    pub format: CrowWorkflowFormat,
    pub image: Option<String>,
    pub nix_image: Option<String>,
    pub platform: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub workspace_base: Option<String>,
    pub skip_clone: bool,
    pub variables: BTreeMap<String, CrowVariable>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrowVariable {
    pub description: Option<String>,
    pub options: Vec<String>,
    pub default: Option<String>,
    pub required: bool,
}

impl Default for CrowCiConfig {
    fn default() -> Self {
        Self {
            format: CrowWorkflowFormat::Yaml,
            image: None,
            nix_image: None,
            platform: None,
            labels: BTreeMap::new(),
            workspace_base: None,
            skip_clone: false,
            variables: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CiComponent {
    FlakeWiring,
    FlakeEvaluation,
    Checks,
}

/// `[ci.pages]` — Codeberg Pages publication through a repository-local deploy app.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodebergPagesConfig {
    /// Codeberg `<owner>/<repo>` receiving the generated `pages` branch.
    pub repo: String,
    /// Canonical hostname expected in the generated `.domains` file.
    #[serde(default)]
    pub canonical_domain: Option<String>,
    /// Nix installable that builds the generated static site.
    #[serde(default = "default_pages_site_output")]
    pub site_output: String,
    /// CI secret exposed as `CODEBERG_TOKEN` for authenticated branch pushes.
    #[serde(default = "default_codeberg_token_secret")]
    pub token_secret: String,
    /// Source branch that triggers the Pages deployment workflow.
    #[serde(default = "default_pages_source_branch")]
    pub source_branch: String,
    /// Nix app that builds and pushes the generated site.
    #[serde(default = "default_pages_deploy_app")]
    pub deploy_app: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Changelog drafting performed before a release commit is created.
    #[serde(default)]
    pub changelog: ReleaseChangelogConfig,
    #[serde(default)]
    pub signing: ReleaseSigningConfig,
    #[serde(default)]
    pub publish: ReleasePublishConfig,
    #[serde(default)]
    pub smoke: ReleaseSmokeConfig,
    /// Codeberg/Forgejo release publication via the REST API.
    #[serde(default)]
    pub codeberg: Option<CodebergReleaseConfig>,
    /// GitHub release publication via the GitHub REST API.
    #[serde(default)]
    pub github: Option<GithubReleaseConfig>,
    /// Build matrix + signing knobs for the comprehensive release workflow.
    #[serde(default)]
    pub artifacts: ArtifactsConfig,
    /// Optional Attic (Nix binary cache) push.
    #[serde(default)]
    pub attic: Option<AtticConfig>,
    /// Optional Mastodon/Matrix stable-release announcements.
    #[serde(default)]
    pub announce: Option<AnnounceConfig>,
    /// Optional Windows Authenticode signing of release `.exe`s.
    #[serde(default)]
    pub windows_signing: Option<WindowsSigningConfig>,
}

/// `[release.changelog]` — optional Codex-assisted Keep a Changelog drafting.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseChangelogConfig {
    /// Draft `[Unreleased]` from commits before `simit release` promotes it.
    #[serde(default)]
    pub auto_draft: bool,
}

/// `[release.publish]` — downstream publisher failure policy.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePublishConfig {
    /// How generated CI decides whether downstream publishers are hard-required.
    #[serde(default)]
    pub enforcement: ReleasePublisherEnforcement,
    /// Per-channel lifecycle overrides. Explicit entries take precedence over
    /// the legacy global enforcement setting.
    #[serde(default)]
    pub channels: BTreeMap<String, ReleasePublisherPolicy>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReleasePublisherPolicy {
    Disabled,
    Staged,
    Required,
}

impl ReleasePublishConfig {
    pub fn policy(&self, channel: &str) -> Option<ReleasePublisherPolicy> {
        self.channels.get(channel).copied()
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReleasePublisherEnforcement {
    /// Preserve the historical behavior: credentials marked required by the
    /// generated contract fail preflight; optional publishers may skip.
    #[default]
    Declared,
    /// Probe public package destinations. Missing credentials/artifacts are
    /// soft until the channel has evidence of a previous successful publish,
    /// then become hard failures.
    ActivatedRemote,
}

/// `[release.announce]` — post a stable-release note to Mastodon and/or Matrix.
/// Presence enables the step; each backend is skipped when its secrets are unset.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AnnounceConfig {
    #[serde(default = "default_mastodon_token_secret")]
    pub mastodon_token_secret: String,
    #[serde(default = "default_mastodon_base_url_secret")]
    pub mastodon_base_url_secret: String,
    #[serde(default = "default_matrix_token_secret")]
    pub matrix_token_secret: String,
    #[serde(default = "default_matrix_homeserver_secret")]
    pub matrix_homeserver_secret: String,
    #[serde(default = "default_matrix_room_secret")]
    pub matrix_room_secret: String,
}

fn default_mastodon_token_secret() -> String {
    "MASTODON_TOKEN".to_owned()
}
fn default_mastodon_base_url_secret() -> String {
    "MASTODON_BASE_URL".to_owned()
}
fn default_matrix_token_secret() -> String {
    "MATRIX_TOKEN".to_owned()
}
fn default_matrix_homeserver_secret() -> String {
    "MATRIX_HOMESERVER".to_owned()
}
fn default_matrix_room_secret() -> String {
    "MATRIX_ROOM".to_owned()
}

/// `[release.windows_signing]` — Authenticode-sign and package Windows `.exe`s.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowsSigningConfig {
    /// Directory holding the built `.exe`s, relative to the workspace.
    #[serde(default = "default_windows_dir")]
    pub dir: String,
    /// Binary basenames (without `.exe`).
    pub binaries: Vec<String>,
    /// osslsigncode `-n` program name.
    pub sign_name: String,
    /// osslsigncode `-i` info URL.
    pub sign_url: String,
    /// RFC3161 timestamp URL.
    #[serde(default = "default_timestamp_url")]
    pub timestamp_url: String,
    /// tar.gz archive name with `{version}` placeholder.
    pub tar_archive: String,
    /// zip archive name with `{version}` placeholder.
    pub zip_archive: String,
    #[serde(default = "default_windows_pfx_secret")]
    pub pfx_secret: String,
    #[serde(default = "default_windows_pass_secret")]
    pub pass_secret: String,
    #[serde(default = "default_windows_subject_secret")]
    pub subject_secret: String,
}

fn default_windows_dir() -> String {
    "release/windows-x86_64".to_owned()
}
fn default_timestamp_url() -> String {
    "http://timestamp.digicert.com".to_owned()
}
fn default_windows_pfx_secret() -> String {
    "WINDOWS_SIGNING_PFX".to_owned()
}
fn default_windows_pass_secret() -> String {
    "WINDOWS_SIGNING_PASS".to_owned()
}
fn default_windows_subject_secret() -> String {
    "WINDOWS_SIGNING_SUBJECT".to_owned()
}

/// `[release.artifacts]` — release workflow build + signing configuration.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactsConfig {
    /// Opt into the conventional rs-harbor `release-bundle` output.
    ///
    /// This is deliberately a small project-facing switch. Projects may keep
    /// an explicit `nix_bundle_attrs` list when they publish more than one
    /// bundle; the explicit list wins in that case.
    #[serde(default)]
    pub prebuild_binaries: bool,
    /// Runner label for the release job; defaults to the CI runner or `atlas`.
    pub runner: Option<String>,
    /// `nix.conf` substituters added in the install-nix step.
    #[serde(default)]
    pub substituters: Vec<String>,
    /// `nix.conf` trusted-public-keys added in the install-nix step.
    #[serde(default)]
    pub trusted_public_keys: Vec<String>,
    /// Flake attribute whose `.version` must equal the tag
    /// (`nix eval --raw .#<attr>.version`). Skipped when unset.
    pub version_attr: Option<String>,
    /// Optional supply-chain gate command run before building.
    pub supply_chain_command: Option<String>,
    /// Build-step body lines, emitted verbatim (project-specific).
    #[serde(default)]
    pub build_commands: Vec<String>,
    /// Explicit Nix output attributes that produce flat release bundles.
    ///
    /// Each bundle must contain regular top-level release files and exactly
    /// one `*-release-manifest.json` whose version matches the release tag.
    /// Project-specific `build_commands` remain additive for formats that
    /// cannot be represented by the generic bundle contract.
    #[serde(default)]
    pub nix_bundle_attrs: Vec<String>,
    /// SBOM / supply-chain report command lines, emitted verbatim before the
    /// build step when non-empty.
    #[serde(default)]
    pub sbom_commands: Vec<String>,
    /// `sha256sum` arguments (globs relative to `release/`) for SHA256SUMS.txt.
    #[serde(default)]
    pub checksum_globs: Vec<String>,
    /// Committed minisign public key used to verify the signed checksums.
    #[serde(default = "default_minisign_pub")]
    pub minisign_pub: String,
    /// Skip artifact signing (minisign + cosign) when false.
    #[serde(default = "default_true")]
    pub sign: bool,
}

impl ArtifactsConfig {
    /// Resolve the Nix outputs that the release workflow must build.
    pub fn effective_nix_bundle_attrs(&self) -> Vec<String> {
        if !self.nix_bundle_attrs.is_empty() {
            self.nix_bundle_attrs.clone()
        } else if self.prebuild_binaries {
            vec!["release-bundle".to_owned()]
        } else {
            Vec::new()
        }
    }
}

fn default_minisign_pub() -> String {
    "keys/minisign.pub".to_owned()
}

/// `[release.attic]` — push built Nix closures to an Attic cache.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AtticConfig {
    /// Cache name, e.g. `canix`.
    pub cache: String,
    /// Attic server URL.
    pub url: String,
    /// Env var holding the directory of per-project token files.
    #[serde(default = "default_attic_token_dir_env")]
    pub token_dir_env: String,
    /// Token file name within `$<token_dir_env>`.
    pub token_name: String,
    /// Optional Actions secret used by GitHub-hosted prebuild jobs.
    #[serde(default)]
    pub token_secret: Option<String>,
    /// `--out-link` result paths pushed to the cache.
    #[serde(default)]
    pub result_links: Vec<String>,
}

fn default_attic_token_dir_env() -> String {
    "ATTIC_TOKENS_DIR".to_owned()
}

/// `[release.codeberg]` — create the Codeberg/Forgejo release and upload assets.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodebergReleaseConfig {
    /// `<owner>/<repo>` whose release receives the uploaded assets.
    pub repo: String,
    /// REST API base URL.
    #[serde(default = "default_codeberg_api_base")]
    pub api_base: String,
    /// CI secret holding the API token.
    #[serde(default = "default_codeberg_token_secret")]
    pub token_secret: String,
    /// `target_commitish` the release tag points at.
    #[serde(default = "default_release_target_branch")]
    pub target_branch: String,
    /// Use `CHANGELOG.md` as the release body.
    #[serde(default = "default_true")]
    pub body_from_changelog: bool,
}

/// `[release.github]` — create a GitHub release and upload assets.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GithubReleaseConfig {
    /// `<owner>/<repo>` whose release receives the uploaded assets.
    pub repo: String,
    /// REST API base URL.
    #[serde(default = "default_github_api_base")]
    pub api_base: String,
    /// CI secret holding the API token. Defaults to GitHub's built-in token.
    #[serde(default = "default_github_token_secret")]
    pub token_secret: String,
    /// `target_commitish` the release tag points at.
    #[serde(default = "default_release_target_branch")]
    pub target_branch: String,
    /// Use `CHANGELOG.md` as the release body.
    #[serde(default = "default_true")]
    pub body_from_changelog: bool,
}

fn default_release_target_branch() -> String {
    "main".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCodebergRelease {
    pub repo: String,
    pub api_base: String,
    pub token_secret: String,
    pub target_branch: String,
    pub body_from_changelog: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseProvider {
    Forgejo,
    Github,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReleaseTarget {
    pub provider: ReleaseProvider,
    pub repo: String,
    pub api_base: String,
    pub token_secret: String,
    pub target_branch: String,
    pub body_from_changelog: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCodebergPages {
    pub repo: String,
    pub owner: String,
    pub canonical_domain: Option<String>,
    pub site_output: String,
    pub token_secret: String,
    pub source_branch: String,
    pub deploy_app: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVscode {
    pub extension_dir: String,
    pub runner: Option<String>,
    pub codeberg_repo: String,
    pub codeberg_owner: String,
    pub codeberg_api_base: String,
    pub codeberg_token_secret: String,
    pub pat_source: VscodePatSource,
    pub vsce_pat_file_env: String,
    pub ovsx_pat_file_env: String,
    pub vsce_pat_secret: String,
    pub ovsx_pat_secret: String,
    pub package_command: String,
    pub cargo_package: Option<String>,
    pub prepublish_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJetbrains {
    pub plugin_dir: String,
    pub plugin_xml_id: String,
    pub package_installable: String,
    pub runner: Option<String>,
    pub cargo_package: Option<String>,
    pub channel: Option<String>,
    pub credential_source: JetbrainsCredentialSource,
    pub marketplace_token_file_env: String,
    pub certificate_chain_file_env: String,
    pub private_key_file_env: String,
    pub private_key_password_file_env: String,
    pub marketplace_token_secret: String,
    pub certificate_chain_secret: String,
    pub private_key_secret: String,
    pub private_key_password_secret: String,
    pub prepublish_commands: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSigningConfig {
    /// OpenPGP fingerprint used to sign and verify release tags.
    pub key: Option<String>,

    /// Public keyring committed for CI-side `git verify-tag`.
    #[serde(default = "default_release_trust_root")]
    pub trust_root: String,

    /// Whether release publishing must have a usable signing trust root.
    #[serde(default = "default_true")]
    pub required: bool,
}

impl Default for ReleaseSigningConfig {
    fn default() -> Self {
        Self {
            key: None,
            trust_root: default_release_trust_root(),
            required: true,
        }
    }
}

fn default_release_trust_root() -> String {
    "keys/maintainers.gpg".to_owned()
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSmokeConfig {
    /// Command run after artifact signing/provenance and before publishing.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HomebrewConfig {
    /// Formula name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Binaries to install. If omitted, defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Tap repo URL. Required when `[homebrew]` is present.
    pub tap_url: String,

    /// Actions secret sourced into `HOMEBREW_TAP_TOKEN`.
    #[serde(default = "default_homebrew_tap_token_secret")]
    pub tap_token_secret: String,

    /// Description for the formula. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Homepage URL. If omitted, derived from Cargo metadata.
    pub homepage: Option<String>,

    /// SPDX license identifier. If omitted, derived from Cargo metadata.
    pub license: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`, `{os}`.
    #[serde(default = "default_archive_pattern")]
    pub archive_pattern: String,

    /// Per-platform enable flags. Each defaults to true.
    #[serde(default)]
    pub platforms: HomebrewPlatformsConfig,
}

fn default_archive_pattern() -> String {
    "{name}-{version}-{arch}-{os}.tar.gz".to_owned()
}

fn default_homebrew_tap_token_secret() -> String {
    "homebrew_tap_token".to_owned()
}

fn default_windows_archive_pattern() -> String {
    "{name}-{version}-{arch}-windows.zip".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HomebrewPlatformsConfig {
    #[serde(default = "default_true")]
    pub darwin_arm: bool,
    #[serde(default)]
    pub darwin_intel: bool,
    #[serde(default = "default_true")]
    pub linux_arm: bool,
    #[serde(default = "default_true")]
    pub linux_intel: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChocolateyConfig {
    /// Chocolatey package display name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Nuspec package identifier; if omitted, derived from the resolved name.
    pub id: Option<String>,

    /// Nuspec title; if omitted, derived from the resolved name.
    pub title: Option<String>,

    /// Nuspec authors. If omitted, derived from Cargo package authors when available.
    pub authors: Option<String>,

    /// Nuspec description. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Nuspec summary.
    pub summary: Option<String>,

    /// Project URL. If omitted, derived from Cargo package homepage.
    pub project_url: Option<String>,

    /// License URL.
    pub license_url: Option<String>,

    /// Package icon URL.
    pub icon_url: Option<String>,

    /// URL for the package source.
    pub package_source_url: Option<String>,

    /// Documentation URL.
    pub docs_url: Option<String>,

    /// Bug tracker URL.
    pub bug_tracker_url: Option<String>,

    /// Project source URL.
    pub project_source_url: Option<String>,

    /// Space-separated Chocolatey tags.
    pub tags: Option<String>,

    /// Release notes URL.
    pub release_notes_url: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`.
    #[serde(default = "default_windows_archive_pattern")]
    pub archive_pattern: String,

    /// Chocolatey push settings.
    #[serde(default)]
    pub push: ChocolateyPushConfig,

    /// `nix shell` packages providing `choco` (and `simit`) for the generated
    /// release step. Lets a project point at a fork that already ships the
    /// chocolatey package — e.g.
    /// `github:caniko/nixpkgs/add-chocolatey-scoop#chocolatey github:caniko/simit`
    /// — until it lands in upstream nixpkgs. Defaults to `nixpkgs#chocolatey`.
    #[serde(default = "default_chocolatey_nix_tool")]
    pub nix_tool: String,

    /// Env var the publish step reads the push API key from (passed to
    /// `--api-key-env`). Defaults to `CHOCOLATEY_API_KEY`.
    #[serde(default = "default_chocolatey_api_key_env")]
    pub api_key_env: String,

    /// Actions secret sourced into `api_key_env`. Defaults to
    /// `chocolatey_api_key`. Ignored when `api_key_from_runner` is true.
    #[serde(default = "default_chocolatey_api_key_secret")]
    pub api_key_secret: String,

    /// When true, do not source the key from an Actions secret — assume the
    /// forge runner already provides `api_key_env` in the job environment (e.g.
    /// a Forgejo runner credential exposed as a container env var).
    #[serde(default)]
    pub api_key_from_runner: bool,
}

fn default_chocolatey_nix_tool() -> String {
    "nixpkgs#chocolatey".to_owned()
}

fn default_chocolatey_api_key_env() -> String {
    "CHOCOLATEY_API_KEY".to_owned()
}

fn default_chocolatey_api_key_secret() -> String {
    "chocolatey_api_key".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChocolateyPushConfig {
    #[serde(default = "default_chocolatey_push_source")]
    pub source: String,
}

impl Default for ChocolateyPushConfig {
    fn default() -> Self {
        Self {
            source: default_chocolatey_push_source(),
        }
    }
}

fn default_chocolatey_push_source() -> String {
    "https://push.chocolatey.org/".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScoopConfig {
    /// Scoop manifest name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Bucket repo URL. Required when `[scoop]` is present.
    pub bucket_url: String,

    /// Actions secret sourced into `SCOOP_BUCKET_TOKEN`.
    #[serde(default = "default_scoop_bucket_token_secret")]
    pub bucket_token_secret: String,

    /// Manifest description. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Manifest homepage. If omitted, derived from Cargo metadata.
    pub homepage: Option<String>,

    /// Manifest license. If omitted, derived from Cargo metadata.
    pub license: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`.
    #[serde(default = "default_windows_archive_pattern")]
    pub archive_pattern: String,

    /// Binaries to expose. If omitted, defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Per-architecture enable flags. Each defaults to true.
    #[serde(default)]
    pub architectures: ScoopArchSet,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScoopArchSet {
    #[serde(default = "default_true")]
    pub x64: bool,
    #[serde(default = "default_true")]
    pub arm64: bool,
}

impl ScoopArchSet {
    pub fn any_enabled(&self) -> bool {
        self.x64 || self.arm64
    }
}

impl Default for ScoopArchSet {
    fn default() -> Self {
        Self {
            x64: true,
            arm64: true,
        }
    }
}

fn default_scoop_bucket_token_secret() -> String {
    "SCOOP_BUCKET_TOKEN".to_owned()
}

impl HomebrewPlatformsConfig {
    pub fn any_enabled(&self) -> bool {
        self.darwin_arm || self.darwin_intel || self.linux_arm || self.linux_intel
    }
}

impl Default for HomebrewPlatformsConfig {
    fn default() -> Self {
        Self {
            darwin_arm: true,
            darwin_intel: false,
            linux_arm: true,
            linux_intel: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_aur_arch() -> String {
    "x86_64".to_owned()
}

fn default_aur_ssh_remote() -> String {
    "ssh://aur@aur.archlinux.org".to_owned()
}

fn default_aur_ssh_key_secret() -> String {
    "AUR_SSH_KEY".to_owned()
}

fn default_aur_source_archive_pattern() -> String {
    "{repo}-{version}.tar.gz".to_owned()
}

fn default_aur_binary_archive_pattern() -> String {
    "{name}-{version}-x86_64-linux.tar.gz".to_owned()
}

fn default_codeberg_api_base() -> String {
    "https://codeberg.org/api/v1".to_owned()
}

fn default_codeberg_token_secret() -> String {
    "codeberg_token".to_owned()
}

fn default_github_api_base() -> String {
    "https://api.github.com".to_owned()
}

fn default_github_token_secret() -> String {
    "GITHUB_TOKEN".to_owned()
}

fn default_pages_source_branch() -> String {
    "trunk".to_owned()
}

fn default_pages_deploy_app() -> String {
    ".#deploy-pages".to_owned()
}

fn default_pages_site_output() -> String {
    ".#site".to_owned()
}

fn default_copr_spec_path() -> Option<String> {
    None
}

fn default_apt_branch() -> String {
    "pages".to_owned()
}

fn default_apt_distribution() -> String {
    "stable".to_owned()
}

fn default_apt_debian_release() -> String {
    "bookworm".to_owned()
}

fn default_aur_doc_changelog() -> Option<String> {
    Some("CHANGELOG.md".to_owned())
}

fn default_aur_license_file() -> Option<String> {
    Some("LICENSE".to_owned())
}

fn default_aur_readme() -> Option<String> {
    Some("README.md".to_owned())
}

/// `[aur]` — Arch User Repository packaging across source/binary/VCS flavors.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AurConfig {
    /// pkgbase / source pkgname; if omitted, derived from the Cargo package name.
    pub name: Option<String>,
    /// `pkgdesc`; if omitted, derived from Cargo metadata.
    pub description: Option<String>,
    /// Project `url`; if omitted, derived from Cargo package homepage.
    pub url: Option<String>,
    /// SPDX `license`; if omitted, derived from Cargo metadata.
    pub license: Option<String>,
    /// Maintainer line, e.g. `Name <email>`. Emitted as a `# Maintainer:` comment.
    pub maintainer: Option<String>,
    /// Maintainer OpenPGP fingerprint, emitted as a `# Maintainer GPG key:` comment.
    pub maintainer_gpg: Option<String>,
    #[serde(default = "default_aur_arch")]
    pub arch: String,
    /// Runtime `depends`.
    #[serde(default)]
    pub depends: Vec<String>,
    /// Build-time `makedepends` for the source/VCS flavors (e.g. cargo, rust, cmake).
    #[serde(default)]
    pub makedepends: Vec<String>,
    /// Minimum glibc for the `-bin` flavor; when set, `glibc` becomes `glibc>=<min>`.
    pub bin_glibc_min: Option<String>,
    /// Binaries installed into `/usr/bin`. If omitted, defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,
    /// Extra non-binary install assets (desktop/icon/metainfo, ...).
    #[serde(default)]
    pub assets: Vec<AurAsset>,
    /// `LICENSE` file installed into `/usr/share/licenses/<pkg>/`.
    #[serde(default = "default_aur_license_file")]
    pub license_file: Option<String>,
    /// README installed into `/usr/share/doc/<pkg>/`.
    #[serde(default = "default_aur_readme")]
    pub readme: Option<String>,
    /// CHANGELOG installed into `/usr/share/doc/<pkg>/`.
    #[serde(default = "default_aur_doc_changelog")]
    pub changelog: Option<String>,
    /// Codeberg/GitHub `<owner>/<repo>` used for release-download source URLs.
    pub download_repo: String,
    /// Source tarball pattern with `{repo}`, `{version}`.
    #[serde(default = "default_aur_source_archive_pattern")]
    pub source_archive_pattern: String,
    /// Prebuilt-binary archive pattern with `{name}`, `{version}`.
    #[serde(default = "default_aur_binary_archive_pattern")]
    pub binary_archive_pattern: String,
    /// `git+` source URL for the `-git` flavor; defaults to `https://<host>/<repo>.git`.
    pub git_url: Option<String>,
    /// Which flavors to emit. Each defaults to true.
    #[serde(default)]
    pub flavors: AurFlavors,
    /// SSH remote base for publishing, e.g. `ssh://aur@aur.archlinux.org`.
    #[serde(default = "default_aur_ssh_remote")]
    pub ssh_remote: String,
    /// CI secret holding the AUR SSH private key.
    #[serde(default = "default_aur_ssh_key_secret")]
    pub ssh_key_secret: String,
    /// Skip AUR publishing for prerelease versions.
    #[serde(default = "default_true")]
    pub stable_only: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AurAsset {
    /// Source path relative to the extracted tree.
    pub source: String,
    /// Install destination under `$pkgdir`.
    pub dest: String,
    /// Install mode; defaults to `644`.
    #[serde(default = "default_asset_mode")]
    pub mode: String,
}

fn default_asset_mode() -> String {
    "644".to_owned()
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AurFlavors {
    #[serde(default = "default_true")]
    pub source: bool,
    #[serde(default = "default_true")]
    pub bin: bool,
    #[serde(default = "default_true")]
    pub git: bool,
}

impl AurFlavors {
    pub fn any_enabled(&self) -> bool {
        self.source || self.bin || self.git
    }
}

impl Default for AurFlavors {
    fn default() -> Self {
        Self {
            source: true,
            bin: true,
            git: true,
        }
    }
}

/// `[copr]` — Fedora COPR packaging (RPM spec + `.copr/Makefile` SRPM build).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoprConfig {
    /// `%global crate` / `Name`; if omitted, derived from the Cargo package name.
    pub name: Option<String>,
    /// `Summary`; if omitted, derived from Cargo metadata.
    pub summary: Option<String>,
    /// Longer `%description` prose; if omitted, falls back to the summary.
    pub description: Option<String>,
    /// SPDX `License`; if omitted, derived from Cargo metadata.
    pub license: Option<String>,
    /// Project `URL`; if omitted, derived from Cargo package homepage.
    pub url: Option<String>,
    /// Codeberg/GitHub `<owner>/<repo>` used for the source-archive URL.
    pub download_repo: String,
    /// `BuildRequires` entries.
    #[serde(default)]
    pub build_requires: Vec<String>,
    /// Binaries installed by `%install` into `%{_bindir}`. Defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,
    /// Spec path relative to the workspace root; defaults to `<name>.spec`.
    #[serde(default = "default_copr_spec_path")]
    pub spec_path: Option<String>,
    /// COPR project for stable releases, e.g. `owner/project`.
    pub project: Option<String>,
    /// COPR project for prereleases; defaults to `<project>-testing`.
    pub testing_project: Option<String>,
    #[serde(default = "default_copr_login_secret")]
    pub login_secret: String,
    #[serde(default = "default_copr_username_secret")]
    pub username_secret: String,
    #[serde(default = "default_copr_token_secret")]
    pub token_secret: String,
    /// Nix installable that provides `copr-cli` for publish steps.
    #[serde(default = "default_copr_nix_tool")]
    pub nix_tool: String,
}

fn default_copr_login_secret() -> String {
    "copr_login".to_owned()
}

fn default_copr_username_secret() -> String {
    "copr_username".to_owned()
}

fn default_copr_token_secret() -> String {
    "copr_token".to_owned()
}

fn default_copr_nix_tool() -> String {
    "nixpkgs#copr-cli".to_owned()
}

/// `[apt]` — Debian packaging published via reprepro to a Codeberg Pages repo.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AptConfig {
    /// Git remote of the apt repository, e.g.
    /// `ssh://git@codeberg.org/<owner>/<name>-apt.git`.
    pub repo_url: String,
    /// Public URL served by the package repository deployment.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Pages deployment settings for a repository-backed APT site.
    #[serde(default)]
    pub pages: AptPagesConfig,
    /// Branch served by Codeberg Pages.
    #[serde(default = "default_apt_branch")]
    pub branch: String,
    /// reprepro `Codename`/distribution, e.g. `stable`.
    #[serde(default = "default_apt_distribution")]
    pub distribution: String,
    /// Human label for the `Origin`/`Label`; defaults to the package name.
    pub label: Option<String>,
    /// reprepro architectures line; defaults to `amd64`.
    #[serde(default = "default_apt_architectures")]
    pub architectures: String,
    /// reprepro components line; defaults to `main`.
    #[serde(default = "default_apt_components")]
    pub components: String,
    /// Debian release used by legacy downstream configuration.
    ///
    /// Generated release flows build Debian packages through the project
    /// devshell and `cargo-deb`; they do not require debootstrap or sudo.
    #[serde(default = "default_apt_debian_release")]
    pub debian_release: String,
    /// Cargo packages built with `cargo deb -p <pkg>`.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Legacy Debian build dependencies retained for configuration
    /// compatibility. Generated local release flows do not create a chroot.
    #[serde(default)]
    pub build_deps: Vec<String>,
    /// `cargo-deb` version expected by legacy downstream configuration.
    #[serde(default = "default_cargo_deb_version")]
    pub cargo_deb_version: String,
    #[serde(default = "default_apt_gpg_key_secret")]
    pub gpg_key_secret: String,
    #[serde(default = "default_apt_gpg_key_id_secret")]
    pub gpg_key_id_secret: String,
    #[serde(default = "default_apt_gpg_passphrase_secret")]
    pub gpg_passphrase_secret: String,
    #[serde(default = "default_apt_ssh_key_secret")]
    pub ssh_key_secret: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AptPagesConfig {
    /// Pages provider used by `simit init apt-repo`.
    #[serde(default = "default_apt_pages_provider")]
    pub provider: String,
    /// Runner label for the tiny Pages deployment workflow.
    #[serde(default)]
    pub runner: Option<String>,
}

impl Default for AptPagesConfig {
    fn default() -> Self {
        Self {
            provider: default_apt_pages_provider(),
            runner: None,
        }
    }
}

fn default_apt_pages_provider() -> String {
    "codeberg-git-pages".to_owned()
}

fn default_apt_architectures() -> String {
    "amd64".to_owned()
}

fn default_apt_components() -> String {
    "main".to_owned()
}

fn default_apt_gpg_key_secret() -> String {
    "apt_repo_gpg_key".to_owned()
}

fn default_apt_gpg_key_id_secret() -> String {
    "apt_repo_gpg_key_id".to_owned()
}

fn default_apt_gpg_passphrase_secret() -> String {
    "apt_repo_gpg_passphrase".to_owned()
}

fn default_apt_ssh_key_secret() -> String {
    "apt_repo_ssh_key".to_owned()
}

fn default_cargo_deb_version() -> String {
    "2.5.0".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHomebrew {
    pub name: String,
    pub binaries: Vec<String>,
    pub tap_url: String,
    pub tap_token_secret: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub download_repo: String,
    pub archive_pattern: String,
    pub platforms: HomebrewPlatformsConfig,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HomebrewOverrides<'a> {
    pub name: Option<&'a str>,
    pub binaries: Option<&'a [String]>,
    pub tap_url: Option<&'a str>,
    pub description: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub license: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub disabled_platforms: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChocolatey {
    pub name: String,
    pub id: String,
    pub title: String,
    pub authors: Option<String>,
    pub description: String,
    pub summary: Option<String>,
    pub project_url: String,
    pub license_url: Option<String>,
    pub icon_url: Option<String>,
    pub package_source_url: Option<String>,
    pub docs_url: Option<String>,
    pub bug_tracker_url: Option<String>,
    pub project_source_url: Option<String>,
    pub tags: Option<String>,
    pub release_notes_url: Option<String>,
    pub download_repo: String,
    pub archive_pattern: String,
    pub push: ChocolateyPushConfig,
    pub nix_tool: String,
    pub api_key_env: String,
    pub api_key_secret: String,
    pub api_key_from_runner: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChocolateyOverrides<'a> {
    pub name: Option<&'a str>,
    pub id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub authors: Option<&'a str>,
    pub description: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub project_url: Option<&'a str>,
    pub license_url: Option<&'a str>,
    pub icon_url: Option<&'a str>,
    pub package_source_url: Option<&'a str>,
    pub docs_url: Option<&'a str>,
    pub bug_tracker_url: Option<&'a str>,
    pub project_source_url: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub release_notes_url: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub push_source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScoop {
    pub name: String,
    pub bucket_url: String,
    pub bucket_token_secret: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub download_repo: String,
    pub archive_pattern: String,
    pub binaries: Vec<String>,
    pub architectures: ScoopArchSet,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScoopOverrides<'a> {
    pub name: Option<&'a str>,
    pub bucket_url: Option<&'a str>,
    pub description: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub license: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub binaries: Option<&'a [String]>,
    pub disabled_architectures: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAur {
    pub name: String,
    pub description: String,
    pub url: String,
    pub license: String,
    pub maintainer: Option<String>,
    pub maintainer_gpg: Option<String>,
    pub arch: String,
    pub depends: Vec<String>,
    pub makedepends: Vec<String>,
    pub bin_glibc_min: Option<String>,
    pub binaries: Vec<String>,
    pub assets: Vec<AurAsset>,
    pub license_file: Option<String>,
    pub readme: Option<String>,
    pub changelog: Option<String>,
    pub download_repo: String,
    /// Repository basename (the `<repo>` of `<owner>/<repo>`).
    pub repo: String,
    pub source_archive_pattern: String,
    pub binary_archive_pattern: String,
    pub git_url: String,
    pub flavors: AurFlavors,
    pub ssh_remote: String,
    pub ssh_key_secret: String,
    pub stable_only: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AurOverrides<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub url: Option<&'a str>,
    pub license: Option<&'a str>,
    pub download_repo: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCopr {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub license: String,
    pub url: String,
    pub download_repo: String,
    pub repo: String,
    pub build_requires: Vec<String>,
    pub binaries: Vec<String>,
    pub spec_path: String,
    pub project: Option<String>,
    pub testing_project: Option<String>,
    pub login_secret: String,
    pub username_secret: String,
    pub token_secret: String,
    pub nix_tool: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CoprOverrides<'a> {
    pub name: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub description: Option<&'a str>,
    pub license: Option<&'a str>,
    pub url: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub project: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedApt {
    pub label: String,
    pub repo_url: String,
    pub public_url: Option<String>,
    pub pages_provider: String,
    pub pages_runner: Option<String>,
    pub branch: String,
    pub distribution: String,
    pub architectures: String,
    pub components: String,
    pub debian_release: String,
    pub packages: Vec<String>,
    pub build_deps: Vec<String>,
    pub cargo_deb_version: String,
    pub gpg_key_secret: String,
    pub gpg_key_id_secret: String,
    pub gpg_passphrase_secret: String,
    pub ssh_key_secret: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AptOverrides<'a> {
    pub repo_url: Option<&'a str>,
    pub label: Option<&'a str>,
    pub public_url: Option<&'a str>,
}

impl ProjectConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let sources = Self::load_sources(workspace_root)?;
        match sources.as_slice() {
            [] => Ok(Self::default()),
            [source] => {
                source.config.validate_common()?;
                Ok(source.config.clone())
            }
            _ => {
                let labels = sources
                    .iter()
                    .map(|source| source.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "multiple simit project config sources found ({labels}); keep exactly one of simit.toml, Cargo.toml [workspace.metadata.simit], Cargo.toml [package.metadata.simit], or flake outputs.simitConfig"
                )
            }
        }
    }

    fn validate_common(&self) -> Result<()> {
        if self.ci.workspace && !self.ci.packages.is_empty() {
            bail!("simit project config: [ci].workspace cannot be true when [ci].packages is set");
        }
        validate_nonempty_strings("simit project config: [ci].packages", &self.ci.packages)?;
        validate_nonempty_strings("simit project config: [ci].nix_builds", &self.ci.nix_builds)?;
        if self
            .ci
            .nix_builds
            .iter()
            .any(|installable| installable.contains('\n') || installable.contains('\r'))
        {
            bail!("simit project config: [ci].nix_builds must contain single-line installables");
        }
        validate_runner_label_opt("[ci].runner", self.ci.runner.as_deref())?;
        validate_runner_label_opt("[ci].windows_runner", self.ci.windows_runner.as_deref())?;
        validate_nix_system_runners(&self.ci)?;
        if let Some(token_secret) = self
            .release
            .attic
            .as_ref()
            .and_then(|attic| attic.token_secret.as_deref())
        {
            validate_github_actions_secret_identifier(
                "simit project config: [release.attic].token_secret",
                token_secret,
            )?;
        }
        if let Some(prebuild) = &self.prebuild {
            if prebuild.effective_system_runners(&self.ci).is_empty() {
                bail!(
                    "simit project config: [prebuild] requires [prebuild.system_runners] or [ci.nix_system_runners]"
                );
            }
            validate_system_runner_map(
                "simit project config: [prebuild].system_runners",
                &prebuild.system_runners,
            )?;
            if self.ci.nix_builds.is_empty()
                && !prebuild.release_archives
                && prebuild.attic_app.is_none()
            {
                bail!(
                    "simit project config: [prebuild] requires [ci].nix_builds, release_archives = true, or attic_app"
                );
            }
            if prebuild.release_archives
                && self
                    .release
                    .artifacts
                    .effective_nix_bundle_attrs()
                    .is_empty()
            {
                bail!(
                    "simit project config: [prebuild].release_archives requires release.artifacts Nix bundle outputs"
                );
            }
            if prebuild.publish_attic {
                let attic_app = prebuild.attic_app.as_deref().context(
                    "simit project config: [prebuild].publish_attic requires [prebuild].attic_app",
                )?;
                validate_nonempty_string("simit project config: [prebuild].attic_app", attic_app)?;
                let attic = self.release.attic.as_ref().context(
                    "simit project config: [prebuild].publish_attic requires [release.attic]",
                )?;
                attic.token_secret.as_deref().context(
                    "simit project config: [prebuild].publish_attic requires [release.attic].token_secret",
                )?;
            }
        }
        if let Some(image) = &self.ci.crow.image {
            validate_nonempty_string("simit project config: [ci.crow].image", image)?;
        }
        if let Some(image) = &self.ci.crow.nix_image {
            validate_nonempty_string("simit project config: [ci.crow].nix_image", image)?;
        }
        if let Some(base) = &self.ci.crow.workspace_base {
            validate_nonempty_string("simit project config: [ci.crow].workspace_base", base)?;
        }
        for (key, value) in &self.ci.crow.labels {
            if key.trim().is_empty()
                || value.trim().is_empty()
                || key.contains('\n')
                || value.contains('\n')
            {
                bail!(
                    "simit project config: [ci.crow].labels must contain non-empty single-line keys and values"
                );
            }
        }
        for (name, variable) in &self.ci.crow.variables {
            validate_nonempty_string("simit project config: [ci.crow.variables] name", name)?;
            if let Some(description) = &variable.description {
                validate_nonempty_string(
                    "simit project config: [ci.crow.variables].description",
                    description,
                )?;
            }
            validate_nonempty_strings(
                "simit project config: [ci.crow.variables].options",
                &variable.options,
            )?;
            if let Some(default) = &variable.default {
                validate_nonempty_string(
                    "simit project config: [ci.crow.variables].default",
                    default,
                )?;
            }
        }
        validate_nonempty_strings(
            "simit project config: [flake.expected_outputs].packages",
            &self.flake.expected_outputs.packages,
        )?;
        validate_nonempty_strings(
            "simit project config: [flake.expected_outputs].apps",
            &self.flake.expected_outputs.apps,
        )?;
        validate_nonempty_strings(
            "simit project config: [flake.expected_outputs].dev_shells",
            &self.flake.expected_outputs.dev_shells,
        )?;
        validate_nonempty_strings(
            "simit project config: [flake.expected_outputs].checks",
            &self.flake.expected_outputs.checks,
        )?;
        validate_nonempty_strings(
            "simit project config: [flake.expected_outputs].top_level",
            &self.flake.expected_outputs.top_level,
        )?;
        validate_nonempty_strings(
            "simit project config: [ci].required_secrets",
            &self.ci.required_secrets,
        )?;
        validate_nonempty_strings(
            "simit project config: [ci].required_env",
            &self.ci.required_env,
        )?;
        validate_nonempty_strings(
            "simit project config: [ci].extra_setup",
            &self.ci.extra_setup,
        )?;
        if let Some(secret) = &self.ci.pypi_token_secret {
            validate_secret_name("simit project config: [ci].pypi_token_secret", secret)?;
        }
        if self.ci.pypi_trusted_publishing && self.ci.pypi_token_secret.is_some() {
            bail!(
                "simit project config: [ci].pypi_trusted_publishing and [ci].pypi_token_secret are mutually exclusive"
            );
        }
        if let Some(pages) = &self.ci.pages {
            validate_owner_repo("simit project config: [ci.pages].repo", &pages.repo)?;
            if let Some(canonical_domain) = &pages.canonical_domain {
                validate_nonempty_string(
                    "simit project config: [ci.pages].canonical_domain",
                    canonical_domain,
                )?;
            }
            validate_nonempty_string(
                "simit project config: [ci.pages].site_output",
                &pages.site_output,
            )?;
            validate_nonempty_string(
                "simit project config: [ci.pages].token_secret",
                &pages.token_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [ci.pages].source_branch",
                &pages.source_branch,
            )?;
            validate_nonempty_string(
                "simit project config: [ci.pages].deploy_app",
                &pages.deploy_app,
            )?;
        }
        if let Some(vscode) = &self.vscode {
            validate_nonempty_string(
                "simit project config: [vscode].extension_dir",
                &vscode.extension_dir,
            )?;
            validate_runner_label_opt("[vscode].runner", vscode.runner.as_deref())?;
            validate_owner_repo(
                "simit project config: [vscode].codeberg_repo",
                &vscode.codeberg_repo,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].codeberg_api_base",
                &vscode.codeberg_api_base,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].codeberg_token_secret",
                &vscode.codeberg_token_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].vsce_pat_file_env",
                &vscode.vsce_pat_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].ovsx_pat_file_env",
                &vscode.ovsx_pat_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].vsce_pat_secret",
                &vscode.vsce_pat_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].ovsx_pat_secret",
                &vscode.ovsx_pat_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [vscode].package_command",
                &vscode.package_command,
            )?;
            if let Some(cargo_package) = &vscode.cargo_package {
                validate_nonempty_string(
                    "simit project config: [vscode].cargo_package",
                    cargo_package,
                )?;
            }
            validate_nonempty_strings(
                "simit project config: [vscode].prepublish_commands",
                &vscode.prepublish_commands,
            )?;
        }
        if let Some(jetbrains) = &self.jetbrains {
            validate_nonempty_string(
                "simit project config: [jetbrains].plugin_dir",
                &jetbrains.plugin_dir,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].plugin_xml_id",
                &jetbrains.plugin_xml_id,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].package_installable",
                &jetbrains.package_installable,
            )?;
            validate_runner_label_opt("[jetbrains].runner", jetbrains.runner.as_deref())?;
            if let Some(cargo_package) = &jetbrains.cargo_package {
                validate_nonempty_string(
                    "simit project config: [jetbrains].cargo_package",
                    cargo_package,
                )?;
            }
            if let Some(channel) = &jetbrains.channel {
                validate_nonempty_string("simit project config: [jetbrains].channel", channel)?;
            }
            validate_nonempty_string(
                "simit project config: [jetbrains].marketplace_token_file_env",
                &jetbrains.marketplace_token_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].certificate_chain_file_env",
                &jetbrains.certificate_chain_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].private_key_file_env",
                &jetbrains.private_key_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].private_key_password_file_env",
                &jetbrains.private_key_password_file_env,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].marketplace_token_secret",
                &jetbrains.marketplace_token_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].certificate_chain_secret",
                &jetbrains.certificate_chain_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].private_key_secret",
                &jetbrains.private_key_secret,
            )?;
            validate_nonempty_string(
                "simit project config: [jetbrains].private_key_password_secret",
                &jetbrains.private_key_password_secret,
            )?;
            validate_nonempty_strings(
                "simit project config: [jetbrains].prepublish_commands",
                &jetbrains.prepublish_commands,
            )?;
        }
        for (key, value) in &self.ci.extra_env {
            if key.trim().is_empty() {
                bail!("simit project config: [ci].extra_env keys must not be empty");
            }
            if key.contains('\n') || value.contains('\n') {
                bail!(
                    "simit project config: [ci].extra_env must not contain multiline keys or values"
                );
            }
        }
        Ok(())
    }

    pub fn write_ci(workspace_root: &Path, ci: &CiConfig) -> Result<bool> {
        let sources = Self::load_sources(workspace_root)?;
        if sources.is_empty() && *ci == CiConfig::default() {
            return Ok(false);
        }
        match sources.as_slice() {
            [] => {}
            [source] if source.label == "simit.toml" => {}
            [source] => {
                bail!(
                    "cannot persist [ci] into simit.toml while project config comes from {}; move project config into simit.toml first",
                    source.label
                );
            }
            _ => {
                let labels = sources
                    .iter()
                    .map(|source| source.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "multiple simit project config sources found ({labels}); keep exactly one of simit.toml, Cargo.toml [workspace.metadata.simit], Cargo.toml [package.metadata.simit], or flake outputs.simitConfig"
                );
            }
        }

        if sources.len() == 1 && sources[0].config.ci == *ci {
            return Ok(false);
        }

        let path = workspace_root.join("simit.toml");
        let current_text = if path.exists() {
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
        } else {
            String::new()
        };
        let mut document = if current_text.trim().is_empty() {
            DocumentMut::new()
        } else {
            current_text
                .parse::<DocumentMut>()
                .with_context(|| format!("parsing {}", path.display()))?
        };
        let mut table = match document.remove("ci") {
            Some(Item::Table(table)) => table,
            Some(_) => bail!("parsing {}: [ci] must be a table", path.display()),
            None => Table::new(),
        };
        table.set_implicit(false);
        set_ci_table(&mut table, ci);
        document["ci"] = Item::Table(table);

        std::fs::write(&path, document.to_string())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(true)
    }

    pub fn can_persist_ci(workspace_root: &Path) -> Result<bool> {
        let sources = Self::load_sources(workspace_root)?;
        match sources.as_slice() {
            [] => Ok(true),
            [source] if source.label == "simit.toml" => Ok(true),
            [_] => Ok(false),
            _ => {
                let labels = sources
                    .iter()
                    .map(|source| source.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "multiple simit project config sources found ({labels}); keep exactly one of simit.toml, Cargo.toml [workspace.metadata.simit], Cargo.toml [package.metadata.simit], or flake outputs.simitConfig"
                );
            }
        }
    }

    fn load_sources(workspace_root: &Path) -> Result<Vec<ProjectConfigSource>> {
        let mut sources = Vec::new();

        if let Some(source) = load_simit_toml(workspace_root)? {
            sources.push(source);
        }
        sources.extend(load_cargo_metadata_config(workspace_root)?);
        if let Some(source) = load_flake_config(workspace_root)? {
            sources.push(source);
        }

        Ok(sources
            .into_iter()
            .filter(|source| !source.config.is_empty())
            .collect())
    }

    /// Validate the optional `[homebrew]` section.
    ///
    /// Validation is intentionally lazy: `load` only parses the file, and
    /// callers that consume Homebrew settings decide when malformed Homebrew
    /// config should become fatal.
    pub fn validate_homebrew(&self) -> Result<()> {
        let Some(homebrew) = &self.homebrew else {
            return Ok(());
        };

        if homebrew.tap_url.is_empty() {
            bail!("simit project config: [homebrew].tap_url is required");
        }
        reject_basic_auth_url(
            "simit project config: [homebrew].tap_url",
            &homebrew.tap_url,
        )?;
        if homebrew.download_repo.is_empty() {
            bail!("simit project config: [homebrew].download_repo is required");
        }
        if let Some(desc) = &homebrew.description {
            if desc.chars().count() > 80 {
                bail!(
                    "simit project config: [homebrew].description must be 80 characters or fewer"
                );
            }
        }
        if let Some(home) = &homebrew.homepage {
            if !home.starts_with("https://") {
                bail!("simit project config: [homebrew].homepage must start with https://");
            }
        }
        if !homebrew.platforms.any_enabled() {
            bail!("simit project config: [homebrew].platforms has all platforms disabled");
        }

        Ok(())
    }

    /// Validate the optional `[chocolatey]` section.
    pub fn validate_chocolatey(&self) -> Result<()> {
        let Some(chocolatey) = &self.chocolatey else {
            return Ok(());
        };

        if chocolatey.download_repo.is_empty() {
            bail!("simit project config: [chocolatey].download_repo is required");
        }
        if let Some(desc) = &chocolatey.description {
            if desc.chars().count() > 4000 {
                bail!(
                    "simit project config: [chocolatey].description must be 4000 characters or fewer"
                );
            }
        }
        if let Some(tags) = &chocolatey.tags {
            if tags.chars().count() > 4000 {
                bail!("simit project config: [chocolatey].tags must be 4000 characters or fewer");
            }
        }
        reject_basic_auth_url(
            "simit project config: [chocolatey].push.source",
            &chocolatey.push.source,
        )?;

        Ok(())
    }

    /// Validate the optional `[scoop]` section.
    pub fn validate_scoop(&self) -> Result<()> {
        let Some(scoop) = &self.scoop else {
            return Ok(());
        };

        if scoop.bucket_url.is_empty() {
            bail!("simit project config: [scoop].bucket_url is required");
        }
        reject_basic_auth_url(
            "simit project config: [scoop].bucket_url",
            &scoop.bucket_url,
        )?;
        if scoop.download_repo.is_empty() {
            bail!("simit project config: [scoop].download_repo is required");
        }
        if !scoop.architectures.any_enabled() {
            bail!("simit project config: [scoop].architectures has all architectures disabled");
        }

        Ok(())
    }

    /// Resolve Homebrew settings for one Cargo package.
    ///
    /// Workspaces may contain multiple packages; the caller is responsible for
    /// selecting the package whose metadata should provide fallbacks.
    pub fn resolve_homebrew(
        &self,
        overrides: HomebrewOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedHomebrew> {
        self.validate_homebrew()?;

        let cfg = self.homebrew.as_ref();
        let name = merge(
            overrides.name.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.name.clone()),
            Some(package.name.clone()),
            "name",
        )?;
        let tap_url = merge(
            overrides.tap_url.map(str::to_owned),
            cfg.map(|homebrew| homebrew.tap_url.clone()),
            None,
            "tap_url",
        )?;
        reject_basic_auth_url("homebrew.tap_url", &tap_url)?;
        let description = merge(
            overrides.description.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.description.clone()),
            package.description.clone(),
            "description",
        )?;
        let homepage = merge(
            overrides.homepage.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.homepage.clone()),
            package.homepage.clone(),
            "homepage",
        )?;
        let license = merge(
            overrides.license.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.license.clone()),
            package.license.clone(),
            "license",
        )?;
        let download_repo = merge(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|homebrew| homebrew.download_repo.clone()),
            None,
            "download_repo",
        )?;
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|homebrew| homebrew.archive_pattern.clone()))
            .unwrap_or_else(default_archive_pattern);
        let binaries = resolve_binaries(overrides.binaries, cfg, &name);
        let mut platforms = cfg
            .map(|homebrew| homebrew.platforms.clone())
            .unwrap_or_default();
        apply_disabled_platforms(&mut platforms, overrides.disabled_platforms)?;

        if !platforms.any_enabled() {
            bail!("homebrew.platforms has all platforms disabled");
        }

        Ok(ResolvedHomebrew {
            name,
            binaries,
            tap_url,
            tap_token_secret: cfg.map_or_else(default_homebrew_tap_token_secret, |homebrew| {
                homebrew.tap_token_secret.clone()
            }),
            description,
            homepage,
            license,
            download_repo,
            archive_pattern,
            platforms,
        })
    }

    /// Resolve Chocolatey settings for one Cargo package.
    pub fn resolve_chocolatey(
        &self,
        overrides: ChocolateyOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedChocolatey> {
        self.validate_chocolatey()?;

        let cfg = self.chocolatey.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.name.clone()),
            Some(package.name.clone()),
            missing_chocolatey_message("name"),
        )?;
        let id = overrides
            .id
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.id.clone()))
            .unwrap_or_else(|| name.clone());
        let title = overrides
            .title
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.title.clone()))
            .unwrap_or_else(|| name.clone());
        let authors = overrides
            .authors
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.authors.clone()));
        let authors =
            authors.or_else(|| (!package.authors.is_empty()).then(|| package.authors.join(", ")));
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|chocolatey| chocolatey.download_repo.clone()),
            None,
            missing_chocolatey_message("download_repo"),
        )?;
        let description = merge_packager(
            overrides.description.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.description.clone()),
            package.description.clone(),
            missing_chocolatey_message("description"),
        )?;
        if description.chars().count() > 4000 {
            bail!("chocolatey.description must be 4000 characters or fewer");
        }
        let summary = overrides
            .summary
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.summary.clone()));
        let project_url = merge_packager(
            overrides.project_url.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.project_url.clone()),
            package.homepage.clone(),
            missing_chocolatey_message("project_url"),
        )?;
        let license_url = overrides
            .license_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.license_url.clone()));
        let icon_url = overrides
            .icon_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.icon_url.clone()));
        let package_source_url = overrides
            .package_source_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.package_source_url.clone()));
        let docs_url = overrides
            .docs_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.docs_url.clone()));
        let bug_tracker_url = overrides
            .bug_tracker_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.bug_tracker_url.clone()));
        let project_source_url = overrides
            .project_source_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.project_source_url.clone()));
        let tags = overrides
            .tags
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.tags.clone()));
        if let Some(tags) = &tags {
            if tags.chars().count() > 4000 {
                bail!("chocolatey.tags must be 4000 characters or fewer");
            }
        }
        let release_notes_url = overrides
            .release_notes_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.release_notes_url.clone()));
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|chocolatey| chocolatey.archive_pattern.clone()))
            .unwrap_or_else(default_windows_archive_pattern);
        let push_source = overrides
            .push_source
            .map(str::to_owned)
            .or_else(|| cfg.map(|chocolatey| chocolatey.push.source.clone()))
            .unwrap_or_else(default_chocolatey_push_source);
        reject_basic_auth_url("chocolatey.push.source", &push_source)?;

        Ok(ResolvedChocolatey {
            name,
            id,
            title,
            authors,
            description,
            summary,
            project_url,
            license_url,
            icon_url,
            package_source_url,
            docs_url,
            bug_tracker_url,
            project_source_url,
            tags,
            release_notes_url,
            download_repo,
            archive_pattern,
            push: ChocolateyPushConfig {
                source: push_source,
            },
            nix_tool: cfg.map_or_else(default_chocolatey_nix_tool, |chocolatey| {
                chocolatey.nix_tool.clone()
            }),
            api_key_env: cfg.map_or_else(default_chocolatey_api_key_env, |chocolatey| {
                chocolatey.api_key_env.clone()
            }),
            api_key_secret: cfg.map_or_else(default_chocolatey_api_key_secret, |chocolatey| {
                chocolatey.api_key_secret.clone()
            }),
            api_key_from_runner: cfg.is_some_and(|chocolatey| chocolatey.api_key_from_runner),
        })
    }

    /// Resolve Scoop settings for one Cargo package.
    pub fn resolve_scoop(
        &self,
        overrides: ScoopOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedScoop> {
        self.validate_scoop()?;

        let cfg = self.scoop.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|scoop| scoop.name.clone()),
            Some(package.name.clone()),
            missing_scoop_message("name"),
        )?;
        let bucket_url = merge_packager(
            overrides.bucket_url.map(str::to_owned),
            cfg.map(|scoop| scoop.bucket_url.clone()),
            None,
            missing_scoop_message("bucket_url"),
        )?;
        reject_basic_auth_url("scoop.bucket_url", &bucket_url)?;
        let description = merge_packager(
            overrides.description.map(str::to_owned),
            cfg.and_then(|scoop| scoop.description.clone()),
            package.description.clone(),
            missing_scoop_message("description"),
        )?;
        let homepage = merge_packager(
            overrides.homepage.map(str::to_owned),
            cfg.and_then(|scoop| scoop.homepage.clone()),
            package.homepage.clone(),
            missing_scoop_message("homepage"),
        )?;
        let license = merge_packager(
            overrides.license.map(str::to_owned),
            cfg.and_then(|scoop| scoop.license.clone()),
            package.license.clone(),
            missing_scoop_message("license"),
        )?;
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|scoop| scoop.download_repo.clone()),
            None,
            missing_scoop_message("download_repo"),
        )?;
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|scoop| scoop.archive_pattern.clone()))
            .unwrap_or_else(default_windows_archive_pattern);
        let binaries = resolve_scoop_binaries(overrides.binaries, cfg, &name);
        let mut architectures = cfg
            .map(|scoop| scoop.architectures.clone())
            .unwrap_or_default();
        apply_disabled_architectures(&mut architectures, overrides.disabled_architectures)?;

        if !architectures.any_enabled() {
            bail!("scoop.architectures has all architectures disabled");
        }

        Ok(ResolvedScoop {
            name,
            bucket_url,
            bucket_token_secret: cfg.map_or_else(default_scoop_bucket_token_secret, |scoop| {
                scoop.bucket_token_secret.clone()
            }),
            description,
            homepage,
            license,
            download_repo,
            archive_pattern,
            binaries,
            architectures,
        })
    }

    /// Validate the optional `[aur]` section.
    pub fn validate_aur(&self) -> Result<()> {
        let Some(aur) = &self.aur else {
            return Ok(());
        };
        validate_owner_repo(
            "simit project config: [aur].download_repo",
            &aur.download_repo,
        )?;
        if !aur.flavors.any_enabled() {
            bail!("simit project config: [aur].flavors has all flavors disabled");
        }
        Ok(())
    }

    /// Validate the optional `[copr]` section.
    pub fn validate_copr(&self) -> Result<()> {
        let Some(copr) = &self.copr else {
            return Ok(());
        };
        validate_owner_repo(
            "simit project config: [copr].download_repo",
            &copr.download_repo,
        )?;
        Ok(())
    }

    /// Validate the optional `[apt]` section.
    pub fn validate_apt(&self) -> Result<()> {
        let Some(apt) = &self.apt else {
            return Ok(());
        };
        if apt.repo_url.is_empty() {
            bail!("simit project config: [apt].repo_url is required");
        }
        // The apt repo_url is an SSH git remote (e.g. ssh://git@host/...), where a
        // `user@` userinfo is expected; only reject an embedded `:password@`.
        reject_password_url("simit project config: [apt].repo_url", &apt.repo_url)?;
        if let Some(public_url) = &apt.public_url {
            if !public_url.starts_with("https://") {
                bail!("simit project config: [apt].public_url must start with https://");
            }
        }
        if apt.pages.provider != "codeberg-git-pages" {
            bail!("simit project config: [apt].pages.provider must be codeberg-git-pages");
        }
        validate_runner_label_opt("[apt.pages].runner", apt.pages.runner.as_deref())?;
        Ok(())
    }

    /// Resolve AUR settings for one Cargo package.
    pub fn resolve_aur(
        &self,
        overrides: AurOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedAur> {
        self.resolve_aur_with_host(overrides, package, "https://codeberg.org")
    }

    pub fn resolve_aur_for_platform(
        &self,
        overrides: AurOverrides<'_>,
        package: &crate::cargo::Package,
        platform: crate::cli::Platform,
    ) -> Result<ResolvedAur> {
        self.resolve_aur_with_host(overrides, package, platform.web_base_url())
    }

    fn resolve_aur_with_host(
        &self,
        overrides: AurOverrides<'_>,
        package: &crate::cargo::Package,
        host: &str,
    ) -> Result<ResolvedAur> {
        self.validate_aur()?;
        let cfg = self.aur.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|aur| aur.name.clone()),
            Some(package.name.clone()),
            missing_packager_message("aur", "name", "--aur-name", "package.name"),
        )?;
        let description = merge_packager(
            overrides.description.map(str::to_owned),
            cfg.and_then(|aur| aur.description.clone()),
            package.description.clone(),
            missing_packager_message(
                "aur",
                "description",
                "--aur-description",
                "package.description",
            ),
        )?;
        let license = merge_packager(
            overrides.license.map(str::to_owned),
            cfg.and_then(|aur| aur.license.clone()),
            package.license.clone(),
            missing_packager_message("aur", "license", "--aur-license", "package.license"),
        )?;
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|aur| aur.download_repo.clone()),
            None,
            missing_packager_message("aur", "download_repo", "--aur-download-repo", ""),
        )?;
        validate_owner_repo("aur.download_repo", &download_repo)?;
        let repo = repo_basename(&download_repo);
        let url = overrides
            .url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|aur| aur.url.clone()))
            .unwrap_or_else(|| format!("{host}/{download_repo}"));
        let binaries = resolve_named_binaries(cfg.map(|aur| aur.binaries.as_slice()), &name);
        let git_url = cfg
            .and_then(|aur| aur.git_url.clone())
            .unwrap_or_else(|| format!("{host}/{download_repo}.git"));

        Ok(ResolvedAur {
            name,
            description,
            url,
            license,
            maintainer: cfg.and_then(|aur| aur.maintainer.clone()),
            maintainer_gpg: cfg.and_then(|aur| aur.maintainer_gpg.clone()),
            arch: cfg.map_or_else(default_aur_arch, |aur| aur.arch.clone()),
            depends: cfg.map(|aur| aur.depends.clone()).unwrap_or_default(),
            makedepends: cfg.map(|aur| aur.makedepends.clone()).unwrap_or_default(),
            bin_glibc_min: cfg.and_then(|aur| aur.bin_glibc_min.clone()),
            binaries,
            assets: cfg.map(|aur| aur.assets.clone()).unwrap_or_default(),
            license_file: cfg.map_or_else(default_aur_license_file, |aur| aur.license_file.clone()),
            readme: cfg.map_or_else(default_aur_readme, |aur| aur.readme.clone()),
            changelog: cfg.map_or_else(default_aur_doc_changelog, |aur| aur.changelog.clone()),
            download_repo,
            repo,
            source_archive_pattern: cfg.map_or_else(default_aur_source_archive_pattern, |aur| {
                aur.source_archive_pattern.clone()
            }),
            binary_archive_pattern: cfg.map_or_else(default_aur_binary_archive_pattern, |aur| {
                aur.binary_archive_pattern.clone()
            }),
            git_url,
            flavors: cfg.map(|aur| aur.flavors).unwrap_or_default(),
            ssh_remote: cfg.map_or_else(default_aur_ssh_remote, |aur| aur.ssh_remote.clone()),
            ssh_key_secret: cfg
                .map_or_else(default_aur_ssh_key_secret, |aur| aur.ssh_key_secret.clone()),
            stable_only: cfg.is_none_or(|aur| aur.stable_only),
        })
    }

    /// Resolve COPR settings for one Cargo package.
    pub fn resolve_copr(
        &self,
        overrides: CoprOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedCopr> {
        self.resolve_copr_with_host(overrides, package, "https://codeberg.org")
    }

    pub fn resolve_copr_for_platform(
        &self,
        overrides: CoprOverrides<'_>,
        package: &crate::cargo::Package,
        platform: crate::cli::Platform,
    ) -> Result<ResolvedCopr> {
        self.resolve_copr_with_host(overrides, package, platform.web_base_url())
    }

    fn resolve_copr_with_host(
        &self,
        overrides: CoprOverrides<'_>,
        package: &crate::cargo::Package,
        host: &str,
    ) -> Result<ResolvedCopr> {
        self.validate_copr()?;
        let cfg = self.copr.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|copr| copr.name.clone()),
            Some(package.name.clone()),
            missing_packager_message("copr", "name", "--copr-name", "package.name"),
        )?;
        let summary = merge_packager(
            overrides.summary.map(str::to_owned),
            cfg.and_then(|copr| copr.summary.clone()),
            package.description.clone(),
            missing_packager_message("copr", "summary", "--copr-summary", "package.description"),
        )?;
        let description = overrides
            .description
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|copr| copr.description.clone()))
            .unwrap_or_else(|| summary.clone());
        let license = merge_packager(
            overrides.license.map(str::to_owned),
            cfg.and_then(|copr| copr.license.clone()),
            package.license.clone(),
            missing_packager_message("copr", "license", "--copr-license", "package.license"),
        )?;
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|copr| copr.download_repo.clone()),
            None,
            missing_packager_message("copr", "download_repo", "--copr-download-repo", ""),
        )?;
        validate_owner_repo("copr.download_repo", &download_repo)?;
        let repo = repo_basename(&download_repo);
        let url = overrides
            .url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|copr| copr.url.clone()))
            .unwrap_or_else(|| format!("{host}/{download_repo}"));
        let binaries = resolve_named_binaries(cfg.map(|copr| copr.binaries.as_slice()), &name);
        let spec_path = cfg
            .and_then(|copr| copr.spec_path.clone())
            .unwrap_or_else(|| format!("{name}.spec"));
        let project = overrides
            .project
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|copr| copr.project.clone()));
        let testing_project = cfg
            .and_then(|copr| copr.testing_project.clone())
            .or_else(|| {
                project
                    .as_deref()
                    .map(|project| format!("{project}-testing"))
            });

        Ok(ResolvedCopr {
            name,
            summary,
            description,
            license,
            url,
            download_repo,
            repo,
            build_requires: cfg
                .map(|copr| copr.build_requires.clone())
                .unwrap_or_default(),
            binaries,
            spec_path,
            project,
            testing_project,
            login_secret: cfg
                .map_or_else(default_copr_login_secret, |copr| copr.login_secret.clone()),
            username_secret: cfg.map_or_else(default_copr_username_secret, |copr| {
                copr.username_secret.clone()
            }),
            token_secret: cfg
                .map_or_else(default_copr_token_secret, |copr| copr.token_secret.clone()),
            nix_tool: cfg.map_or_else(default_copr_nix_tool, |copr| copr.nix_tool.clone()),
        })
    }

    /// Resolve APT settings for one Cargo package.
    pub fn resolve_apt(
        &self,
        overrides: AptOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedApt> {
        self.validate_apt()?;
        let cfg = self.apt.as_ref();
        let repo_url = merge_packager(
            overrides.repo_url.map(str::to_owned),
            cfg.map(|apt| apt.repo_url.clone()),
            None,
            missing_packager_message("apt", "repo_url", "--apt-repo-url", ""),
        )?;
        reject_password_url("apt.repo_url", &repo_url)?;
        let label = overrides
            .label
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|apt| apt.label.clone()))
            .unwrap_or_else(|| package.name.clone());
        let packages = cfg
            .map(|apt| apt.packages.clone())
            .filter(|packages| !packages.is_empty())
            .unwrap_or_else(|| vec![package.name.clone()]);

        Ok(ResolvedApt {
            label,
            repo_url,
            public_url: overrides
                .public_url
                .map(str::to_owned)
                .or_else(|| cfg.and_then(|apt| apt.public_url.clone())),
            pages_provider: cfg
                .map_or_else(default_apt_pages_provider, |apt| apt.pages.provider.clone()),
            pages_runner: cfg.and_then(|apt| apt.pages.runner.clone()),
            branch: cfg.map_or_else(default_apt_branch, |apt| apt.branch.clone()),
            distribution: cfg.map_or_else(default_apt_distribution, |apt| apt.distribution.clone()),
            architectures: cfg
                .map_or_else(default_apt_architectures, |apt| apt.architectures.clone()),
            components: cfg.map_or_else(default_apt_components, |apt| apt.components.clone()),
            debian_release: cfg
                .map_or_else(default_apt_debian_release, |apt| apt.debian_release.clone()),
            packages,
            build_deps: cfg.map(|apt| apt.build_deps.clone()).unwrap_or_default(),
            cargo_deb_version: cfg.map_or_else(default_cargo_deb_version, |apt| {
                apt.cargo_deb_version.clone()
            }),
            gpg_key_secret: cfg
                .map_or_else(default_apt_gpg_key_secret, |apt| apt.gpg_key_secret.clone()),
            gpg_key_id_secret: cfg.map_or_else(default_apt_gpg_key_id_secret, |apt| {
                apt.gpg_key_id_secret.clone()
            }),
            gpg_passphrase_secret: cfg.map_or_else(default_apt_gpg_passphrase_secret, |apt| {
                apt.gpg_passphrase_secret.clone()
            }),
            ssh_key_secret: cfg
                .map_or_else(default_apt_ssh_key_secret, |apt| apt.ssh_key_secret.clone()),
        })
    }

    /// Resolve the `[release.codeberg]` section, if present.
    pub fn resolve_codeberg_release(&self) -> Result<Option<ResolvedCodebergRelease>> {
        let Some(codeberg) = &self.release.codeberg else {
            return Ok(None);
        };
        validate_owner_repo(
            "simit project config: [release.codeberg].repo",
            &codeberg.repo,
        )?;
        Ok(Some(ResolvedCodebergRelease {
            repo: codeberg.repo.clone(),
            api_base: codeberg.api_base.clone(),
            token_secret: codeberg.token_secret.clone(),
            target_branch: codeberg.target_branch.clone(),
            body_from_changelog: codeberg.body_from_changelog,
        }))
    }

    /// Resolve the selected hosted release target, if configured.
    pub fn resolve_release_target(
        &self,
        platform: crate::cli::Platform,
    ) -> Result<Option<ResolvedReleaseTarget>> {
        if platform == crate::cli::Platform::Gitlab {
            return Ok(None);
        }
        let (provider, target) = match platform {
            crate::cli::Platform::Forgejo => (
                ReleaseProvider::Forgejo,
                self.release.codeberg.as_ref().map(|target| {
                    (
                        target.repo.clone(),
                        target.api_base.clone(),
                        target.token_secret.clone(),
                        target.target_branch.clone(),
                        target.body_from_changelog,
                    )
                }),
            ),
            crate::cli::Platform::Github => (
                ReleaseProvider::Github,
                self.release.github.as_ref().map(|target| {
                    (
                        target.repo.clone(),
                        target.api_base.clone(),
                        target.token_secret.clone(),
                        target.target_branch.clone(),
                        target.body_from_changelog,
                    )
                }),
            ),
            crate::cli::Platform::Gitlab => (ReleaseProvider::Github, None),
        };
        let Some((repo, api_base, token_secret, target_branch, body_from_changelog)) = target
        else {
            return Ok(None);
        };
        validate_owner_repo(
            match provider {
                ReleaseProvider::Forgejo => "simit project config: [release.codeberg].repo",
                ReleaseProvider::Github => "simit project config: [release.github].repo",
            },
            &repo,
        )?;
        Ok(Some(ResolvedReleaseTarget {
            provider,
            repo,
            api_base,
            token_secret,
            target_branch,
            body_from_changelog,
        }))
    }

    /// Resolve the `[ci.pages]` section, if present.
    pub fn resolve_codeberg_pages(&self) -> Result<Option<ResolvedCodebergPages>> {
        let Some(pages) = &self.ci.pages else {
            return Ok(None);
        };
        validate_owner_repo("simit project config: [ci.pages].repo", &pages.repo)?;
        let owner = pages
            .repo
            .split_once('/')
            .expect("validated owner/repo")
            .0
            .to_owned();
        Ok(Some(ResolvedCodebergPages {
            repo: pages.repo.clone(),
            owner,
            canonical_domain: pages.canonical_domain.clone(),
            site_output: pages.site_output.clone(),
            token_secret: pages.token_secret.clone(),
            source_branch: pages.source_branch.clone(),
            deploy_app: pages.deploy_app.clone(),
        }))
    }

    /// Resolve the `[vscode]` section, if present.
    pub fn resolve_vscode(&self) -> Result<Option<ResolvedVscode>> {
        let Some(vscode) = &self.vscode else {
            return Ok(None);
        };
        validate_owner_repo(
            "simit project config: [vscode].codeberg_repo",
            &vscode.codeberg_repo,
        )?;
        let codeberg_owner = vscode
            .codeberg_repo
            .split_once('/')
            .expect("validated owner/repo")
            .0
            .to_owned();
        Ok(Some(ResolvedVscode {
            extension_dir: vscode.extension_dir.clone(),
            runner: vscode.runner.clone(),
            codeberg_repo: vscode.codeberg_repo.clone(),
            codeberg_owner,
            codeberg_api_base: vscode.codeberg_api_base.clone(),
            codeberg_token_secret: vscode.codeberg_token_secret.clone(),
            pat_source: vscode.pat_source,
            vsce_pat_file_env: vscode.vsce_pat_file_env.clone(),
            ovsx_pat_file_env: vscode.ovsx_pat_file_env.clone(),
            vsce_pat_secret: vscode.vsce_pat_secret.clone(),
            ovsx_pat_secret: vscode.ovsx_pat_secret.clone(),
            package_command: vscode.package_command.clone(),
            cargo_package: vscode.cargo_package.clone(),
            prepublish_commands: vscode.prepublish_commands.clone(),
        }))
    }

    /// Resolve the `[jetbrains]` section, if present.
    pub fn resolve_jetbrains(&self) -> Result<Option<ResolvedJetbrains>> {
        let Some(jetbrains) = &self.jetbrains else {
            return Ok(None);
        };
        Ok(Some(ResolvedJetbrains {
            plugin_dir: jetbrains.plugin_dir.clone(),
            plugin_xml_id: jetbrains.plugin_xml_id.clone(),
            package_installable: jetbrains.package_installable.clone(),
            runner: jetbrains.runner.clone(),
            cargo_package: jetbrains.cargo_package.clone(),
            channel: jetbrains.channel.clone(),
            credential_source: jetbrains.credential_source,
            marketplace_token_file_env: jetbrains.marketplace_token_file_env.clone(),
            certificate_chain_file_env: jetbrains.certificate_chain_file_env.clone(),
            private_key_file_env: jetbrains.private_key_file_env.clone(),
            private_key_password_file_env: jetbrains.private_key_password_file_env.clone(),
            marketplace_token_secret: jetbrains.marketplace_token_secret.clone(),
            certificate_chain_secret: jetbrains.certificate_chain_secret.clone(),
            private_key_secret: jetbrains.private_key_secret.clone(),
            private_key_password_secret: jetbrains.private_key_password_secret.clone(),
            prepublish_commands: jetbrains.prepublish_commands.clone(),
        }))
    }

    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Reject a `scheme://user:password@host/...` URL while permitting a bare
/// `scheme://user@host/...` userinfo, which SSH git remotes legitimately use.
fn reject_password_url(name: &str, value: &str) -> Result<()> {
    let Some(scheme_end) = value.find("://") else {
        return Ok(());
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    let authority = &value[authority_start..authority_end];
    if let Some((userinfo, _host)) = authority.rsplit_once('@') {
        if userinfo.contains(':') {
            bail!("{name} must not include an embedded password");
        }
    }
    Ok(())
}

fn validate_owner_repo(name: &str, value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("{name} must be OWNER/REPO");
    }
    Ok(())
}

fn repo_basename(download_repo: &str) -> String {
    download_repo
        .rsplit('/')
        .next()
        .unwrap_or(download_repo)
        .to_owned()
}

fn resolve_named_binaries(cfg: Option<&[String]>, name: &str) -> Vec<String> {
    cfg.filter(|binaries| !binaries.is_empty())
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| vec![name.to_owned()])
}

fn missing_packager_message(
    channel: &str,
    field: &str,
    flag: &str,
    cargo_fallback: &str,
) -> String {
    let config_hint = config_hint();
    if cargo_fallback.is_empty() {
        format!(
            "{channel}.{field} not set: provide it via {flag} or {config_hint} [{channel}].{field}"
        )
    } else {
        format!(
            "{channel}.{field} not set: provide it via {flag}, {config_hint} [{channel}].{field}, or Cargo.toml {cargo_fallback}"
        )
    }
}

fn validate_nonempty_strings(name: &str, values: &[String]) -> Result<()> {
    if values.iter().any(|value| value.trim().is_empty()) {
        bail!("{name} must not contain empty values");
    }
    Ok(())
}

fn validate_nonempty_string(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(())
}

fn validate_github_actions_secret_identifier(name: &str, value: &str) -> Result<()> {
    validate_secret_name(name, value)?;
    if value.to_ascii_uppercase().starts_with("GITHUB_") {
        bail!(
            "{name} must be a GitHub Actions secret identifier and not use the reserved GITHUB_ prefix"
        );
    }
    Ok(())
}

fn validate_secret_name(name: &str, value: &str) -> Result<()> {
    validate_nonempty_string(name, value)?;
    if value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!(
            "{name} must contain only ASCII letters, digits, and underscores and must not start with a digit"
        );
    }
    Ok(())
}

fn validate_runner_label_opt(name: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        validate_runner_label(value).map_err(|err| anyhow!("{name}: {err}"))?;
    }
    Ok(())
}

fn validate_nix_system_runners(ci: &CiConfig) -> Result<()> {
    if ci.nix_system_runners.is_empty() {
        return Ok(());
    }

    if ci.platform != Some(Platform::Github) || ci.runtime != Some(Runtime::Nix) {
        bail!(
            "simit project config: [ci].nix_system_runners requires [ci].platform = \"github\" and [ci].runtime = \"nix\""
        );
    }
    if ci
        .provider
        .is_some_and(|provider| provider != CiProvider::Actions)
    {
        bail!("simit project config: [ci].nix_system_runners requires the Actions provider");
    }
    if ci.runner.is_some() {
        bail!("simit project config: [ci].runner cannot be combined with [ci].nix_system_runners");
    }

    validate_system_runner_map(
        "simit project config: [ci].nix_system_runners",
        &ci.nix_system_runners,
    )
}

fn validate_system_runner_map(name: &str, runners: &BTreeMap<String, String>) -> Result<()> {
    for (system, runner) in runners {
        if system.trim().is_empty()
            || !system
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!("{name} has invalid Nix system `{system}`");
        }
        validate_runner_label(runner).map_err(|err| anyhow!("{name}.{system}: {err}"))?;
    }
    Ok(())
}

fn set_ci_table(table: &mut Table, ci: &CiConfig) {
    set_optional_string(table, "provider", ci.provider.map(ci_provider_name));
    set_optional_string(table, "platform", ci.platform.map(Platform::as_str));
    set_optional_string(table, "runtime", ci.runtime.map(runtime_name));
    set_optional_string(table, "runner", ci.runner.as_deref());
    set_string_map(table, "nix_system_runners", &ci.nix_system_runners);
    set_optional_string(table, "windows_runner", ci.windows_runner.as_deref());
    set_bool(table, "workspace", ci.workspace);
    set_optional_string(
        table,
        "workspace_strategy",
        (ci.workspace_strategy != WorkspaceStrategy::Members).then_some(
            match ci.workspace_strategy {
                WorkspaceStrategy::Members => "members",
                WorkspaceStrategy::Aggregate => "aggregate",
            },
        ),
    );
    set_string_array(table, "packages", &ci.packages);
    set_string_array(table, "nix_builds", &ci.nix_builds);
    set_bool(table, "with_nextest", ci.with_nextest);
    set_bool(table, "with_msrv", ci.with_msrv);
    set_bool(table, "with_audit", ci.with_audit);
    set_bool(table, "with_deny", ci.with_deny);
    set_bool(table, "with_docs", ci.with_docs);
    set_bool(table, "with_artifacts", ci.with_artifacts);
    set_bool(table, "with_pypi_publish", ci.with_pypi_publish);
    set_optional_string(table, "pypi_token_secret", ci.pypi_token_secret.as_deref());
    set_bool(table, "pypi_trusted_publishing", ci.pypi_trusted_publishing);
    set_bool(table, "publish_crates", ci.publish_crates);
    set_string_array(table, "extra_setup", &ci.extra_setup);
    set_string_map(table, "extra_env", &ci.extra_env);
    set_string_map(table, "step_runners", &ci.step_runners);
    set_string_array(table, "required_secrets", &ci.required_secrets);
    set_string_array(table, "required_env", &ci.required_env);
    set_bool(table, "om_ci", ci.om_ci);
    set_bool(table, "om_ci_augment", ci.om_ci_augment);
    set_optional_string(table, "omnix_ref", ci.omnix_ref.as_deref());
    set_optional_pages_table(table, ci.pages.as_ref());
    set_crow_table(table, &ci.crow);
}

fn set_crow_table(table: &mut Table, crow: &CrowCiConfig) {
    if *crow == CrowCiConfig::default() {
        table.remove("crow");
        return;
    }
    let mut crow_table = Table::new();
    crow_table.set_implicit(false);
    if crow.format != CrowWorkflowFormat::Yaml {
        crow_table["format"] = value(match crow.format {
            CrowWorkflowFormat::Yaml => "yaml",
            CrowWorkflowFormat::Jsonnet => "jsonnet",
        });
    }
    if let Some(image) = &crow.image {
        crow_table["image"] = value(image.as_str());
    }
    if let Some(nix_image) = &crow.nix_image {
        crow_table["nix_image"] = value(nix_image.as_str());
    }
    if let Some(platform) = &crow.platform {
        crow_table["platform"] = value(platform.as_str());
    }
    if !crow.labels.is_empty() {
        set_string_map(&mut crow_table, "labels", &crow.labels);
    }
    if let Some(workspace_base) = &crow.workspace_base {
        crow_table["workspace_base"] = value(workspace_base.as_str());
    }
    set_bool(&mut crow_table, "skip_clone", crow.skip_clone);
    if !crow.variables.is_empty() {
        let mut variables = Table::new();
        variables.set_implicit(false);
        for (name, variable) in &crow.variables {
            let mut item = Table::new();
            item.set_implicit(false);
            set_optional_string(&mut item, "description", variable.description.as_deref());
            set_string_array(&mut item, "options", &variable.options);
            set_optional_string(&mut item, "default", variable.default.as_deref());
            set_bool(&mut item, "required", variable.required);
            variables[name] = Item::Table(item);
        }
        crow_table["variables"] = Item::Table(variables);
    }
    table["crow"] = Item::Table(crow_table);
}

fn ci_provider_name(provider: CiProvider) -> &'static str {
    match provider {
        CiProvider::Actions => "actions",
        CiProvider::Crow => "crow",
    }
}

fn set_optional_pages_table(table: &mut Table, pages: Option<&CodebergPagesConfig>) {
    let Some(pages) = pages else {
        table.remove("pages");
        return;
    };

    let mut pages_table = Table::new();
    pages_table.set_implicit(false);
    pages_table["repo"] = value(pages.repo.as_str());
    if let Some(canonical_domain) = &pages.canonical_domain {
        pages_table["canonical_domain"] = value(canonical_domain.as_str());
    }
    if pages.site_output != default_pages_site_output() {
        pages_table["site_output"] = value(pages.site_output.as_str());
    }
    if pages.token_secret != default_codeberg_token_secret() {
        pages_table["token_secret"] = value(pages.token_secret.as_str());
    }
    if pages.source_branch != default_pages_source_branch() {
        pages_table["source_branch"] = value(pages.source_branch.as_str());
    }
    if pages.deploy_app != default_pages_deploy_app() {
        pages_table["deploy_app"] = value(pages.deploy_app.as_str());
    }
    table["pages"] = Item::Table(pages_table);
}

fn set_optional_string(table: &mut Table, key: &str, value_text: Option<&str>) {
    match value_text {
        Some(value_text) => table[key] = value(value_text),
        None => {
            table.remove(key);
        }
    }
}

fn set_bool(table: &mut Table, key: &str, enabled: bool) {
    if enabled {
        table[key] = value(enabled);
    } else {
        table.remove(key);
    }
}

fn set_string_array(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
        return;
    }
    let mut array = Array::new();
    for value_text in values {
        array.push(value_text.as_str());
    }
    table[key] = Item::Value(Value::Array(array));
}

fn set_string_map(table: &mut Table, key: &str, values: &BTreeMap<String, String>) {
    if values.is_empty() {
        table.remove(key);
        return;
    }
    let mut inline = InlineTable::default();
    for (map_key, map_value) in values {
        inline.insert(map_key, Value::from(map_value.as_str()));
    }
    table[key] = Item::Value(Value::InlineTable(inline));
}

fn runtime_name(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Cargo => "cargo",
        Runtime::Nix => "nix",
    }
}

#[derive(Debug)]
struct ProjectConfigSource {
    label: String,
    config: ProjectConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoManifestConfig {
    #[serde(default)]
    package: Option<CargoPackageConfig>,
    #[serde(default)]
    workspace: Option<CargoWorkspaceConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageConfig {
    #[serde(default)]
    metadata: CargoMetadataConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWorkspaceConfig {
    #[serde(default)]
    metadata: CargoMetadataConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoMetadataConfig {
    #[serde(default)]
    simit: Option<ProjectConfig>,
}

fn load_simit_toml(workspace_root: &Path) -> Result<Option<ProjectConfigSource>> {
    let path = workspace_root.join("simit.toml");
    if !path.exists() {
        return Ok(None);
    }

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config =
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(ProjectConfigSource {
        label: "simit.toml".to_owned(),
        config,
    }))
}

fn load_cargo_metadata_config(workspace_root: &Path) -> Result<Vec<ProjectConfigSource>> {
    let path = workspace_root.join("Cargo.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: CargoManifestConfig =
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut sources = Vec::new();
    if let Some(config) = manifest
        .workspace
        .and_then(|workspace| workspace.metadata.simit)
    {
        sources.push(ProjectConfigSource {
            label: "Cargo.toml [workspace.metadata.simit]".to_owned(),
            config,
        });
    }
    if let Some(config) = manifest.package.and_then(|package| package.metadata.simit) {
        sources.push(ProjectConfigSource {
            label: "Cargo.toml [package.metadata.simit]".to_owned(),
            config,
        });
    }

    Ok(sources)
}

fn load_flake_config(workspace_root: &Path) -> Result<Option<ProjectConfigSource>> {
    let path = workspace_root.join("flake.nix");
    if !path.exists() || !flake_declares_simit_config(&path)? {
        return Ok(None);
    }

    let command_text = "nix --extra-experimental-features 'nix-command flakes' eval --json --no-write-lock-file .#simitConfig";
    let output = Command::new("nix")
        .current_dir(workspace_root)
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--json",
            "--no-write-lock-file",
            ".#simitConfig",
        ])
        .output()
        .with_context(|| {
            format!(
                "running `{command_text}` to read flake outputs.simitConfig; ensure Nix is installed and flakes are enabled"
            )
        })?;

    if !output.status.success() {
        bail!(
            "flake outputs.simitConfig could not be evaluated; fix the flake and rerun `{command_text}` to validate it:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let config =
        serde_json::from_slice(&output.stdout).context("parsing flake outputs.simitConfig JSON")?;
    Ok(Some(ProjectConfigSource {
        label: "flake outputs.simitConfig".to_owned(),
        config,
    }))
}

fn flake_declares_simit_config(path: &Path) -> Result<bool> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim_start)
        .any(|line| !line.starts_with('#') && line.contains("simitConfig") && line.contains('=')))
}

fn reject_basic_auth_url(name: &str, value: &str) -> Result<()> {
    let Some(scheme_end) = value.find("://") else {
        return Ok(());
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    if value[authority_start..authority_end].contains('@') {
        bail!("{name} must not include embedded credentials");
    }
    Ok(())
}

fn resolve_binaries(
    cli: Option<&[String]>,
    cfg: Option<&HomebrewConfig>,
    name: &str,
) -> Vec<String> {
    cli.filter(|values| !values.is_empty())
        .map(|values| values.to_vec())
        .or_else(|| {
            cfg.and_then(|homebrew| {
                (!homebrew.binaries.is_empty()).then(|| homebrew.binaries.clone())
            })
        })
        .unwrap_or_else(|| vec![name.to_owned()])
}

fn resolve_scoop_binaries(
    cli: Option<&[String]>,
    cfg: Option<&ScoopConfig>,
    name: &str,
) -> Vec<String> {
    cli.filter(|values| !values.is_empty())
        .map(|values| values.to_vec())
        .or_else(|| scoop_config_binaries(cfg))
        .unwrap_or_else(|| vec![name.to_owned()])
}

fn scoop_config_binaries(cfg: Option<&ScoopConfig>) -> Option<Vec<String>> {
    cfg.and_then(|scoop| (!scoop.binaries.is_empty()).then(|| scoop.binaries.clone()))
}

fn apply_disabled_platforms(
    platforms: &mut HomebrewPlatformsConfig,
    disabled: &[String],
) -> Result<()> {
    for key in disabled {
        match key.as_str() {
            "darwin_arm" => platforms.darwin_arm = false,
            "darwin_intel" => platforms.darwin_intel = false,
            "linux_arm" => platforms.linux_arm = false,
            "linux_intel" => platforms.linux_intel = false,
            _ => bail!(
                "homebrew disabled platform must be one of darwin_arm, darwin_intel, linux_arm, linux_intel"
            ),
        }
    }

    Ok(())
}

fn apply_disabled_architectures(
    architectures: &mut ScoopArchSet,
    disabled: &[String],
) -> Result<()> {
    for key in disabled {
        match key.as_str() {
            "x64" => architectures.x64 = false,
            "arm64" => architectures.arm64 = false,
            _ => bail!("scoop disabled architecture must be one of x64, arm64"),
        }
    }

    Ok(())
}

fn merge<T>(cli: Option<T>, cfg: Option<T>, metadata: Option<T>, name: &str) -> Result<T> {
    cli.or(cfg)
        .or(metadata)
        .ok_or_else(|| anyhow!("{}", missing_message(name)))
}

fn merge_packager<T>(
    cli: Option<T>,
    cfg: Option<T>,
    metadata: Option<T>,
    missing: String,
) -> Result<T> {
    cli.or(cfg)
        .or(metadata)
        .ok_or_else(|| anyhow!("{}", missing))
}

fn missing_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "homebrew.name not set: provide it via --homebrew-name, {config_hint} [homebrew].name, or Cargo.toml package.name"
            )
        }
        "tap_url" => {
            format!(
                "homebrew.tap_url not set: provide it via --homebrew-tap or {config_hint} [homebrew].tap_url"
            )
        }
        "description" => {
            format!(
                "homebrew.description not set: provide it via --homebrew-description, {config_hint} [homebrew].description, or Cargo.toml package.description"
            )
        }
        "homepage" => {
            format!(
                "homebrew.homepage not set: provide it via --homebrew-homepage, {config_hint} [homebrew].homepage, or Cargo.toml package.homepage"
            )
        }
        "license" => {
            format!(
                "homebrew.license not set: provide it via --homebrew-license, {config_hint} [homebrew].license, or Cargo.toml package.license"
            )
        }
        "download_repo" => {
            format!(
                "homebrew.download_repo not set: provide it via --homebrew-download-repo or {config_hint} [homebrew].download_repo"
            )
        }
        _ => "homebrew setting not set".to_owned(),
    }
}

fn missing_chocolatey_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "chocolatey.name not set: provide it via --choco-name, {config_hint} [chocolatey].name, or Cargo.toml package.name"
            )
        }
        "description" => {
            format!(
                "chocolatey.description not set: provide it via --choco-description, {config_hint} [chocolatey].description, or Cargo.toml package.description"
            )
        }
        "project_url" => {
            format!(
                "chocolatey.project_url not set: provide it via --choco-project-url, {config_hint} [chocolatey].project_url, or Cargo.toml package.homepage"
            )
        }
        "download_repo" => {
            format!(
                "chocolatey.download_repo not set: provide it via --choco-download-repo or {config_hint} [chocolatey].download_repo"
            )
        }
        _ => "chocolatey setting not set".to_owned(),
    }
}

fn missing_scoop_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "scoop.name not set: provide it via --scoop-name, {config_hint} [scoop].name, or Cargo.toml package.name"
            )
        }
        "bucket_url" => {
            format!(
                "scoop.bucket_url not set: provide it via --scoop-bucket or {config_hint} [scoop].bucket_url"
            )
        }
        "description" => {
            format!(
                "scoop.description not set: provide it via --scoop-description, {config_hint} [scoop].description, or Cargo.toml package.description"
            )
        }
        "homepage" => {
            format!(
                "scoop.homepage not set: provide it via --scoop-homepage, {config_hint} [scoop].homepage, or Cargo.toml package.homepage"
            )
        }
        "license" => {
            format!(
                "scoop.license not set: provide it via --scoop-license, {config_hint} [scoop].license, or Cargo.toml package.license"
            )
        }
        "download_repo" => {
            format!(
                "scoop.download_repo not set: provide it via --scoop-download-repo or {config_hint} [scoop].download_repo"
            )
        }
        _ => "scoop setting not set".to_owned(),
    }
}

fn config_hint() -> &'static str {
    "simit.toml, Cargo.toml [workspace.metadata.simit]/[package.metadata.simit], or flake outputs.simitConfig"
}
