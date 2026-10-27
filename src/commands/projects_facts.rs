//! `simit projects facts`: which simit usages each checkout has and which
//! upgrade paths cover them, as one `fleetix.supply-facts.v1` document with
//! the `simit.usage-facts.v1` vocabulary. Read-only: it never writes files or
//! the project registry, and reports per-repository failures in `errors`.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::ProjectsFactsArgs;
use crate::commands::init_ci;
use crate::commands::projects::CommandExit;
use crate::config::{ProjectConfig, flake_declares_simit_config};
use crate::registry::{self, FeatureStatus};

/// Canonical coordinate of the simit source that pins resolve to.
const SIMIT: &str = "github.com/caniko/simit";

pub fn run(args: ProjectsFactsArgs) -> Result<()> {
    if !args.eval_flake {
        crate::config::disable_flake_eval();
    }
    let paths = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.into_iter().map(PathBuf::from).collect()
    };
    let mut facts = Facts::default();
    for path in &paths {
        facts.observe(path, args.eval_flake);
    }
    let document = json!({
        "contract": "fleetix.supply-facts.v1",
        "vocabulary": "simit.usage-facts.v1",
        "producer": {"name": "simit", "version": env!("CARGO_PKG_VERSION")},
        "observedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "nodes": facts.nodes.into_values().collect::<Vec<_>>(),
        "edges": facts
            .edges
            .into_iter()
            .map(|(from, to, kind)| json!({"from": from, "to": to, "kind": kind}))
            .collect::<Vec<_>>(),
        "capabilities": capabilities(),
        "errors": facts.errors,
    });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &document)
        .map_err(std::io::Error::from)
        .and_then(|()| writeln!(stdout))
        .map_err(|error| CommandExit::new(2, format!("writing facts: {error}")).into())
}

type Attrs = BTreeMap<&'static str, Value>;

#[derive(Serialize)]
struct Node {
    id: String,
    kind: &'static str,
    attrs: Attrs,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    expects: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blockers: Vec<Value>,
}

#[derive(Default)]
struct Facts {
    nodes: BTreeMap<String, Node>,
    edges: BTreeSet<(String, String, &'static str)>,
    errors: Vec<Value>,
}

/// Simit references found in one file.
#[derive(Default)]
struct Refs {
    revisions: BTreeSet<String>,
    tags: BTreeSet<String>,
    floating: BTreeSet<String>,
    /// Variables that supply the reference at run time.
    variables: BTreeSet<String>,
}

fn blocker(code: &str, reason: impl Into<String>) -> Value {
    json!({"code": code, "reason": reason.into()})
}

impl Facts {
    fn node(&mut self, id: String, kind: &'static str, attrs: Attrs) -> &mut Node {
        self.nodes.entry(id.clone()).or_insert(Node {
            id,
            kind,
            attrs,
            expects: Vec::new(),
            blockers: Vec::new(),
        })
    }

    fn error(&mut self, subject: &str, code: &str, message: String) {
        self.errors
            .push(json!({"subject": subject, "code": code, "message": message}));
    }

    fn usage(
        &mut self,
        coordinate: &str,
        kind: &'static str,
        locator: Option<&str>,
        state: &str,
        mut attrs: Attrs,
    ) -> &mut Node {
        debug_assert!(
            capabilities().iter().any(|row| row.kind == kind),
            "usage kind {kind} has no capability row"
        );
        let id = match locator {
            Some(locator) => format!("usage:{coordinate}#{kind}:{locator}"),
            None => format!("usage:{coordinate}#{kind}"),
        };
        attrs.insert("usageKind", kind.into());
        attrs.insert("state", state.into());
        self.edges
            .insert((format!("repo:{coordinate}"), id.clone(), "uses"));
        self.node(id, "usage", attrs)
    }

    /// A usage whose files simit generates.
    fn generated(&mut self, coordinate: &str, kind: &'static str, state: &str, paths: Vec<String>) {
        let node = self.usage(
            coordinate,
            kind,
            None,
            state,
            Attrs::from([("paths", json!(paths))]),
        );
        if state != "hand-rolled" {
            node.expects.push(json!({
                "attr": "generatorRevision",
                "blocker": "unknown-generator-revision",
                "reason": "No receipt or pin proves which generator revision rendered these files.",
            }));
        }
    }

    /// A per-file reference to simit, pinned or not.
    fn reference(
        &mut self,
        coordinate: &str,
        kind: &'static str,
        path: &str,
        refs: Refs,
    ) -> String {
        let mut attrs = Attrs::from([("path", json!(path))]);
        if !refs.revisions.is_empty() {
            attrs.insert("revisions", json!(refs.revisions));
        }
        if !refs.tags.is_empty() {
            attrs.insert("tags", json!(refs.tags));
        }
        if !refs.variables.is_empty() {
            attrs.insert("variables", json!(refs.variables));
        }
        let node = self.usage(coordinate, kind, Some(path), "unknown", attrs);
        let id = node.id.clone();
        if !refs.variables.is_empty() {
            node.expects.push(json!({
                "attr": "revision",
                "blocker": "unknown-generator-revision",
                "reason": format!("{path} takes the simit revision from a run-time variable"),
            }));
        }
        for floating in &refs.floating {
            node.blockers.push(blocker(
                "unpinned-runtime",
                format!("{path} runs simit from the floating reference {floating}"),
            ));
        }
        for revision in refs.revisions {
            self.pin(&id, revision);
        }
        id
    }

    fn pin(&mut self, usage: &str, revision: String) {
        let rev = format!("rev:{SIMIT}@{revision}");
        self.node(
            rev.clone(),
            "rev",
            Attrs::from([("source", json!(SIMIT)), ("revision", json!(revision))]),
        );
        self.edges.insert((usage.to_owned(), rev, "pins"));
    }

    fn observe(&mut self, path: &Path, eval_flake: bool) {
        let subject = path.display().to_string();
        let root = match fs::canonicalize(path) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => return self.error(&subject, "path-unreadable", "not a directory".into()),
            Err(error) => return self.error(&subject, "path-unreadable", error.to_string()),
        };
        let coordinate = match registry::origin_coordinate(&root) {
            Ok(coordinate) => coordinate,
            Err(error) => {
                return self.error(&subject, "coordinate-unresolved", format!("{error:#}"));
            }
        };
        let repo = format!("repo:{coordinate}");
        if self.nodes.contains_key(&repo) {
            return self.error(
                &subject,
                "duplicate-coordinate",
                format!("{repo} was already observed"),
            );
        }
        let host = coordinate.split('/').next().unwrap_or_default().to_owned();
        let target = registry::infer_project_ci_target(&root);
        // ponytail: public hosts by name; any other host takes the platform its
        // committed CI targets, else stays unknown. Read operator forge policy
        // if that guess stops holding.
        let forge = match (host.as_str(), &target) {
            ("github.com", _) => "github",
            ("gitlab.com", _) => "gitlab",
            ("codeberg.org", _) => "forgejo",
            (_, Ok(Some(backend))) => backend.platform().as_str(),
            _ => "unknown",
        };
        let mut attrs = Attrs::from([("forge", json!(forge)), ("forgeHost", json!(host))]);
        let mut blockers = Vec::new();
        match target {
            Ok(Some(backend)) => {
                let platform = backend.platform().as_str();
                let target = format!("{}/{platform}", backend.provider().as_str());
                if platform != forge {
                    blockers.push(blocker(
                        "provider-mismatch",
                        format!("{target} CI cannot run on {host}"),
                    ));
                }
                attrs.insert("ciTarget", json!(target));
            }
            Ok(None) => {}
            Err(error) => self.error(&repo, "ci-target-unresolved", format!("{error:#}")),
        }

        // Config sources. A flake source is never evaluated without --eval-flake.
        let mut sources = Vec::new();
        let mut explained = false;
        match ProjectConfig::static_source_labels(&root) {
            Ok(labels) => sources.extend(labels.iter().map(|label| match label.as_str() {
                "simit.toml" => "config.simit-toml",
                "Cargo.toml [workspace.metadata.simit]" => "config.cargo-workspace",
                _ => "config.cargo-package",
            })),
            Err(error) => {
                explained = true;
                self.error(&repo, "config-error", format!("{error:#}"));
                if root.join("simit.toml").is_file() {
                    sources.push("config.simit-toml");
                }
            }
        }
        let flake = root.join("flake.nix");
        let declares = if flake.is_file() {
            flake_declares_simit_config(&flake).unwrap_or_else(|error| {
                self.error(&repo, "config-error", format!("{error:#}"));
                false
            })
        } else {
            false
        };
        if declares {
            sources.push("config.flake");
            blockers.push(blocker(
                "config-not-persistable",
                "flake outputs.simitConfig cannot be rewritten by the generator",
            ));
        }
        if sources.len() > 1 {
            explained = true;
            blockers.push(blocker(
                "config-conflict",
                format!("several simit config sources: {}", sources.join(", ")),
            ));
        }
        let config = ProjectConfig::load(&root);
        match &config {
            Err(error) if sources.contains(&"config.flake") => {
                explained = true;
                blockers.push(blocker(
                    "config-unresolved",
                    if eval_flake {
                        format!("{error:#}")
                    } else {
                        "flake outputs.simitConfig is only evaluated with --eval-flake".into()
                    },
                ));
            }
            Err(error) if !explained => {
                explained = true;
                self.error(&repo, "config-error", format!("{error:#}"));
            }
            _ => {}
        }
        let config_state = if config.is_ok() { "current" } else { "unknown" };
        for kind in &sources {
            let path = match *kind {
                "config.simit-toml" => "simit.toml",
                "config.flake" => "flake.nix",
                _ => "Cargo.toml",
            };
            self.usage(
                &coordinate,
                kind,
                None,
                config_state,
                Attrs::from([("paths", json!([path]))]),
            );
        }
        let repo_node = self.node(repo.clone(), "repo", attrs);
        repo_node.blockers.extend(blockers);

        let workflows = match registry::collect_workflow_files(&root) {
            Ok(workflows) => workflows,
            Err(error) => {
                self.error(&repo, "workflows-unreadable", format!("{error:#}"));
                Vec::new()
            }
        };
        self.observe_ci(
            &root,
            &coordinate,
            &workflows,
            config.as_ref().ok(),
            explained,
        );
        self.observe_generated(
            &root,
            &coordinate,
            &workflows,
            config.as_ref().ok(),
            eval_flake,
            explained,
        );
        self.observe_workflows(&coordinate, &host, &workflows);
        self.observe_references(&root, &coordinate, &workflows);
        self.observe_locks(&root, &coordinate);
    }

    fn observe_ci(
        &mut self,
        root: &Path,
        coordinate: &str,
        workflows: &[registry::WorkflowFile],
        config: Option<&ProjectConfig>,
        explained: bool,
    ) {
        let repo = format!("repo:{coordinate}");
        let ci = workflows
            .iter()
            .filter(|file| !is_release(&file.relative_path))
            .collect::<Vec<_>>();
        // kind -> (present paths, drifted, unaudited)
        let mut kinds: BTreeMap<&'static str, (BTreeSet<String>, bool, bool)> = BTreeMap::new();
        let (state, drifted) = match registry::audit_ci(root) {
            Ok(audit) if audit.status == FeatureStatus::HandRolled => {
                for file in &ci {
                    let entry = kinds.entry(ci_kind(&file.relative_path)).or_default();
                    entry.0.insert(file.relative_path.display().to_string());
                }
                for (kind, (paths, ..)) in kinds {
                    self.generated(coordinate, kind, "hand-rolled", paths.into_iter().collect());
                }
                return;
            }
            Ok(audit) => (
                "current",
                [
                    audit.changed_files,
                    audit.missing_files,
                    audit.extra_generated_files,
                ]
                .concat(),
            ),
            Err(error) => {
                if !explained {
                    self.error(&repo, "ci-audit-failed", format!("{error:#}"));
                }
                ("unknown", Vec::new())
            }
        };
        for file in ci.iter().filter(|file| file.marked) {
            let entry = kinds.entry(ci_kind(&file.relative_path)).or_default();
            entry.0.insert(file.relative_path.display().to_string());
            // The audit leaves out the VS Code and JetBrains publishers.
            entry.2 |= registry::is_release_workflow_path(&file.relative_path);
        }
        // Hand-written files beside generated ones: join their kind, or stand alone.
        let mut hand: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
        for file in ci.iter().filter(|file| !file.marked) {
            let kind = ci_kind(&file.relative_path);
            let path = file.relative_path.display().to_string();
            match kinds.get_mut(kind) {
                Some(entry) => {
                    entry.0.insert(path);
                }
                None => {
                    hand.entry(kind).or_default().insert(path);
                }
            }
        }
        for path in drifted {
            let entry = kinds.entry(ci_kind(&path)).or_default();
            if root.join(&path).is_file() {
                entry.0.insert(path.display().to_string());
            }
            entry.1 = true;
        }
        let gate = crate::render::ci::local::LOCAL_GATE_PATH;
        let content = fs::read_to_string(root.join(gate))
            .ok()
            .filter(|content| crate::render::ci::is_generated_workflow_marker(content));
        let configured = config.filter(|config| config.ci.local_gate);
        // A configured gate is expected wherever generated CI exists.
        if content.is_some() || (configured.is_some() && !kinds.is_empty()) {
            let expected = configured.and_then(|config| {
                registry::expected_ci_files(root)
                    .and_then(|files| {
                        crate::render::ci::local::local_gate_script(&files, &config.ci)
                    })
                    .ok()
            });
            // None: the policy or the expected script could not be resolved.
            let drift = match (&content, configured, expected) {
                (_, Some(_), None) => None,
                (Some(_), None, _) if config.is_none() => None,
                (Some(content), Some(_), Some(expected)) => Some(expected.content != *content),
                _ => Some(true),
            };
            let paths = content.iter().map(|_| gate.to_owned()).collect();
            kinds.insert(
                "ci.local-gate",
                (paths, drift == Some(true), drift.is_none()),
            );
        }
        for (kind, (paths, drift, unaudited)) in kinds {
            let state = if state == "unknown" {
                state
            } else if drift {
                "drift"
            } else if unaudited {
                "unknown"
            } else {
                "current"
            };
            self.generated(coordinate, kind, state, paths.into_iter().collect());
        }
        for (kind, paths) in hand {
            if !self
                .nodes
                .contains_key(&format!("usage:{coordinate}#{kind}"))
            {
                self.generated(coordinate, kind, "hand-rolled", paths.into_iter().collect());
            }
        }
    }

    /// Generated files outside the CI renderer: release, flake, hooks,
    /// README badges, in-repository distribution files and the trust root.
    fn observe_generated(
        &mut self,
        root: &Path,
        coordinate: &str,
        workflows: &[registry::WorkflowFile],
        config: Option<&ProjectConfig>,
        eval_flake: bool,
        explained: bool,
    ) {
        let repo = format!("repo:{coordinate}");
        let rust = root.join("Cargo.toml").is_file();
        let check = |facts: &mut Self, kind: &'static str, args: &[&str], paths: Vec<String>| {
            let state = if !rust {
                "unknown"
            } else {
                match check_generated(root, args, eval_flake) {
                    Ok(state) => state,
                    Err(error) => {
                        if !explained {
                            facts.error(&repo, "check-failed", format!("{kind}: {error:#}"));
                        }
                        "unknown"
                    }
                }
            };
            facts.generated(coordinate, kind, state, paths);
        };

        let release = workflows
            .iter()
            .filter(|file| {
                file.relative_path
                    .file_stem()
                    .is_some_and(|stem| stem == "release")
            })
            .collect::<Vec<_>>();
        let release_paths = release
            .iter()
            .map(|file| file.relative_path.display().to_string())
            .collect::<Vec<_>>();
        if let Some(file) = release.iter().find(|file| file.marked) {
            // Check the backend the committed workflow was rendered for.
            let backend = if file.relative_path.starts_with(".github") {
                ["--platform", "github"]
            } else if file.relative_path.starts_with(".crow") {
                ["--ci-provider", "crow"]
            } else {
                ["--platform", "forgejo"]
            };
            let args = [["init", "release", "--check"].as_slice(), &backend].concat();
            check(self, "release.workflow", &args, release_paths);
        } else if !release.is_empty() {
            self.generated(coordinate, "release.workflow", "hand-rolled", release_paths);
        }
        let copr = config
            .and_then(|config| config.copr.as_ref())
            .map_or(".copr/Makefile", |copr| copr.makefile_path.as_str());
        for (kind, channel, marker) in [
            ("dist.aur", "aur", "dist/aur"),
            ("dist.copr", "copr", copr),
            ("dist.apt", "apt", crate::commands::apt::DISTRIBUTIONS_PATH),
        ] {
            if root.join(marker).exists() {
                check(
                    self,
                    kind,
                    &["init", channel, "--check"],
                    vec![marker.to_owned()],
                );
            }
        }

        let state = |status| match status {
            FeatureStatus::Managed => "current",
            FeatureStatus::Drift => "drift",
            FeatureStatus::Absent => "hand-rolled",
            _ => "unknown",
        };
        let flake_status = registry::detect_flake_status(root);
        let present = |paths: &[&str]| {
            paths
                .iter()
                .filter(|path| root.join(path).is_file())
                .map(|path| path.to_string())
                .collect::<Vec<_>>()
        };
        if fs::read_to_string(root.join("flake.nix"))
            .is_ok_and(|content| crate::render::flake::has_required_wiring(&content))
        {
            self.generated(
                coordinate,
                "flake.wiring",
                state(flake_status),
                present(&["flake.nix", "nix/treefmt.nix"]),
            );
        }
        if registry::has_hooks_config(root) {
            let paths = present(&[
                "nix/pre-commit.nix",
                ".pre-commit-config.yaml",
                ".pre-commit-config.yml",
            ]);
            let hooks = if root.join("nix/pre-commit.nix").is_file() {
                state(flake_status)
            } else {
                "hand-rolled"
            };
            self.generated(coordinate, "hooks.config", hooks, paths);
        }
        if fs::read_to_string(root.join("README.md"))
            .is_ok_and(|readme| readme.contains(crate::readme_badges::START_MARKER))
        {
            let state = match crate::readme_badges::plan(root) {
                Ok(upgrade) if upgrade.changed() => "drift",
                Ok(_) => "current",
                Err(error) => {
                    if !explained {
                        self.error(&repo, "check-failed", format!("readme.badges: {error:#}"));
                    }
                    "unknown"
                }
            };
            self.generated(coordinate, "readme.badges", state, vec!["README.md".into()]);
        }
        // Without a loaded config (an unevaluated flake policy) neither the
        // path nor the requirement is known: report only what is on disk.
        let trust_root = config.map_or_else(
            || "keys/maintainers.gpg".to_owned(),
            |config| config.release.signing.trust_root.clone(),
        );
        let required = release.iter().any(|file| file.marked)
            && config.is_some_and(|config| config.release.signing.required);
        if root.join(&trust_root).is_file() {
            self.usage(
                coordinate,
                "keys.trust-root",
                None,
                if config.is_some() {
                    "current"
                } else {
                    "unknown"
                },
                Attrs::from([("paths", json!([trust_root]))]),
            );
        } else if required {
            // Generated release workflows verify tags against this keyring.
            self.usage(
                coordinate,
                "keys.trust-root",
                None,
                "drift",
                Attrs::from([("paths", json!([]))]),
            )
            .blockers
            .push(blocker(
                "trust-root-missing",
                format!("the release workflow verifies tags against the missing {trust_root}"),
            ));
        }
        if let Some(attic) = config.and_then(|config| config.release.attic.as_ref()) {
            // The server URL is part of the identity: one name on two servers is two caches.
            let url = redact_url(&attic.url);
            let cache = format!("cache:{}@{url}", attic.cache);
            let mut publishers = vec!["release.workflow"];
            if config
                .and_then(|config| config.prebuild.as_ref())
                .is_some_and(|prebuild| prebuild.publish_attic)
            {
                publishers.push("ci.primary");
            }
            for kind in publishers {
                let usage = format!("usage:{coordinate}#{kind}");
                if self.nodes.contains_key(&usage) {
                    self.node(
                        cache.clone(),
                        "cache",
                        Attrs::from([("name", json!(attic.cache)), ("url", json!(url))]),
                    );
                    self.edges.insert((usage, cache.clone(), "publishesTo"));
                }
            }
        }
    }

    /// One node per CI workflow file with its static triggers, runner labels
    /// and step names, contained by the usage that owns it (else the repo).
    fn observe_workflows(
        &mut self,
        coordinate: &str,
        host: &str,
        workflows: &[registry::WorkflowFile],
    ) {
        for file in workflows {
            let path = file.relative_path.display().to_string();
            let id = format!("workflow:{coordinate}#{path}");
            let kind = if is_release(&file.relative_path) {
                "release.workflow"
            } else {
                ci_kind(&file.relative_path)
            };
            let usage = format!("usage:{coordinate}#{kind}");
            let owned = self.nodes.contains_key(&usage);
            let owner = if owned {
                usage.clone()
            } else {
                format!("repo:{coordinate}")
            };
            let crow = file.relative_path.starts_with(".crow");
            let shape = workflow_shape(&file.content, crow);
            let mut attrs = Attrs::from([
                ("path", json!(path)),
                ("generated", json!(file.marked)),
                ("triggers", shape.triggers),
                ("labels", json!(shape.labels)),
                ("steps", json!(shape.steps)),
            ]);
            // Jsonnet and GitLab CI need evaluation; their triggers stay unknown.
            if file
                .relative_path
                .extension()
                .is_some_and(|ext| ext == "jsonnet")
                || file.relative_path == Path::new(".gitlab-ci.yml")
            {
                attrs.insert("triggers", Value::Null);
            }
            self.node(id.clone(), "workflow", attrs);
            self.edges.insert((owner, id.clone(), "contains"));
            // Crow label maps select agents; they are not runner pools.
            if !crow {
                let host = if file.relative_path.starts_with(".github") {
                    "github.com"
                } else {
                    host
                };
                for label in &shape.labels {
                    let runner = format!("runner:{host}/{label}");
                    self.node(
                        runner.clone(),
                        "runner",
                        Attrs::from([("forgeHost", json!(host)), ("label", json!(label))]),
                    );
                    if owned && file.marked {
                        self.edges.insert((usage.clone(), runner.clone(), "runsOn"));
                    }
                    self.edges.insert((id.clone(), runner, "runsOn"));
                }
            }
        }
    }

    /// Runtime references in workflows and `scripts/`, and pins in `simit.toml`.
    fn observe_references(
        &mut self,
        root: &Path,
        coordinate: &str,
        workflows: &[registry::WorkflowFile],
    ) {
        let mut files = workflows
            .iter()
            .map(|file| {
                (
                    file.relative_path.clone(),
                    file.content.clone(),
                    false,
                    file.marked,
                )
            })
            .collect::<Vec<_>>();
        let mut pending = vec![root.join("scripts")];
        while let Some(directory) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    let subject = format!("repo:{coordinate}");
                    self.error(
                        &subject,
                        "scripts-unreadable",
                        format!(
                            "{}: {error}",
                            directory.strip_prefix(root).unwrap_or(&directory).display()
                        ),
                    );
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => pending.push(path),
                    Ok(kind) if kind.is_file() => {
                        // Generated scripts (the local gate) echo workflow steps.
                        if let Ok(content) = fs::read_to_string(&path)
                            && !crate::render::ci::is_generated_workflow_marker(&content)
                        {
                            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                            files.push((relative, content, true, false));
                        }
                    }
                    _ => {}
                }
            }
        }
        // Config text plus its decoded string values: a command with quotes or
        // backslashes is escaped on disk but verbatim in generated files.
        let mut config = String::new();
        for name in ["simit.toml", "Cargo.toml"] {
            let text = fs::read_to_string(root.join(name)).unwrap_or_default();
            if let Ok(decoded) = toml_edit::de::from_str::<Value>(&text) {
                collect_strings(&decoded, &mut config);
            }
            config.push_str(&text);
        }
        files.sort();
        for (path, content, script, generated) in files {
            let path = path.display().to_string();
            let mut found: BTreeMap<&'static str, Refs> = BTreeMap::new();
            for (first, line) in logical_lines(&content) {
                let line = line.as_str();
                if first.starts_with('#') || first.starts_with("//") {
                    continue;
                }
                // Simit pins its own invocations in generated files to the generator
                // revision, so they move with their file. Generated files also copy
                // config commands (required gates, setup) verbatim; those stay human.
                let command = first
                    .trim_start_matches("- ")
                    .trim_start_matches("run:")
                    .trim();
                let (flake_ref, cargo_install) = if generated && !config.contains(command) {
                    ("runtime.generated-ref", "runtime.generated-ref")
                } else {
                    ("runtime.flake-ref", "runtime.cargo-install")
                };
                let words = line
                    .split(|c: char| c.is_whitespace() || ";|&()`\"'".contains(c))
                    .filter(|word| !word.is_empty())
                    // `--rev=<sha>` is `--rev <sha>`.
                    .flat_map(|word| match word.split_once('=') {
                        Some((flag, value)) if flag.starts_with("--") => vec![flag, value],
                        _ => vec![word],
                    })
                    .collect::<Vec<_>>();
                for (variable, kind) in [
                    ("SIMIT_PUBLISHER_REVISION", "runtime.publisher-var"),
                    ("SIMIT_CI_ENGINE_REVISION", "runtime.ci-engine"),
                ] {
                    if line.contains(variable) {
                        let refs = found.entry(kind).or_default();
                        // A committed fallback: `vars.X || '<revision>'`.
                        if let Some(fallback) =
                            line.split("|| '").nth(1).and_then(|rest| rest.get(..40))
                            && is_revision(fallback)
                        {
                            refs.revisions.insert(fallback.to_owned());
                        }
                    }
                }
                let mut refs = Refs::default();
                let runtime = flake_refs(line, &mut refs);
                if runtime {
                    merge(found.entry(flake_ref).or_default(), refs);
                }
                // A `--git` source decides on its own: a fork's `simit`
                // package is not simit.
                let git = words
                    .iter()
                    .position(|word| *word == "--git")
                    .and_then(|index| words.get(index + 1));
                let install = line.contains("cargo install")
                    && match git {
                        Some(url) => is_simit_repo_url(url),
                        // A path or alternative registry installs other code.
                        None => {
                            !words
                                .iter()
                                .any(|word| matches!(*word, "--path" | "--registry" | "--index"))
                                && words
                                    .iter()
                                    .any(|word| *word == "simit" || word.starts_with("simit@"))
                        }
                    };
                if install {
                    let refs = found.entry(cargo_install).or_default();
                    match words
                        .iter()
                        .position(|word| matches!(*word, "--rev" | "--version" | "--tag"))
                    {
                        // `--rev "$REV"`: the value arrives at run time.
                        Some(index)
                            if words
                                .get(index + 1)
                                .is_some_and(|value| value.starts_with('$')) =>
                        {
                            let variable =
                                words[index + 1].trim_matches(|c| matches!(c, '$' | '{' | '}'));
                            refs.variables.insert(variable.to_owned());
                        }
                        Some(index)
                            if words.get(index + 1).is_some_and(|value| is_revision(value)) =>
                        {
                            refs.revisions.insert(words[index + 1].to_owned());
                        }
                        Some(index) if words.get(index + 1).is_some() => {
                            refs.tags
                                .insert(words[index + 1].trim_start_matches('=').to_owned());
                        }
                        _ => match words.iter().find_map(|word| word.strip_prefix("simit@")) {
                            Some(version) => {
                                refs.tags.insert(version.to_owned());
                            }
                            None => {
                                refs.floating.insert("cargo install simit".to_owned());
                            }
                        },
                    }
                }
                let devshell = line.contains("nix develop") && words.contains(&"simit");
                if devshell {
                    found.entry("runtime.devshell").or_default();
                }
                // ponytail: a bare `simit` word in a script line counts as an
                // invocation, including inside echoed text; parse shell if that misleads.
                if script && !runtime && !install && !devshell && words.contains(&"simit") {
                    found.entry("script.invocation").or_default();
                }
            }
            for (kind, refs) in found {
                let id = self.reference(coordinate, kind, &path, refs);
                let node = self.nodes.get_mut(&id).expect("usage recorded above");
                // References in simit-generated files follow their generator.
                node.attrs.insert("generated", json!(generated));
                if matches!(kind, "runtime.publisher-var" | "runtime.ci-engine") {
                    node.expects.push(json!({
                        "attr": "revision",
                        "blocker": "unknown-generator-revision",
                        "reason": "The repository variable's value lives on the forge.",
                    }));
                }
            }
        }
        if let Ok(content) = fs::read_to_string(root.join("simit.toml")) {
            let mut refs = Refs::default();
            let mut pinned = false;
            for line in content
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#'))
            {
                pinned |= flake_refs(line, &mut refs);
            }
            if pinned {
                self.reference(coordinate, "pin.config-command", "simit.toml", refs);
            }
        }
    }

    /// A lock file's text; absence is not an error, any other failure is.
    fn read_lock(&mut self, root: &Path, repo: &str, name: &str) -> Option<String> {
        match fs::read_to_string(root.join(name)) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                self.error(repo, "lock-unreadable", format!("{name}: {error}"));
                None
            }
        }
    }

    /// Simit pinned by `flake.lock` inputs and `Cargo.lock` packages.
    fn observe_locks(&mut self, root: &Path, coordinate: &str) {
        let repo = format!("repo:{coordinate}");
        let lock = self.read_lock(root, &repo, "flake.lock").and_then(|text| {
            match serde_json::from_str::<Value>(&text) {
                Ok(lock) => Some(lock),
                Err(error) => {
                    self.error(&repo, "lock-unreadable", format!("flake.lock: {error}"));
                    None
                }
            }
        });
        let inputs = lock.as_ref().map_or_else(Vec::new, |lock| {
            flake_inputs(lock).unwrap_or_else(|message| {
                self.error(&repo, "lock-unreadable", format!("flake.lock: {message}"));
                Vec::new()
            })
        });
        for (kind, input, revision) in inputs {
            let mut attrs = Attrs::from([("path", json!("flake.lock")), ("input", json!(input))]);
            if let Some(revision) = &revision {
                attrs.insert("revisions", json!([revision]));
            }
            let state = if kind == "flake-input.cut" {
                "absent"
            } else {
                "unknown"
            };
            let id = self
                .usage(coordinate, kind, Some(&input), state, attrs)
                .id
                .clone();
            if let Some(revision) = revision {
                self.pin(&id, revision);
            }
        }
        let cargo =
            self.read_lock(root, &repo, "Cargo.lock")
                .and_then(|text| match text.parse::<toml_edit::DocumentMut>() {
                    Ok(lock) => Some(lock),
                    Err(error) => {
                        self.error(&repo, "lock-unreadable", format!("Cargo.lock: {error}"));
                        None
                    }
                });
        let packages = cargo
            .as_ref()
            .and_then(|lock| lock.get("package"))
            .and_then(|packages| packages.as_array_of_tables());
        let mut locked = Refs::default();
        let mut found = false;
        for package in packages.into_iter().flatten() {
            let field = |key| package.get(key).and_then(|value| value.as_str());
            // Workspace members carry no source; only dependencies on simit count.
            let (Some("simit"), Some(source)) = (field("name"), field("source")) else {
                continue;
            };
            if !is_canonical_simit_source(source) {
                continue;
            }
            found = true;
            if let Some((_, revision)) = source.rsplit_once('#')
                && is_revision(revision)
            {
                locked.revisions.insert(revision.to_owned());
            }
            locked.tags.extend(field("version").map(str::to_owned));
        }
        // One usage per lock file, whatever number of simit records it holds.
        if found {
            self.reference(coordinate, "cargo.dependency", "Cargo.lock", locked);
        }
    }
}

/// Trimmed lines paired with their shell logical line: a trailing `\` joins
/// the next line, so a wrapped command is classified as one. Comments never join.
fn logical_lines(content: &str) -> Vec<(&str, String)> {
    let mut lines = content.lines().map(str::trim);
    let mut out = Vec::new();
    while let Some(first) = lines.next() {
        let mut joined = first.to_owned();
        if !first.starts_with('#') && !first.starts_with("//") {
            while let Some(head) = joined.strip_suffix('\\') {
                let Some(next) = lines.next() else { break };
                joined = format!("{} {next}", head.trim_end());
            }
        }
        out.push((first, joined));
    }
    out
}

/// Every string value of a config document, one per line.
fn collect_strings(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => {
            out.push_str(text);
            out.push('\n');
        }
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// Whether a Cargo.lock package source is simit from crates.io or the canonical
/// repository; forks and path sources are other code under the same name.
fn is_canonical_simit_source(source: &str) -> bool {
    // crates.io's git and sparse index ids; other registries are other code.
    if matches!(
        source,
        "registry+https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/"
    ) {
        return true;
    }
    source.strip_prefix("git+").is_some_and(is_simit_repo_url)
}

/// Whether a Git URL (https, ssh or scp-style) names exactly the canonical
/// repository, ignoring credentials, query, fragment, trailing slash, `.git`
/// and case; mirrors and forks do not match.
fn is_simit_repo_url(url: &str) -> bool {
    registry::coordinate_from_remote(url.strip_prefix("git+").unwrap_or(url)).as_deref()
        == Some(SIMIT)
}

/// A URL without userinfo, query or fragment: facts must not carry credentials.
fn redact_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return "<unparseable-url>".to_owned();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string().trim_end_matches('/').to_owned()
}

fn merge(into: &mut Refs, from: Refs) {
    into.revisions.extend(from.revisions);
    into.tags.extend(from.tags);
    into.floating.extend(from.floating);
    into.variables.extend(from.variables);
}

fn is_revision(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Record every `github:caniko/simit` / `git+…github.com/caniko/simit` flake
/// reference on `line`; returns whether any was found.
fn flake_refs(line: &str, refs: &mut Refs) -> bool {
    const NAME: &str = "caniko/simit";
    let mut found = false;
    // GitHub names are case-insensitive; ASCII lowercasing keeps byte offsets.
    let lowered = line.to_ascii_lowercase();
    for (start, _) in lowered.match_indices(NAME) {
        let end = start + NAME.len();
        if line[end..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue; // another repository, such as caniko/simit-extra
        }
        let token_start = line[..start]
            .rfind(|c: char| c.is_whitespace() || "\"'=(".contains(c))
            .map_or(0, |index| index + 1);
        let token_end = line[end..]
            .find(|c: char| c.is_whitespace() || "\"'),".contains(c))
            .map_or(line.len(), |index| end + index);
        let prefix = &line[token_start..start];
        // A `git+` source must name github.com itself, not a host that ends in it.
        if !prefix.eq_ignore_ascii_case("github:")
            && !(prefix.starts_with("git+") && is_simit_repo_url(&format!("{prefix}{NAME}")))
        {
            continue;
        }
        found = true;
        let rest = &line[end..token_end];
        let query = |key: &str| {
            rest.split(['?', '&', '#'])
                .find_map(|pair| pair.strip_prefix(key))
        };
        // `?rev=` wins over `?ref=`; a ref names a tag or branch.
        let reference = query("rev=")
            .or_else(|| {
                query("ref=").map(|name| {
                    name.strip_prefix("refs/tags/")
                        .or_else(|| name.strip_prefix("refs/heads/"))
                        .unwrap_or(name)
                })
            })
            .or_else(|| {
                rest.strip_prefix(".git")
                    .unwrap_or(rest)
                    .strip_prefix('/')
                    .and_then(|reference| reference.split(['#', '?']).next())
            });
        match reference {
            Some(reference) if reference.starts_with('$') => {
                let variable = reference.trim_matches(|c| matches!(c, '$' | '{' | '}'));
                // The two simit revision variables are usages of their own.
                if !matches!(
                    variable,
                    "SIMIT_CI_ENGINE_REVISION" | "SIMIT_PUBLISHER_REVISION"
                ) {
                    refs.variables.insert(variable.to_owned());
                }
            }
            Some(reference) if is_revision(reference) => {
                refs.revisions.insert(reference.to_owned());
            }
            Some(reference)
                if reference
                    .strip_prefix('v')
                    .unwrap_or(reference)
                    .starts_with(|c: char| c.is_ascii_digit()) =>
            {
                refs.tags.insert(reference.to_owned());
            }
            Some(reference) if !reference.is_empty() => {
                refs.floating.insert(format!("github:{NAME}/{reference}"));
            }
            _ => {
                refs.floating.insert(format!("github:{NAME}"));
            }
        }
    }
    found
}

type FlakeInput = (&'static str, String, Option<String>);

/// `(usage kind, input path, locked revision)` for every simit node a
/// `flake.lock` owns, plus `simit` inputs cut with `follows = ""`.
/// Errors when the lock has no root or a reachable input names a missing node.
fn flake_inputs(lock: &Value) -> Result<Vec<FlakeInput>, String> {
    let nodes = &lock["nodes"];
    let Some(root) = lock["root"].as_str() else {
        return Err("no root node".into());
    };
    if nodes.get(root).is_none() {
        return Err(format!("root node {root:?} is missing"));
    }
    let is_simit = |node: &Value| {
        let locked = &node["locked"];
        match locked["type"].as_str() {
            Some("github") => {
                locked["owner"]
                    .as_str()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case("caniko"))
                    && locked["repo"]
                        .as_str()
                        .is_some_and(|repo| repo.eq_ignore_ascii_case("simit"))
                    // A GitHub Enterprise host is another forge.
                    && locked["host"]
                        .as_str()
                        .is_none_or(|host| host.eq_ignore_ascii_case("github.com"))
            }
            Some("git") => locked["url"].as_str().is_some_and(is_simit_repo_url),
            _ => false,
        }
    };
    // Breadth-first over owning (string) edges, so each node keeps its
    // shortest input path; `follows` arrays never own a lock entry.
    let mut seen = BTreeSet::from([root.to_owned()]);
    let mut queue = VecDeque::from([(root.to_owned(), Vec::<String>::new())]);
    let mut found = Vec::new();
    while let Some((key, path)) = queue.pop_front() {
        for (name, target) in nodes[&key]["inputs"].as_object().into_iter().flatten() {
            let mut input = path.clone();
            input.push(name.clone());
            match target {
                Value::String(child) if seen.insert(child.clone()) => {
                    if nodes.get(child).is_none() {
                        return Err(format!(
                            "input {} references missing node {child:?}",
                            input.join("/")
                        ));
                    }
                    if is_simit(&nodes[child]) {
                        let kind = if path.is_empty() {
                            "flake-input.direct"
                        } else {
                            "flake-input.transitive"
                        };
                        let revision = nodes[child]["locked"]["rev"].as_str().map(str::to_owned);
                        found.push((kind, input.join("/"), revision));
                    }
                    queue.push_back((child.clone(), input));
                }
                Value::Array(follows) if follows.is_empty() && name == "simit" => {
                    found.push(("flake-input.cut", input.join("/"), None));
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

/// Static facts of one workflow file.
struct Shape {
    triggers: Value,
    labels: BTreeSet<String>,
    steps: Vec<String>,
}

fn yaml_strings(value: Option<&serde_yaml::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml::Value::String(text)) => vec![text.clone()],
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(serde_yaml::Value::Mapping(map)) => yaml_strings(map.get("include")),
        _ => Vec::new(),
    }
}

/// Actions `runs-on` as a label, a label list, or a `{group, labels}` mapping.
fn runs_on_labels(value: Option<&serde_yaml::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml::Value::Mapping(map)) => yaml_strings(map.get("labels"))
            .into_iter()
            .chain(
                map.get("group")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|group| format!("group={group}")),
            )
            .collect(),
        other => yaml_strings(other),
    }
}

/// Triggers (`events`, plus the push `branches` filter, empty when push runs
/// on every branch, and any `branchesIgnore` exclusions), runner labels and step names of an Actions or Crow
/// workflow. A file that does not parse reports `triggers: null`.
// ponytail: top-level triggers only; per-step Crow `when` and YAML merge keys
// are ignored. Evaluate them if budget estimates need that precision.
fn workflow_shape(content: &str, crow: bool) -> Shape {
    let mut shape = Shape {
        triggers: Value::Null,
        labels: BTreeSet::new(),
        steps: Vec::new(),
    };
    let Ok(document) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
        return shape;
    };
    let mut events = BTreeSet::new();
    let mut branches = BTreeSet::new();
    let mut ignored = BTreeSet::new();
    let mut unrestricted = false;
    if crow {
        let entries = match document.get("when") {
            Some(serde_yaml::Value::Sequence(entries)) => entries.iter().collect(),
            Some(entry) => vec![entry],
            None => Vec::new(),
        };
        if entries.is_empty() {
            // Without `when`, Crow runs the workflow for every event and branch.
            events.extend(
                [
                    "push",
                    "pull_request",
                    "tag",
                    "manual",
                    "cron",
                    "deployment",
                    "release",
                ]
                .map(String::from),
            );
            unrestricted = true;
        }
        for entry in entries {
            let mut named = yaml_strings(entry.get("event"));
            if named.is_empty() {
                named = [
                    "push",
                    "pull_request",
                    "tag",
                    "manual",
                    "cron",
                    "deployment",
                    "release",
                ]
                .map(String::from)
                .to_vec();
            }
            if named.iter().any(|event| event == "push") {
                let filter = yaml_strings(entry.get("branch"));
                unrestricted |= filter.is_empty();
                branches.extend(filter);
                ignored.extend(yaml_strings(
                    entry.get("branch").and_then(|branch| branch.get("exclude")),
                ));
            }
            events.extend(named);
        }
        if let Some(serde_yaml::Value::Mapping(labels)) = document.get("labels") {
            for (key, value) in labels {
                if let (Some(key), Some(value)) = (key.as_str(), value.as_str()) {
                    shape.labels.insert(format!("{key}={value}"));
                }
            }
        }
        match document.get("steps") {
            Some(serde_yaml::Value::Mapping(steps)) => shape.steps.extend(
                steps
                    .keys()
                    .filter_map(|key| key.as_str().map(str::to_owned)),
            ),
            Some(serde_yaml::Value::Sequence(steps)) => shape.steps.extend(
                steps
                    .iter()
                    .filter_map(|step| step.get("name")?.as_str().map(str::to_owned)),
            ),
            _ => {}
        }
    } else {
        let triggers = match document.get("on") {
            Some(serde_yaml::Value::Mapping(map)) => map
                .iter()
                .filter_map(|(name, config)| Some((name.as_str()?.to_owned(), Some(config))))
                .collect(),
            other => yaml_strings(other)
                .into_iter()
                .map(|name| (name, None))
                .collect::<Vec<_>>(),
        };
        for (name, config) in triggers {
            let filter = |key| config.and_then(|config| config.get(key));
            match name.as_str() {
                "push" => {
                    if filter("tags").is_some() {
                        events.insert("tag".to_owned());
                    }
                    if filter("branches").is_some() || filter("tags").is_none() {
                        events.insert("push".to_owned());
                        let pushed = yaml_strings(filter("branches"));
                        unrestricted |= pushed.is_empty();
                        branches.extend(pushed);
                        ignored.extend(yaml_strings(filter("branches-ignore")));
                    }
                }
                "pull_request" | "pull_request_target" => {
                    events.insert("pull_request".to_owned());
                }
                "workflow_dispatch" => {
                    events.insert("manual".to_owned());
                }
                "schedule" => {
                    events.insert("cron".to_owned());
                }
                _ => {
                    events.insert(name);
                }
            }
        }
        for job in document
            .get("jobs")
            .and_then(serde_yaml::Value::as_mapping)
            .into_iter()
            .flat_map(|jobs| jobs.values())
        {
            shape.labels.extend(
                runs_on_labels(job.get("runs-on"))
                    .into_iter()
                    .filter(|label| !label.contains("${{")),
            );
            shape.steps.extend(
                job.get("steps")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .filter_map(|step| step.get("name")?.as_str().map(str::to_owned)),
            );
        }
    }
    if unrestricted {
        branches.clear();
    }
    shape.triggers = json!({"events": events, "branches": branches});
    if !ignored.is_empty() {
        shape.triggers["branchesIgnore"] = json!(ignored);
    }
    shape
}
/// Release workflows belong to `simit init release`.
fn is_release(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| stem == "release")
}

fn ci_kind(path: &Path) -> &'static str {
    let name = path.file_name().unwrap_or_default();
    if crate::review::generation::is_review_path(path)
        || path == Path::new(crate::render::review_policy::PATH)
    {
        "ci.review"
    } else if path == Path::new(crate::render::ci::custody::PATH) {
        "ci.custody"
    } else if path.starts_with(".github/actions") {
        "ci.primary"
    } else if crate::render::release_plz::auxiliary_path(path) {
        "ci.publish"
    } else if path.file_stem().is_some_and(|stem| stem == "pages") {
        "ci.pages"
    } else if init_ci::is_primary_ci_workflow_name(name) {
        "ci.primary"
    } else if init_ci::is_ci_managed_workflow_name(name) {
        "ci.publish"
    } else {
        "ci.template"
    }
}

/// Run `simit <args>` (a `--check`) in `root`. README badge drift is reported
/// by `readme.badges`, so it does not make these files drift.
fn check_generated(root: &Path, args: &[&str], eval_flake: bool) -> Result<&'static str> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .current_dir(root)
        .args(args)
        .env("SIMIT_NO_REGISTRY", "1");
    if !eval_flake {
        command.env("SIMIT_NO_FLAKE_EVAL", "1");
    }
    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() || stderr.contains("README badges are not up to date") {
        Ok("current")
    } else if stderr.contains("is not up to date") {
        Ok("drift")
    } else {
        bail!("{}", stderr.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Implemented,
    Planned,
    Unsupported,
}

/// One upgrade path for a usage kind on repositories whose attributes match.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    #[serde(rename = "usageKind")]
    kind: &'static str,
    #[serde(rename = "match", serialize_with = "as_map")]
    matches: &'static [(&'static str, &'static str)],
    path: &'static str,
    via: &'static str,
    lands: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestrator: Option<&'static str>,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocker: Option<&'static str>,
    reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback: Option<&'static str>,
    /// `[scope, adapter, landing]` an updater path needs; only the honesty test reads it.
    #[cfg_attr(not(test), allow(dead_code))]
    #[serde(skip)]
    updater: Option<[&'static str; 3]>,
}

fn as_map<S: serde::Serializer>(
    pairs: &&'static [(&'static str, &'static str)],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_map(pairs.iter().copied())
}

/// The usage is maintained without a generator upgrade.
fn settled(kind: &'static str, path: &'static str, via: &'static str, reason: &'static str) -> Row {
    Row {
        kind,
        matches: &[],
        path,
        via,
        lands: true,
        orchestrator: None,
        status: Status::Implemented,
        blocker: None,
        reason,
        fallback: None,
        updater: None,
    }
}

/// A generator command refreshes the files, but nothing lands them.
fn regenerate(kind: &'static str, via: &'static str) -> Row {
    Row {
        lands: false,
        blocker: Some("no-orchestrator"),
        ..settled(
            kind,
            "regenerate",
            via,
            "The generator refreshes these files, but no orchestrator lands them for this repository.",
        )
    }
}

/// A pin that simit never rewrites.
fn manual(
    kind: &'static str,
    via: &'static str,
    blocker: &'static str,
    reason: &'static str,
) -> Row {
    Row {
        lands: false,
        status: Status::Unsupported,
        blocker: Some(blocker),
        ..settled(kind, "manual", via, reason)
    }
}

/// The workstation updater, `simit projects update`, which needs
/// `[scope, adapter, landing]`.
fn updater(
    kind: &'static str,
    matches: &'static [(&'static str, &'static str)],
    needs: [&'static str; 3],
    status: Status,
    blocker: Option<&'static str>,
    reason: &'static str,
    fallback: Option<&'static str>,
) -> Row {
    Row {
        matches,
        orchestrator: Some("simit-updater"),
        status,
        blocker,
        fallback,
        updater: Some(needs),
        ..settled(kind, "updater", "simit projects update", reason)
    }
}

const GITHUB: &[(&str, &str)] = &[("forge", "github"), ("ciTarget", "actions/github")];
const GITHUB_PR: &[(&str, &str)] = &[
    ("forge", "github"),
    ("ciTarget", "actions/github"),
    ("landing", "pull-request"),
];
const CROW: &[(&str, &str)] = &[("forge", "forgejo"), ("ciTarget", "crow/forgejo")];
const CROW_PR: &[(&str, &str)] = &[
    ("forge", "forgejo"),
    ("ciTarget", "crow/forgejo"),
    ("landing", "pull-request"),
];
const FORGEJO_ACTIONS: &[(&str, &str)] = &[("forge", "forgejo"), ("ciTarget", "actions/forgejo")];
const CROW_ON_GITHUB: &[(&str, &str)] = &[("forge", "github"), ("ciTarget", "crow/forgejo")];

const CONFIG: &str = "Config sources are maintained by the repository owner; generator upgrades read them and never rewrite them.";
const NO_FORGEJO_PR: &str = "The Forgejo and Crow adapter lands directly only.";
const VARIABLE: &str = "Repository variable values are forge state; simit never changes them.";

/// The capability matrix. Each updater feature flips its own rows;
/// `updater_rows_are_implemented_exactly_when_supported` enforces that.
fn capabilities() -> Vec<Row> {
    use Status::{Implemented, Planned, Unsupported};
    let mut rows = vec![
        settled("config.simit-toml", "human", "repository owner", CONFIG),
        settled(
            "config.cargo-workspace",
            "human",
            "repository owner",
            CONFIG,
        ),
        settled("config.cargo-package", "human", "repository owner", CONFIG),
        settled("config.flake", "human", "repository owner", CONFIG),
        regenerate("ci.primary", "simit init ci --primary-only"),
        updater(
            "ci.primary",
            GITHUB,
            ["ci", "github", "direct"],
            Implemented,
            None,
            "The updater regenerates, qualifies and lands primary CI on GitHub.",
            None,
        ),
        updater(
            "ci.primary",
            GITHUB_PR,
            ["ci", "github", "pull-request"],
            Implemented,
            None,
            "The updater proposes and merges a pull request on GitHub.",
            None,
        ),
        updater(
            "ci.primary",
            CROW,
            ["ci", "forgejo", "direct"],
            Implemented,
            None,
            "The updater regenerates, qualifies and lands primary CI on Forgejo with Crow.",
            None,
        ),
        updater(
            "ci.primary",
            CROW_PR,
            ["ci", "forgejo", "pull-request"],
            Planned,
            Some("adapter-missing"),
            NO_FORGEJO_PR,
            Some("simit init ci"),
        ),
        updater(
            "ci.primary",
            FORGEJO_ACTIONS,
            ["ci", "forgejo-actions", "direct"],
            Unsupported,
            Some("adapter-missing"),
            "No updater adapter qualifies Forgejo Actions.",
            Some("simit init ci"),
        ),
        // Dispatch follows the repository's forge: the GitHub adapter qualifies
        // only .github/workflows, and the Crow adapter needs a Forgejo repository.
        Row {
            matches: CROW_ON_GITHUB,
            orchestrator: Some("simit-updater"),
            status: Unsupported,
            blocker: Some("adapter-missing"),
            fallback: Some("simit init ci"),
            ..settled(
                "ci.primary",
                "migration",
                "simit projects update with a provider migration",
                "No updater adapter qualifies Crow pipelines for a GitHub-hosted repository.",
            )
        },
    ];
    // Generated files outside the primary scope: regenerate anywhere; the
    // updater path waits on its scope and, on Forgejo, on the adapter.
    for (kind, via, scope) in [
        ("ci.publish", "simit init ci", "ci-extra"),
        ("ci.pages", "simit init ci", "ci-extra"),
        ("ci.review", "simit init ci", "ci-extra"),
        ("ci.custody", "simit init ci", "ci-extra"),
        ("ci.template", "simit init ci", "ci-extra"),
        ("ci.local-gate", "simit init ci", "ci-extra"),
        ("release.workflow", "simit init release", "release"),
        ("flake.wiring", "simit init flake", "flake"),
        ("hooks.config", "simit init flake", "flake"),
        ("readme.badges", "simit upgrade", "readme"),
    ] {
        rows.extend([
            regenerate(kind, via),
            updater(
                kind,
                GITHUB,
                [scope, "github", "direct"],
                Implemented,
                None,
                "The updater carries this scope with the primary CI candidate.",
                None,
            ),
            updater(
                kind,
                CROW,
                [scope, "forgejo", "direct"],
                Implemented,
                None,
                "The updater carries this scope with the primary CI candidate.",
                None,
            ),
        ]);
    }
    rows.extend([
        regenerate("dist.aur", "simit init aur"),
        regenerate("dist.copr", "simit init copr"),
        regenerate("dist.apt", "simit init apt"),
        settled(
            "keys.trust-root",
            "human",
            "maintainer signing key",
            "Trust roots are maintainer key material; generator upgrades never change them.",
        ),
        manual(
            "flake-input.direct",
            "nix flake update simit",
            "manual-pin",
            "Flake inputs need a networked lock update, which the updater sandbox cannot run.",
        ),
        manual(
            "flake-input.transitive",
            "update the input that locks simit",
            "transitive-pin",
            "Another input's lock pins this simit; update that input instead.",
        ),
        settled(
            "flake-input.cut",
            "none",
            "follows = \"\"",
            "A cut input is never fetched.",
        ),
        manual(
            "runtime.publisher-var",
            "repository variable SIMIT_PUBLISHER_REVISION",
            "manual-pin",
            VARIABLE,
        ),
        manual(
            "runtime.ci-engine",
            "repository variable SIMIT_CI_ENGINE_REVISION",
            "manual-pin",
            VARIABLE,
        ),
        manual(
            "runtime.flake-ref",
            "edit the reference",
            "manual-pin",
            "simit does not rewrite runtime flake references.",
        ),
        manual(
            "runtime.cargo-install",
            "edit the cargo install command",
            "manual-pin",
            "simit does not rewrite cargo install commands.",
        ),
        settled(
            "runtime.generated-ref",
            "regenerate",
            "the generated file that contains it",
            "Generated files pin simit's own invocations to the generator revision; they move whenever that file's usage is regenerated and landed.",
        ),
        settled(
            "runtime.devshell",
            "flake-input",
            "the repository's simit flake input",
            "nix develop runs the simit the flake inputs pin; that input carries the upgrade path.",
        ),
        manual(
            "pin.config-command",
            "edit simit.toml",
            "manual-pin",
            "Pins inside human-owned simit.toml command strings are never rewritten.",
        ),
        settled(
            "script.invocation",
            "environment",
            "the simit on the script's PATH",
            "Scripts run the simit their environment provides; the devshell or flake input carries the upgrade path.",
        ),
        manual(
            "cargo.dependency",
            "cargo update -p simit",
            "manual-pin",
            "Cargo dependency updates are outside every simit orchestrator.",
        ),
    ]);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::projects_update::schema::{
        SUPPORTED_LANDINGS, SUPPORTED_SCOPES, UPDATER_ADAPTERS,
    };

    #[test]
    fn updater_rows_are_implemented_exactly_when_supported() {
        for row in capabilities() {
            let Some([scope, adapter, landing]) = row.updater else {
                continue;
            };
            let supported = SUPPORTED_SCOPES.contains(&scope)
                && UPDATER_ADAPTERS.contains(&adapter)
                && SUPPORTED_LANDINGS.contains(&landing)
                // The Forgejo adapter rejects pull-request landing.
                && !(adapter == "forgejo" && landing == "pull-request");
            assert_eq!(
                row.status == Status::Implemented && row.blocker.is_none(),
                supported,
                "{} {:?}",
                row.kind,
                row.matches
            );
            assert_eq!(
                row.matches.contains(&("landing", "pull-request")),
                landing == "pull-request",
                "{} {:?}",
                row.kind,
                row.matches
            );
        }
    }

    #[test]
    fn every_kind_has_a_fallback_row_and_no_ambiguous_rows() {
        let rows = capabilities();
        for row in &rows {
            assert!(
                rows.iter()
                    .any(|other| other.kind == row.kind && other.matches.is_empty()),
                "{} has no unconditional row",
                row.kind
            );
            assert_eq!(
                row.status == Status::Implemented && row.lands,
                row.blocker.is_none(),
                "{}: rows that do not land need a blocker",
                row.kind
            );
            // Fleetix picks the most specific matching row; equally specific
            // rows for one kind must contradict on some key, or they tie.
            for other in &rows {
                let disjoint = row.matches.iter().any(|(key, value)| {
                    other
                        .matches
                        .iter()
                        .any(|(other_key, other_value)| key == other_key && value != other_value)
                });
                assert!(
                    std::ptr::eq(row, other)
                        || other.kind != row.kind
                        || other.matches.len() != row.matches.len()
                        || disjoint,
                    "{} {:?} ties with {:?}",
                    row.kind,
                    row.matches,
                    other.matches
                );
            }
        }
    }

    #[test]
    fn flake_references_classify_revisions_tags_and_floating_refs() {
        let revision = "4c6841cce19327927ea8a8f6e856276dd5c878f0";
        let mut refs = Refs::default();
        for line in [
            format!("nix run github:caniko/simit/{revision} -- init ci --check"),
            format!("url = \"git+https://github.com/caniko/simit.git?ref=trunk&rev={revision}\";"),
            "simit.url = \"github:caniko/simit/0.19.0\";".to_owned(),
            "nix run --no-write-lock-file git+https://github.com/caniko/simit.git -- init flake"
                .to_owned(),
            "nix run github:caniko/simit/trunk".to_owned(),
            "nix build github:caniko/simit/$SIMIT_CI_ENGINE_REVISION#default".to_owned(),
            format!(
                "nix run --no-write-lock-file git+https://github.com/caniko/simit.git?rev={revision} -- init flake --check"
            ),
            "nix run git+https://github.com/caniko/simit.git?ref=refs/tags/0.19.0 -- init flake"
                .to_owned(),
            "nix run git+https://github.com/caniko/simit?ref=trunk#default".to_owned(),
        ] {
            assert!(flake_refs(&line, &mut refs), "{line}");
        }
        for line in [
            "see https://github.com/caniko/simit for details",
            "nix run github:caniko/simit-extra",
        ] {
            assert!(!flake_refs(line, &mut refs), "{line}");
        }
        assert_eq!(refs.revisions, BTreeSet::from([revision.to_owned()]));
        assert_eq!(refs.tags, BTreeSet::from(["0.19.0".to_owned()]));
        assert_eq!(
            refs.floating,
            BTreeSet::from([
                "github:caniko/simit".to_owned(),
                "github:caniko/simit/trunk".to_owned()
            ])
        );
    }

    #[test]
    fn flake_locks_report_direct_transitive_and_cut_simit_inputs() {
        let simit = |rev: &str| json!({"locked": {"type": "github", "owner": "caniko", "repo": "simit", "rev": rev}});
        let lock = json!({
            "root": "root",
            "nodes": {
                "root": {"inputs": {"simit": "simit", "harbor": "harbor", "tool": "tool"}},
                "simit": simit("a"),
                "harbor": {"inputs": {"simit": "simit_2", "nixpkgs": ["simit"]}},
                "simit_2": simit("b"),
                "tool": {"inputs": {"simit": [], "harbor": ["harbor"]}}
            }
        });
        assert_eq!(
            flake_inputs(&lock).unwrap(),
            vec![
                (
                    "flake-input.direct",
                    "simit".to_owned(),
                    Some("a".to_owned())
                ),
                (
                    "flake-input.transitive",
                    "harbor/simit".to_owned(),
                    Some("b".to_owned())
                ),
                ("flake-input.cut", "tool/simit".to_owned(), None),
            ]
        );
    }

    #[test]
    fn workflow_shapes_read_triggers_labels_and_steps() {
        let crow = workflow_shape(
            "when:\n  - event: pull_request\n  - event: push\n    branch: ${CI_REPO_DEFAULT_BRANCH}\nlabels:\n  platform: linux/amd64\nsteps:\n  test:\n    image: rust\n",
            true,
        );
        assert_eq!(
            crow.triggers,
            json!({"events": ["pull_request", "push"], "branches": ["${CI_REPO_DEFAULT_BRANCH}"]})
        );
        assert_eq!(
            crow.labels,
            BTreeSet::from(["platform=linux/amd64".to_owned()])
        );
        assert_eq!(crow.steps, ["test"]);
        let everywhere = workflow_shape("steps:\n  - name: build\n", true);
        assert_eq!(everywhere.triggers["branches"], json!([]));
        assert_eq!(everywhere.steps, ["build"]);

        let actions = workflow_shape(
            "on:\n  push:\n    branches: [trunk]\n    tags: ['*']\n  pull_request:\n  workflow_dispatch:\njobs:\n  test:\n    runs-on: [self-hosted, atlas]\n    steps:\n      - name: Test\n        run: cargo test\n      - run: unnamed\n  matrix:\n    runs-on: ${{ matrix.os }}\n",
            false,
        );
        assert_eq!(
            actions.triggers,
            json!({"events": ["manual", "pull_request", "push", "tag"], "branches": ["trunk"]})
        );
        assert_eq!(
            actions.labels,
            BTreeSet::from(["atlas".to_owned(), "self-hosted".to_owned()])
        );
        assert_eq!(actions.steps, ["Test"]);
        assert_eq!(
            workflow_shape("on: [push, pull_request]\n", false).triggers,
            json!({"events": ["pull_request", "push"], "branches": []})
        );
        assert!(
            workflow_shape("{ not: yaml: here", false)
                .triggers
                .is_null()
        );
    }

    #[test]
    fn workflow_shapes_read_runner_mappings_block_lists_and_exclusions() {
        let actions = workflow_shape(
            "on:\n  push:\n    branches-ignore: [gh-pages]\njobs:\n  grouped:\n    runs-on:\n      group: atlas-pool\n      labels: [nix]\n  listed:\n    runs-on:\n      - self-hosted\n      - linux\n",
            false,
        );
        assert_eq!(
            actions.triggers,
            json!({"events": ["push"], "branches": [], "branchesIgnore": ["gh-pages"]})
        );
        assert_eq!(
            actions.labels,
            BTreeSet::from(["group=atlas-pool", "linux", "nix", "self-hosted"].map(String::from))
        );
        let crow = workflow_shape(
            "when:\n  - event: push\n    branch:\n      exclude: [simit/update/*]\n",
            true,
        );
        assert_eq!(
            crow.triggers,
            json!({"events": ["push"], "branches": [], "branchesIgnore": ["simit/update/*"]})
        );
    }

    #[test]
    fn cargo_sources_and_config_commands_are_classified() {
        for source in [
            "git+https://github.com/caniko/simit?rev=abc#abc",
            "git+https://github.com/caniko/simit.git#abc",
            "registry+https://github.com/rust-lang/crates.io-index",
            "sparse+https://index.crates.io/",
        ] {
            assert!(is_canonical_simit_source(source), "{source}");
        }
        for source in [
            "git+https://github.com/someone/simit?rev=abc#abc",
            "git+https://github.com/caniko/simit-fork#abc",
            "registry+https://registry.example/index",
            "sparse+https://cargo.example/index/",
        ] {
            assert!(!is_canonical_simit_source(source), "{source}");
        }
        assert!(is_simit_repo_url("https://github.com/caniko/simit.git/"));
        for url in [
            "https://github.com/caniko/simit-fork",
            "https://mirror.example/github.com/caniko/simit",
        ] {
            assert!(!is_simit_repo_url(url), "{url}");
        }
        let decoded: Value = toml_edit::de::from_str("run = \"echo \\\"a\\\" \\\\ b\"").unwrap();
        let mut out = String::new();
        collect_strings(&decoded, &mut out);
        assert_eq!(out, "echo \"a\" \\ b\n");
    }

    #[test]
    fn cache_urls_lose_credentials() {
        assert_eq!(
            redact_url("https://user:secret@attic.example:8080/cache?token=abc#frag"),
            "https://attic.example:8080/cache"
        );
        assert_eq!(
            redact_url("https://attic.example/"),
            "https://attic.example"
        );
        assert_eq!(redact_url("not a url secret"), "<unparseable-url>");
    }

    #[test]
    fn flake_references_keep_v_tags_and_run_time_variables() {
        let mut refs = Refs::default();
        for line in [
            "nix run github:caniko/simit/v0.19.0",
            "nix run git+https://github.com/caniko/simit.git?ref=v0.20.0",
            "nix run github:caniko/simit/$SIMIT_REVISION -- release",
            "nix run github:caniko/simit/${SIMIT_CI_ENGINE_REVISION}#default",
        ] {
            assert!(flake_refs(line, &mut refs), "{line}");
        }
        assert_eq!(
            refs.tags,
            BTreeSet::from(["v0.19.0", "v0.20.0"].map(String::from))
        );
        assert_eq!(
            refs.variables,
            BTreeSet::from(["SIMIT_REVISION".to_owned()])
        );
        assert!(refs.floating.is_empty());
    }

    #[test]
    fn flake_references_need_the_exact_host_and_ignore_case() {
        let mut refs = Refs::default();
        assert!(flake_refs("nix run GitHub:Caniko/Simit/v0.19.0", &mut refs));
        assert!(flake_refs(
            "nix run git+ssh://git@github.com/caniko/simit.git?ref=v0.20.0",
            &mut refs
        ));
        for line in [
            "nix run git+https://notgithub.com/caniko/simit",
            "nix run git+https://mirror.example/github.com/caniko/simit",
        ] {
            assert!(!flake_refs(line, &mut refs), "{line}");
        }
        assert_eq!(
            refs.tags,
            BTreeSet::from(["v0.19.0", "v0.20.0"].map(String::from))
        );
        assert!(is_simit_repo_url("git@github.com:Caniko/simit.git"));
        assert!(is_simit_repo_url("ssh://git@github.com/caniko/simit"));
    }

    #[test]
    fn wrapped_commands_join_before_classification() {
        let lines = logical_lines(
            "cargo install --git https://github.com/caniko/simit \\\n  --rev 4c6841cce19327927ea8a8f6e856276dd5c878f0 \\\n  simit\n# keep \\\nnext\n",
        );
        assert_eq!(lines.len(), 3);
        assert!(lines[0].1.contains("--rev 4c68") && lines[0].1.ends_with(" simit"));
        assert_eq!(
            lines[0].0,
            "cargo install --git https://github.com/caniko/simit \\"
        );
        assert_eq!(lines[1].1, "# keep \\");
        assert_eq!(lines[2].1, "next");
    }

    #[test]
    fn flake_locks_with_dangling_references_are_errors() {
        let dangling = json!({"root": "root", "nodes": {"root": {"inputs": {"simit": "gone"}}}});
        assert!(flake_inputs(&dangling).unwrap_err().contains("gone"));
        let no_root = json!({"root": "root", "nodes": {}});
        assert!(flake_inputs(&no_root).is_err());
        assert!(flake_inputs(&json!({"nodes": {}})).is_err());
        // A GitHub Enterprise simit is another forge.
        let ghe = json!({"root": "root", "nodes": {
            "root": {"inputs": {"simit": "simit"}},
            "simit": {"locked": {"type": "github", "owner": "caniko", "repo": "simit", "host": "ghe.example", "rev": "a"}}
        }});
        assert!(flake_inputs(&ghe).unwrap().is_empty());
    }
}
