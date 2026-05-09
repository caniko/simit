use std::ffi::OsString;

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    #[command(about = "Generate or verify Nix-integrated formatter and pre-commit hooks")]
    InitHooks(InitHooksCommand),
    #[command(about = "Generate or verify a canonical Rust crane flake")]
    InitFlake(InitFlakeCommand),
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
        help = "Release commit message and fallback changelog entry"
    )]
    pub message: String,
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
pub struct InitHooksCommand {
    #[arg(long, help = "Verify committed hook files match generated output")]
    pub check: bool,
    #[arg(long, help = "Print generated hook files without writing them")]
    pub print: bool,
    #[arg(long, help = "Show a unified diff when --check finds stale files")]
    pub diff: bool,
}

#[derive(Debug, Args)]
pub struct InitFlakeCommand {
    #[arg(long, help = "Verify flake.nix matches the generated template")]
    pub check: bool,
    #[arg(long, help = "Print the generated flake without writing it")]
    pub print: bool,
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
