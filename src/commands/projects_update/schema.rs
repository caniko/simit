use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub workspaces: BTreeMap<String, Workspace>,
    pub forges: BTreeMap<String, Forge>,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    pub root: PathBuf,
    pub state_directory: PathBuf,
    pub inventory: InventorySource,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InventorySource {
    pub file: Option<PathBuf>,
    pub command: Option<Command>,
    #[serde(default = "inventory_age")]
    pub max_age_seconds: u64,
}
fn inventory_age() -> u64 {
    300
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub program: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default = "command_timeout")]
    pub timeout_seconds: u64,
}
fn command_timeout() -> u64 {
    120
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Forge {
    pub host: String,
    pub api_url: String,
    pub credential_file: Option<PathBuf>,
    pub adapter: Command,
    pub adapter_sha256: Option<String>,
    #[serde(default)]
    pub adapter_inputs: BTreeMap<PathBuf, String>,
    /// Provider-specific check identity, interpreted by the enrolled adapter.
    pub check_authority: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub workspace: String,
    pub generator: Generator,
    #[serde(default)]
    pub select: Selection,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub rollout: Rollout,
    #[serde(default)]
    pub project_overrides: BTreeMap<String, Override>,
    /// Explicit initial baseline enrollment; subsequent rollouts use adopted receipts.
    #[serde(default)]
    pub enroll_current: bool,
    #[serde(default)]
    pub required_checks: Vec<Requirement>,
    /// `direct` pushes the target branch; `pull-request` has the updater merge
    /// a pull request. Omitted when direct, so existing policy digests hold.
    #[serde(default = "direct", skip_serializing_if = "is_direct")]
    pub landing: String,
}
fn direct() -> String {
    "direct".into()
}
fn is_direct(landing: &str) -> bool {
    landing == "direct"
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generator {
    pub executable: PathBuf,
    pub source_revision: String,
    pub source_coordinate: String,
    pub forge: String,
    pub executable_sha256: Option<String>,
    pub required_checks: Vec<Requirement>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    #[serde(default = "owned")]
    pub kinds: Vec<String>,
    #[serde(default = "active")]
    pub lifecycles: Vec<String>,
    #[serde(default = "normal")]
    pub access: Vec<String>,
    #[serde(default = "canonical")]
    pub checkout_roles: Vec<String>,
    #[serde(default = "all_paths")]
    pub include_paths: Vec<String>,
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    #[serde(default)]
    pub coordinates: Vec<String>,
}
fn owned() -> Vec<String> {
    vec!["owned-repo".into()]
}
fn active() -> Vec<String> {
    vec!["active".into()]
}
fn normal() -> Vec<String> {
    vec!["normal".into()]
}
fn canonical() -> Vec<String> {
    vec!["canonical".into()]
}
fn all_paths() -> Vec<String> {
    vec!["**".into()]
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            kinds: owned(),
            lifecycles: active(),
            access: normal(),
            checkout_roles: canonical(),
            include_paths: all_paths(),
            exclude_paths: vec![],
            coordinates: vec![],
        }
    }
}

impl Selection {
    pub fn matches(&self, project: &Project) -> Result<bool> {
        Ok(self.exclusion_reason(project)?.is_none())
    }

    pub fn exclusion_reason(&self, project: &Project) -> Result<Option<&'static str>> {
        let matches = |patterns: &[String], value: &str| -> Result<bool> {
            Ok(patterns
                .iter()
                .map(|p| glob::Pattern::new(p))
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .any(|p| {
                    p.matches_with(
                        value,
                        glob::MatchOptions {
                            case_sensitive: true,
                            require_literal_separator: true,
                            require_literal_leading_dot: false,
                        },
                    )
                }))
        };
        Ok([
            (self.kinds.contains(&project.kind), "kind"),
            (self.lifecycles.contains(&project.lifecycle), "lifecycle"),
            (self.access.contains(&project.access), "access"),
            (
                self.checkout_roles.contains(&project.checkout_role),
                "checkout-role",
            ),
            (
                !matches!(project.simit_ci.as_str(), "excluded" | "custom"),
                "ci-policy",
            ),
            (
                matches(&self.include_paths, &project.relative_path)?,
                "include-path",
            ),
            (
                !matches(&self.exclude_paths, &project.relative_path)?,
                "exclude-path",
            ),
            (
                self.coordinates.contains(&project.coordinate),
                "coordinate-enrollment",
            ),
        ]
        .into_iter()
        .find_map(|(accepted, reason)| (!accepted).then_some(reason)))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rollout {
    #[serde(default)]
    pub canaries: Vec<String>,
    #[serde(default = "one")]
    pub batch_size: u32,
    #[serde(default = "one")]
    pub max_concurrent: u32,
    #[serde(default = "yes")]
    pub stop_on_failure: bool,
    #[serde(default = "maintenance_prefix")]
    pub maintenance_ref_prefix: String,
}
fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}
pub(crate) fn maintenance_prefix() -> String {
    "simit/update/".into()
}
impl Default for Rollout {
    fn default() -> Self {
        Self {
            canaries: vec![],
            batch_size: 1,
            max_concurrent: 1,
            stop_on_failure: true,
            maintenance_ref_prefix: maintenance_prefix(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Override {
    #[serde(default = "yes")]
    pub enable: bool,
    pub reason: Option<String>,
    pub target_branch: Option<String>,
    #[serde(default)]
    pub required_checks: Vec<Requirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requirement {
    pub name: String,
    pub authority: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    pub schema_version: u32,
    pub root: PathBuf,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub producer: String,
    pub policy_digest: String,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    pub coordinate: String,
    pub forge: String,
    pub relative_path: String,
    pub kind: String,
    pub lifecycle: String,
    pub access: String,
    pub checkout_role: String,
    pub simit_ci: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
}

impl Config {
    pub fn state_directory(&self, workspace: &Workspace) -> Result<PathBuf> {
        let state = resolved_path(&workspace.state_directory)?;
        for source in self.workspaces.values() {
            let root = resolved_path(&source.root)?;
            ensure!(
                !state.starts_with(&root) && !root.starts_with(&state),
                "resolved update state and source workspace trees must be disjoint"
            );
        }
        Ok(state)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported updater config schema"
        );
        for (name, workspace) in &self.workspaces {
            ensure!(
                workspace.root.is_absolute() && workspace.state_directory.is_absolute(),
                "workspace {name} paths must be absolute runtime strings"
            );
            absolute(&workspace.root)?;
            absolute(&workspace.state_directory)?;
            let state = self.state_directory(workspace)?;
            for (other_name, other) in &self.workspaces {
                if name != other_name {
                    let other = resolved_path(&other.state_directory)?;
                    ensure!(
                        !state.starts_with(&other) && !other.starts_with(&state),
                        "workspace state trees must be disjoint"
                    );
                }
            }
            ensure!(
                workspace.inventory.file.is_some() != workspace.inventory.command.is_some(),
                "choose exactly one inventory file or command"
            );
            ensure!(
                (1..=86400).contains(&workspace.inventory.max_age_seconds),
                "invalid inventory age"
            );
            if let Some(command) = &workspace.inventory.command {
                command.validate()?;
            }
            if let Some(path) = &workspace.inventory.file {
                ensure!(path.is_absolute(), "inventory path must be absolute");
            }
        }
        let mut forge_hosts = BTreeSet::new();
        for forge in self.forges.values() {
            ensure!(
                forge.host == forge.host.to_ascii_lowercase(),
                "forge hostnames must use canonical lowercase"
            );
            ensure!(
                forge_hosts.insert(&forge.host),
                "forge hosts must be unique; inventory cannot select a different policy for the same host"
            );
            let url = url::Url::parse(&forge.api_url)?;
            ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "forge API must be credential-free HTTPS"
            );
            ensure!(
                !forge.host.is_empty()
                    && !forge.host.contains(['/', ':', '\0'])
                    && !forge.check_authority.is_empty(),
                "invalid forge identity"
            );
            forge.adapter.validate()?;
            absolute(&forge.adapter.program)?;
            ensure!(
                forge.adapter_sha256.is_some() || forge.adapter.program.starts_with("/nix/store"),
                "non-store forge adapters require an enrolled executable SHA-256"
            );
            if let Some(hash) = &forge.adapter_sha256 {
                sha256(hash)?;
            }
            for (path, hash) in &forge.adapter_inputs {
                absolute(path)?;
                sha256(hash)?;
                ensure!(
                    path != &forge.adapter.program,
                    "adapter input overlaps its executable"
                );
            }
            for argument in &forge.adapter.arguments {
                let value = if argument.starts_with('-') {
                    argument
                        .split_once('=')
                        .map_or(argument.as_str(), |(_, value)| value)
                } else {
                    argument
                };
                let path = Path::new(value);
                if path.is_absolute() {
                    absolute(path)?;
                    ensure!(
                        path.starts_with("/nix/store") || forge.adapter_inputs.contains_key(path),
                        "file-valued adapter arguments require a store path or enrolled SHA-256"
                    );
                } else {
                    ensure!(
                        !value.contains(['/', '\\']) && !matches!(value, "." | ".."),
                        "adapter file arguments must be absolute"
                    );
                }
            }
            if let Some(path) = &forge.credential_file {
                absolute(path)?;
                ensure!(
                    !path.starts_with("/nix/store"),
                    "forge credentials must be runtime files outside the Nix store"
                );
                ensure!(
                    path != &forge.adapter.program && !forge.adapter_inputs.contains_key(path),
                    "adapter credential overlaps code inputs"
                );
            }
        }
        for profile in self.profiles.values() {
            ensure!(
                self.workspaces.contains_key(&profile.workspace),
                "unknown workspace"
            );
            ensure!(
                profile.scopes == ["ci"],
                "only primary CI scope is supported"
            );
            ensure!(
                !profile.select.coordinates.is_empty(),
                "profiles require operator-owned coordinate enrollment"
            );
            for enrolled in &profile.select.coordinates {
                coordinate(enrolled)?;
            }
            ensure!(
                profile
                    .rollout
                    .canaries
                    .iter()
                    .all(|canary| profile.select.coordinates.contains(canary)),
                "configured canaries must belong to operator-owned coordinate enrollment"
            );
            ensure!(
                self.forges.contains_key(&profile.generator.forge),
                "unknown generator forge"
            );
            coordinate(&profile.generator.source_coordinate)?;
            ensure!(
                profile
                    .generator
                    .source_coordinate
                    .starts_with(&format!("{}/", self.forges[&profile.generator.forge].host)),
                "generator coordinate differs from selected forge host"
            );
            crate::review::contract::sha(&profile.generator.source_revision)?;
            absolute(&profile.generator.executable)?;
            if let Some(digest) = &profile.generator.executable_sha256 {
                sha256(digest)?;
            }
            ensure!(
                profile.generator.executable_sha256.is_some()
                    || profile.generator.executable.starts_with("/nix/store"),
                "non-store generators require an enrolled executable SHA-256"
            );
            requirements(&profile.generator.required_checks)?;
            ensure!(
                !profile.generator.required_checks.is_empty(),
                "generator requires explicit qualification checks"
            );
            requirements(&profile.required_checks)?;
            ensure!(
                profile.rollout.batch_size > 0 && profile.rollout.max_concurrent > 0,
                "rollout limits must be positive"
            );
            ensure!(
                profile.rollout.maintenance_ref_prefix.ends_with('/')
                    && !profile.rollout.maintenance_ref_prefix.starts_with('-'),
                "maintenance prefix must be a ref namespace ending in /"
            );
            // Use Git's complete ref grammar before state creation or generation.
            // The candidate suffix is always hexadecimal, so a fixed suffix
            // validates every branch this policy can construct.
            branch(&format!(
                "{}0000000000000000",
                profile.rollout.maintenance_ref_prefix
            ))
            .context("maintenance prefix is not a valid Git ref namespace")?;
            for pattern in profile
                .select
                .include_paths
                .iter()
                .chain(&profile.select.exclude_paths)
            {
                glob::Pattern::new(pattern)?;
                ensure!(
                    !pattern.starts_with('/') && !pattern.split('/').any(|p| p == ".."),
                    "selection paths must be workspace-relative"
                );
            }
            for landing in std::iter::once(&profile.landing)
                .chain(profile.project_overrides.values().flat_map(|o| &o.landing))
            {
                ensure!(
                    matches!(landing.as_str(), "direct" | "pull-request"),
                    "unsupported landing mode {landing}"
                );
            }
            for (key, value) in &profile.project_overrides {
                coordinate(key)?;
                requirements(&value.required_checks)?;
                if let Some(target) = &value.target_branch {
                    branch(target).context("invalid override target branch")?;
                }
                ensure!(
                    value.enable || value.reason.as_ref().is_some_and(|s| !s.trim().is_empty()),
                    "disabled project override requires a reason"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_programs_and_file_arguments_require_enrollment() {
        let mut cfg = config();
        cfg.forges.get_mut("example").unwrap().adapter_sha256 = None;
        assert!(cfg.validate().is_err());
        let forge = cfg.forges.get_mut("example").unwrap();
        forge.adapter.program = "/nix/store/fixture/bin/python".into();
        forge.adapter.arguments = vec!["/mutable/adapter.py".into()];
        assert!(cfg.validate().is_err());
        cfg.forges
            .get_mut("example")
            .unwrap()
            .adapter_inputs
            .insert("/mutable/adapter.py".into(), "a".repeat(64));
        cfg.validate().unwrap();
        cfg.forges.get_mut("example").unwrap().adapter.arguments = vec!["../adapter.py".into()];
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn coordinate_enrollment_is_operator_owned_and_required() {
        let mut cfg = config();
        cfg.profiles
            .get_mut("fleet")
            .unwrap()
            .select
            .coordinates
            .clear();
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("operator-owned coordinate enrollment")
        );
    }

    #[test]
    fn adapter_executable_cannot_also_be_a_declared_input() {
        for program in [
            "/runtime/adapter",
            "/runtime//adapter",
            "/nix/store/fixture/bin/adapter",
        ] {
            let mut cfg = config();
            let forge = cfg.forges.get_mut("example").unwrap();
            forge.adapter.program = program.into();
            forge.adapter_sha256 = Some("a".repeat(64));
            forge
                .adapter_inputs
                .insert(PathBuf::from(program), "a".repeat(64));
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("adapter input overlaps its executable")
            );
        }
        let mut cfg = config();
        let forge = cfg.forges.get_mut("example").unwrap();
        forge.adapter.program = "/runtime/adapter".into();
        forge.adapter_sha256 = Some("a".repeat(64));
        forge
            .adapter_inputs
            .insert("/runtime//adapter".into(), "a".repeat(64));
        assert!(cfg.validate().is_err());
        let mut cfg = config();
        let forge = cfg.forges.get_mut("example").unwrap();
        forge.adapter.program = "/nix/store/fixture/bin/python3".into();
        forge.adapter.arguments = vec!["-I".into(), "/runtime/adapter.py".into()];
        forge
            .adapter_inputs
            .insert("/runtime/adapter.py".into(), "a".repeat(64));
        cfg.validate().unwrap();
    }

    #[test]
    fn credentials_cannot_overlap_adapter_code_including_path_aliases() {
        for credential in [
            "/runtime/adapter",
            "/runtime//adapter",
            "/runtime/helper.py",
            "/runtime//helper.py",
        ] {
            let mut cfg = config();
            let forge = cfg.forges.get_mut("example").unwrap();
            forge.adapter.program = "/runtime/adapter".into();
            forge.adapter_sha256 = Some("a".repeat(64));
            forge
                .adapter_inputs
                .insert("/runtime/helper.py".into(), "b".repeat(64));
            forge.credential_file = Some(credential.into());
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("credential overlaps code")
            );
            cfg.forges.get_mut("example").unwrap().credential_file =
                Some("/run/credentials/forge".into());
            cfg.validate().unwrap();
        }
    }

    #[test]
    fn canonical_hosts_reject_case_aliases_without_folding_repository_paths() {
        for host in ["Forge.example", "FORGE.EXAMPLE"] {
            let mut cfg = config();
            cfg.forges.get_mut("example").unwrap().host = host.into();
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("canonical lowercase")
            );
            assert!(
                coordinate(&format!("{host}/Team/Demo"))
                    .unwrap_err()
                    .to_string()
                    .contains("canonical lowercase")
            );
        }
        coordinate("forge.example/Team/Demo").unwrap();
        let mut cfg = config();
        cfg.profiles.get_mut("fleet").unwrap().select.coordinates =
            vec!["Forge.example/team/demo".into()];
        assert!(cfg.validate().is_err());
        let mut cfg = config();
        let mut alias: Forge =
            serde_json::from_value(serde_json::to_value(&cfg.forges["example"]).unwrap()).unwrap();
        alias.host = "Forge.example".into();
        cfg.forges.insert("alias".into(), alias);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn forge_hosts_cannot_delegate_policy_choice_to_inventory() {
        let mut cfg = config();
        let mut weaker: Forge =
            serde_json::from_value(serde_json::to_value(&cfg.forges["example"]).unwrap()).unwrap();
        weaker.check_authority = "999".into();
        cfg.forges.insert("weaker".into(), weaker);
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("forge hosts must be unique")
        );
        cfg.forges.get_mut("weaker").unwrap().host = "other.example".into();
        cfg.validate().unwrap();
    }

    #[test]
    fn canary_enrollment_is_validated_before_inventory_discovery() {
        let mut cfg = config();
        cfg.profiles.get_mut("fleet").unwrap().rollout.canaries =
            vec!["forge.example/team/typo".into()];
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("canaries must belong")
        );
        cfg.profiles.get_mut("fleet").unwrap().rollout.canaries =
            cfg.profiles["fleet"].select.coordinates.clone();
        cfg.validate().unwrap();
    }

    fn config() -> Config {
        serde_json::from_value(serde_json::json!({
            "schemaVersion":1,
            "workspaces":{"projects":{"root":"/work/projects","stateDirectory":"/state/updater","inventory":{"file":"/work/inventory.json"}}},
            "forges":{"example":{"host":"forge.example","apiUrl":"https://forge.example/api","adapter":{"program":"/bin/adapter"},"adapterSha256":"b".repeat(64),"checkAuthority":"777"}},
            "profiles":{"fleet":{"workspace":"projects","scopes":["ci"],"select":{"coordinates":["forge.example/team/demo"]},"generator":{"executable":"/nix/store/fixture/bin/simit","sourceRevision":"a".repeat(40),"sourceCoordinate":"forge.example/team/generator","forge":"example","requiredChecks":[{"name":"test","authority":"777"}]}}}
        })).unwrap()
    }

    #[test]
    fn rejects_generator_traversal_and_mismatched_forge_before_execution() {
        let mut cfg = config();
        cfg.validate().unwrap();
        cfg.profiles.get_mut("fleet").unwrap().generator.executable =
            "/nix/store/../../tmp/generator".into();
        assert!(cfg.validate().is_err());
        cfg.profiles.get_mut("fleet").unwrap().generator.executable =
            "/nix/store/fixture/bin/simit".into();
        cfg.profiles
            .get_mut("fleet")
            .unwrap()
            .generator
            .source_coordinate = "other.example/team/generator".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn all_workspace_roots_and_state_trees_must_be_disjoint() {
        for state in ["/", "/work", "/work/projects", "/work/projects/state"] {
            let mut cfg = config();
            cfg.workspaces.get_mut("projects").unwrap().state_directory = state.into();
            assert!(cfg.validate().is_err(), "{state}");
        }
        let mut cfg = config();
        cfg.workspaces.insert(
            "other".into(),
            Workspace {
                root: "/state/updater/checkouts".into(),
                state_directory: "/other/state".into(),
                inventory: InventorySource {
                    file: Some("/work/other.json".into()),
                    command: None,
                    max_age_seconds: 300,
                },
            },
        );
        assert!(cfg.validate().is_err());
        cfg.workspaces.get_mut("other").unwrap().root = "/work/other".into();
        cfg.validate().unwrap();
        for state in ["/state", "/state/updater", "/state/updater/child"] {
            cfg.workspaces.get_mut("other").unwrap().state_directory = state.into();
            assert!(cfg.validate().is_err(), "{state}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_state_aliases_cannot_bypass_disjointness() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&state, &alias).unwrap();
        let mut cfg = config();
        cfg.workspaces.get_mut("projects").unwrap().state_directory = state;
        let mut other: Workspace =
            serde_json::from_value(serde_json::to_value(&cfg.workspaces["projects"]).unwrap())
                .unwrap();
        other.state_directory = alias.join("not-created");
        cfg.workspaces.insert("other".into(), other);
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("workspace state trees must be disjoint")
        );
        assert!(!alias.join("not-created").exists());
    }

    #[test]
    fn maintenance_prefixes_are_checked_before_transactions() {
        for prefix in [
            "simit..update/",
            "simit@{update/",
            ".hidden/update/",
            "simit.lock/update/",
            "simit//update/",
            "simit/update ",
            "simit/update?/",
            "simit/update\\/",
            "simit/update\n/",
        ] {
            let mut cfg = config();
            cfg.profiles
                .get_mut("fleet")
                .unwrap()
                .rollout
                .maintenance_ref_prefix = prefix.into();
            assert!(cfg.validate().is_err(), "{prefix:?}");
        }
        for prefix in [
            "simit/update/",
            "custom-layout/updates/",
            "simit.update/",
            "simit./update/",
        ] {
            let mut cfg = config();
            cfg.profiles
                .get_mut("fleet")
                .unwrap()
                .rollout
                .maintenance_ref_prefix = prefix.into();
            cfg.validate().unwrap();
        }
    }

    #[test]
    fn override_targets_are_validated_before_transactions() {
        for target in [
            "",
            "-option",
            "feature..ci",
            "feature/",
            "feature.",
            ".hidden/ci",
            "feature.lock",
            "feature//ci",
            "feature@{ci",
            "feature ci",
        ] {
            let mut cfg = config();
            cfg.profiles
                .get_mut("fleet")
                .unwrap()
                .project_overrides
                .insert(
                    "forge.example/team/demo".into(),
                    serde_json::from_value(serde_json::json!({"targetBranch": target})).unwrap(),
                );
            assert!(cfg.validate().is_err(), "{target:?}");
        }
        for target in ["trunk", "custom-layout/ci", "feature./ci"] {
            let mut cfg = config();
            cfg.profiles
                .get_mut("fleet")
                .unwrap()
                .project_overrides
                .insert(
                    "forge.example/team/demo".into(),
                    serde_json::from_value(serde_json::json!({"targetBranch": target})).unwrap(),
                );
            cfg.validate().unwrap();
        }
    }

    #[test]
    fn selection_is_layout_independent_and_exclusions_win() {
        let selection: Selection =
            serde_json::from_value(serde_json::json!({"coordinates":["forge.example/team/demo"]}))
                .unwrap();
        for path in ["repos/owned/demo", "owned/demo", "another/layout/demo"] {
            let project = Project {
                coordinate: "forge.example/team/demo".into(),
                forge: "example".into(),
                relative_path: path.into(),
                kind: "owned-repo".into(),
                lifecycle: "active".into(),
                access: "normal".into(),
                checkout_role: "canonical".into(),
                simit_ci: "auto".into(),
                mirrors: vec![],
            };
            assert!(selection.matches(&project).unwrap());
            let mut fabricated = project.clone();
            fabricated.coordinate = "forge.example/team/unlisted".into();
            assert!(!selection.matches(&fabricated).unwrap());
            assert!(!Selection::default().matches(&project).unwrap());
            let mut denied = selection.clone();
            denied.exclude_paths = vec![format!("{}/**", path.rsplit_once('/').unwrap().0)];
            assert!(!denied.matches(&project).unwrap());
            for role in ["worktree", "mirror"] {
                let mut other = project.clone();
                other.checkout_role = role.into();
                assert!(!selection.matches(&other).unwrap());
            }
            for policy in ["custom", "excluded"] {
                let mut other = project.clone();
                other.simit_ci = policy.into();
                assert!(!selection.matches(&other).unwrap());
            }
        }
    }

    #[test]
    fn malformed_or_non_original_evidence_never_authorizes_success() {
        let request = AdapterRequest {
            schema_version: 1,
            operation: "qualify".into(),
            coordinate: "forge.example/team/demo".into(),
            api_url: "https://forge.example/api".into(),
            credential_file: None,
            authority: "777".into(),
            revision: Some("a".repeat(40)),
            branch: Some("maintenance/candidate".into()),
            workflows: vec![],
            required_checks: vec![Requirement {
                name: "test".into(),
                authority: "777".into(),
            }],
            plan: None,
            landing: None,
            target: None,
            change: None,
        };
        let mut response = AdapterResponse {
            schema_version: 1,
            coordinate: request.coordinate.clone(),
            repository_id: "42".into(),
            default_branch: "trunk".into(),
            revision: "a".repeat(40),
            remote: "https://forge.example/team/demo.git".into(),
            policy_digest: "b".repeat(64),
            requirements: vec![],
            status: "success".into(),
            checks: vec![],
            change: None,
        };
        assert!(response.validate(&request).is_err());
        response.checks.push(Check {
            name: "test".into(),
            authority: "777".into(),
            revision: "a".repeat(40),
            run: "55".into(),
            attempt: 1,
            status: "success".into(),
        });
        assert!(response.validate(&request).is_ok());
        response.checks[0].attempt = 2;
        assert!(response.validate(&request).is_err());
        response.checks[0].attempt = 1;
        for status in ["skipped", "cancelled", "failure", "neutral", "pending"] {
            response.checks[0].status = status.into();
            assert!(response.validate(&request).is_err());
        }
    }

    #[test]
    fn landing_policy_is_validated_and_direct_keeps_existing_digests() {
        let mut cfg = config();
        assert!(
            !serde_json::to_value(&cfg.profiles["fleet"])
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("landing")
        );
        for landing in ["pull-request", "direct"] {
            cfg.profiles.get_mut("fleet").unwrap().landing = landing.into();
            cfg.validate().unwrap();
        }
        cfg.profiles.get_mut("fleet").unwrap().landing = "merge-queue".into();
        assert!(cfg.validate().is_err());
        let mut cfg = config();
        cfg.profiles
            .get_mut("fleet")
            .unwrap()
            .project_overrides
            .insert(
                "forge.example/team/demo".into(),
                serde_json::from_value(serde_json::json!({"landing":"merge-queue"})).unwrap(),
            );
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pull_request_protocol_is_v2_and_binds_candidate_head_and_target() {
        let qualify = serde_json::json!({
            "schemaVersion":1,"operation":"qualify","coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api","authority":"777","revision":"a".repeat(40),
            "branch":"maintenance/candidate","workflows":[],"requiredChecks":[]
        });
        let legacy: AdapterRequest = serde_json::from_value(qualify.clone()).unwrap();
        legacy.validate().unwrap();
        let bytes = serde_json::to_value(&legacy).unwrap();
        assert!(
            ["landing", "target", "change"]
                .iter()
                .all(|key| bytes.get(key).is_none())
        );
        let mut request: AdapterRequest = serde_json::from_value(qualify).unwrap();
        request.landing = Some("pull-request".into());
        request.schema_version = request.derived_version();
        assert!(
            request.validate().is_err(),
            "qualification carries no landing"
        );
        request.operation = "merge".into();
        request.target = Some("trunk".into());
        request.change = Some("7".into());
        request.schema_version = 1;
        assert!(request.validate().is_err(), "declared v1");
        request.schema_version = request.derived_version();
        request.validate().unwrap();
        request.change = None;
        assert!(request.validate().is_err(), "merge names its change");
        request.change = Some("7".into());
        let mut response: AdapterResponse = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"coordinate":request.coordinate,"repositoryId":"2","defaultBranch":"trunk",
            "revision":request.revision,"remote":"https://forge.example/team/demo.git","policyDigest":"b".repeat(64),
            "requirements":[],"status":"success","checks":[],
            "change":{"id":"7","state":"merged","head":request.revision,"base":"trunk","mergeCommit":"c".repeat(40)}
        })).unwrap();
        response.validate(&request).unwrap();
        response.schema_version = 1;
        assert!(response.validate(&request).is_err());
        response.schema_version = 2;
        for (key, value) in [
            ("head", serde_json::json!("b".repeat(40))),
            ("base", serde_json::json!("main")),
            ("id", serde_json::json!("8")),
            ("mergeCommit", serde_json::Value::Null),
            ("state", serde_json::json!("open")),
        ] {
            let mut changed = serde_json::to_value(&response).unwrap();
            changed["change"][key] = value;
            let changed: AdapterResponse = serde_json::from_value(changed).unwrap();
            assert!(changed.validate(&request).is_err(), "{key}");
        }
        let change = response.change.as_mut().unwrap();
        change.state = "open".into();
        change.merge_commit = None;
        assert!(
            response.validate(&request).is_err(),
            "open merge is pending"
        );
        response.status = "pending".into();
        response.validate(&request).unwrap();
        request.operation = "close".into();
        assert!(
            response.validate(&request).is_err(),
            "close leaves no open change"
        );
        request.change = None;
        response.status = "success".into();
        response.change = None;
        response.validate(&request).unwrap();
        request.change = Some("7".into());
        assert!(
            response.validate(&request).is_err(),
            "a named change is reported"
        );
    }

    #[test]
    fn pull_request_admission_relaxes_only_the_target_branch() {
        let mut request: AdapterRequest = serde_json::from_value(serde_json::json!({
            "schemaVersion":2, "operation":"admit", "coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api", "authority":"777", "revision":"a".repeat(40),
            "branch":"trunk", "workflows":[], "requiredChecks":[], "landing":"pull-request", "target":"trunk",
            "plan":{"schema":"simit.ci-update-plan.v1","files":[],"obsolete":[]}
        })).unwrap();
        request.validate().unwrap();
        request.branch = Some("maintenance/candidate".into());
        assert!(request.validate().is_err());
        request.operation = "inspect".into();
        request.plan = None;
        request.revision = None;
        request.branch = None;
        assert!(request.validate().is_err(), "inspection names no target");
        request.target = None;
        request.validate().unwrap();
    }

    #[test]
    fn admission_attestation_binds_the_exact_plan_and_branch() {
        let mut request: AdapterRequest = serde_json::from_value(serde_json::json!({
            "schemaVersion":1, "operation":"admit", "coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api", "authority":"777", "revision":"a".repeat(40),
            "branch":"maintenance/candidate", "workflows":[".github/workflows/ci.yaml"], "requiredChecks":[],
            "plan":{"schema":"simit.ci-update-plan.v1","files":[{"relative_path":".github/workflows/ci.yaml","content":"on: push\n"}],"obsolete":[]}
        })).unwrap();
        request.validate().unwrap();
        let mut response: AdapterResponse = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"coordinate":request.coordinate,"repositoryId":"42","defaultBranch":"trunk",
            "revision":request.revision,"remote":"https://forge.example/team/demo.git","policyDigest":"b".repeat(64),
            "requirements":[],"status":"success","checks":[{"name":"admission","authority":"777",
            "revision":request.revision,"run":crate::review::canonical_digest(&request).unwrap(),"attempt":1,"status":"success"}]
        })).unwrap();
        response.validate(&request).unwrap();
        request.branch = Some("maintenance/other".into());
        assert!(response.validate(&request).is_err());
        request.branch = Some("maintenance/candidate".into());
        request.plan.as_mut().unwrap().files[0].content = "on: workflow_dispatch\n".into();
        assert!(response.validate(&request).is_err());
        response.checks.clear();
        assert!(response.validate(&request).is_err());
    }
}

impl Inventory {
    pub fn validate(&self, workspace: &Workspace) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.root == workspace.root,
            "inventory schema/root mismatch"
        );
        let age = chrono::Utc::now()
            .signed_duration_since(self.observed_at)
            .num_seconds();
        ensure!(
            (-30..=workspace.inventory.max_age_seconds as i64).contains(&age),
            "inventory is stale or from the future"
        );
        ensure!(!self.producer.is_empty(), "inventory producer is required");
        sha256(&self.policy_digest)?;
        for p in &self.projects {
            coordinate(&p.coordinate)?;
            relative(Path::new(&p.relative_path))?;
        }
        Ok(())
    }
}
impl Command {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.program.is_absolute() && (1..=900).contains(&self.timeout_seconds),
            "adapter commands require absolute executables and bounded timeouts"
        );
        Ok(())
    }
}
pub fn coordinate(value: &str) -> Result<()> {
    let host = value.split('/').next().unwrap_or_default();
    ensure!(
        host == host.to_ascii_lowercase(),
        "repository hostnames must use canonical lowercase"
    );
    ensure!(
        value.split('/').count() >= 3
            && !value.contains(['\\', ':', '\0', '?', '#', '@'])
            && value.split('/').all(|p| !p.is_empty()
                && p != "."
                && p != ".."
                && !p.chars().any(char::is_whitespace)),
        "invalid canonical repository coordinate"
    );
    Ok(())
}
pub fn relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|p| matches!(p, std::path::Component::Normal(_)))
            && !path
                .components()
                .any(|p| p.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git")),
        "invalid generated repository path"
    );
    Ok(())
}
fn absolute(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path.components().all(|p| matches!(
                p,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "runtime paths must be absolute and must not contain traversal components"
    );
    Ok(())
}

fn branch(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && !value.starts_with('-')
            && std::process::Command::new("git")
                .args(["check-ref-format", &format!("refs/heads/{value}")])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .context("validate branch name with Git")?
                .success(),
        "invalid Git branch name"
    );
    Ok(())
}

pub fn resolved_path(path: &Path) -> Result<PathBuf> {
    absolute(path)?;
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut resolved = ancestor.canonicalize()?;
                for component in missing.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .context("missing runtime path ancestor")?,
                );
                ancestor = ancestor.parent().context("missing runtime path parent")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
pub fn sha256(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid SHA-256"
    );
    Ok(())
}

/// Read the verified opened object, not a path that can be redirected after a
/// canonicalization check. Adapter execution already requires Linux namespaces.
pub(super) fn credential_bytes(path: &Path) -> Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    {
        use std::{io::Read, os::fd::AsRawFd};
        absolute(path)?;
        let file = std::fs::File::open(path)?;
        ensure!(
            file.metadata()?.is_file(),
            "credential must be a regular runtime file"
        );
        let resolved = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        ensure!(
            resolved.is_absolute() && !resolved.starts_with("/nix/store"),
            "forge credentials must resolve outside the Nix store"
        );
        let mut bytes = Vec::new();
        file.take(crate::review::MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= crate::review::MAX_JSON_BYTES,
            "credential file is too large"
        );
        Ok(bytes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        anyhow::bail!("updater credentials require Linux file-identity verification")
    }
}
fn requirements(values: &[Requirement]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for r in values {
        ensure!(
            !r.name.trim().is_empty()
                && !r.authority.trim().is_empty()
                && seen.insert((&r.name, &r.authority)),
            "invalid/duplicate qualification requirement"
        );
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterRequest {
    pub schema_version: u32,
    pub operation: String,
    pub coordinate: String,
    pub api_url: String,
    pub credential_file: Option<PathBuf>,
    pub authority: String,
    pub revision: Option<String>,
    pub branch: Option<String>,
    /// All generated output paths. Each adapter discovers its own workflows and triggers.
    pub workflows: Vec<String>,
    pub required_checks: Vec<Requirement>,
    /// Exact generated bytes for pre-push backend admission. Omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::project::CiUpdatePlan>,
    /// `pull-request` relaxes only approval-review protection of the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
    /// Branch a pull request merges into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Forge identifier of an existing pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
}
impl AdapterRequest {
    /// 1 unless a v2 feature is present, so v1 bytes and admission digests hold.
    pub fn derived_version(&self) -> u32 {
        if self.landing.is_some()
            || self.target.is_some()
            || self.change.is_some()
            || matches!(self.operation.as_str(), "propose" | "merge" | "close")
            || self
                .plan
                .as_ref()
                .is_some_and(|p| p.schema == "simit.ci-update-plan.v2")
        {
            2
        } else {
            1
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == self.derived_version()
                && matches!(
                    self.operation.as_str(),
                    "inspect" | "admit" | "qualify" | "propose" | "merge" | "close"
                ),
            "unsupported adapter request"
        );
        coordinate(&self.coordinate)?;
        if let Some(rev) = &self.revision {
            crate::review::contract::sha(rev)?;
        }
        requirements(&self.required_checks)?;
        ensure!(
            self.landing
                .as_deref()
                .is_none_or(|l| matches!(l, "direct" | "pull-request")),
            "unsupported landing mode"
        );
        let pull_request = self.landing.as_deref() == Some("pull-request");
        match self.operation.as_str() {
            "propose" | "merge" | "close" => ensure!(
                pull_request
                    && self.revision.is_some()
                    && self.branch.is_some()
                    && self.target.is_some()
                    && (self.operation != "merge" || self.change.is_some()),
                "pull-request operations need the candidate, its branch, target and change"
            ),
            // Inspection observes the target itself; admission relaxes only it.
            "inspect" => ensure!(
                self.target.is_none() && self.change.is_none(),
                "inspection carries no pull request"
            ),
            "admit" => ensure!(
                self.change.is_none() && (self.landing.is_none() || self.target == self.branch),
                "pull-request admission applies only to its target branch"
            ),
            _ => ensure!(
                self.landing.is_none() && self.target.is_none() && self.change.is_none(),
                "qualification carries no landing"
            ),
        }
        if self.operation == "admit" {
            ensure!(
                self.revision.is_some()
                    && self.branch.is_some()
                    && self
                        .plan
                        .as_ref()
                        .is_some_and(|p| p.schema == "simit.ci-update-plan.v1"),
                "admission needs exact candidate bytes and branch"
            );
        } else {
            ensure!(
                self.plan.is_none(),
                "generation plans are only valid for admission"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterResponse {
    pub schema_version: u32,
    pub coordinate: String,
    pub repository_id: String,
    pub default_branch: String,
    pub revision: String,
    pub remote: String,
    pub policy_digest: String,
    pub requirements: Vec<Requirement>,
    pub status: String,
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub id: String,
    /// `open`, `merged` or `closed` (without merging).
    pub state: String,
    pub head: String,
    pub base: String,
    pub merge_commit: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub authority: String,
    pub revision: String,
    pub run: String,
    pub attempt: u32,
    pub status: String,
}

impl AdapterResponse {
    pub fn validate(&self, request: &AdapterRequest) -> Result<()> {
        ensure!(
            self.schema_version == request.schema_version
                && self.coordinate == request.coordinate
                && !self.repository_id.is_empty(),
            "adapter identity mismatch"
        );
        if !matches!(request.operation.as_str(), "propose" | "merge" | "close") {
            ensure!(self.change.is_none(), "unexpected pull request");
        } else if matches!(self.status.as_str(), "success" | "pending") {
            // Closing an untracked candidate may find no pull request at all.
            let Some(change) = &self.change else {
                ensure!(
                    request.operation == "close" && request.change.is_none(),
                    "pull-request operation returned no change"
                );
                return Ok(());
            };
            crate::review::contract::sha(&change.head)?;
            if let Some(commit) = &change.merge_commit {
                crate::review::contract::sha(commit)?;
            }
            ensure!(
                !change.id.is_empty()
                    && request.change.as_ref().is_none_or(|id| id == &change.id)
                    // Closing retires a stale change whose head or target moved.
                    && (request.operation == "close"
                        || (request.target.as_ref() == Some(&change.base)
                            && request.revision.as_ref() == Some(&change.head)))
                    && matches!(change.state.as_str(), "open" | "merged" | "closed")
                    && (change.state == "merged") == change.merge_commit.is_some()
                    // Only an unmerged open merge is pending; close never leaves it open.
                    && match request.operation.as_str() {
                        "merge" => (self.status == "pending") == (change.state == "open"),
                        "close" => self.status == "success" && change.state != "open",
                        _ => self.status == "success",
                    },
                "pull request differs from the qualified candidate or its target"
            );
        }
        crate::review::contract::sha(&self.revision)?;
        sha256(&self.policy_digest)?;
        requirements(&self.requirements)?;
        ensure!(
            matches!(
                self.status.as_str(),
                "success" | "pending" | "failed" | "unsupported"
            ),
            "invalid adapter status"
        );
        if let Some(rev) = &request.revision {
            ensure!(&self.revision == rev, "adapter source mismatch");
        }
        if request.operation == "admit" && self.status == "success" {
            let digest = crate::review::canonical_digest(request)?;
            ensure!(
                self.checks.len() == 1
                    && self.checks.iter().all(|c| c.name == "admission"
                        && c.authority == request.authority
                        && c.revision == self.revision
                        && c.attempt == 1
                        && c.status == "success"
                        && c.run == digest),
                "adapter did not attest exact-branch generation-plan admission"
            );
        }
        if request.operation == "qualify" && self.status == "success" {
            ensure!(
                !self.checks.is_empty(),
                "empty qualification is not acceptance"
            );
            let mut seen = BTreeSet::new();
            for check in &self.checks {
                ensure!(
                    check.revision == self.revision
                        && check.attempt == 1
                        && check.status == "success"
                        && !check.run.is_empty()
                        && seen.insert((&check.name, &check.authority)),
                    "qualification contains non-original, duplicate or non-success cases"
                );
            }
            for required in &request.required_checks {
                ensure!(
                    self.checks
                        .iter()
                        .any(|c| c.name == required.name && c.authority == required.authority),
                    "missing required qualification: {}",
                    required.name
                );
            }
        }
        Ok(())
    }
}
