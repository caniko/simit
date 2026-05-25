//! User-scoped simit configuration.
//!
//! Project config describes portable release/package settings. User config
//! describes local infrastructure, such as private runner labels.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{Platform, Runtime};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct UserConfig {
    pub ci: UserCiConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct UserCiConfig {
    pub runners: BTreeMap<String, UserRunnerConfig>,
    pub defaults: BTreeMap<String, UserCiPlatformDefaults>,
    pub tools: CiTools,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CiTools {
    pub omnix: OmnixToolConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OmnixToolConfig {
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UserRunnerConfig {
    pub platform: PlatformName,
    pub labels: Vec<String>,
    pub os: RunnerOs,
    pub arch: String,
    pub runtimes: Vec<RunnerRuntime>,
    #[serde(default)]
    pub trusted: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct UserCiPlatformDefaults {
    pub cargo: Option<String>,
    pub nix: Option<String>,
    pub release: Option<String>,
    pub windows: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformName {
    Forgejo,
    Github,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerOs {
    Linux,
    Windows,
    Macos,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerRuntime {
    Cargo,
    Nix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRunner {
    pub name: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCiRunners {
    pub ci: ResolvedRunner,
    pub release: ResolvedRunner,
    pub windows: Option<ResolvedRunner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunnerKind {
    Cargo,
    Nix,
    Release,
    Windows,
}

impl UserConfig {
    pub fn path() -> PathBuf {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
        {
            return PathBuf::from(config_home).join("simit/config.toml");
        }

        let home = env::var_os("HOME").unwrap_or_else(|| ".".into());
        PathBuf::from(home).join(".config/simit/config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "reading simit user config at {}; run `simit config init` to create a starter config",
                path.display()
            )
        })?;
        let config: Self = toml_edit::de::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn write_starter() -> Result<PathBuf> {
        let path = Self::path();
        if path.exists() {
            bail!("simit user config already exists at {}", path.display());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, starter_config())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    pub fn validate(&self) -> Result<()> {
        for (name, runner) in &self.ci.runners {
            validate_runner_name(name)?;
            runner.validate(name)?;
        }

        for (platform, defaults) in &self.ci.defaults {
            let platform_name = parse_platform_name(platform)
                .with_context(|| format!("validating [ci.defaults.{platform}]"))?;
            defaults.validate(platform_name, &self.ci.runners)?;
        }

        Ok(())
    }

    pub fn resolve_ci_runners(
        &self,
        platform: Platform,
        runtime: Runtime,
        runner_override: Option<&str>,
        windows_runner_override: Option<&str>,
        needs_windows: bool,
    ) -> Result<ResolvedCiRunners> {
        let ci = if let Some(label) = runner_override {
            ResolvedRunner::literal(label)?
        } else {
            self.default_runner(
                platform,
                RunnerKind::for_runtime(runtime),
                RunnerOs::Linux,
                runtime,
            )?
        };

        let release = if let Some(label) = runner_override {
            ResolvedRunner::literal(label)?
        } else {
            let release_kind = match runtime {
                Runtime::Cargo => RunnerKind::Release,
                Runtime::Nix => RunnerKind::Nix,
            };
            self.default_runner(platform, release_kind, RunnerOs::Linux, runtime)?
        };

        let windows = if needs_windows {
            Some(if let Some(label) = windows_runner_override {
                ResolvedRunner::literal(label)?
            } else {
                self.default_runner(
                    platform,
                    RunnerKind::Windows,
                    RunnerOs::Windows,
                    Runtime::Cargo,
                )?
            })
        } else {
            None
        };

        Ok(ResolvedCiRunners {
            ci,
            release,
            windows,
        })
    }

    fn default_runner(
        &self,
        platform: Platform,
        kind: RunnerKind,
        os: RunnerOs,
        runtime: Runtime,
    ) -> Result<ResolvedRunner> {
        if platform == Platform::Github {
            if let Some(runner) = self.configured_default(platform, kind, os, runtime)? {
                return Ok(runner);
            }
            return Ok(github_builtin_runner(os));
        }

        self.configured_default(platform, kind, os, runtime)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Forgejo runner default `{}` is not configured in simit user config at {}; run `simit config init`, edit [ci.runners] and [ci.defaults.forgejo], then validate with `simit config check`",
                kind.key(),
                Self::path().display()
            )
        })
    }

    fn configured_default(
        &self,
        platform: Platform,
        kind: RunnerKind,
        os: RunnerOs,
        runtime: Runtime,
    ) -> Result<Option<ResolvedRunner>> {
        let Some(defaults) = self.ci.defaults.get(platform.as_str()) else {
            return Ok(None);
        };
        let Some(name) = kind.default_name(defaults) else {
            return Ok(None);
        };
        let runner = self.ci.runners.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "[ci.defaults.{}].{} references unknown runner `{}`",
                platform.as_str(),
                kind.key(),
                name
            )
        })?;
        runner.validate_for(name, platform.into(), os, runtime)?;
        Ok(Some(ResolvedRunner {
            name: Some(name.clone()),
            labels: runner.labels.clone(),
        }))
    }

    pub fn to_toml_pretty(&self) -> Result<String> {
        toml_edit::ser::to_string_pretty(self).context("serializing simit user config")
    }
}

impl UserRunnerConfig {
    fn validate(&self, name: &str) -> Result<()> {
        if self.labels.is_empty() {
            bail!("runner `{name}` must define at least one label");
        }
        for label in &self.labels {
            validate_runner_label(label)
                .with_context(|| format!("validating label in runner `{name}`"))?;
        }
        if self.arch.trim().is_empty() {
            bail!("runner `{name}` arch cannot be empty");
        }
        if self.runtimes.is_empty() {
            bail!("runner `{name}` must support at least one runtime");
        }
        let mut seen = BTreeSet::new();
        for runtime in &self.runtimes {
            if !seen.insert(*runtime) {
                bail!("runner `{name}` repeats runtime `{runtime}`");
            }
        }
        Ok(())
    }

    fn validate_for(
        &self,
        name: &str,
        platform: PlatformName,
        os: RunnerOs,
        runtime: Runtime,
    ) -> Result<()> {
        if self.platform != platform {
            bail!(
                "runner `{name}` targets platform `{}`, but `{}` is required",
                self.platform,
                platform
            );
        }
        if self.os != os {
            bail!(
                "runner `{name}` targets os `{}`, but `{}` is required",
                self.os,
                os
            );
        }
        let runtime = RunnerRuntime::from(runtime);
        if !self.runtimes.contains(&runtime) {
            bail!("runner `{name}` does not support runtime `{runtime}`");
        }
        Ok(())
    }
}

impl UserCiPlatformDefaults {
    fn validate(
        &self,
        platform: PlatformName,
        runners: &BTreeMap<String, UserRunnerConfig>,
    ) -> Result<()> {
        for (kind, runner_name, os, runtime) in [
            (
                RunnerKind::Cargo,
                self.cargo.as_ref(),
                RunnerOs::Linux,
                Runtime::Cargo,
            ),
            (
                RunnerKind::Nix,
                self.nix.as_ref(),
                RunnerOs::Linux,
                Runtime::Nix,
            ),
            (
                RunnerKind::Release,
                self.release.as_ref(),
                RunnerOs::Linux,
                Runtime::Cargo,
            ),
            (
                RunnerKind::Windows,
                self.windows.as_ref(),
                RunnerOs::Windows,
                Runtime::Cargo,
            ),
        ] {
            let Some(runner_name) = runner_name else {
                continue;
            };
            let runner = runners.get(runner_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "[ci.defaults.{}].{} references unknown runner `{}`",
                    platform,
                    kind.key(),
                    runner_name
                )
            })?;
            runner.validate_for(runner_name, platform, os, runtime)?;
        }
        Ok(())
    }
}

impl ResolvedRunner {
    fn literal(label: &str) -> Result<Self> {
        validate_runner_label(label)?;
        Ok(Self {
            name: None,
            labels: vec![label.to_owned()],
        })
    }
}

impl RunnerKind {
    fn for_runtime(runtime: Runtime) -> Self {
        match runtime {
            Runtime::Cargo => Self::Cargo,
            Runtime::Nix => Self::Nix,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Nix => "nix",
            Self::Release => "release",
            Self::Windows => "windows",
        }
    }

    fn default_name(self, defaults: &UserCiPlatformDefaults) -> Option<&String> {
        match self {
            Self::Cargo => defaults.cargo.as_ref(),
            Self::Nix => defaults.nix.as_ref(),
            Self::Release => defaults.release.as_ref(),
            Self::Windows => defaults.windows.as_ref(),
        }
    }
}

impl From<Platform> for PlatformName {
    fn from(value: Platform) -> Self {
        match value {
            Platform::Forgejo => Self::Forgejo,
            Platform::Github => Self::Github,
        }
    }
}

impl From<Runtime> for RunnerRuntime {
    fn from(value: Runtime) -> Self {
        match value {
            Runtime::Cargo => Self::Cargo,
            Runtime::Nix => Self::Nix,
        }
    }
}

impl fmt::Display for PlatformName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Forgejo => "forgejo",
            Self::Github => "github",
        })
    }
}

impl fmt::Display for RunnerOs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Macos => "macos",
        })
    }
}

impl fmt::Display for RunnerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cargo => "cargo",
            Self::Nix => "nix",
        })
    }
}

fn github_builtin_runner(os: RunnerOs) -> ResolvedRunner {
    let label = match os {
        RunnerOs::Linux => "ubuntu-latest",
        RunnerOs::Windows => "windows-latest",
        RunnerOs::Macos => "macos-latest",
    };
    ResolvedRunner {
        name: None,
        labels: vec![label.to_owned()],
    }
}

fn parse_platform_name(value: &str) -> Result<PlatformName> {
    match value {
        "forgejo" => Ok(PlatformName::Forgejo),
        "github" => Ok(PlatformName::Github),
        _ => bail!("unsupported platform `{value}`; expected `forgejo` or `github`"),
    }
}

fn validate_runner_name(value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("runner name cannot be empty");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("runner name `{value}` may only contain ASCII letters, digits, '.', '_', and '-'");
    }
    Ok(())
}

pub fn validate_runner_label(value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("runner label cannot be empty");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("runner label `{value}` may only contain ASCII letters, digits, '.', '_', and '-'");
    }
    Ok(())
}

fn starter_config() -> &'static str {
    r#"[ci.runners.forgejo_linux]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.forgejo_windows]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "forgejo_linux"
nix = "forgejo_linux"
release = "forgejo_linux"
windows = "forgejo_windows"
"#
}
