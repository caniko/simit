//! Forgejo + Crow adapter. Forgejo owns repository identity and branch policy;
//! the Crow origin named by the check authority owns workflow outcomes.
use super::{
    github::{Api, branch_pattern, encoded, number, text, workflow_bytes},
    schema::*,
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub fn execute(request: &AdapterRequest) -> Result<AdapterResponse> {
    request.validate()?;
    let forge = Api::with(
        &request.api_url,
        request.credential_file.as_deref(),
        "application/json",
    )?;
    let crow = Api::with(
        &format!("{}/api/v1", origin(&request.authority)?),
        request.ci_credential_file.as_deref(),
        "application/json",
    )?;
    execute_with(request, |path| forge.get(path), |path| crow.get(path))
}

/// The check authority is the canonical Crow origin, e.g. `https://ci.example`.
fn origin(authority: &str) -> Result<&str> {
    let url = url::Url::parse(authority)?;
    ensure!(
        url.scheme() == "https" && url.origin().ascii_serialization() == authority,
        "Crow check authority must be a canonical HTTPS origin"
    );
    Ok(authority)
}

/// Source-bound outcome of one Crow workflow for a push to one branch.
struct Workflow {
    name: String,
    steps: BTreeSet<String>,
}

fn strings(value: Option<&serde_yaml::Value>) -> Result<Vec<&str>> {
    match value {
        None => Ok(vec![]),
        Some(serde_yaml::Value::String(value)) => Ok(vec![value]),
        Some(serde_yaml::Value::Sequence(values)) => values
            .iter()
            .map(|value| value.as_str().context("non-string Crow condition"))
            .collect(),
        Some(_) => bail!("include/exclude Crow conditions need a source-aware adapter"),
    }
}

/// Crow `when`: any listed constraint may match, and an absent key matches all.
fn push_runs(when: Option<&serde_yaml::Value>, branch: &str, default_branch: &str) -> Result<bool> {
    let constraints = match when {
        None => vec![],
        Some(serde_yaml::Value::Sequence(constraints)) => constraints.iter().collect(),
        Some(constraint @ serde_yaml::Value::Mapping(_)) => vec![constraint],
        Some(_) => bail!("invalid Crow when condition"),
    };
    let mut runs = constraints.is_empty();
    for constraint in constraints {
        let constraint = constraint
            .as_mapping()
            .context("invalid Crow when condition")?;
        ensure!(
            constraint
                .keys()
                .all(|key| matches!(key.as_str(), Some("event" | "branch"))),
            "Crow conditions other than event and branch need a source-aware adapter"
        );
        let mut matched = true;
        if let Some(branches) = constraint.get("branch") {
            matched = false;
            for pattern in strings(Some(branches))? {
                // Crow substitutes its environment into the whole document.
                let pattern = if pattern == "${CI_REPO_DEFAULT_BRANCH}" {
                    default_branch
                } else {
                    pattern
                };
                ensure!(
                    !pattern.contains('$'),
                    "Crow branch conditions may only substitute CI_REPO_DEFAULT_BRANCH"
                );
                matched |= branch_pattern(pattern, branch)?;
            }
        }
        let events = strings(constraint.get("event"))?;
        ensure!(
            events.iter().all(|event| !event.contains('$')),
            "Crow event conditions must be literal"
        );
        runs |= matched && (events.is_empty() || events.contains(&"push"));
    }
    Ok(runs)
}

fn has_event(constraint: &serde_yaml::Value) -> bool {
    match constraint.get("event") {
        None | Some(serde_yaml::Value::Null) => false,
        Some(serde_yaml::Value::Sequence(events)) => !events.is_empty(),
        Some(_) => true,
    }
}

/// Crow substitutes `$` expressions before it names workflows and steps.
fn literal(value: &serde_yaml::Value) -> Result<String> {
    let value = value
        .as_str()
        .filter(|value| !value.is_empty() && !value.contains('$'))
        .context("Crow workflow and step names must be literal")?;
    Ok(value.to_owned())
}

/// Static admission of one generated Crow workflow; `None` when a push to
/// `branch` creates no workflow.
fn workflow(
    path: &str,
    source: &[u8],
    branch: &str,
    default_branch: &str,
) -> Result<Option<Workflow>> {
    let yaml: serde_yaml::Value = serde_yaml::from_slice(source)?;
    // ponytail: admit the generated Crow vocabulary only. Matrices, services,
    // dependencies, approval gates, caches and custom clones change the
    // workflow/step inventory and need an outcome-aware adapter first.
    ensure!(
        yaml.as_mapping()
            .context("Crow workflow must be a mapping")?
            .keys()
            .all(|key| matches!(
                key.as_str(),
                Some(
                    "name"
                        | "labels"
                        | "platform"
                        | "when"
                        | "skip_clone"
                        | "variables"
                        | "workspace"
                        | "steps"
                )
            )),
        "Crow workflow {path} uses keys that need a source-aware adapter"
    );
    let name = match yaml.get("name") {
        Some(name) => literal(name)?,
        // Crow's SanitizePath: file name without extension or leading dot.
        None => {
            let base = path.rsplit('/').next().unwrap_or(path);
            let base = base.strip_suffix(".yml").unwrap_or(base);
            let base = base.strip_suffix(".yaml").unwrap_or(base);
            literal(&base.strip_prefix('.').unwrap_or(base).into())?
        }
    };
    // Crow collects only workflows whose `when` names an event.
    ensure!(
        match yaml.get("when") {
            Some(serde_yaml::Value::Sequence(constraints)) => constraints.iter().any(has_event),
            Some(constraint) => has_event(constraint),
            None => false,
        },
        "Crow workflow {path} needs an event trigger"
    );
    let clone = !yaml
        .get("skip_clone")
        .map_or(Some(false), serde_yaml::Value::as_bool)
        .context("Crow skip_clone must be a literal boolean")?;
    // Crow names the implicit clone step "clone" and renames colliding steps.
    let mut names = BTreeSet::from(["clone".to_owned()]);
    let mut steps = BTreeSet::new();
    for step in yaml
        .get("steps")
        .and_then(serde_yaml::Value::as_sequence)
        .context("missing Crow workflow steps")?
    {
        ensure!(
            step.as_mapping()
                .context("invalid Crow step")?
                .keys()
                .all(|key| matches!(
                    key.as_str(),
                    Some(
                        "name"
                            | "image"
                            | "commands"
                            | "environment"
                            | "directory"
                            | "entrypoint"
                            | "failure"
                            | "temp_volumes"
                            | "when"
                    )
                )),
            "Crow step in {path} uses keys that need a source-aware adapter"
        );
        ensure!(
            step.get("failure")
                .is_none_or(|failure| failure.as_str() == Some("fail")),
            "Crow workflow {path} can mask failed step outcomes"
        );
        let step_name = literal(step.get("name").context("Crow steps need names")?)?;
        ensure!(
            names.insert(step_name.clone()),
            "duplicate or reserved Crow step name {step_name}"
        );
        // Crow drops non-matching steps at compile time; they never report.
        if push_runs(step.get("when"), branch, default_branch)? {
            steps.insert(step_name);
        }
    }
    // A workflow whose work steps are all filtered out is skipped entirely.
    if !push_runs(yaml.get("when"), branch, default_branch)? || steps.is_empty() {
        return Ok(None);
    }
    if clone {
        steps.insert("clone".into());
    }
    Ok(Some(Workflow { name, steps }))
}

/// Owned `.crow/*.y{a}ml` workflows that a push to `branch` runs, by path.
fn owned(
    paths: &[String],
    branch: &str,
    default_branch: &str,
    mut source: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<BTreeMap<String, Workflow>> {
    let mut workflows = BTreeMap::new();
    let mut names = BTreeSet::new();
    for path in paths {
        if std::path::Path::new(path).parent() != Some(std::path::Path::new(".crow")) {
            continue;
        }
        ensure!(
            path.ends_with(".yaml") || path.ends_with(".yml"),
            "Crow workflow {path} needs a source-aware adapter (Jsonnet or unknown format)"
        );
        if let Some(workflow) = workflow(path, &source(path)?, branch, default_branch)? {
            ensure!(
                names.insert(workflow.name.clone()),
                "owned Crow workflows share the name {}",
                workflow.name
            );
            workflows.insert(path.clone(), workflow);
        }
    }
    Ok(workflows)
}

/// Every branch-protection rule; Forgejo pages the inventory, so read until a
/// page adds nothing (also ends a server that ignores `page`).
fn branch_protections(endpoint: &str, forge: impl Fn(&str) -> Result<Value>) -> Result<Value> {
    // ponytail: 20 pages of 50 rules cap the walk; raise if a forge exceeds it.
    let mut rules: Vec<Value> = Vec::new();
    for page in 1..=20 {
        let fetched = forge(&format!(
            "{endpoint}/branch_protections?page={page}&limit=50"
        ))?;
        let fetched = fetched
            .as_array()
            .context("missing branch protection inventory")?;
        let before = rules.len();
        for rule in fetched {
            if !rules.contains(rule) {
                rules.push(rule.clone());
            }
        }
        if rules.len() == before {
            return Ok(Value::Array(rules));
        }
    }
    bail!("branch protection inventory exceeds adapter pagination ceiling")
}

/// Matching Forgejo rules; rejects anything direct landing cannot satisfy.
fn admit_protections(rules: &Value, branch: &str) -> Result<Vec<Value>> {
    let mut matched = Vec::new();
    for rule in rules
        .as_array()
        .context("missing branch protection inventory")?
    {
        // ponytail: evaluate every matching rule regardless of Forgejo's
        // precedence; add provider-native rule priority before relaxing this.
        if !branch_pattern(&text(rule, "/rule_name")?, branch)? {
            continue;
        }
        ensure!(
            rule["required_approvals"] == 0,
            "approval rules cover {branch}; direct updater landing is unsupported"
        );
        ensure!(
            rule["require_signed_commits"] == false,
            "signature-required branches need an adapter that proves forge-accepted candidate signatures"
        );
        ensure!(
            rule["protected_file_patterns"] == "",
            "protected file patterns need an adapter with path-aware push admission"
        );
        ensure!(
            rule["enable_push"] == true && rule["enable_push_whitelist"] == false,
            "push-restricted target requires an adapter that binds the Git transport actor"
        );
        matched.push(rule.clone());
    }
    Ok(matched)
}

/// Literal Crow push contexts become requirements on the Crow authority.
fn branch_requirements(info: &Value, authority: &str) -> Result<Vec<Requirement>> {
    ensure!(
        info["user_can_push"] == true,
        "credential cannot push the target branch; direct updater landing is unsupported"
    );
    ensure!(
        info["required_approvals"] == 0,
        "repository requires approvals; direct updater landing is unsupported"
    );
    if info["enable_status_check"] == false {
        return Ok(vec![]);
    }
    ensure!(
        info["enable_status_check"] == true,
        "missing status check protection state"
    );
    info["status_check_contexts"]
        .as_array()
        .filter(|contexts| !contexts.is_empty())
        .context("status checks without explicit contexts cannot be bound")?
        .iter()
        .map(|context| {
            // Forgejo contexts are globs; only literal Crow push names bind.
            let name = context
                .as_str()
                .and_then(|context| context.strip_prefix("ci/crow/push/"))
                .filter(|name| {
                    !name.is_empty()
                        && !name.contains(['*', '?', '[', ']', '{', '}', '!', '\\', '$'])
                })
                .context("external or unbound required context needs a supporting adapter")?;
            Ok(Requirement {
                name: name.into(),
                authority: authority.into(),
            })
        })
        .collect()
}

fn execute_with(
    request: &AdapterRequest,
    forge: impl Fn(&str) -> Result<Value>,
    crow: impl Fn(&str) -> Result<Value>,
) -> Result<AdapterResponse> {
    request.validate()?;
    ensure!(
        request.landing.is_none()
            && !matches!(request.operation.as_str(), "propose" | "merge" | "close"),
        "the Forgejo adapter supports direct landing only"
    );
    origin(&request.authority)?;
    ensure!(
        request
            .required_checks
            .iter()
            .all(|r| r.authority == request.authority),
        "external required checks need a supporting adapter"
    );
    let (_, repository) = request
        .coordinate
        .split_once('/')
        .context("missing repository")?;
    ensure!(
        repository.split('/').count() == 2,
        "Forgejo adapter requires owner/repository identity"
    );
    let endpoint = format!("repos/{repository}");
    let repo = forge(&endpoint)?;
    ensure!(
        repo["full_name"] == repository && repo["archived"] == false && repo["mirror"] == false,
        "repository renamed, archived or mirrored"
    );
    let id = number(&repo, "/id")?;
    let default_branch = text(&repo, "/default_branch")?;
    let branch = request.branch.as_deref().unwrap_or(&default_branch);
    // Crow must build this exact forge repository. Remote ids are only unique
    // per forge connection, so the clone URL binds the Forgejo instance too.
    let ci = crow(&format!("repos/lookup/{repository}"))?;
    ensure!(
        ci["forge_remote_id"].as_str() == Some(id.to_string().as_str())
            && ci["clone_url"] == repo["clone_url"],
        "Crow repository is bound to another forge repository"
    );
    // Crow expands ${CI_REPO_DEFAULT_BRANCH} from its own record.
    ensure!(
        ci["default_branch"] == default_branch.as_str(),
        "Crow and Forgejo disagree on the default branch; resynchronize the Crow repository"
    );
    ensure!(ci["active"] == true, "Crow repository is not active");
    ensure!(
        matches!(
            ci["require_approval"].as_str(),
            Some("none" | "forks" | "pull_requests")
        ),
        "Crow approval gates on push events need an adapter with approval qualification"
    );
    ensure!(
        ci["config_file"] == "",
        "custom Crow configuration paths need a source-aware adapter"
    );
    let ci_id = number(&ci, "/id")?;
    // Rules can govern a maintenance ref before it exists.
    let rules = admit_protections(&branch_protections(&endpoint, &forge)?, branch)?;
    let mut response = AdapterResponse {
        schema_version: request.schema_version,
        coordinate: request.coordinate.clone(),
        repository_id: id.to_string(),
        default_branch: default_branch.clone(),
        revision: String::new(),
        remote: text(&repo, "/clone_url")?,
        policy_digest: crate::review::canonical_digest(&Value::Null)?,
        requirements: vec![],
        status: "success".into(),
        checks: vec![],
        change: None,
    };
    if request.operation == "inspect" {
        let info = forge(&format!("{endpoint}/branches/{}", encoded(branch)))?;
        response.revision = text(&info, "/commit/id")?;
        response.requirements = branch_requirements(&info, &request.authority)?;
        let subset = |value: &Value, keys: &[&str]| {
            keys.iter()
                .map(|key| (key.to_string(), value[key].clone()))
                .collect::<serde_json::Map<_, _>>()
        };
        response.policy_digest = crate::review::canonical_digest(&serde_json::json!({
            "protection": rules,
            "branch": subset(&info, &["protected", "required_approvals", "enable_status_check", "status_check_contexts", "user_can_push", "effective_branch_protection_name"]),
            "crow": subset(&ci, &["id", "forge_remote_id", "clone_url", "default_branch", "require_approval", "config_file"]),
        }))?;
        response.validate(request)?;
        return Ok(response);
    }
    let revision = request
        .revision
        .clone()
        .context("admission and qualification need an exact revision")?;
    response.revision = revision.clone();
    if request.operation == "admit" {
        let plan = request.plan.as_ref().context("missing admission plan")?;
        let workflows = owned(&request.workflows, branch, &default_branch, |path| {
            Ok(plan
                .files
                .iter()
                .find(|file| file.relative_path == std::path::Path::new(path))
                .context("workflow is absent from candidate plan")?
                .content
                .clone()
                .into_bytes())
        })?;
        ensure!(
            !workflows.is_empty(),
            "candidate has no owned Crow workflow for a push to {branch}"
        );
        for check in &request.required_checks {
            ensure!(
                workflows
                    .values()
                    .any(|workflow| workflow.name == check.name),
                "required check {} is not provably produced by an owned Crow push workflow",
                check.name
            );
        }
        response.checks.push(Check {
            name: "admission".into(),
            authority: request.authority.clone(),
            revision,
            run: crate::review::canonical_digest(request)?,
            attempt: 1,
            status: "success".into(),
        });
        response.validate(request)?;
        return Ok(response);
    }
    if request.workflows.is_empty() {
        // The generator remains qualified by its enrolled GitHub Actions forge.
        response.status = "unsupported".into();
        return Ok(response);
    }
    let mut sources = BTreeMap::new();
    let workflows = owned(&request.workflows, branch, &default_branch, |path| {
        let encoded_path = path.split('/').map(encoded).collect::<Vec<_>>().join("/");
        let bytes = workflow_bytes(
            path,
            &forge(&format!(
                "{endpoint}/contents/{encoded_path}?ref={revision}"
            ))?,
        )?;
        sources.insert(path.to_owned(), bytes.clone());
        Ok(bytes)
    })?;
    ensure!(
        !workflows.is_empty(),
        "revision has no owned Crow workflow for a push to {branch}"
    );
    let committed = text(
        &forge(&format!(
            "{endpoint}/git/commits/{revision}?stat=false&verification=false&files=false"
        ))?,
        "/commit/committer/date",
    )?;
    // ponytail: a pipeline follows its push, so committer time less an hour of
    // clock skew bounds the scan and 20 pages of 50 cap it; widen if exceeded.
    let after = (chrono::DateTime::parse_from_rfc3339(&committed)? - chrono::Duration::hours(1))
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut originals = BTreeSet::new();
    for page in 1.. {
        ensure!(
            page <= 20,
            "Crow pipeline history exceeds adapter pagination ceiling"
        );
        let pipelines = crow(&format!(
            "repos/{ci_id}/pipelines?branch={}&event=push&after={}&page={page}&perPage=50",
            encoded(branch),
            encoded(&after)
        ))?;
        let pipelines = pipelines
            .as_array()
            .context("missing Crow pipeline inventory")?;
        for pipeline in pipelines {
            // Restarts (parent > 0) replay an earlier run and never qualify.
            if pipeline["commit"] == revision.as_str() && pipeline["parent"] == 0 {
                originals.insert(number(pipeline, "/number")?);
            }
        }
        if pipelines.len() < 50 {
            break;
        }
    }
    ensure!(
        originals.len() <= 1,
        "ambiguous original Crow pipelines for candidate"
    );
    let Some(pipeline_number) = originals.first() else {
        response.status = "pending".into();
        return Ok(response);
    };
    let pipeline = crow(&format!("repos/{ci_id}/pipelines/{pipeline_number}"))?;
    ensure!(
        pipeline["number"] == *pipeline_number
            && pipeline["commit"] == revision.as_str()
            && pipeline["parent"] == 0
            && pipeline["event"] == "push"
            && pipeline["branch"] == branch
            && pipeline["ref"] == format!("refs/heads/{branch}"),
        "untrusted Crow pipeline source/identity/event"
    );
    let configs = crow(&format!("repos/{ci_id}/pipelines/{pipeline_number}/config"))?;
    let configs = configs
        .as_array()
        .context("missing Crow pipeline configuration")?;
    let rows = pipeline["workflows"]
        .as_array()
        .context("missing Crow workflow inventory")?;
    for (path, workflow) in &workflows {
        let config = configs
            .iter()
            .filter(|config| config["name"] == path.as_str())
            .collect::<Vec<_>>();
        ensure!(
            config.len() == 1
                && config[0]["generated_from"]
                    .as_str()
                    .is_none_or(str::is_empty)
                && base64::engine::general_purpose::STANDARD.decode(text(config[0], "/data")?)?
                    == sources[path],
            "Crow pipeline configuration for {path} differs from the forge revision"
        );
        // Workflow and step reruns add attempts beside the original records.
        let row = rows
            .iter()
            .filter(|row| row["name"] == workflow.name.as_str())
            .collect::<Vec<_>>();
        ensure!(
            row.len() == 1
                && row[0]["pipeline_name"] == workflow.name.as_str()
                && row[0]["attempt"] == 1
                && row[0]["original_workflow_id"] == 0,
            "Crow workflow {} is missing, duplicated or rerun",
            workflow.name
        );
        match row[0]["state"].as_str() {
            Some("success") => {}
            Some("created" | "pending" | "running" | "blocked") => {
                response.status = "pending".into();
                continue;
            }
            // Quota denial fails the workflow before it starts; it never ran.
            _ if row[0]["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("CI quota denied")) =>
            {
                bail!(
                    "Crow CI quota exhausted for this period; rerun after reset ({})",
                    workflow.name
                )
            }
            _ => bail!("Crow workflow {} did not succeed", workflow.name),
        }
        let mut expected = workflow.steps.clone();
        for step in row[0]["children"]
            .as_array()
            .context("missing Crow step inventory")?
        {
            let name = step["name"].as_str().unwrap_or_default();
            ensure!(
                expected.remove(name)
                    && (name != "clone" || step["type"] == "clone")
                    && step["attempt"] == 1
                    && step["original_step_id"] == 0
                    && step["state"] == "success"
                    && step["exit_code"] == 0,
                "Crow workflow {} has an unexpected, rerun or non-success step",
                workflow.name
            );
        }
        ensure!(
            expected.is_empty(),
            "Crow workflow {} is missing source-bound steps",
            workflow.name
        );
        response.checks.push(Check {
            name: workflow.name.clone(),
            authority: request.authority.clone(),
            revision: revision.clone(),
            run: format!("{ci_id}/{pipeline_number}"),
            attempt: 1,
            status: "success".into(),
        });
    }
    // Absence of an enrolled required workflow is pending, never success.
    if request.required_checks.iter().any(|required| {
        !response
            .checks
            .iter()
            .any(|check| check.name == required.name)
    }) {
        response.status = "pending".into();
    }
    response.validate(request)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const REVISION: &str = "a96a22ab6f554631e72068acdf22baab4bdbe622";
    const ORIGIN: &str = "https://ci.codefloe.com";
    const BUILD: &str = ".crow/build-pink-raven.yaml";
    const VM: &str = ".crow/vm-checks.yaml";
    const MAINTENANCE: &str = "simit/update/0123456789abcdef";
    const BRANCH: &str = "repos/caniko/pink-raven/branches/trunk";
    const LIST: &str = "repos/1906/pipelines?branch=trunk&event=push&after=2026-10-02T06%3A34%3A59Z&page=1&perPage=50";
    const PIPELINE: &str = "repos/1906/pipelines/52";
    const CONFIG: &str = "repos/1906/pipelines/52/config";

    /// Real codefloe Forgejo and Crow replies: (fixture, forge, crow).
    fn fixture() -> (Value, Value, Value) {
        let value: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/forgejo-adapter/pink-raven.json"
        ))
        .unwrap();
        (value.clone(), value["forge"].clone(), value["crow"].clone())
    }

    /// Fixtures hold the whole branch-protection inventory as page one.
    fn forge_reply(forge: &Value, path: &str) -> Result<Value> {
        if let Some((inventory, page)) = path.split_once("?page=")
            && inventory.ends_with("/branch_protections")
        {
            return Ok(if page.starts_with("1&") {
                forge[inventory].clone()
            } else {
                json!([])
            });
        }
        forge
            .get(path)
            .cloned()
            .with_context(|| format!("unexpected forge endpoint {path}"))
    }

    fn replay(request: &AdapterRequest, forge: &Value, crow: &Value) -> Result<AdapterResponse> {
        execute_with(
            request,
            |path| forge_reply(forge, path),
            |path| {
                crow.get(path)
                    .cloned()
                    .with_context(|| format!("unexpected Crow endpoint {path}"))
            },
        )
    }

    fn source(forge: &Value, path: &str) -> String {
        let file = &forge[format!("repos/caniko/pink-raven/contents/{path}?ref={REVISION}")];
        String::from_utf8(workflow_bytes(path, file).unwrap()).unwrap()
    }

    fn request(operation: &str) -> AdapterRequest {
        serde_json::from_value(json!({
            "schemaVersion":2,"operation":operation,"coordinate":"codefloe.com/caniko/pink-raven",
            "apiUrl":"https://codefloe.com/api/v1","credentialFile":"/run/credentials/forgejo",
            "ciCredentialFile":"/run/credentials/crow","authority":ORIGIN,
            "revision":(operation != "inspect").then_some(REVISION),"branch":null,
            "workflows":[],"requiredChecks":[]
        }))
        .unwrap()
    }

    fn admission(forge: &Value, files: &[(&str, String)]) -> AdapterRequest {
        let mut request = request("admit");
        request.branch = Some(MAINTENANCE.into());
        request.workflows = files.iter().map(|(path, _)| (*path).into()).collect();
        request.plan = Some(crate::project::CiUpdatePlan {
            schema: "simit.ci-update-plan.v1".into(),
            files: files
                .iter()
                .map(|(path, content)| crate::project::GeneratedFile {
                    relative_path: path.into(),
                    content: if content.is_empty() {
                        source(forge, path)
                    } else {
                        content.clone()
                    },
                })
                .collect(),
            obsolete: vec![],
            patches: vec![],
        });
        request
    }

    #[test]
    fn production_adapter_replays_codefloe_inspection_admission_and_original_qualification() {
        let (fixture, mut forge, crow) = fixture();
        let mut request = request("inspect");
        let inspected = replay(&request, &forge, &crow).unwrap();
        assert_eq!(inspected.revision, REVISION);
        assert!(inspected.requirements.is_empty());
        // The commit's real push status contexts bind to Crow workflow names.
        forge[BRANCH]["enable_status_check"] = json!(true);
        forge[BRANCH]["status_check_contexts"] = json!(
            fixture["statusContexts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|context| context.as_str().unwrap().starts_with("ci/crow/push/"))
                .collect::<Vec<_>>()
        );
        let inspected = replay(&request, &forge, &crow).unwrap();
        assert_eq!(
            inspected.requirements,
            ["build", "vm-checks"].map(|name| Requirement {
                name: name.into(),
                authority: ORIGIN.into()
            })
        );
        request = admission(&forge, &[(BUILD, String::new()), (VM, String::new())]);
        request.required_checks = inspected.requirements;
        for branch in [MAINTENANCE, "trunk"] {
            request.branch = Some(branch.into());
            let admitted = replay(&request, &forge, &crow).unwrap();
            assert_eq!(
                admitted.checks[0].run,
                crate::review::canonical_digest(&request).unwrap()
            );
        }
        request.operation = "qualify".into();
        request.plan = None;
        request.branch = Some("trunk".into());
        let qualified = replay(&request, &forge, &crow).unwrap();
        assert_eq!(qualified.status, "success");
        assert_eq!(
            qualified
                .checks
                .iter()
                .map(|check| (check.name.as_str(), check.run.as_str()))
                .collect::<Vec<_>>(),
            [("build", "1906/52"), ("vm-checks", "1906/52")]
        );
        let mutated = |change: &dyn Fn(&mut Value)| {
            let mut crow = crow.clone();
            change(&mut crow);
            replay(&request, &forge, &crow)
        };
        let status = |change: &dyn Fn(&mut Value)| mutated(change).unwrap().status;
        // A restart alone is not original evidence, and cannot rescue a failure.
        assert_eq!(
            status(&|crow| crow[LIST][1]["parent"] = json!(51)),
            "pending"
        );
        assert!(
            mutated(&|crow| {
                let mut restart = crow[LIST][1].clone();
                restart["number"] = json!(60);
                restart["parent"] = json!(52);
                crow[LIST].as_array_mut().unwrap().insert(0, restart);
                crow[PIPELINE]["workflows"][0]["state"] = json!("failure");
            })
            .is_err()
        );
        assert_eq!(
            status(&|crow| crow[PIPELINE]["workflows"][0]["state"] = json!("running")),
            "pending"
        );
        let message = |change: &dyn Fn(&mut Value)| mutated(change).unwrap_err().to_string();
        assert!(
            message(&|crow| crow["repos/lookup/caniko/pink-raven"]["active"] = json!(false))
                .contains("not active")
        );
        // Quota denial as codefloe reports it (pink-raven pipelines 118-120).
        assert!(
            message(&|crow| {
                let build = &mut crow[PIPELINE]["workflows"][0];
                build["state"] = json!("failure");
                build["error"] = json!(
                    "CI quota denied (linux): the CI time quota \"CI quota (all)\" is used up: 1 day 19 hours spent of 1 day 17 hours this period"
                );
                for step in build["children"].as_array_mut().unwrap() {
                    step["state"] = json!("skipped");
                }
            })
            .contains("quota exhausted")
        );
        let changes: Vec<&dyn Fn(&mut Value)> = vec![
            &|crow| crow["repos/lookup/caniko/pink-raven"]["forge_remote_id"] = json!("5405"),
            // Same name and id on another forge connection.
            &|crow| {
                crow["repos/lookup/caniko/pink-raven"]["clone_url"] =
                    json!("https://forge.example/caniko/pink-raven.git")
            },
            &|crow| crow["repos/lookup/caniko/pink-raven"]["default_branch"] = json!("main"),
            &|crow| {
                crow["repos/lookup/caniko/pink-raven"]["require_approval"] = json!("all_events")
            },
            &|crow| crow["repos/lookup/caniko/pink-raven"]["config_file"] = json!(".ci/"),
            // Duplicate originals for one commit are ambiguous.
            &|crow| {
                let mut duplicate = crow[LIST][1].clone();
                duplicate["number"] = json!(53);
                crow[LIST].as_array_mut().unwrap().push(duplicate);
            },
            &|crow| crow[PIPELINE]["branch"] = json!("other"),
            &|crow| crow[PIPELINE]["event"] = json!("manual"),
            &|crow| {
                crow[CONFIG][0]["data"] =
                    json!(base64::engine::general_purpose::STANDARD.encode("name: build\n"))
            },
            &|crow| crow[CONFIG][0]["generated_from"] = json!(".crow/build.jsonnet"),
            &|crow| crow[CONFIG].as_array_mut().unwrap().truncate(1),
            &|crow| crow[PIPELINE]["workflows"][0]["attempt"] = json!(2),
            &|crow| crow[PIPELINE]["workflows"][0]["original_workflow_id"] = json!(150914),
            &|crow| {
                let rerun = crow[PIPELINE]["workflows"][0].clone();
                crow[PIPELINE]["workflows"]
                    .as_array_mut()
                    .unwrap()
                    .push(rerun);
            },
            &|crow| crow[PIPELINE]["workflows"][0]["state"] = json!("skipped"),
            &|crow| crow[PIPELINE]["workflows"][0]["children"][2]["attempt"] = json!(2),
            &|crow| crow[PIPELINE]["workflows"][0]["children"][2]["state"] = json!("failure"),
            &|crow| crow[PIPELINE]["workflows"][0]["children"][2]["state"] = json!("skipped"),
            &|crow| crow[PIPELINE]["workflows"][0]["children"][0]["type"] = json!("commands"),
            &|crow| {
                crow[PIPELINE]["workflows"][0]["children"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            },
            &|crow| {
                let extra = crow[PIPELINE]["workflows"][0]["children"][2].clone();
                let mut extra = extra;
                extra["name"] = json!("project-check-pr");
                crow[PIPELINE]["workflows"][0]["children"]
                    .as_array_mut()
                    .unwrap()
                    .push(extra);
            },
        ];
        for change in changes {
            assert!(mutated(change).is_err());
        }
        // Generator qualification stays with its GitHub Actions forge.
        request.workflows.clear();
        assert_eq!(
            replay(&request, &forge, &crow).unwrap().status,
            "unsupported"
        );
    }

    #[test]
    fn inspection_and_admission_reject_unbound_policy_on_target_and_maintenance_refs() {
        let (_, forge, crow) = fixture();
        let inspect = request("inspect");
        let mutated = |request: &AdapterRequest, change: &dyn Fn(&mut Value)| {
            let mut forge = forge.clone();
            change(&mut forge);
            replay(request, &forge, &crow)
        };
        let changes: Vec<&dyn Fn(&mut Value)> = vec![
            &|forge| forge[BRANCH]["required_approvals"] = json!(1),
            &|forge| forge[BRANCH]["user_can_push"] = json!(false),
            &|forge| forge["repos/caniko/pink-raven"]["archived"] = json!(true),
            &|forge| forge["repos/caniko/pink-raven"]["mirror"] = json!(true),
            &|forge| forge["repos/caniko/pink-raven"]["full_name"] = json!("caniko/renamed"),
        ];
        for change in changes {
            assert!(mutated(&inspect, change).is_err());
        }
        for contexts in [
            json!([]),
            json!(["ci/crow/pr/build"]),
            json!(["ci/crow/push/*"]),
            json!(["ci/crow/push/"]),
            json!(["ci/woodpecker/push/build"]),
            json!(["build"]),
        ] {
            assert!(
                mutated(&inspect, &|forge| {
                    forge[BRANCH]["enable_status_check"] = json!(true);
                    forge[BRANCH]["status_check_contexts"] = contexts.clone();
                })
                .is_err(),
                "{contexts}"
            );
        }
        let rule = json!({"rule_name":"trunk","required_approvals":0,"require_signed_commits":false,
            "protected_file_patterns":"","enable_push":true,"enable_push_whitelist":false});
        let protected = |rule: Value| {
            move |forge: &mut Value| {
                forge["repos/caniko/pink-raven/branch_protections"] = json!([rule.clone()])
            }
        };
        mutated(&inspect, &protected(rule.clone())).unwrap();
        for (key, value) in [
            ("required_approvals", json!(1)),
            ("require_signed_commits", json!(true)),
            ("protected_file_patterns", json!(".crow/*.yaml")),
            ("enable_push", json!(false)),
            ("enable_push_whitelist", json!(true)),
            ("rule_name", json!("tr[u]nk")),
        ] {
            let mut rule = rule.clone();
            rule[key] = value;
            assert!(
                mutated(&inspect, &protected(rule.clone())).is_err(),
                "{key}"
            );
        }
        // A maintenance-ref rule applies before that branch exists.
        let mut maintenance = rule.clone();
        maintenance["rule_name"] = json!("simit/update/*");
        maintenance["require_signed_commits"] = json!(true);
        let mut admit = admission(&forge, &[(BUILD, String::new())]);
        assert!(mutated(&admit, &protected(maintenance.clone())).is_err());
        admit.branch = Some("trunk".into());
        mutated(&admit, &protected(maintenance.clone())).unwrap();
        mutated(&inspect, &protected(maintenance.clone())).unwrap();
        let mut authority = inspect;
        for origin in ["https://ci.codefloe.com/", "http://ci.codefloe.com", "777"] {
            authority.authority = origin.into();
            assert!(replay(&authority, &forge, &crow).is_err(), "{origin}");
        }
    }

    #[test]
    fn admission_rejects_crow_constructs_without_static_outcome_binding() {
        let (_, forge, crow) = fixture();
        let build = source(&forge, BUILD);
        replay(&admission(&forge, &[(BUILD, build.clone())]), &forge, &crow).unwrap();
        for (from, to) in [
            (
                "when:\n- event: push\n",
                "when:\n- event: push\n  path: src/**\n",
            ),
            (
                "when:\n- event: push\n",
                "when:\n- event: push\n  evaluate: 'true'\n",
            ),
            (
                "when:\n- event: push\n",
                "when:\n- event: ${CI_PIPELINE_EVENT}\n",
            ),
            (
                "when:\n- event: push\n",
                "when:\n- event: push\n  branch: ${CI_COMMIT_BRANCH}\n",
            ),
            (
                "when:\n- event: push\n",
                "when:\n- event: push\n  branch:\n    exclude: [main]\n",
            ),
            ("name: build\n", "name: build\nmatrix:\n  X: [a, b]\n"),
            ("name: build\n", "name: build\ndepends_on: [lint]\n"),
            (
                "name: build\n",
                "name: build\nruns_on: [success, failure]\n",
            ),
            (
                "name: build\n",
                "name: build\napproval:\n  required: true\n",
            ),
            ("name: build\n", "name: ${CI_REPO_NAME}\n"),
            (
                "- name: nix-config\n",
                "- name: nix-config\n  failure: ignore\n",
            ),
            (
                "- name: nix-config\n",
                "- name: nix-config\n  detach: true\n",
            ),
            ("- name: nix-config\n", "- name: clone\n"),
            ("- name: nix-config\n", "- name: project-check-pr\n"),
        ] {
            assert!(build.contains(from), "{from}");
            let changed = admission(&forge, &[(BUILD, build.replace(from, to))]);
            assert!(replay(&changed, &forge, &crow).is_err(), "{to}");
        }
        let jsonnet = admission(&forge, &[(".crow/build.jsonnet", "{}".into())]);
        assert!(replay(&jsonnet, &forge, &crow).is_err());
        let mut external = admission(&forge, &[(BUILD, build.clone())]);
        external.required_checks = vec![Requirement {
            name: "vm-checks".into(),
            authority: ORIGIN.into(),
        }];
        assert!(replay(&external, &forge, &crow).is_err());
        // A default-branch-only workflow cannot qualify a maintenance branch.
        let mut trunk_only = admission(
            &forge,
            &[(
                BUILD,
                build.replace(
                    "- event: push\n- event: pull_request\n",
                    "- event: push\n  branch: ${CI_REPO_DEFAULT_BRANCH}\n",
                ),
            )],
        );
        assert!(replay(&trunk_only, &forge, &crow).is_err());
        trunk_only.branch = Some("trunk".into());
        replay(&trunk_only, &forge, &crow).unwrap();
    }

    #[test]
    fn pipeline_search_pages_until_a_short_page_within_the_ceiling() {
        let (_, forge, crow) = fixture();
        let mut request = request("qualify");
        request.branch = Some("trunk".into());
        request.workflows = vec![BUILD.into(), VM.into()];
        let search = |last: usize| {
            execute_with(
                &request,
                |path| forge_reply(&forge, path),
                |path| {
                    let Some(page) = path
                        .strip_prefix(LIST.trim_end_matches("page=1&perPage=50"))
                        .and_then(|rest| rest.strip_suffix("&perPage=50"))
                        .and_then(|rest| rest.strip_prefix("page="))
                    else {
                        return crow.get(path).cloned().context("Crow endpoint");
                    };
                    let page: usize = page.parse()?;
                    Ok(if page < last {
                        json!(vec![
                            json!({"number":1,"parent":0,"commit":"b".repeat(40)});
                            50
                        ])
                    } else {
                        crow[LIST].clone()
                    })
                },
            )
        };
        assert_eq!(search(2).unwrap().status, "success");
        assert_eq!(search(20).unwrap().status, "success");
        assert!(search(21).is_err());
    }

    #[test]
    fn branch_protections_are_read_across_pages() {
        let (_, forge, crow) = fixture();
        let mut request = request("inspect");
        request.branch = Some("trunk".into());
        let rule = |name: &str| json!({"rule_name":name,"required_approvals":0,"require_signed_commits":false,"protected_file_patterns":"","enable_push":true,"enable_push_whitelist":false});
        let mut restrictive = rule("trunk");
        restrictive["required_approvals"] = json!(1);
        let run = |second: Value| {
            execute_with(
                &request,
                |path| match path.split_once("/branch_protections?page=") {
                    Some((_, "1&limit=50")) => Ok(json!([rule("other")])),
                    Some((_, "2&limit=50")) => Ok(second.clone()),
                    Some(_) => Ok(json!([])),
                    None => forge_reply(&forge, path),
                },
                |path| crow.get(path).cloned().context("Crow endpoint"),
            )
        };
        run(json!([rule("unrelated")])).unwrap();
        // A matching rule on page two still gates direct landing.
        assert!(run(json!([restrictive])).is_err());
    }

    #[test]
    fn crow_conditions_names_and_compile_time_step_filters_follow_crow() {
        let steps = "steps:\n- name: test\n  image: alpine\n  commands: [true]\n- name: pr\n  image: alpine\n  commands: [true]\n  when:\n    event: pull_request\n";
        const RESTRICTED: &str = "when:\n- event: push\n  branch: ['${CI_REPO_DEFAULT_BRANCH}', 'simit/update/**']\n- event: pull_request\n";
        for (when, branch, runs) in [
            ("when:\n  event: push\n", MAINTENANCE, true),
            ("when:\n  event: [tag, pull_request]\n", MAINTENANCE, false),
            (
                "when:\n- event: tag\n- branch: simit/update/*\n",
                MAINTENANCE,
                true,
            ),
            (
                "when:\n- event: push\n  branch: [main, 'simit/**']\n",
                MAINTENANCE,
                true,
            ),
            (
                "when:\n- event: push\n  branch: simit/*\n",
                MAINTENANCE,
                false,
            ),
            (
                "when:\n- event: push\n  branch: ${CI_REPO_DEFAULT_BRANCH}\n",
                MAINTENANCE,
                false,
            ),
            (
                "when:\n- event: push\n  branch: ${CI_REPO_DEFAULT_BRANCH}\n",
                "trunk",
                true,
            ),
            // Generated push triggers: default branch plus the maintenance namespace.
            (RESTRICTED, MAINTENANCE, true),
            (RESTRICTED, "trunk", true),
            (RESTRICTED, "feature/x", false),
        ] {
            let parsed = workflow(
                ".crow/.test.yml",
                format!("{when}{steps}").as_bytes(),
                branch,
                "trunk",
            )
            .unwrap();
            assert_eq!(parsed.is_some(), runs, "{when}");
            if let Some(parsed) = parsed {
                assert_eq!(parsed.name, "test");
                assert_eq!(
                    parsed.steps,
                    BTreeSet::from(["clone".into(), "test".into()])
                );
            }
        }
        // Crow collects no workflow without an event trigger.
        for when in [
            "",
            "when: []\n",
            "when:\n- branch: trunk\n",
            "when:\n  event: []\n",
        ] {
            let source = format!("{when}{steps}");
            assert!(
                workflow(".crow/ci.yaml", source.as_bytes(), "trunk", "trunk").is_err(),
                "{when}"
            );
        }
        let skip = format!("when:\n  event: push\nskip_clone: true\n{steps}");
        let parsed = workflow(".crow/ci.yaml", skip.as_bytes(), "trunk", "trunk")
            .unwrap()
            .unwrap();
        assert_eq!((parsed.name.as_str(), parsed.steps.len()), ("ci", 1));
        // Only filtered steps remain: Crow skips the workflow.
        let steps = format!("when:\n  event: push\n{steps}");
        let filtered = steps.replace(
            "  commands: [true]\n- name: pr",
            "  commands: [true]\n  when:\n    event: tag\n- name: pr",
        );
        assert!(
            workflow(".crow/ci.yaml", filtered.as_bytes(), "trunk", "trunk")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn generated_crow_primary_ci_is_admitted_on_maintenance_and_default_branches() {
        use crate::{
            cli::{CiProvider, Platform, Runtime},
            config::CrowCiConfig,
            render::ci::{self, CiOptions, FilesRequest, SelfCheckOptions},
            user_config::{ResolvedCiRunners, ResolvedRunner},
        };
        let package = serde_json::from_value(json!({
            "id":"demo", "name":"demo", "version":"0.1.0", "manifest_path":"/demo/Cargo.toml"
        }))
        .unwrap();
        let runner = ResolvedRunner {
            name: None,
            labels: vec!["codefloe-infra".into()],
        };
        let runners = ResolvedCiRunners {
            ci: runner.clone(),
            release: runner,
            windows: None,
        };
        let gated = CrowCiConfig {
            check_command: Some("bash scripts/ci/crow-nix-gate.sh project-check".into()),
            check_push_secret: Some("ATTIC_TOKEN".into()),
            ..Default::default()
        };
        for runtime in [Runtime::Cargo, Runtime::Nix] {
            for crow in [&CrowCiConfig::default(), &gated] {
                let files = ci::primary_files(FilesRequest {
                    provider: CiProvider::Crow,
                    platform: Platform::Forgejo,
                    crow,
                    runtime,
                    package: &package,
                    file_suffix: None,
                    self_check: SelfCheckOptions {
                        enabled: false,
                        release: false,
                        runner_override: None,
                        windows_runner_override: None,
                        packages: &[],
                        workspace: true,
                    },
                    runners: &runners,
                    step_runners: &Default::default(),
                    options: CiOptions {
                        repository_checks: true,
                        ..Default::default()
                    },
                })
                .unwrap();
                let paths = files
                    .iter()
                    .map(|file| file.relative_path.to_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                for branch in [MAINTENANCE, "trunk"] {
                    let admitted = owned(&paths, branch, "trunk", |path| {
                        Ok(files
                            .iter()
                            .find(|file| file.relative_path == std::path::Path::new(path))
                            .unwrap()
                            .content
                            .clone()
                            .into_bytes())
                    })
                    .unwrap();
                    assert!(!admitted.is_empty(), "{runtime:?} {paths:?}");
                }
            }
        }
    }
}
