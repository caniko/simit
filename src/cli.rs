use std::ffi::OsString;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use semver::Version;

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
    #[command(about = "Generate or verify Rust CI workflows")]
    InitCi(InitCiCommand),
    #[command(about = "Generate or verify a canonical Rust crane flake and hook wiring")]
    InitFlake(InitFlakeCommand),
    #[command(about = "Manage a Keep a Changelog file")]
    Changelog(ChangelogCommand),
    #[command(about = "Print shell completion scripts")]
    Completions(CompletionsCommand),
    #[command(about = "Print a roff manpage for simit")]
    Man(ManCommand),
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
    #[arg(value_enum, value_name = "BUMP", help = "Version bump to apply")]
    pub bump: BumpKind,
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
    pub message: String,
    #[arg(
        long = "no-changelog",
        help = "Skip promoting CHANGELOG.md even when it exists"
    )]
    pub no_changelog: bool,
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
        long,
        value_enum,
        value_name = "PLATFORM",
        help = "Workflow platform to generate"
    )]
    pub platform: Platform,
    #[arg(
        long,
        value_enum,
        value_name = "RUNTIME",
        default_value_t = RuntimeChoice::Auto,
        help = "Command runtime used inside generated workflows"
    )]
    pub runtime: RuntimeChoice,
    #[arg(
        long,
        value_name = "LABEL",
        help = "Override the generated runner label for all jobs"
    )]
    pub runner: Option<String>,
    #[arg(long, help = "Verify committed workflow files match generated output")]
    pub check: bool,
    #[arg(long, help = "Show a unified diff when --check finds stale files")]
    pub diff: bool,
    #[arg(long = "with-nextest", help = "Use cargo-nextest for test steps")]
    pub with_nextest: bool,
    #[arg(
        long = "with-msrv",
        help = "Add an MSRV check using package.rust-version"
    )]
    pub with_msrv: bool,
    #[arg(long = "with-audit", help = "Install and run cargo-audit")]
    pub with_audit: bool,
    #[arg(long = "with-deny", help = "Install and run cargo-deny")]
    pub with_deny: bool,
    #[arg(long = "with-docs", help = "Build package documentation in CI")]
    pub with_docs: bool,
    #[arg(
        long = "with-artifacts",
        help = "Generate a tagged-release artifact workflow"
    )]
    pub with_artifacts: bool,
    #[arg(
        long = "with-homebrew",
        help = "Add a Homebrew tap publishing step (forgejo + nix only)"
    )]
    pub with_homebrew: bool,
    #[arg(
        long = "homebrew-tap",
        value_name = "URL",
        requires = "with_homebrew",
        help = "Homebrew tap repo URL, e.g. https://codeberg.org/foo/homebrew-bar.git"
    )]
    pub homebrew_tap: Option<String>,
    #[arg(
        long = "homebrew-binary",
        value_name = "NAME",
        requires = "with_homebrew",
        help = "Binary to install via the formula; repeatable"
    )]
    pub homebrew_binary: Vec<String>,
    #[arg(
        long = "homebrew-description",
        value_name = "TEXT",
        requires = "with_homebrew",
        help = "Formula description (<= 80 chars)"
    )]
    pub homebrew_description: Option<String>,
    #[arg(
        long = "homebrew-homepage",
        value_name = "URL",
        requires = "with_homebrew",
        help = "Project homepage URL"
    )]
    pub homebrew_homepage: Option<String>,
    #[arg(
        long = "homebrew-license",
        value_name = "SPDX",
        requires = "with_homebrew",
        help = "SPDX license identifier (defaults to package.license)"
    )]
    pub homebrew_license: Option<String>,
    #[arg(
        long = "homebrew-download-repo",
        value_name = "OWNER/REPO",
        requires = "with_homebrew",
        help = "Codeberg/GitHub owner/repo for release downloads"
    )]
    pub homebrew_download_repo: Option<String>,
    #[arg(
        long = "homebrew-archive-pattern",
        value_name = "PATTERN",
        requires = "with_homebrew",
        default_value = "{name}-{version}-{arch}-{os}.tar.gz",
        help = "Filename pattern for release archives"
    )]
    pub homebrew_archive_pattern: String,
    #[arg(
        long = "homebrew-no-platform",
        value_name = "KEY",
        requires = "with_homebrew",
        help = "Disable a platform (darwin_arm|darwin_intel|linux_arm|linux_intel); repeatable"
    )]
    pub homebrew_no_platform: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Platform {
    #[value(help = "Forgejo Actions workflows under .forgejo/workflows")]
    Forgejo,
    #[value(help = "GitHub Actions workflows under .github/workflows")]
    Github,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RuntimeChoice {
    #[value(help = "Use simit's default runtime choice for the target platform")]
    Auto,
    #[value(help = "Use direct cargo commands and Rust toolchain setup")]
    Cargo,
    #[value(help = "Use nix develop and flake checks; requires flake.nix")]
    Nix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Cargo,
    Nix,
}

#[derive(Debug, Args)]
pub struct InitFlakeCommand {
    #[arg(long, help = "Verify flake and hook files match generated output")]
    pub check: bool,
    #[arg(
        long,
        help = "Print generated flake and hook files without writing them"
    )]
    pub print: bool,
    #[arg(long, help = "Show a unified diff when --check finds stale files")]
    pub diff: bool,
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
