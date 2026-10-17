use anyhow::{Result, ensure};
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
        Ok(self.kinds.contains(&project.kind)
            && self.lifecycles.contains(&project.lifecycle)
            && self.access.contains(&project.access)
            && self.checkout_roles.contains(&project.checkout_role)
            && project.simit_ci != "excluded"
            && project.simit_ci != "custom"
            && matches(&self.include_paths, &project.relative_path)?
            && !matches(&self.exclude_paths, &project.relative_path)?
            && (self.coordinates.is_empty() || self.coordinates.contains(&project.coordinate)))
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
fn maintenance_prefix() -> String {
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
            ensure!(
                self.workspaces.values().all(|source| !workspace
                    .state_directory
                    .starts_with(&source.root)
                    && !source.root.starts_with(&workspace.state_directory)),
                "update state and all source workspace trees must be disjoint"
            );
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
        for forge in self.forges.values() {
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
            if let Some(path) = &forge.credential_file {
                absolute(path)?;
                ensure!(
                    !path.starts_with("/nix/store"),
                    "forge credentials must be runtime files outside the Nix store"
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
            for (key, value) in &profile.project_overrides {
                coordinate(key)?;
                requirements(&value.required_checks)?;
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
    fn config() -> Config {
        serde_json::from_value(serde_json::json!({
            "schemaVersion":1,
            "workspaces":{"projects":{"root":"/work/projects","stateDirectory":"/state/updater","inventory":{"file":"/work/inventory.json"}}},
            "forges":{"example":{"host":"forge.example","apiUrl":"https://forge.example/api","adapter":{"program":"/bin/adapter"},"checkAuthority":"777"}},
            "profiles":{"fleet":{"workspace":"projects","scopes":["ci"],"generator":{"executable":"/nix/store/fixture/bin/simit","sourceRevision":"a".repeat(40),"sourceCoordinate":"forge.example/team/generator","forge":"example","requiredChecks":[{"name":"test","authority":"777"}]}}}
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
    }

    #[test]
    fn selection_is_layout_independent_and_exclusions_win() {
        let selection: Selection = serde_json::from_str("{}").unwrap();
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
}
impl AdapterRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && matches!(self.operation.as_str(), "inspect" | "qualify"),
            "unsupported adapter request"
        );
        coordinate(&self.coordinate)?;
        if let Some(rev) = &self.revision {
            crate::review::contract::sha(rev)?;
        }
        requirements(&self.required_checks)?;
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
            self.schema_version == 1
                && self.coordinate == request.coordinate
                && !self.repository_id.is_empty(),
            "adapter identity mismatch"
        );
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
