use std::ffi::OsString;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(
    name = "simit",
    version,
    about = "Semver-aware Rust project helper",
    long_about = "simit helps Rust projects prepare release commits, generate CI workflows, wire formatter/pre-commit hooks, and emit shell integration artifacts."
)]
pub struct Cli {
    #[command(subcommand, help = "Command to run")]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "Bump package versions, commit the change, and optionally tag it")]
    Commit(CommitCommand),
    #[command(about = "Run local release checks, update the changelog, commit, and tag")]
    Release(ReleaseCommand),
    #[command(
        about = "Bootstrap generated project files and distribution skeletons",
        disable_help_subcommand = true
    )]
    Init(InitCommand),
    #[command(about = "Distribution channel helpers")]
    Dist(DistCommand),
    #[command(about = "Manage a Keep a Changelog file")]
    Changelog(ChangelogCommand),
    #[command(about = "Install and verify project Git hooks")]
    Hooks(HooksCommand),
    #[command(about = "Manage user-scoped simit configuration")]
    Config(ConfigCommand),
    #[command(about = "Inspect and maintain the per-user project registry")]
    Projects(ProjectsCommand),
    #[command(about = "Apply deterministic simit project upgrades")]
    Upgrade(UpgradeCommand),
    #[command(about = "Print shell completion scripts")]
    Completions(CompletionsCommand),
    #[command(about = "Print a roff manpage for simit")]
    Man(ManCommand),
}

#[derive(Debug, Args)]
pub struct InitCommand {
    #[command(subcommand)]
    pub action: InitAction,
}

#[derive(Debug, Args)]
pub struct UpgradeCommand {
    #[arg(
        long = "path",
        value_name = "PATH",
        action = clap::ArgAction::Append,
        conflicts_with = "all",
        help = "Workspace path to upgrade; repeatable. Defaults to the current workspace"
    )]
    pub paths: Vec<Utf8PathBuf>,
    #[arg(
        long,
        conflicts_with = "paths",
        help = "Upgrade every registered non-ephemeral simit-managed project"
    )]
    pub all: bool,
    #[arg(
        long,
        conflicts_with = "check",
        help = "Report intended changes without writing files"
    )]
    pub dry_run: bool,
    #[arg(long, help = "Exit nonzero when an upgrade would change files")]
    pub check: bool,
    #[arg(long, help = "Show unified diffs; valid with --dry-run or --check")]
    pub diff: bool,
    #[arg(
        long = "pages-only",
        help = "Only apply Codeberg Pages workflow upgrades, skipping README badge upgrades"
    )]
    pub pages_only: bool,
}

#[derive(Debug, Args)]
pub struct DistCommand {
    #[command(subcommand)]
    pub action: DistAction,
}

#[derive(Debug, Subcommand)]
pub enum InitAction {
    #[command(about = "Generate or verify Rust CI workflows")]
    Ci(Box<InitCiCommand>),
    #[command(about = "Generate or verify a canonical Rust crane flake and hook wiring")]
    Flake(InitFlakeCommand),
    #[command(about = "Generate or verify a language-aware .gitignore")]
    Gitignore(InitGitignoreCommand),
    #[command(about = "Bootstrap a Homebrew tap repo with a formula skeleton")]
    HomebrewTap(InitHomebrewTapCommand),
    #[command(about = "Bootstrap a Chocolatey package directory")]
    Chocolatey(Box<InitChocolateyCommand>),
    #[command(about = "Bootstrap a Scoop bucket repo with a manifest skeleton")]
    ScoopBucket(InitScoopBucketCommand),
    #[command(about = "Bootstrap AUR PKGBUILDs (source/-bin/-git flavors)")]
    Aur(InitAurCommand),
    #[command(about = "Bootstrap a Fedora COPR RPM spec and .copr/Makefile")]
    Copr(InitCoprCommand),
    #[command(about = "Bootstrap an apt (reprepro) repository config")]
    Apt(InitAptCommand),
    #[command(about = "Generate the comprehensive multi-channel release workflow")]
    Release(InitReleaseCommand),
}

#[derive(Debug, Subcommand)]
pub enum DistAction {
    #[command(about = "Homebrew formula helpers (render, bump, push)")]
    Homebrew(HomebrewCommand),
    #[command(about = "Chocolatey package helpers (render, bump, push)")]
    Chocolatey(ChocolateyCommand),
    #[command(about = "Scoop manifest helpers (render, bump, push)")]
    Scoop(ScoopCommand),
    #[command(about = "Winget manifest submission helpers")]
    Winget(WingetCommand),
    #[command(about = "Windows community packager publishing helpers")]
    Windows(WindowsCommand),
    #[command(about = "AUR PKGBUILD helpers (render)")]
    Aur(AurCommand),
    #[command(about = "Fedora COPR helpers (render)")]
    Copr(CoprCommand),
    #[command(about = "apt repository helpers (render)")]
    Apt(AptCommand),
}

#[derive(Debug, Args)]
pub struct CommitCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package to bump; may be repeated"
    )]
    pub packages: Vec<String>,
    #[arg(long, help = "Bump every package in the workspace")]
    pub workspace: bool,
    #[arg(long = "no-tag", help = "Skip creating the release tag")]
    pub no_tag: bool,
    #[arg(
        long = "no-sign",
        help = "Create an unsigned tag instead of a signed tag"
    )]
    pub no_sign: bool,
    #[arg(long, help = "Print the planned release commit without changing files")]
    pub dry_run: bool,
    #[arg(value_enum, value_name = "BUMP", help = "Version bump to apply")]
    pub bump: BumpKind,
    #[arg(
        long = "pre",
        value_name = "ID",
        help = "Prerelease identifier to attach, such as alpha.1 or rc.1"
    )]
    pub pre: Option<String>,
    #[arg(
        value_name = "GIT_ARGS",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Arguments passed through to git commit, such as -m <message>"
    )]
    pub git_args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct ReleaseCommand {
    #[arg(
        long,
        value_enum,
        help = "Hosted Git provider for release secret management"
    )]
    pub platform: Option<Platform>,
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package to release; may be repeated"
    )]
    pub packages: Vec<String>,
    #[arg(long, help = "Release every package in the workspace")]
    pub workspace: bool,
    #[arg(long = "no-tag", help = "Skip creating the release tag")]
    pub no_tag: bool,
    #[arg(
        long = "no-sign",
        help = "Create an unsigned tag instead of a signed tag"
    )]
    pub no_sign: bool,
    #[arg(long, help = "Print the planned release without changing files")]
    pub dry_run: bool,
    #[arg(
        value_enum,
        value_name = "ACTION",
        help = "Version bump to apply, or sync-up to rerun a failed release from HEAD"
    )]
    pub action: ReleaseAction,
    #[arg(value_enum, value_name = "TRUST_ACTION", help = "Release trust action")]
    pub trust_action: Option<ReleaseTrustAction>,
    #[arg(
        value_enum,
        value_name = "SECRETS_ACTION",
        help = "Release secrets action"
    )]
    pub secrets_action: Option<ReleaseSecretsAction>,
    #[arg(
        long = "key",
        value_name = "FINGERPRINT",
        help = "OpenPGP key fingerprint for `simit release trust`"
    )]
    pub trust_key: Option<String>,
    #[arg(
        long = "trust-root",
        value_name = "PATH",
        help = "Path to the release maintainer public keyring"
    )]
    pub trust_root: Option<Utf8PathBuf>,
    #[arg(
        long = "pre",
        value_name = "ID",
        help = "Prerelease identifier to attach, such as alpha.1 or rc.1"
    )]
    pub pre: Option<String>,
    #[arg(
        short = 'm',
        long = "message",
        value_name = "MESSAGE",
        help = "Release commit message"
    )]
    pub message: Option<String>,
    #[arg(
        long = "no-changelog",
        help = "Skip promoting CHANGELOG.md even when it exists"
    )]
    pub no_changelog: bool,
    #[arg(long, help = "Push a sync-up tag move to the remote")]
    pub push: bool,
    #[arg(
        long,
        value_name = "REMOTE",
        default_value = "origin",
        help = "Remote to push sync-up tag moves to"
    )]
    pub remote: String,
    #[arg(
        long,
        help = "Print machine-readable JSON for `simit release verify`, `simit release plan`, or `simit release secrets contract`"
    )]
    pub json: bool,
    #[arg(
        long = "dry-run-package",
        help = "Run `cargo package` checks for each crate selected by `simit release plan`"
    )]
    pub dry_run_package: bool,
    #[arg(
        long = "version",
        value_name = "VERSION",
        help = "Version to verify instead of the selected package's current Cargo.toml version"
    )]
    pub verify_version: Option<Version>,
    #[arg(
        long = "push-target",
        value_name = "REMOTE",
        help = "Remote checked by `simit release verify` for the release tag"
    )]
    pub push_target: Option<String>,
    #[arg(
        long = "repo",
        value_name = "OWNER/REPO",
        help = "Hosted Git repository for `simit release secrets`"
    )]
    pub secrets_repo: Option<String>,
    #[arg(
        long = "api-base",
        value_name = "URL",
        default_value = "https://codeberg.org/api/v1",
        help = "Forgejo/Codeberg API base URL for Forgejo secret management"
    )]
    pub secrets_api_base: String,
    #[arg(
        long = "token-file",
        value_name = "PATH",
        help = "File containing the hosted Git provider token for `simit release secrets`"
    )]
    pub secrets_token_file: Option<Utf8PathBuf>,
    #[arg(
        long = "assume-account-secret",
        value_name = "NAME",
        action = clap::ArgAction::Append,
        help = "Secret name managed at account/org scope and not visible in repo secret listings"
    )]
    pub assumed_account_secrets: Vec<String>,
    #[arg(
        long = "minisign-secret-key-file",
        value_name = "PATH",
        help = "Existing minisign secret key file to import as MINISIGN_SECRET_KEY"
    )]
    pub minisign_secret_key_file: Option<Utf8PathBuf>,
    #[arg(
        long = "minisign-password-file",
        value_name = "PATH",
        help = "Existing minisign password file to import as MINISIGN_PASSWORD"
    )]
    pub minisign_password_file: Option<Utf8PathBuf>,
    #[arg(
        long = "minisign-public-key",
        value_name = "PATH",
        default_value = "keys/minisign.pub",
        help = "Committed minisign public key path"
    )]
    pub minisign_public_key: Utf8PathBuf,
    #[arg(
        long = "rotate-minisign",
        help = "Generate a new encrypted minisign keypair and replace the committed public key"
    )]
    pub rotate_minisign: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReleaseAction {
    #[value(help = "Increment patch: 1.2.3 -> 1.2.4")]
    Patch,
    #[value(help = "Increment minor and reset patch: 1.2.3 -> 1.3.0")]
    Minor,
    #[value(help = "Increment major and reset minor/patch: 1.2.3 -> 2.0.0")]
    Major,
    #[value(help = "Keep the numeric version and replace prerelease metadata")]
    Prerelease,
    #[value(
        name = "sync-up",
        help = "Move the current-version release tag to HEAD"
    )]
    SyncUp,
    #[value(name = "trust", help = "Manage release maintainer trust roots")]
    Trust,
    #[value(
        name = "verify",
        help = "Run the read-only release-bar verification report"
    )]
    Verify,
    #[value(
        name = "plan",
        help = "Print the dependency-ordered publish plan for the current workspace"
    )]
    Plan,
    #[value(name = "secrets", help = "Manage remote release workflow secrets")]
    Secrets,
}

impl ReleaseAction {
    pub fn bump_kind(self) -> Option<BumpKind> {
        match self {
            Self::Patch => Some(BumpKind::Patch),
            Self::Minor => Some(BumpKind::Minor),
            Self::Major => Some(BumpKind::Major),
            Self::Prerelease => Some(BumpKind::Prerelease),
            Self::SyncUp | Self::Trust | Self::Verify | Self::Plan | Self::Secrets => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReleaseTrustAction {
    #[value(help = "Show detected release signing key and trust-root state")]
    Status,
    #[value(help = "Export the maintainer public key into the release trust root")]
    Init,
    #[value(help = "Verify the release trust root exists and matches the signing key")]
    Check,
    #[value(
        name = "inspect-minisign-input",
        help = "Classify minisign import inputs without printing secret values"
    )]
    InspectMinisignInput,
    #[value(name = "contract", help = "Print the release secrets contract")]
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReleaseSecretsAction {
    #[value(help = "Upload or rotate release workflow secrets")]
    Init,
    #[value(help = "Check release workflow secret names without reading values")]
    Check,
    #[value(
        name = "contract",
        help = "Print the generic release credential contract as JSON"
    )]
    Contract,
    #[value(
        name = "inspect-minisign-input",
        help = "Classify minisign import inputs without printing secret values"
    )]
    InspectMinisignInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BumpKind {
    #[value(help = "Increment patch: 1.2.3 -> 1.2.4")]
    Patch,
    #[value(help = "Increment minor and reset patch: 1.2.3 -> 1.3.0")]
    Minor,
    #[value(help = "Increment major and reset minor/patch: 1.2.3 -> 2.0.0")]
    Major,
    #[value(help = "Keep the numeric version and replace prerelease metadata")]
    Prerelease,
}

#[derive(Debug, Args)]
pub struct InitCiCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        conflicts_with = "workspace",
        help = "Workspace package to generate CI for; may be repeated"
    )]
    pub packages: Vec<String>,
    #[arg(long, help = "Generate CI for every package in the workspace")]
    pub workspace: bool,
    #[arg(
        long,
        value_enum,
        value_name = "PLATFORM",
        help = "Hosted forge for Actions workflows, or source forge hint for Crow"
    )]
    pub platform: Option<Platform>,
    #[arg(
        long = "ci-provider",
        value_enum,
        value_name = "PROVIDER",
        help = "CI provider to generate; defaults to Actions for backward compatibility"
    )]
    pub ci_provider: Option<CiProvider>,
    #[arg(
        long = "crow-format",
        value_enum,
        value_name = "FORMAT",
        help = "Crow workflow format when --ci-provider crow is selected"
    )]
    pub crow_format: Option<CrowWorkflowFormat>,
    #[arg(
        long,
        value_enum,
        value_name = "RUNTIME",
        help = "Command runtime used inside generated workflows"
    )]
    pub runtime: Option<RuntimeChoice>,
    #[arg(
        long,
        value_name = "LABEL",
        help = "Override the generated runner label for Linux jobs"
    )]
    pub runner: Option<String>,
    #[arg(
        long = "windows-runner",
        value_name = "LABEL",
        help = "Override the generated runner label for Windows artifact jobs"
    )]
    pub windows_runner: Option<String>,
    #[arg(
        long = "step-runner",
        value_name = "STEP=LABEL",
        help = "Override runner for a specific step key (repeatable, e.g. --step-runner nix-check=atlas-nix-trusted)"
    )]
    pub step_runner: Vec<String>,
    #[arg(
        long,
        help = "Generate granular split of CI steps across Codeberg runner tiers (fmt→tiny, clippy/deny/audit→small, test/doc/package→medium); forgejo-only"
    )]
    pub granular: bool,
    #[arg(
        long = "maintainer-key",
        value_name = "FINGERPRINT",
        help = "OpenPGP key fingerprint to export into keys/maintainers.gpg"
    )]
    pub maintainer_key: Option<String>,
    #[arg(
        long = "maintainers-gpg",
        value_name = "PATH",
        help = "Path to the generated maintainer public keyring"
    )]
    pub maintainers_gpg: Option<Utf8PathBuf>,
    #[arg(
        long = "release-smoke-command",
        value_name = "COMMAND",
        help = "Command to run after signing release artifacts and before publishing"
    )]
    pub release_smoke_command: Option<String>,
    #[arg(long, help = "Verify committed workflow files match generated output")]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds stale files")]
    pub diff: bool,
    #[arg(
        long = "with-nextest",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Use cargo-nextest for test steps; pass `--with-nextest=false` to override project config"
    )]
    pub with_nextest: Option<bool>,
    #[arg(
        long = "with-msrv",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Add an MSRV check using package.rust-version; pass `--with-msrv=false` to override project config"
    )]
    pub with_msrv: Option<bool>,
    #[arg(
        long = "with-audit",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Install and run cargo-audit; pass `--with-audit=false` to override project config"
    )]
    pub with_audit: Option<bool>,
    #[arg(
        long = "with-deny",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Install and run cargo-deny; pass `--with-deny=false` to override project config"
    )]
    pub with_deny: Option<bool>,
    #[arg(
        long = "with-docs",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Build package documentation in CI; pass `--with-docs=false` to override project config"
    )]
    pub with_docs: Option<bool>,
    #[arg(
        long = "with-om-ci",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Replace flake check and cargo dev-shell steps with om ci run (requires --runtime nix)"
    )]
    pub with_om_ci: Option<bool>,
    #[arg(
        long = "om-ci-augment",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Run om ci run alongside the legacy cargo dev-shell steps; implies --with-om-ci"
    )]
    pub om_ci_augment: Option<bool>,
    #[arg(
        long = "omnix-ref",
        value_name = "REF",
        help = "Override the pinned omnix flakeref"
    )]
    pub omnix_ref: Option<String>,
    #[arg(
        long = "with-artifacts",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Generate a tagged-release artifact workflow; pass `--with-artifacts=false` to override project config"
    )]
    pub with_artifacts: Option<bool>,
    #[arg(
        long = "with-pypi-publish",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Generate a PyPI publish workflow for Python uv projects; pass `--with-pypi-publish=false` to override project config"
    )]
    pub with_pypi_publish: Option<bool>,
    #[arg(
        long = "publish-crates",
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        help = "Generate crates.io publish workflows for publishable Rust packages; pass `--publish-crates=false` for CI-only projects"
    )]
    pub publish_crates: Option<bool>,
    #[arg(
        long = "with-homebrew",
        help = "Add a Homebrew tap publishing step (Nix runtime)"
    )]
    pub with_homebrew: bool,
    #[arg(
        long = "with-chocolatey",
        help = "Validate Chocolatey package publishing configuration"
    )]
    pub with_chocolatey: bool,
    #[arg(
        long = "with-scoop",
        help = "Validate Scoop bucket publishing configuration"
    )]
    pub with_scoop: bool,
    #[arg(
        long = "with-codeberg-pages",
        help = "Generate a Pages workflow that publishes .#site with .#deploy-pages"
    )]
    pub with_codeberg_pages: bool,
    #[arg(
        long = "with-vscode",
        help = "Generate a workflow that publishes a VS Code/Open VSX extension"
    )]
    pub with_vscode: bool,
    #[arg(
        long = "with-jetbrains",
        help = "Generate a workflow that publishes an IntelliJ Platform plugin"
    )]
    pub with_jetbrains: bool,
    #[arg(
        long = "pages-repo",
        value_name = "OWNER/REPO",
        requires = "with_codeberg_pages",
        help = "Codeberg repository receiving the generated pages branch"
    )]
    pub pages_repo: Option<String>,
    #[arg(
        long = "pages-canonical-domain",
        value_name = "DOMAIN",
        requires = "with_codeberg_pages",
        help = "Canonical domain that must appear in the generated Pages .domains file"
    )]
    pub pages_canonical_domain: Option<String>,
    #[arg(
        long = "pages-site-output",
        value_name = "INSTALLABLE",
        requires = "with_codeberg_pages",
        help = "Nix installable used to build the Pages site for domain validation"
    )]
    pub pages_site_output: Option<String>,
    #[arg(
        long = "pages-token-secret",
        value_name = "SECRET",
        requires = "with_codeberg_pages",
        help = "Actions secret exposed as CODEBERG_TOKEN for Pages deployment"
    )]
    pub pages_token_secret: Option<String>,
    #[arg(
        long = "pages-source-branch",
        value_name = "BRANCH",
        requires = "with_codeberg_pages",
        help = "Branch whose pushes publish Codeberg Pages"
    )]
    pub pages_source_branch: Option<String>,
    #[arg(
        long = "pages-deploy-app",
        value_name = "INSTALLABLE",
        requires = "with_codeberg_pages",
        help = "Nix app used to build and push the generated Pages branch"
    )]
    pub pages_deploy_app: Option<String>,
    #[command(flatten)]
    pub homebrew: HomebrewOverridesArgs,
    #[command(flatten)]
    pub chocolatey: ChocolateyOverridesArgs,
    #[command(flatten)]
    pub scoop: ScoopOverridesArgs,
}

#[derive(Debug, Args)]
pub struct HomebrewOverridesArgs {
    #[arg(
        long = "homebrew-name",
        value_name = "NAME",
        help = "Homebrew formula name"
    )]
    pub name: Option<String>,
    #[arg(
        long = "homebrew-tap",
        id = "homebrew_tap",
        value_name = "URL",
        help = "Homebrew tap repo URL, e.g. https://codeberg.org/foo/homebrew-bar.git"
    )]
    pub tap: Option<String>,
    #[arg(
        long = "homebrew-binary",
        value_name = "NAME",
        help = "Binary to install via the formula; repeatable"
    )]
    pub binary: Vec<String>,
    #[arg(
        long = "homebrew-description",
        value_name = "TEXT",
        help = "Formula description (<= 80 chars)"
    )]
    pub description: Option<String>,
    #[arg(
        long = "homebrew-homepage",
        value_name = "URL",
        help = "Project homepage URL"
    )]
    pub homepage: Option<String>,
    #[arg(
        long = "homebrew-license",
        value_name = "SPDX",
        help = "SPDX license identifier (defaults to package.license)"
    )]
    pub license: Option<String>,
    #[arg(
        long = "homebrew-download-repo",
        value_name = "OWNER/REPO",
        help = "Codeberg/GitHub owner/repo for release downloads"
    )]
    pub download_repo: Option<String>,
    #[arg(
        long = "homebrew-archive-pattern",
        value_name = "PATTERN",
        help = "Filename pattern for release archives"
    )]
    pub archive_pattern: Option<String>,
    #[arg(
        long = "homebrew-no-platform",
        value_name = "KEY",
        help = "Disable a platform (darwin_arm|darwin_intel|linux_arm|linux_intel); repeatable"
    )]
    pub no_platform: Vec<String>,
}

impl HomebrewOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::HomebrewOverrides<'_> {
        crate::config::HomebrewOverrides {
            name: self.name.as_deref(),
            binaries: Some(&self.binary),
            tap_url: self.tap.as_deref(),
            description: self.description.as_deref(),
            homepage: self.homepage.as_deref(),
            license: self.license.as_deref(),
            download_repo: self.download_repo.as_deref(),
            archive_pattern: self.archive_pattern.as_deref(),
            disabled_platforms: &self.no_platform,
        }
    }
}

#[derive(Debug, Default, Args)]
pub struct ChocolateyOverridesArgs {
    #[arg(
        long = "choco-name",
        id = "choco_name",
        value_name = "NAME",
        help = "Chocolatey package display name"
    )]
    pub name: Option<String>,
    #[arg(
        long = "choco-id",
        id = "choco_id",
        value_name = "ID",
        help = "Chocolatey nuspec package id"
    )]
    pub id: Option<String>,
    #[arg(
        long = "choco-title",
        id = "choco_title",
        value_name = "TEXT",
        help = "Chocolatey nuspec title"
    )]
    pub title: Option<String>,
    #[arg(
        long = "choco-authors",
        id = "choco_authors",
        value_name = "TEXT",
        help = "Chocolatey nuspec authors"
    )]
    pub authors: Option<String>,
    #[arg(
        long = "choco-description",
        id = "choco_description",
        value_name = "TEXT",
        help = "Chocolatey package description"
    )]
    pub description: Option<String>,
    #[arg(
        long = "choco-summary",
        id = "choco_summary",
        value_name = "TEXT",
        help = "Chocolatey package summary"
    )]
    pub summary: Option<String>,
    #[arg(
        long = "choco-project-url",
        id = "choco_project_url",
        value_name = "URL",
        help = "Chocolatey project URL"
    )]
    pub project_url: Option<String>,
    #[arg(
        long = "choco-license-url",
        id = "choco_license_url",
        value_name = "URL",
        help = "Chocolatey license URL"
    )]
    pub license_url: Option<String>,
    #[arg(
        long = "choco-icon-url",
        id = "choco_icon_url",
        value_name = "URL",
        help = "Chocolatey icon URL"
    )]
    pub icon_url: Option<String>,
    #[arg(
        long = "choco-package-source-url",
        id = "choco_package_source_url",
        value_name = "URL",
        help = "Chocolatey package source URL"
    )]
    pub package_source_url: Option<String>,
    #[arg(
        long = "choco-docs-url",
        id = "choco_docs_url",
        value_name = "URL",
        help = "Chocolatey documentation URL"
    )]
    pub docs_url: Option<String>,
    #[arg(
        long = "choco-bug-tracker-url",
        id = "choco_bug_tracker_url",
        value_name = "URL",
        help = "Chocolatey bug tracker URL"
    )]
    pub bug_tracker_url: Option<String>,
    #[arg(
        long = "choco-project-source-url",
        id = "choco_project_source_url",
        value_name = "URL",
        help = "Chocolatey project source URL"
    )]
    pub project_source_url: Option<String>,
    #[arg(
        long = "choco-tags",
        id = "choco_tags",
        value_name = "TEXT",
        help = "Chocolatey package tags"
    )]
    pub tags: Option<String>,
    #[arg(
        long = "choco-release-notes-url",
        id = "choco_release_notes_url",
        value_name = "URL",
        help = "Chocolatey release notes URL"
    )]
    pub release_notes_url: Option<String>,
    #[arg(
        long = "choco-download-repo",
        id = "choco_download_repo",
        value_name = "OWNER/REPO",
        help = "Codeberg/GitHub owner/repo for Chocolatey release downloads"
    )]
    pub download_repo: Option<String>,
    #[arg(
        long = "choco-archive-pattern",
        id = "choco_archive_pattern",
        value_name = "PATTERN",
        help = "Filename pattern for Chocolatey release archives"
    )]
    pub archive_pattern: Option<String>,
    #[arg(
        long = "choco-push-source",
        id = "choco_push_source",
        value_name = "URL",
        help = "Chocolatey push source URL"
    )]
    pub push_source: Option<String>,
}

impl ChocolateyOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::ChocolateyOverrides<'_> {
        crate::config::ChocolateyOverrides {
            name: self.name.as_deref(),
            id: self.id.as_deref(),
            title: self.title.as_deref(),
            authors: self.authors.as_deref(),
            description: self.description.as_deref(),
            summary: self.summary.as_deref(),
            project_url: self.project_url.as_deref(),
            license_url: self.license_url.as_deref(),
            icon_url: self.icon_url.as_deref(),
            package_source_url: self.package_source_url.as_deref(),
            docs_url: self.docs_url.as_deref(),
            bug_tracker_url: self.bug_tracker_url.as_deref(),
            project_source_url: self.project_source_url.as_deref(),
            tags: self.tags.as_deref(),
            release_notes_url: self.release_notes_url.as_deref(),
            download_repo: self.download_repo.as_deref(),
            archive_pattern: self.archive_pattern.as_deref(),
            push_source: self.push_source.as_deref(),
        }
    }
}

#[derive(Debug, Default, Args)]
pub struct ScoopOverridesArgs {
    #[arg(
        long = "scoop-name",
        id = "scoop_name",
        value_name = "NAME",
        help = "Scoop manifest name"
    )]
    pub name: Option<String>,
    #[arg(
        long = "scoop-bucket",
        id = "scoop_bucket",
        value_name = "URL",
        help = "Scoop bucket repo URL"
    )]
    pub bucket: Option<String>,
    #[arg(
        long = "scoop-description",
        id = "scoop_description",
        value_name = "TEXT",
        help = "Scoop manifest description"
    )]
    pub description: Option<String>,
    #[arg(
        long = "scoop-homepage",
        id = "scoop_homepage",
        value_name = "URL",
        help = "Scoop manifest homepage"
    )]
    pub homepage: Option<String>,
    #[arg(
        long = "scoop-license",
        id = "scoop_license",
        value_name = "SPDX",
        help = "Scoop manifest license"
    )]
    pub license: Option<String>,
    #[arg(
        long = "scoop-download-repo",
        id = "scoop_download_repo",
        value_name = "OWNER/REPO",
        help = "Codeberg/GitHub owner/repo for Scoop release downloads"
    )]
    pub download_repo: Option<String>,
    #[arg(
        long = "scoop-archive-pattern",
        id = "scoop_archive_pattern",
        value_name = "PATTERN",
        help = "Filename pattern for Scoop release archives"
    )]
    pub archive_pattern: Option<String>,
    #[arg(
        long = "scoop-binary",
        id = "scoop_binary",
        value_name = "NAME",
        help = "Binary to expose in the Scoop manifest; repeatable"
    )]
    pub binary: Vec<String>,
    #[arg(
        long = "scoop-no-arch",
        id = "scoop_no_arch",
        value_name = "KEY",
        help = "Disable an architecture (x64|arm64); repeatable"
    )]
    pub no_arch: Vec<String>,
}

impl ScoopOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::ScoopOverrides<'_> {
        crate::config::ScoopOverrides {
            name: self.name.as_deref(),
            bucket_url: self.bucket.as_deref(),
            description: self.description.as_deref(),
            homepage: self.homepage.as_deref(),
            license: self.license.as_deref(),
            download_repo: self.download_repo.as_deref(),
            archive_pattern: self.archive_pattern.as_deref(),
            binaries: Some(&self.binary),
            disabled_architectures: &self.no_arch,
        }
    }
}

#[derive(Debug, Args)]
pub struct InitHomebrewTapCommand {
    #[arg(
        long,
        value_name = "DIR",
        help = "Path to the tap repo to bootstrap (created if missing)"
    )]
    pub target: Utf8PathBuf,
    #[arg(
        long,
        help = "Verify the existing Formula/<name>.rb matches the rendered template"
    )]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered formula without writing files")]
    pub print: bool,
    #[arg(long, help = "Skip `git init` and remote wiring (formula write only)")]
    pub no_git: bool,
    #[command(flatten)]
    pub homebrew: HomebrewOverridesArgs,
}

#[derive(Debug, Args)]
pub struct HomebrewCommand {
    #[command(subcommand)]
    pub action: HomebrewAction,
}

#[derive(Debug, Subcommand)]
pub enum HomebrewAction {
    #[command(about = "Render the formula to stdout or a file (no sha256)")]
    Render(HomebrewRenderArgs),
    #[command(about = "Render with real sha256 sums and write to a tap repo")]
    Bump(HomebrewBumpArgs),
}

#[derive(Debug, Args)]
pub struct HomebrewRenderArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Version to render (no leading 'v'). Defaults to cargo package.version"
    )]
    pub version: Option<String>,
    #[arg(long, value_name = "PATH", help = "Write to PATH instead of stdout")]
    pub output: Option<Utf8PathBuf>,
    #[command(flatten)]
    pub homebrew: HomebrewOverridesArgs,
}

#[derive(Debug, Args)]
pub struct HomebrewBumpArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Release version (no leading 'v')"
    )]
    pub version: String,
    #[arg(
        long,
        value_name = "DIR",
        help = "Tap repo directory (Formula/<name>.rb written here)"
    )]
    pub tap: Utf8PathBuf,
    #[arg(
        long,
        value_name = "PLATFORM=PATH",
        help = "Per-platform archive path for sha256 computation"
    )]
    pub archive: Vec<String>,
    #[arg(
        long,
        help = "After writing, run git add + commit + push. Requires git credentials"
    )]
    pub push: bool,
    #[arg(
        long,
        value_name = "MESSAGE",
        help = "Commit message (defaults to '<name> <version>')"
    )]
    pub commit_message: Option<String>,
    #[command(flatten)]
    pub homebrew: HomebrewOverridesArgs,
}

#[derive(Debug, Args)]
pub struct InitScoopBucketCommand {
    #[arg(
        long,
        value_name = "DIR",
        help = "Path to the bucket repo to bootstrap (created if missing)"
    )]
    pub target: Utf8PathBuf,
    #[arg(
        long,
        help = "Verify the existing bucket/<name>.json matches the rendered template"
    )]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered manifest without writing files")]
    pub print: bool,
    #[arg(long, help = "Skip `git init` and remote wiring (manifest write only)")]
    pub no_git: bool,
    #[command(flatten)]
    pub scoop: ScoopOverridesArgs,
}

#[derive(Debug, Args)]
pub struct ScoopCommand {
    #[command(subcommand)]
    pub action: ScoopAction,
}

#[derive(Debug, Subcommand)]
pub enum ScoopAction {
    #[command(about = "Render the manifest to stdout or a file with placeholder hashes")]
    Render(ScoopRenderArgs),
    #[command(about = "Render with real sha256 sums and write to a bucket repo")]
    Bump(ScoopBumpArgs),
}

#[derive(Debug, Args)]
pub struct ScoopRenderArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Version to render (no leading 'v'). Defaults to cargo package.version"
    )]
    pub version: Option<String>,
    #[arg(long, value_name = "PATH", help = "Write to PATH instead of stdout")]
    pub output: Option<Utf8PathBuf>,
    #[command(flatten)]
    pub scoop: ScoopOverridesArgs,
}

#[derive(Debug, Args)]
pub struct ScoopBumpArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Release version (no leading 'v')"
    )]
    pub version: String,
    #[arg(
        long,
        value_name = "DIR",
        help = "Bucket repo directory (bucket/<name>.json written here)"
    )]
    pub bucket: Option<Utf8PathBuf>,
    #[arg(
        long = "bucket-url",
        value_name = "URL",
        help = "Clone this bucket repo URL before writing when --bucket is omitted"
    )]
    pub bucket_url: Option<String>,
    #[arg(
        long = "bucket-token-env",
        value_name = "ENV",
        help = "Environment variable containing a token for authenticated HTTPS bucket pushes"
    )]
    pub bucket_token_env: Option<String>,
    #[arg(
        long = "work-dir",
        value_name = "DIR",
        help = "Working directory for cloned bucket repos (defaults to target/simit-scoop)"
    )]
    pub work_dir: Option<Utf8PathBuf>,
    #[arg(
        long,
        value_name = "ARCH=PATH",
        help = "Per-architecture archive path for sha256 computation"
    )]
    pub archive: Vec<String>,
    #[arg(
        long,
        help = "After writing, run git add + commit + push. Requires git credentials"
    )]
    pub push: bool,
    #[arg(
        long,
        value_name = "MESSAGE",
        help = "Commit message (defaults to '<name> <version>')"
    )]
    pub commit_message: Option<String>,
    #[arg(
        long,
        help = "Print the planned bucket update without writing or pushing"
    )]
    pub dry_run: bool,
    #[command(flatten)]
    pub scoop: ScoopOverridesArgs,
}

#[derive(Debug, Args)]
pub struct WingetCommand {
    #[command(subcommand)]
    pub action: WingetAction,
}

#[derive(Debug, Subcommand)]
pub enum WingetAction {
    #[command(about = "Submit a winget-pkgs manifest update with wingetcreate")]
    Submit(WingetSubmitArgs),
}

#[derive(Debug, Args)]
pub struct WingetSubmitArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Release version (no leading 'v')"
    )]
    pub version: String,
    #[arg(
        long = "package-id",
        value_name = "ID",
        help = "Winget PackageIdentifier"
    )]
    pub package_id: Option<String>,
    #[arg(
        long = "download-repo",
        value_name = "OWNER/REPO",
        help = "Release download repo"
    )]
    pub download_repo: Option<String>,
    #[arg(
        long = "zip-archive",
        value_name = "NAME",
        help = "Windows zip filename or pattern containing {version}"
    )]
    pub zip_archive: Option<String>,
    #[arg(long = "url", value_name = "URL", help = "Installer URL to submit")]
    pub url: Option<String>,
    #[arg(
        long = "token-env",
        value_name = "ENV",
        help = "Environment variable containing the GitHub PAT"
    )]
    pub token_env: Option<String>,
    #[arg(
        long = "wingetcreate",
        value_name = "PATH",
        help = "wingetcreate executable path"
    )]
    pub wingetcreate: Option<Utf8PathBuf>,
    #[arg(
        long = "wingetcreate-url",
        value_name = "URL",
        help = "Download wingetcreate.exe from this URL instead of GitHub latest"
    )]
    pub wingetcreate_url: Option<String>,
    #[arg(
        long = "work-dir",
        value_name = "DIR",
        help = "Working directory for downloaded wingetcreate.exe"
    )]
    pub work_dir: Option<Utf8PathBuf>,
    #[arg(
        long = "wine",
        value_name = "PATH",
        default_value = "wine",
        help = "Wine executable used to run wingetcreate"
    )]
    pub wine: String,
    #[arg(long, help = "Print the wingetcreate command without running it")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct WindowsCommand {
    #[command(subcommand)]
    pub action: WindowsAction,
}

#[derive(Debug, Subcommand)]
pub enum WindowsAction {
    #[command(about = "Publish configured Windows package channels")]
    Publish(WindowsPublishArgs),
}

#[derive(Debug, Args)]
pub struct WindowsPublishArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Release version (no leading 'v')"
    )]
    pub version: String,
    #[arg(
        long,
        value_name = "ARCH=PATH_OR_URL",
        help = "Per-architecture release archive; local paths are required for checksum-based channels"
    )]
    pub archive: Vec<String>,
    #[arg(
        long,
        value_name = "DIR",
        help = "Working directory for generated package state"
    )]
    pub work_dir: Utf8PathBuf,
    #[arg(long, help = "Publish Chocolatey")]
    pub chocolatey: bool,
    #[arg(long, help = "Publish Scoop")]
    pub scoop: bool,
    #[arg(long, help = "Submit Winget")]
    pub winget: bool,
    #[arg(long, help = "Run all configured Windows channels")]
    pub all: bool,
    #[arg(long, help = "Print planned work without writing or publishing")]
    pub dry_run: bool,
    #[arg(long, help = "Bypass Chocolatey duplicate-version protection")]
    pub force_resubmit: bool,
    #[arg(
        long = "choco-push-source",
        value_name = "URL",
        help = "Chocolatey push source override"
    )]
    pub choco_push_source: Option<String>,
    #[arg(
        long = "scoop-bucket-url",
        value_name = "URL",
        help = "Scoop bucket URL override"
    )]
    pub scoop_bucket_url: Option<String>,
    #[arg(
        long = "scoop-bucket-token-env",
        value_name = "ENV",
        help = "Scoop bucket token environment variable override"
    )]
    pub scoop_bucket_token_env: Option<String>,
}

#[derive(Debug, Args)]
pub struct InitChocolateyCommand {
    #[arg(
        long,
        value_name = "DIR",
        help = "Path to the Chocolatey package directory to bootstrap"
    )]
    pub target: Utf8PathBuf,
    #[arg(long, help = "Verify the package files match the rendered template")]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered package files without writing files")]
    pub print: bool,
    #[command(flatten)]
    pub chocolatey: ChocolateyOverridesArgs,
}

#[derive(Debug, Args)]
pub struct ChocolateyCommand {
    #[command(subcommand)]
    pub action: ChocolateyAction,
}

#[derive(Debug, Subcommand)]
pub enum ChocolateyAction {
    #[command(about = "Render package files to stdout or a directory (no sha256)")]
    Render(ChocolateyRenderArgs),
    #[command(about = "Render with real sha256 sums and optionally push")]
    Bump(ChocolateyBumpArgs),
}

#[derive(Debug, Args)]
pub struct ChocolateyRenderArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Version to render (no leading 'v'). Defaults to cargo package.version"
    )]
    pub version: Option<String>,
    #[arg(long, value_name = "DIR", help = "Write package files under DIR")]
    pub output_dir: Option<Utf8PathBuf>,
    #[command(flatten)]
    pub chocolatey: ChocolateyOverridesArgs,
}

#[derive(Debug, Args)]
pub struct ChocolateyBumpArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Release version (no leading 'v')"
    )]
    pub version: String,
    #[arg(
        long,
        value_name = "DIR",
        help = "Chocolatey package directory to update"
    )]
    pub package_dir: Utf8PathBuf,
    #[arg(
        long,
        value_name = "ARCH=PATH",
        help = "Per-architecture archive path for sha256 computation (x64 or x86)"
    )]
    pub archive: Vec<String>,
    #[arg(long, help = "After writing, run choco pack and choco push")]
    pub push: bool,
    #[arg(
        long,
        value_name = "URL",
        help = "Chocolatey push source URL (defaults to config or community feed)"
    )]
    pub push_source: Option<String>,
    #[arg(
        long,
        value_name = "ENV",
        help = "Environment variable containing the Chocolatey API key"
    )]
    pub api_key_env: Option<String>,
    #[arg(
        long,
        help = "Bypass Chocolatey duplicate-version protection before pushing"
    )]
    pub force_resubmit: bool,
    #[arg(long, help = "Print planned work without writing or pushing")]
    pub dry_run: bool,
    #[command(flatten)]
    pub chocolatey: ChocolateyOverridesArgs,
}

#[derive(Debug, Args)]
pub struct AurOverridesArgs {
    #[arg(long = "aur-name", value_name = "NAME", help = "AUR pkgbase")]
    pub name: Option<String>,
    #[arg(
        long = "aur-description",
        value_name = "TEXT",
        help = "Package description (pkgdesc)"
    )]
    pub description: Option<String>,
    #[arg(long = "aur-url", value_name = "URL", help = "Project url")]
    pub url: Option<String>,
    #[arg(
        long = "aur-license",
        value_name = "SPDX",
        help = "SPDX license identifier"
    )]
    pub license: Option<String>,
    #[arg(
        long = "aur-download-repo",
        value_name = "OWNER/REPO",
        help = "Codeberg/GitHub owner/repo for release downloads"
    )]
    pub download_repo: Option<String>,
}

impl AurOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::AurOverrides<'_> {
        crate::config::AurOverrides {
            name: self.name.as_deref(),
            description: self.description.as_deref(),
            url: self.url.as_deref(),
            license: self.license.as_deref(),
            download_repo: self.download_repo.as_deref(),
        }
    }
}

#[derive(Debug, Args)]
pub struct CoprOverridesArgs {
    #[arg(long = "copr-name", value_name = "NAME", help = "RPM package name")]
    pub name: Option<String>,
    #[arg(long = "copr-summary", value_name = "TEXT", help = "RPM Summary")]
    pub summary: Option<String>,
    #[arg(
        long = "copr-description",
        value_name = "TEXT",
        help = "RPM %description prose"
    )]
    pub description: Option<String>,
    #[arg(
        long = "copr-license",
        value_name = "SPDX",
        help = "SPDX license identifier"
    )]
    pub license: Option<String>,
    #[arg(long = "copr-url", value_name = "URL", help = "Project URL")]
    pub url: Option<String>,
    #[arg(
        long = "copr-download-repo",
        value_name = "OWNER/REPO",
        help = "Codeberg/GitHub owner/repo for the source archive"
    )]
    pub download_repo: Option<String>,
    #[arg(
        long = "copr-project",
        value_name = "OWNER/PROJECT",
        help = "COPR project for stable releases"
    )]
    pub project: Option<String>,
}

impl CoprOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::CoprOverrides<'_> {
        crate::config::CoprOverrides {
            name: self.name.as_deref(),
            summary: self.summary.as_deref(),
            description: self.description.as_deref(),
            license: self.license.as_deref(),
            url: self.url.as_deref(),
            download_repo: self.download_repo.as_deref(),
            project: self.project.as_deref(),
        }
    }
}

#[derive(Debug, Args)]
pub struct AptOverridesArgs {
    #[arg(
        long = "apt-repo-url",
        value_name = "URL",
        help = "Git remote of the apt repository"
    )]
    pub repo_url: Option<String>,
    #[arg(
        long = "apt-label",
        value_name = "TEXT",
        help = "reprepro Origin/Label"
    )]
    pub label: Option<String>,
}

impl AptOverridesArgs {
    pub fn as_overrides(&self) -> crate::config::AptOverrides<'_> {
        crate::config::AptOverrides {
            repo_url: self.repo_url.as_deref(),
            label: self.label.as_deref(),
        }
    }
}

#[derive(Debug, Args)]
pub struct InitAurCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package providing metadata fallbacks"
    )]
    pub package: Option<String>,
    #[arg(long, help = "Verify dist/aur/*/PKGBUILD match the rendered template")]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered PKGBUILDs without writing files")]
    pub print: bool,
    #[command(flatten)]
    pub aur: AurOverridesArgs,
}

#[derive(Debug, Args)]
pub struct InitCoprCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package providing metadata fallbacks"
    )]
    pub package: Option<String>,
    #[arg(
        long,
        help = "Verify the spec and .copr/Makefile match the rendered template"
    )]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered files without writing them")]
    pub print: bool,
    #[command(flatten)]
    pub copr: CoprOverridesArgs,
}

#[derive(Debug, Args)]
pub struct InitAptCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package providing metadata fallbacks"
    )]
    pub package: Option<String>,
    #[arg(
        long,
        help = "Verify dist/apt/conf/distributions matches the rendered template"
    )]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered config without writing files")]
    pub print: bool,
    #[command(flatten)]
    pub apt: AptOverridesArgs,
}

#[derive(Debug, Args)]
pub struct InitReleaseCommand {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Workspace package providing metadata fallbacks"
    )]
    pub package: Option<String>,
    #[arg(
        long,
        value_enum,
        help = "Workflow platform; defaults to [ci].platform or forgejo"
    )]
    pub platform: Option<Platform>,
    #[arg(
        long = "ci-provider",
        value_enum,
        value_name = "PROVIDER",
        help = "CI provider for the generated release workflow"
    )]
    pub ci_provider: Option<CiProvider>,
    #[arg(
        long,
        help = "Verify the platform release workflow matches generated output"
    )]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
    #[arg(long, help = "Print the rendered workflow without writing files")]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct AurCommand {
    #[command(subcommand)]
    pub action: AurAction,
}

#[derive(Debug, Subcommand)]
pub enum AurAction {
    #[command(about = "Render PKGBUILDs to stdout for the given version")]
    Render(AurRenderArgs),
}

#[derive(Debug, Args)]
pub struct AurRenderArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Version to render (no leading 'v')"
    )]
    pub version: Option<String>,
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Metadata-fallback package"
    )]
    pub package: Option<String>,
    #[command(flatten)]
    pub aur: AurOverridesArgs,
}

#[derive(Debug, Args)]
pub struct CoprCommand {
    #[command(subcommand)]
    pub action: CoprAction,
}

#[derive(Debug, Subcommand)]
pub enum CoprAction {
    #[command(about = "Render the RPM spec and .copr/Makefile to stdout")]
    Render(CoprRenderArgs),
}

#[derive(Debug, Args)]
pub struct CoprRenderArgs {
    #[arg(
        long,
        value_name = "VERSION",
        help = "Version to render (no leading 'v')"
    )]
    pub version: Option<String>,
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Metadata-fallback package"
    )]
    pub package: Option<String>,
    #[command(flatten)]
    pub copr: CoprOverridesArgs,
}

#[derive(Debug, Args)]
pub struct AptCommand {
    #[command(subcommand)]
    pub action: AptAction,
}

#[derive(Debug, Subcommand)]
pub enum AptAction {
    #[command(about = "Render the reprepro distributions config to stdout")]
    Render(AptRenderArgs),
}

#[derive(Debug, Args)]
pub struct AptRenderArgs {
    #[arg(
        long = "package",
        value_name = "NAME",
        help = "Metadata-fallback package"
    )]
    pub package: Option<String>,
    #[command(flatten)]
    pub apt: AptOverridesArgs,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[value(help = "Forgejo Actions workflows under .forgejo/workflows")]
    Forgejo,
    #[value(help = "GitHub Actions workflows under .github/workflows")]
    Github,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CiProvider {
    #[value(name = "actions", help = "Forgejo or GitHub Actions")]
    Actions,
    #[value(name = "crow", help = "Crow CI workflows under .crow")]
    Crow,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CrowWorkflowFormat {
    Yaml,
    Jsonnet,
}

impl Default for CrowWorkflowFormat {
    fn default() -> Self {
        Self::Yaml
    }
}

impl Platform {
    pub fn workflow_dir(self) -> &'static str {
        match self {
            Self::Forgejo => ".forgejo/workflows",
            Self::Github => ".github/workflows",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forgejo => "forgejo",
            Self::Github => "github",
        }
    }

    pub fn web_base_url(self) -> &'static str {
        match self {
            Self::Forgejo => "https://codeberg.org",
            Self::Github => "https://github.com",
        }
    }

    pub fn api_base_url(self) -> &'static str {
        match self {
            Self::Forgejo => "https://codeberg.org/api/v1",
            Self::Github => "https://api.github.com",
        }
    }

    pub fn release_download_url(self, repo: &str, version: &str, file: &str) -> String {
        format!(
            "{}/{}/releases/download/{}/{}",
            self.web_base_url(),
            repo,
            version,
            file
        )
    }

    pub fn release_tag_url(self, repo: &str, version: &str) -> String {
        format!("{}/{}/releases/tag/{}", self.web_base_url(), repo, version)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeChoice {
    #[value(help = "Use simit's default runtime choice for the target platform")]
    Auto,
    #[value(help = "Use direct cargo commands and Rust toolchain setup")]
    Cargo,
    #[value(help = "Use nix develop and flake checks; requires flake.nix")]
    Nix,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Cargo,
    Nix,
}

#[derive(Debug, Args)]
pub struct InitFlakeCommand {
    #[arg(
        long,
        value_enum,
        help = "Generated ownership scope: hooks-only manages nix/pre-commit.nix; full also manages flake.nix and formatter wiring"
    )]
    pub scope: Option<FlakeScopeArg>,
    #[arg(long, help = "Verify flake and hook files match generated output")]
    pub check: bool,
    #[arg(
        long,
        help = "Print generated flake and hook files without writing them"
    )]
    pub print: bool,
    #[arg(long, help = "Show a unified diff when --check finds stale files")]
    pub diff: bool,
    #[arg(
        long,
        help = "Emit an rs-harbor multi-target flake that builds via rs-harbor.lib.mkCrossPackages"
    )]
    pub cross: bool,
    #[arg(
        long = "target",
        value_enum,
        value_name = "TARGET",
        requires = "cross",
        action = clap::ArgAction::Append,
        help = "Cross target to build; repeatable. Defaults to all targets when --cross is set without any --target"
    )]
    pub targets: Vec<FlakeTargetArg>,
}

#[derive(Debug, Args)]
pub struct InitGitignoreCommand {
    #[arg(long, help = "Verify .gitignore matches the generated template")]
    pub check: bool,
    #[arg(long, help = "Print the generated .gitignore without writing it")]
    pub print: bool,
    #[arg(long, help = "Show a unified diff when --check finds drift")]
    pub diff: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlakeScopeArg {
    HooksOnly,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlakeTargetArg {
    #[value(help = "Native craneLib.buildPackage for the host system")]
    Native,
    #[value(
        name = "aarch64-linux",
        help = "Cross-compile to aarch64-unknown-linux-gnu"
    )]
    Aarch64Linux,
    #[value(help = "Cross-compile to x86_64-pc-windows-gnu via mingw")]
    Windows,
    #[value(
        name = "darwin-x86_64",
        help = "Cross-compile to x86_64-apple-darwin via osxcross"
    )]
    DarwinX86_64,
    #[value(
        name = "darwin-aarch64",
        help = "Cross-compile to aarch64-apple-darwin via osxcross"
    )]
    DarwinAarch64,
}

impl FlakeTargetArg {
    /// The mkCrossPackages target name (the key in the `targets = [ ... ]` list).
    pub fn key(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Aarch64Linux => "aarch64-linux",
            Self::Windows => "windows",
            Self::DarwinX86_64 => "darwin-x86_64",
            Self::DarwinAarch64 => "darwin-aarch64",
        }
    }

    /// Every target, in canonical order. Used as the default when `--cross` is
    /// passed without any explicit `--target`.
    pub fn all() -> [Self; 5] {
        [
            Self::Native,
            Self::Aarch64Linux,
            Self::Windows,
            Self::DarwinX86_64,
            Self::DarwinAarch64,
        ]
    }
}

#[derive(Debug, Args)]
pub struct ChangelogCommand {
    #[arg(
        long,
        default_value = "CHANGELOG.md",
        value_name = "PATH",
        help = "Path to the changelog file"
    )]
    pub file: Utf8PathBuf,
    #[command(subcommand)]
    pub action: ChangelogAction,
}

#[derive(Debug, Args)]
pub struct HooksCommand {
    #[command(subcommand, help = "Hooks action to run")]
    pub action: HooksAction,
}

#[derive(Debug, Subcommand)]
pub enum HooksAction {
    #[command(about = "Install pre-commit hooks into this repository")]
    Install(HooksInstallCommand),
}

#[derive(Debug, Args)]
pub struct HooksInstallCommand {
    #[arg(long, help = "Verify installed hook wrappers without changing files")]
    pub check: bool,
    #[arg(long, help = "Show a unified diff for stale hook wrappers")]
    pub diff: bool,
    #[arg(
        long,
        help = "Unset rogue repo-local core.hooksPath values that shadow a friendly system dispatcher (no-op otherwise)"
    )]
    pub fix: bool,
}

#[derive(Debug, Args)]
pub struct ConfigCommand {
    #[command(subcommand, help = "User config action to run")]
    pub action: ConfigAction,
}

#[derive(Debug, Args)]
pub struct ProjectsCommand {
    #[command(subcommand, help = "Project registry action to run")]
    pub action: ProjectsAction,
}

#[derive(Debug, Subcommand)]
pub enum ProjectsAction {
    #[command(about = "List registered projects")]
    List(ProjectsListArgs),
    #[command(about = "Show one registered project's feature status")]
    Show(ProjectsShowArgs),
    #[command(about = "Re-run feature detection across registered projects")]
    Scan(ProjectsScanArgs),
    #[command(about = "Audit live project CI without reading or writing the registry")]
    Audit(ProjectsAuditArgs),
    #[command(about = "Discover Rust workspaces under a filesystem subtree")]
    Discover(ProjectsDiscoverArgs),
    #[command(about = "Forget one registered project")]
    Forget(ProjectsForgetArgs),
    #[command(about = "Remove registered projects whose paths no longer exist")]
    Prune(ProjectsPruneArgs),
    #[command(about = "Clear all per-user project registry state")]
    ClearState(ProjectsClearStateArgs),
}

#[derive(Debug, Args)]
pub struct ProjectsListArgs {
    #[arg(
        long = "feature",
        value_name = "NAME[=STATUS]",
        action = clap::ArgAction::Append,
        help = "Filter to projects where feature NAME has STATUS, or any non-absent status"
    )]
    pub features: Vec<String>,
    #[arg(
        long = "include-ephemeral",
        help = "Include ephemeral /tmp project paths in the listing"
    )]
    pub include_ephemeral: bool,
    #[arg(long, help = "Print a machine-readable JSON array")]
    pub json: bool,
    #[arg(
        long,
        conflicts_with = "no_issues",
        help = "Mark projects with drift, hook conflicts, or missing paths and print an issues footer"
    )]
    pub issues: bool,
    #[arg(
        long = "no-issues",
        help = "Suppress issue markers and footer in human output"
    )]
    pub no_issues: bool,
    #[arg(
        long,
        value_enum,
        value_name = "KEY",
        default_value = "last-seen",
        help = "Sort by name, path, last-seen, or first-seen"
    )]
    pub sort: ProjectsSort,
}

#[derive(Debug, Args)]
pub struct ProjectsShowArgs {
    #[arg(
        value_name = "PATH",
        help = "Project path; defaults to the current workspace root"
    )]
    pub path: Option<Utf8PathBuf>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub json: bool,
    #[arg(
        long,
        conflicts_with = "no_issues",
        help = "Print an attention header for drift, hook conflicts, or missing paths"
    )]
    pub issues: bool,
    #[arg(long = "no-issues", help = "Suppress the attention header")]
    pub no_issues: bool,
}

#[derive(Debug, Args)]
pub struct ProjectsScanArgs {
    #[arg(long, help = "Also drop entries whose paths no longer exist")]
    pub prune: bool,
    #[arg(long, help = "Print intended changes without writing the registry")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct ProjectsAuditArgs {
    #[arg(
        value_name = "PATH",
        help = "Project path; defaults to the current workspace root; may be repeated"
    )]
    pub paths: Vec<Utf8PathBuf>,
    #[arg(long, help = "Print a stable machine-readable JSON report")]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ProjectsDiscoverArgs {
    #[arg(
        value_name = "ROOT",
        help = "Filesystem subtree to walk; defaults to the current directory"
    )]
    pub root: Option<Utf8PathBuf>,
    #[arg(
        long,
        help = "Walk and report discoverable projects without writing the registry"
    )]
    pub dry_run: bool,
    #[arg(long, help = "Print the discovery report as machine-readable JSON")]
    pub json: bool,
    #[arg(
        long,
        value_name = "NAME",
        action = clap::ArgAction::Append,
        help = "Also skip directories with this basename, in addition to built-in skips; may be repeated"
    )]
    pub skip: Vec<String>,
    #[arg(
        long,
        value_name = "N",
        help = "Maximum recursion depth from ROOT; defaults to 8"
    )]
    pub max_depth: Option<usize>,
    #[arg(long, help = "Follow symlinked directories during discovery")]
    pub follow_symlinks: bool,
    #[arg(
        long,
        help = "Register Cargo workspaces even when no simit features are detected"
    )]
    pub include_empty: bool,
}

#[derive(Debug, Args)]
pub struct ProjectsForgetArgs {
    #[arg(value_name = "PATH", help = "Project path to remove from the registry")]
    pub path: Utf8PathBuf,
}

#[derive(Debug, Args)]
pub struct ProjectsPruneArgs {
    #[arg(long, help = "Print stale entries without writing the registry")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct ProjectsClearStateArgs {
    #[arg(
        long,
        help = "Print the registry path that would be cleared without writing"
    )]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProjectsSort {
    Name,
    Path,
    #[value(name = "last-seen")]
    LastSeen,
    #[value(name = "first-seen")]
    FirstSeen,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    #[command(about = "Print the resolved user config path")]
    Path,
    #[command(about = "Print the normalized user config")]
    Show,
    #[command(about = "Validate the user config")]
    Check,
    #[command(about = "Write a starter user config if one does not exist")]
    Init,
}

#[derive(Debug, Subcommand)]
pub enum ChangelogAction {
    #[command(about = "Create a Keep a Changelog skeleton")]
    Init,
    #[command(about = "Add an entry under [Unreleased]")]
    Add {
        #[arg(value_enum, value_name = "KIND", help = "Entry kind to append")]
        kind: ChangelogEntryKind,
        #[arg(value_name = "TEXT", help = "Entry text to add")]
        text: String,
    },
    #[command(about = "Promote [Unreleased] into a dated release section")]
    Release {
        #[arg(value_name = "VERSION", help = "Release version to create")]
        version: Version,
        #[arg(
            long,
            value_name = "YYYY-MM-DD",
            help = "Override the release date instead of using today in UTC"
        )]
        date: Option<String>,
        #[arg(
            long = "repo-url",
            value_name = "URL",
            help = "Repository URL used for compare links"
        )]
        repo_url: Option<String>,
    },
    #[command(about = "Validate that the file matches simit's Keep a Changelog rules")]
    Check,
    #[command(about = "Print the body of one changelog section")]
    Show {
        #[arg(
            value_name = "VERSION",
            help = "Version to print; omit to show [Unreleased]"
        )]
        version: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ChangelogEntryKind {
    Added,
    Changed,
    Deprecated,
    Removed,
    Fixed,
    Security,
}

#[derive(Debug, Args)]
pub struct CompletionsCommand {
    #[arg(
        value_enum,
        value_name = "SHELL",
        help = "Shell to generate completions for"
    )]
    pub shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
pub struct ManCommand {}
