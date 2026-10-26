//! GitHub Actions adapter. Provider identifiers and API location come from policy.
use super::schema::*;
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    time::Duration,
};

struct Api {
    client: reqwest::blocking::Client,
    base: url::Url,
    token: Option<String>,
}
impl Api {
    fn new(request: &AdapterRequest) -> Result<Self> {
        let base = url::Url::parse(&format!("{}/", request.api_url.trim_end_matches('/')))?;
        ensure!(
            base.scheme() == "https" && base.username().is_empty() && base.password().is_none(),
            "GitHub API requires credential-free HTTPS"
        );
        let token = request
            .credential_file
            .as_ref()
            .map(|path| {
                String::from_utf8(credential_bytes(path)?).context("GitHub credential is not UTF-8")
            })
            .transpose()?
            .map(|s| s.trim().to_owned());
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base,
            token,
        })
    }
    fn get(&self, path: &str) -> Result<Value> {
        self.send(self.client.get(self.base.join(path)?))
    }
    fn send(&self, request: reqwest::blocking::RequestBuilder) -> Result<Value> {
        let mut request = request
            .header("User-Agent", "simit-updater")
            .header("Accept", "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().context("GitHub API transport failed")?;
        ensure!(
            response.status().is_success(),
            "GitHub API returned {}",
            response.status()
        );
        let mut bytes = Vec::new();
        response
            .take(crate::review::MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= crate::review::MAX_JSON_BYTES,
            "GitHub response too large"
        );
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn admit_classic(&self, repository: &str, branch: &str) -> Result<()> {
        let (owner, name) = repository
            .split_once('/')
            .context("missing repository owner")?;
        // GitHub Enterprise Server puts GraphQL at /api/graphql, beside REST
        // /api/v3; other configured API roots use the relative /graphql route.
        let endpoint = self.base.join(if self.base.path().ends_with("/v3/") {
            "../graphql"
        } else {
            "graphql"
        })?;
        admit_classic_rules(branch, |cursor| {
            self.send(self.client.post(endpoint.clone()).json(&serde_json::json!({
            "query":"query($owner:String!,$name:String!,$cursor:String){repository(owner:$owner,name:$name){branchProtectionRules(first:100,after:$cursor){totalCount nodes{id pattern restrictsPushes blocksCreations requiresCommitSignatures requiresApprovingReviews} pageInfo{hasNextPage endCursor}}}}",
            "variables":{"owner":owner,"name":name,"cursor":cursor}
        })))
        })
    }
}

fn list(path: &str, key: &str, get: &impl Fn(&str) -> Result<Value>) -> Result<Value> {
    collection(key, |page| {
        let separator = if path.contains('?') { '&' } else { '?' };
        get(&format!("{path}{separator}per_page=100&page={page}"))
    })
}

fn collection(key: &str, mut page: impl FnMut(u32) -> Result<Value>) -> Result<Value> {
    let mut result = Vec::new();
    let mut reported = None;
    for index in 1..=20 {
        let mut value = page(index)?;
        let entries = if key.is_empty() {
            value.as_array()
        } else {
            let count = number(&value, "/total_count")?;
            ensure!(
                reported.is_none_or(|previous| previous == count),
                "GitHub collection count changed during pagination"
            );
            reported = Some(count);
            value[key].as_array()
        }
        .context("missing GitHub collection")?;
        let complete = entries.len() < 100;
        result.extend(entries.iter().cloned());
        if complete {
            if key.is_empty() {
                return Ok(Value::Array(result));
            }
            ensure!(
                reported == Some(result.len() as u64),
                "incomplete GitHub collection inventory"
            );
            value[key] = Value::Array(result);
            return Ok(value);
        }
    }
    anyhow::bail!("GitHub collection exceeds adapter pagination ceiling")
}

fn text(value: &Value, key: &str) -> Result<String> {
    Ok(value
        .pointer(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing GitHub {key}"))?
        .to_owned())
}
fn number(value: &Value, key: &str) -> Result<u64> {
    value
        .pointer(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("missing GitHub {key}"))
}
fn encoded(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn branch_pattern(pattern: &str, branch: &str) -> Result<bool> {
    // ponytail: only literal names and * / ** patterns are admitted. Reject
    // richer GitHub syntax until it has an exact, provider-native evaluator.
    ensure!(
        !pattern.is_empty()
            && !pattern.contains(['!', '?', '+', '[', ']', '{', '}', '(', ')', '\\']),
        "unsupported GitHub branch filter pattern"
    );
    Ok(glob::Pattern::new(pattern)?.matches_with(
        branch,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        },
    ))
}

fn admit_branch(trigger: &serde_yaml::Value, branch: &str) -> Result<()> {
    ensure!(
        trigger.is_null() || trigger.is_mapping(),
        "invalid push trigger configuration"
    );
    ensure!(
        trigger.get("paths").is_none() && trigger.get("paths-ignore").is_none(),
        "path-filtered push workflows require an adapter with exact-change trigger admission"
    );
    let branches = trigger.get("branches");
    let ignored = trigger.get("branches-ignore");
    ensure!(
        branches.is_none() || ignored.is_none(),
        "workflow combines incompatible branch filters"
    );
    ensure!(
        branches.is_some()
            || ignored.is_some()
            || (trigger.get("tags").is_none() && trigger.get("tags-ignore").is_none()),
        "tag-only workflow cannot qualify a branch update"
    );
    let mut admitted = branches.is_none();
    if let Some(patterns) = branches {
        for value in patterns.as_sequence().context("invalid branches filter")? {
            let pattern = value.as_str().context("non-string branch filter")?;
            let (negated, pattern) = pattern
                .strip_prefix('!')
                .map_or((false, pattern), |p| (true, p));
            if branch_pattern(pattern, branch)? {
                admitted = !negated;
            }
        }
    }
    if let Some(patterns) = ignored {
        for value in patterns
            .as_sequence()
            .context("invalid branches-ignore filter")?
        {
            if branch_pattern(value.as_str().context("non-string branch filter")?, branch)? {
                admitted = false;
            }
        }
    }
    ensure!(
        admitted,
        "owned workflow excludes qualification branch {branch}"
    );
    Ok(())
}

fn workflow_source(path: &str, file: &Value) -> Result<serde_yaml::Value> {
    ensure!(
        file["path"] == path && file["type"] == "file" && file["encoding"] == "base64",
        "workflow source inventory is incomplete"
    );
    let content = text(file, "/content")?;
    let content =
        base64::engine::general_purpose::STANDARD.decode(content.replace(['\r', '\n'], ""))?;
    Ok(serde_yaml::from_slice(&content)?)
}

#[derive(Default)]
struct PushSteps {
    jobs: Option<BTreeSet<String>>,
    outcomes: BTreeMap<u64, (String, bool)>,
}

fn push_step_runs(condition: &str, default_branch: bool) -> Result<bool> {
    // ponytail: prove only these generated branch-push predicates. Inputs are
    // absent on push, hence false; extend with source-aware proofs, not eval.
    // https://docs.github.com/en/actions/reference/workflows-and-actions/contexts#inputs-context
    match condition {
        "${{ inputs.release }}"
        | "${{ inputs.release && github.event_name != 'pull_request' }}" => Ok(false),
        "${{ (github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)) || (inputs.release && github.event_name != 'pull_request') }}" => {
            Ok(default_branch)
        }
        "${{ always() }}" => Ok(true),
        _ => anyhow::bail!(
            "conditional jobs or steps require an adapter with a proven push predicate"
        ),
    }
}

fn push_steps(workflow: &serde_yaml::Value, default_branch: bool) -> Result<PushSteps> {
    let jobs = workflow["jobs"]
        .as_mapping()
        .context("missing workflow jobs")?;
    let mut expected = PushSteps::default();
    if !jobs.values().any(|job| {
        job["steps"]
            .as_sequence()
            .is_some_and(|steps| steps.iter().any(|step| step.get("if").is_some()))
    }) {
        return Ok(expected);
    }
    // ponytail: conditional steps support one source job and a static include
    // matrix; add provider-native job-id binding before supporting other shapes.
    ensure!(jobs.len() == 1, "conditional steps require one source job");
    for (id, job) in jobs {
        let steps = job["steps"]
            .as_sequence()
            .context("missing workflow steps")?;
        for (index, step) in steps.iter().enumerate() {
            let runs = if let Some(condition) = step.get("if") {
                let condition = condition
                    .as_str()
                    .context("conditional jobs or steps need a literal predicate")?;
                push_step_runs(condition, default_branch)?
            } else {
                true
            };
            let name = step["name"]
                .as_str()
                .context("conditional workflow steps need literal names")?;
            ensure!(
                !name.is_empty() && !name.contains("${{"),
                "conditional workflow steps need literal names"
            );
            expected
                .outcomes
                .insert(index as u64 + 2, (name.into(), runs));
        }
        if !expected.outcomes.is_empty() {
            let name = job
                .get("name")
                .unwrap_or(id)
                .as_str()
                .context("conditional steps need a literal job name")?;
            ensure!(
                !name.is_empty() && !name.contains("${{"),
                "conditional steps need a literal job name"
            );
            let names = if let Some(matrix) = job
                .get("strategy")
                .and_then(|strategy| strategy.get("matrix"))
            {
                let mapping = matrix
                    .as_mapping()
                    .context("conditional steps need a static include matrix")?;
                let include = matrix["include"]
                    .as_sequence()
                    .context("conditional steps need a static include matrix")?;
                ensure!(
                    mapping.len() == 1 && !include.is_empty(),
                    "conditional steps need a nonempty include-only matrix"
                );
                ensure!(
                    job.get("name").is_none(),
                    "conditional matrix job names must use native expansion"
                );
                let mut names = BTreeSet::new();
                for row in include {
                    let row = row
                        .as_mapping()
                        .context("conditional steps need literal matrix rows")?;
                    ensure!(
                        row.len() == 2
                            && row
                                .get(serde_yaml::Value::String("system".into()))
                                .is_some()
                            && row
                                .get(serde_yaml::Value::String("runner".into()))
                                .is_some()
                            && row.values().all(|value| value
                                .as_str()
                                .is_some_and(|value| !value.is_empty() && !value.contains("${{"))),
                        "conditional steps need literal native system/runner matrix rows"
                    );
                    let values = row
                        .values()
                        .map(|value| value.as_str().unwrap())
                        .collect::<Vec<_>>();
                    ensure!(
                        names.insert(format!("{name} ({})", values.join(", "))),
                        "conditional matrix job names collide"
                    );
                }
                names
            } else {
                BTreeSet::from([name.to_owned()])
            };
            expected.jobs = Some(names);
        }
    }
    Ok(expected)
}

fn push_workflow(path: &str, file: &Value, branch: Option<&str>) -> Result<bool> {
    let yaml = workflow_source(path, file)?;
    let unfiltered = serde_yaml::Value::Null;
    let trigger = match yaml.get("on") {
        Some(serde_yaml::Value::String(event)) => (event == "push").then_some(&unfiltered),
        Some(serde_yaml::Value::Sequence(events)) => events
            .iter()
            .any(|e| e.as_str() == Some("push"))
            .then_some(&unfiltered),
        Some(serde_yaml::Value::Mapping(events)) => {
            events.get(serde_yaml::Value::String("push".into()))
        }
        _ => None,
    };
    if let (Some(trigger), Some(branch)) = (trigger, branch) {
        admit_branch(trigger, branch)?;
    }
    if let Some(trigger) = trigger {
        admit_concurrency(path, &yaml, trigger)?;
        let jobs = yaml
            .get("jobs")
            .and_then(serde_yaml::Value::as_mapping)
            .context("missing workflow jobs")?;
        ensure!(!jobs.is_empty(), "empty workflow job inventory");
        for job in jobs.values() {
            ensure!(
                job.get("if").is_none(),
                "conditional jobs or steps require an adapter with job-outcome admission"
            );
            ensure!(
                job.get("concurrency").is_none(),
                "job concurrency requires an adapter with cancellation-safe scheduling admission"
            );
            ensure!(
                job.get("uses").is_none(),
                "reusable workflow outcomes require a source-aware adapter"
            );
            let steps = job
                .get("steps")
                .and_then(serde_yaml::Value::as_sequence)
                .context("missing workflow steps")?;
            ensure!(!steps.is_empty(), "empty workflow step inventory");
            for node in std::iter::once(job).chain(steps) {
                ensure!(
                    node.get("continue-on-error")
                        .is_none_or(|value| value.as_bool() == Some(false)),
                    "workflow can mask failed job or step outcomes"
                );
            }
        }
        push_steps(&yaml, false)?;
    }
    Ok(trigger.is_some())
}

fn admit_concurrency(
    path: &str,
    workflow: &serde_yaml::Value,
    trigger: &serde_yaml::Value,
) -> Result<()> {
    let Some(concurrency) = workflow.get("concurrency") else {
        return Ok(());
    };
    let group = concurrency
        .get("group")
        .and_then(serde_yaml::Value::as_str)
        .context("workflow concurrency requires an explicit workflow/ref-isolated group")?;
    // Aggregate CI coalesces push and PR runs by source repository and branch.
    // Its literal prefix must be this workflow's path; tags must be excluded so
    // a same-named tag cannot cancel a qualification branch's original run.
    let aggregate = group
        == format!(
            "{path}-${{{{ github.event.pull_request.head.repo.full_name || github.repository }}}}-${{{{ github.event.pull_request.head.ref || github.ref_name }}}}"
        )
        && trigger.get("tags").is_none()
        && trigger
            .get("tags-ignore")
            .and_then(serde_yaml::Value::as_sequence)
            .is_some_and(|patterns| patterns.len() == 1 && patterns[0].as_str() == Some("**"));
    let prebuild = group
        == "${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.head_ref || github.ref_name }}-prebuild";
    // ponytail: prove only these path/ref-scoped forms (including generated CI).
    // Other expressions need a provider-native concurrency evaluator. Even
    // cancel-in-progress:false replaces pending runs in a shared group.
    ensure!(
        (aggregate
            || prebuild
            || matches!(
                group,
                "${{ github.workflow_ref }}"
                    | "${{ github.workflow_ref }}-${{ github.ref }}"
                    | "${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}"
                    | "${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.ref_type == 'tag' && github.ref || format('refs/heads/{0}', github.event.pull_request.head.ref || github.ref_name) }}"
            ))
            && concurrency
                .get("cancel-in-progress")
                .is_none_or(|value| value.as_bool().is_some() || (prebuild && value.as_str() == Some("${{ !inputs.release && (github.event_name == 'push' || github.event_name == 'pull_request') }}"))),
        "workflow concurrency is not provably isolated by workflow path and qualification ref"
    );
    Ok(())
}

fn requested_workflows(
    paths: &[String],
    branch: Option<&str>,
    mut source: impl FnMut(&str) -> Result<Value>,
) -> Result<BTreeSet<String>> {
    let mut workflows = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for path in paths {
        if std::path::Path::new(path).parent() == Some(std::path::Path::new(".github/workflows"))
            && (path.ends_with(".yaml") || path.ends_with(".yml"))
            && push_workflow(path, &source(path)?, branch)?
        {
            ensure!(
                identities.insert(path.to_ascii_lowercase()),
                "workflow paths collide in case-insensitive concurrency groups"
            );
            workflows.insert(path.clone());
        }
    }
    Ok(workflows)
}

fn admit_workflows(request: &AdapterRequest) -> Result<BTreeSet<String>> {
    let plan = request.plan.as_ref().context("missing admission plan")?;
    let paths = requested_workflows(&request.workflows, request.branch.as_deref(), |path| {
        let file = plan
            .files
            .iter()
            .find(|file| file.relative_path == std::path::Path::new(path))
            .context("workflow is absent from candidate plan")?;
        Ok(
            serde_json::json!({"path": path, "type": "file", "encoding": "base64", "content": base64::engine::general_purpose::STANDARD.encode(&file.content)}),
        )
    })?;
    ensure!(
        !paths.is_empty(),
        "candidate has no owned push workflow for qualification"
    );
    let mut names = BTreeSet::new();
    for file in &plan.files {
        if !paths.contains(
            file.relative_path
                .to_str()
                .context("non-UTF-8 workflow path")?,
        ) {
            continue;
        }
        let yaml: serde_yaml::Value = serde_yaml::from_str(&file.content)?;
        let jobs = yaml["jobs"].as_mapping().context("missing workflow jobs")?;
        for (id, job) in jobs {
            // Matrix expansion and expression-derived names need a source-aware
            // adapter. A literal non-matrix name is the exact Actions context.
            if job.get("strategy").and_then(|s| s.get("matrix")).is_some() {
                continue;
            }
            if let Some(name) = job.get("name").unwrap_or(id).as_str()
                && !name.contains("${{")
            {
                names.insert(name.to_owned());
            }
        }
    }
    for check in &request.required_checks {
        ensure!(
            names.contains(&check.name),
            "required check {} is not provably produced by an owned push workflow; external or dynamic contexts require a supporting adapter",
            check.name
        );
    }
    Ok(paths)
}

fn admit_workflow_states(paths: &BTreeSet<String>, inventory: &Value) -> Result<()> {
    let entries = inventory["workflows"]
        .as_array()
        .context("missing workflow inventory")?;
    ensure!(
        inventory["total_count"].as_u64() == Some(entries.len() as u64),
        "incomplete workflow inventory"
    );
    let mut ids = BTreeSet::new();
    let mut registered = BTreeSet::new();
    for workflow in entries {
        let path = text(workflow, "/path")?;
        let state = text(workflow, "/state")?;
        ensure!(
            ids.insert(number(workflow, "/id")?) && registered.insert(path.clone()),
            "duplicate workflow inventory"
        );
        if paths.contains(&path) {
            ensure!(
                state == "active",
                "owned workflow {path} is not active ({state})"
            );
        }
    }
    // An absent path in a complete inventory can be introduced by this candidate.
    // Qualification still requires its genuine registered original push run.
    Ok(())
}

fn admit_signatures(signatures: &Value) -> Result<()> {
    ensure!(
        signatures["enabled"].as_bool() == Some(false),
        "signature-required branches need an adapter that proves forge-accepted candidate signatures"
    );
    Ok(())
}

fn generator_workflow_paths(
    runs: &[Value],
    branch: &str,
    mut required: impl FnMut(&Value) -> Result<bool>,
) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for run in runs.iter().filter(|run| run["head_branch"] == branch) {
        if required(run)? {
            paths.insert(text(run, "/path")?);
        }
    }
    Ok(paths)
}

fn admit_rules(
    endpoint: &str,
    branch: &str,
    mut get: impl FnMut(&str) -> Result<Value>,
) -> Result<()> {
    let rules = get(&format!("{endpoint}/rules/branches/{}", encoded(branch)))?;
    ensure!(
        rules.as_array().context("missing branch rules")?.is_empty(),
        "active rulesets require an adapter with ruleset-transition support"
    );
    Ok(())
}

fn protection_requirements(protection: &Value, authority: u64) -> Result<Vec<Requirement>> {
    ensure!(
        protection["restrictions"].is_null(),
        "push-restricted target requires an adapter that binds the Git transport actor"
    );
    ensure!(
        protection
            .pointer("/lock_branch/enabled")
            .is_none_or(|value| value.as_bool() == Some(false)),
        "target branch is locked; direct updater landing is unsupported"
    );
    let status = &protection["required_status_checks"];
    if status.is_null() {
        return Ok(vec![]);
    }
    let contexts = status["contexts"]
        .as_array()
        .context("missing required contexts")?;
    let checks = status["checks"]
        .as_array()
        .context("required context authorities are unavailable")?;
    let mut requirements = Vec::new();
    for check in checks {
        ensure!(
            check["app_id"].as_u64() == Some(authority),
            "external or unbound required checks need an adapter with external-check qualification support"
        );
        let name = text(check, "/context")?;
        ensure!(
            contexts.iter().any(|context| context == &name),
            "required check inventory differs from contexts"
        );
        requirements.push(Requirement {
            name,
            authority: authority.to_string(),
        });
    }
    ensure!(
        contexts
            .iter()
            .all(|name| requirements.iter().any(|r| name == &r.name)),
        "required context lacks application binding"
    );
    Ok(requirements)
}

fn admit_classic_rules(
    branch: &str,
    mut page: impl FnMut(Option<&str>) -> Result<Value>,
) -> Result<()> {
    let mut cursor = None;
    let mut total = None;
    let mut seen = BTreeSet::new();
    for _ in 0..20 {
        let response = page(cursor.as_deref())?;
        ensure!(
            response["errors"].is_null(),
            "classic protection inventory has GraphQL errors"
        );
        let rules = response
            .pointer("/data/repository/branchProtectionRules")
            .context("missing classic protection inventory")?;
        let count = number(rules, "/totalCount")?;
        ensure!(
            total.is_none_or(|previous| previous == count),
            "classic protection inventory changed during pagination"
        );
        total = Some(count);
        let nodes = rules["nodes"]
            .as_array()
            .context("missing classic protection rules")?;
        ensure!(nodes.len() <= 100, "oversized classic protection page");
        for rule in nodes {
            ensure!(
                seen.insert(text(rule, "/id")?),
                "duplicate classic protection rule"
            );
            let pattern = text(rule, "/pattern")?;
            let pushes = rule["restrictsPushes"]
                .as_bool()
                .context("missing classic push restriction")?;
            let creations = rule["blocksCreations"]
                .as_bool()
                .context("missing classic creation restriction")?;
            let signatures = rule["requiresCommitSignatures"]
                .as_bool()
                .context("missing classic signature protection")?;
            let reviews = rule["requiresApprovingReviews"]
                .as_bool()
                .context("missing classic pull-request protection")?;
            if pushes || creations || signatures || reviews {
                // ponytail: reject any matching restricted rule, irrespective
                // of bypasses/priority, until an adapter binds the Git actor.
                ensure!(
                    !branch_pattern(&pattern, branch)?,
                    "restricted classic protection covers qualification branch {branch}"
                );
            }
        }
        let more = rules
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .context("missing classic protection pagination")?;
        if !more {
            ensure!(
                count == seen.len() as u64,
                "incomplete classic protection inventory"
            );
            return Ok(());
        }
        let next = text(rules, "/pageInfo/endCursor")?;
        ensure!(
            !nodes.is_empty() && !next.is_empty() && cursor.as_ref() != Some(&next),
            "classic protection pagination did not advance"
        );
        cursor = Some(next);
    }
    anyhow::bail!("classic protection exceeds adapter pagination ceiling")
}

pub fn execute(request: &AdapterRequest) -> Result<AdapterResponse> {
    request.validate()?;
    let api = Api::new(request)?;
    execute_with(
        request,
        |path| api.get(path),
        |repository, branch| api.admit_classic(repository, branch),
    )
}

fn execute_with(
    request: &AdapterRequest,
    get: impl Fn(&str) -> Result<Value>,
    classic: impl Fn(&str, &str) -> Result<()>,
) -> Result<AdapterResponse> {
    request.validate()?;
    let authority: u64 = request
        .authority
        .parse()
        .context("GitHub check authority must be a numeric application ID")?;
    ensure!(
        request
            .required_checks
            .iter()
            .all(|r| r.authority == authority.to_string()),
        "external required checks need a supporting adapter"
    );
    let (_, repository) = request
        .coordinate
        .split_once('/')
        .context("missing repository")?;
    ensure!(
        repository.split('/').count() == 2,
        "GitHub adapter requires owner/repository identity"
    );
    let endpoint = format!("repos/{repository}");
    let repo = get(&endpoint)?;
    ensure!(
        repo["full_name"] == repository && repo["archived"] == false && repo["disabled"] == false,
        "repository renamed, archived or disabled"
    );
    let id = number(&repo, "/id")?;
    let default_branch = text(&repo, "/default_branch")?;
    let branch = request.branch.as_deref().unwrap_or(&default_branch);
    if matches!(request.operation.as_str(), "inspect" | "admit") {
        // Rules can govern a maintenance ref before it exists. Query that exact
        // name without requiring a branch object or substituting the default.
        admit_rules(&endpoint, branch, |path| list(path, "", &get))?;
        if request.operation == "admit" {
            classic(repository, branch)?;
        }
    }
    let info = if request.operation == "admit" {
        Value::Null
    } else {
        get(&format!("{endpoint}/branches/{}", encoded(branch)))?
    };
    let revision = if matches!(request.operation.as_str(), "qualify" | "admit") {
        request
            .revision
            .clone()
            .context("qualification needs exact revision")?
    } else {
        text(&info, "/commit/sha")?
    };
    let mut requirements = Vec::new();
    let policy = if request.operation == "inspect" {
        let protected = info["protected"]
            .as_bool()
            .context("missing branch protection state")?;
        let protection = if protected {
            get(&format!(
                "{endpoint}/branches/{}/protection",
                encoded(branch)
            ))?
        } else {
            Value::Null
        };
        ensure!(
            protection["required_pull_request_reviews"].is_null(),
            "repository requires pull requests; direct updater landing is unsupported"
        );
        requirements = protection_requirements(&protection, authority)?;
        let signatures = if protected {
            get(&format!(
                "{endpoint}/branches/{}/protection/required_signatures",
                encoded(branch)
            ))?
        } else {
            serde_json::json!({"enabled":false})
        };
        admit_signatures(&signatures)?;
        serde_json::json!({"protection":protection,"signatures":signatures})
    } else {
        Value::Null
    };
    let mut response = AdapterResponse {
        schema_version: request.schema_version,
        coordinate: request.coordinate.clone(),
        repository_id: id.to_string(),
        default_branch: default_branch.clone(),
        revision: revision.clone(),
        remote: text(&repo, "/clone_url")?,
        policy_digest: crate::review::canonical_digest(&policy)?,
        requirements,
        status: "success".into(),
        checks: vec![],
    };
    if request.operation == "inspect" {
        return Ok(response);
    }
    if request.operation == "admit" {
        let paths = admit_workflows(request)?;
        admit_workflow_states(
            &paths,
            &list(&format!("{endpoint}/actions/workflows"), "workflows", &get)?,
        )?;
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
    // Only original push runs can qualify the exact candidate/default-branch bytes.
    let query = format!(
        "{endpoint}/actions/runs?head_sha={revision}&event=push&branch={}",
        encoded(branch)
    );
    let runs = list(&query, "workflow_runs", &get)?;
    let runs = runs["workflow_runs"]
        .as_array()
        .context("missing workflow runs")?;
    let mut generator_jobs = std::collections::BTreeMap::new();
    let mut sources: BTreeMap<String, Value> = BTreeMap::new();
    let mut source = |path: &str| -> Result<Value> {
        if let Some(file) = sources.get(path) {
            return Ok(file.clone());
        }
        let encoded_path = path.split('/').map(encoded).collect::<Vec<_>>().join("/");
        let file = get(&format!(
            "{endpoint}/contents/{encoded_path}?ref={revision}"
        ))?;
        sources.insert(path.to_owned(), file.clone());
        Ok(file)
    };
    let paths = if request.workflows.is_empty() {
        generator_workflow_paths(runs, branch, |run| {
            let run_id = number(run, "/id")?;
            let jobs = list(
                &format!("{endpoint}/actions/runs/{run_id}/attempts/1/jobs"),
                "jobs",
                &get,
            )?;
            let job_entries = jobs["jobs"].as_array().context("missing job inventory")?;
            let suite = get(&format!(
                "{endpoint}/check-suites/{}",
                number(run, "/check_suite_id")?
            ))?;
            let required = request.required_checks.iter().any(|requirement| {
                suite["app"]["id"]
                    .as_u64()
                    .is_some_and(|id| id.to_string() == requirement.authority)
                    && job_entries
                        .iter()
                        .any(|job| job["name"] == requirement.name)
            });
            generator_jobs.insert(run_id, (jobs, suite));
            Ok(required)
        })?
    } else {
        requested_workflows(&request.workflows, Some(branch), &mut source)?
    };
    if paths.is_empty() {
        response.status = "pending".into();
        return Ok(response);
    }
    admit_workflow_states(
        &paths,
        &list(&format!("{endpoint}/actions/workflows"), "workflows", &get)?,
    )?;
    for path in paths {
        ensure!(
            path.starts_with(".github/workflows/")
                && (path.ends_with(".yaml") || path.ends_with(".yml")),
            "GitHub adapter cannot qualify this workflow format"
        );
        let file = source(&path)?;
        if request.workflows.is_empty() {
            ensure!(
                push_workflow(&path, &file, Some(branch))?,
                "generator qualification requires a push workflow"
            );
        }
        let expected = push_steps(
            &workflow_source(&path, &file)?,
            branch.eq_ignore_ascii_case(&default_branch),
        )?;
        ensure!(
            expected.outcomes.is_empty()
                || (branch.is_ascii() && default_branch.is_ascii())
                || branch == default_branch,
            "conditional push qualification needs a provable branch-name comparison"
        );
        let matching = runs
            .iter()
            .filter(|r| r["path"] == path && r["head_branch"] == branch)
            .collect::<Vec<_>>();
        ensure!(
            matching.len() <= 1,
            "ambiguous original runs for candidate workflow"
        );
        let Some(run) = matching.first() else {
            response.status = "pending".into();
            continue;
        };
        let run_id = number(run, "/id")?;
        let (jobs, suite) = if let Some(evidence) = generator_jobs.remove(&run_id) {
            evidence
        } else {
            (
                list(
                    &format!("{endpoint}/actions/runs/{run_id}/attempts/1/jobs"),
                    "jobs",
                    &get,
                )?,
                get(&format!(
                    "{endpoint}/check-suites/{}",
                    number(run, "/check_suite_id")?
                ))?,
            )
        };
        if !admit_run(
            (id, authority),
            (&revision, Some(branch), &path),
            run,
            &jobs,
            &suite,
            &expected,
        )? {
            response.status = "pending".into();
            continue;
        }
        record_checks(
            &mut response,
            jobs["jobs"].as_array().context("missing job inventory")?,
            authority,
            run_id,
        )?;
    }
    // Absence of an enrolled required context is pending, never implicit success.
    if request.required_checks.iter().any(|r| {
        !response
            .checks
            .iter()
            .any(|c| c.name == r.name && c.authority == r.authority)
    }) {
        response.status = "pending".into();
    }
    response.validate(request)?;
    Ok(response)
}

fn record_checks(
    response: &mut AdapterResponse,
    jobs: &[Value],
    authority: u64,
    run_id: u64,
) -> Result<()> {
    for job in jobs {
        response.checks.push(Check {
            name: text(job, "/name")?,
            authority: authority.to_string(),
            revision: response.revision.clone(),
            run: run_id.to_string(),
            attempt: 1,
            status: "success".into(),
        });
    }
    let mut seen = BTreeSet::new();
    response
        .checks
        .retain(|check| seen.insert((check.name.clone(), check.authority.clone())));
    Ok(())
}

fn admit_run(
    (repository_id, authority): (u64, u64),
    (revision, branch, workflow): (&str, Option<&str>, &str),
    run: &Value,
    completed_jobs: &Value,
    suite: &Value,
    expected: &PushSteps,
) -> Result<bool> {
    ensure!(
        run["repository"]["id"] == repository_id
            && run["head_repository"]["id"] == repository_id
            && run["head_sha"] == revision
            && run["path"] == workflow
            && run["event"] == "push"
            && run["run_attempt"] == 1,
        "untrusted workflow source/identity/attempt/event"
    );
    if let Some(branch) = branch {
        ensure!(
            run["head_branch"] == branch,
            "workflow branch differs from candidate"
        );
    }
    ensure!(
        suite["id"] == run["check_suite_id"]
            && suite["app"]["id"] == authority
            && suite["head_sha"] == revision,
        "workflow application or suite mismatch"
    );
    if run["status"] != "completed" {
        return Ok(false);
    }
    ensure!(run["conclusion"] == "success", "workflow did not succeed");
    let jobs = completed_jobs["jobs"].as_array().context("missing jobs")?;
    ensure!(
        !jobs.is_empty() && completed_jobs["total_count"].as_u64() == Some(jobs.len() as u64),
        "incomplete job inventory"
    );
    ensure!(
        expected
            .jobs
            .as_ref()
            .is_none_or(|names| names.len() == jobs.len()),
        "incomplete source matrix job inventory"
    );
    let mut job_names = expected.jobs.clone();
    let mut seen = BTreeSet::new();
    for job in jobs {
        ensure!(
            job_names
                .as_mut()
                .is_none_or(|names| job["name"].as_str().is_some_and(|name| names.remove(name))),
            "missing, duplicate or mismatched source job name"
        );
        ensure!(
            job["run_id"] == run["id"]
                && job["run_attempt"] == 1
                && job["head_sha"] == revision
                && job["status"] == "completed"
                && job["conclusion"] == "success"
                && seen.insert(number(job, "/id")?),
            "job is missing, duplicated, rerun or non-success"
        );
        let steps = job["steps"].as_array().context("missing step inventory")?;
        ensure!(!steps.is_empty(), "job has an empty step inventory");
        let mut outcomes = expected.outcomes.clone();
        let mut step_numbers = BTreeSet::new();
        for step in steps {
            ensure!(
                expected.outcomes.is_empty()
                    || step["number"]
                        .as_u64()
                        .is_some_and(|number| step_numbers.insert(number)),
                "missing or duplicate source-bound step number"
            );
            let conclusion = if let Some((name, runs)) = step["number"]
                .as_u64()
                .and_then(|number| outcomes.remove(&number))
            {
                ensure!(
                    step["name"] == name,
                    "conditional step source identity mismatch"
                );
                if runs { "success" } else { "skipped" }
            } else {
                "success"
            };
            ensure!(
                step["status"] == "completed" && step["conclusion"] == conclusion,
                "job has an unproved step outcome"
            );
        }
        ensure!(
            outcomes.is_empty(),
            "job is missing source-bound conditional steps"
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn production_adapter_replays_inspection_admission_and_original_qualification() {
        use std::{cell::RefCell, collections::BTreeMap};
        let revision = "a".repeat(40);
        let path = ".github/workflows/ci.yaml";
        let workflow = "on: push\nconcurrency:\n  group: '${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}'\n  cancel-in-progress: true\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n";
        let mut request: AdapterRequest = serde_json::from_value(json!({
            "schemaVersion":1,"operation":"inspect","coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api","authority":"777","revision":null,"branch":null,
            "workflows":[],"requiredChecks":[]
        }))
        .unwrap();
        let mut responses = BTreeMap::from([
            (
                "repos/team/demo".into(),
                json!({"id":2,"full_name":"team/demo","archived":false,"disabled":false,"default_branch":"trunk","clone_url":"https://forge.example/team/demo.git"}),
            ),
            (
                "repos/team/demo/branches/trunk/protection".into(),
                json!({"required_status_checks":{"contexts":["test"],"checks":[{"context":"test","app_id":777}]}}),
            ),
            (
                "repos/team/demo/branches/trunk/protection/required_signatures".into(),
                json!({"enabled":false}),
            ),
            (
                "repos/team/demo/actions/workflows?per_page=100&page=1".into(),
                json!({"total_count":1,"workflows":[{"id":10,"path":path,"state":"active"}]}),
            ),
            (
                format!("repos/team/demo/contents/.github/workflows/ci.yaml?ref={revision}"),
                json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(workflow)}),
            ),
        ]);
        for (branch, run_id) in [("trunk", 42), ("maintenance/candidate", 43)] {
            let branch_key = encoded(branch);
            responses.insert(
                format!("repos/team/demo/branches/{branch_key}"),
                json!({"protected":branch == "trunk","commit":{"sha":revision}}),
            );
            responses.insert(
                format!("repos/team/demo/rules/branches/{branch_key}?per_page=100&page=1"),
                json!([]),
            );
            responses.insert(format!("repos/team/demo/actions/runs?head_sha={revision}&event=push&branch={branch_key}&per_page=100&page=1"), json!({"total_count":1,"workflow_runs":[{
                "id":run_id,"repository":{"id":2},"head_repository":{"id":2},"event":"push","head_sha":revision,
                "head_branch":branch,"path":path,"run_attempt":1,"check_suite_id":run_id+100,"status":"completed","conclusion":"success"
            }]}));
            responses.insert(format!("repos/team/demo/actions/runs/{run_id}/attempts/1/jobs?per_page=100&page=1"), json!({"total_count":1,"jobs":[{
                "id":run_id+200,"run_id":run_id,"run_attempt":1,"head_sha":revision,"name":"test","status":"completed","conclusion":"success",
                "steps":[{"status":"completed","conclusion":"success"}]
            }]}));
            responses.insert(
                format!("repos/team/demo/check-suites/{}", run_id + 100),
                json!({"id":run_id+100,"app":{"id":777},"head_sha":revision}),
            );
        }
        let responses = RefCell::new(responses);
        let calls = RefCell::new(Vec::new());
        let get = |path: &str| -> Result<Value> {
            calls.borrow_mut().push(path.to_owned());
            responses
                .borrow()
                .get(path)
                .cloned()
                .with_context(|| format!("unexpected replay endpoint {path}"))
        };
        let classic_calls = RefCell::new(Vec::new());
        let classic = |repository: &str, branch: &str| {
            assert_eq!(repository, "team/demo");
            classic_calls.borrow_mut().push(branch.to_owned());
            admit_classic_rules(branch, |_| {
                Ok(json!({"data":{"repository":{"branchProtectionRules":{
                    "totalCount":0,"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}
                }}}}))
            })
        };
        let inspected = execute_with(&request, get, classic).unwrap();
        inspected.validate(&request).unwrap();
        assert_eq!(inspected.requirements.len(), 1);
        assert!(
            calls
                .borrow()
                .iter()
                .any(|path| path.ends_with("/protection/required_signatures"))
        );
        responses
            .borrow_mut()
            .get_mut("repos/team/demo/branches/trunk/protection/required_signatures")
            .unwrap()["enabled"] = json!(true);
        assert!(execute_with(&request, get, classic).is_err());
        responses
            .borrow_mut()
            .get_mut("repos/team/demo/branches/trunk/protection/required_signatures")
            .unwrap()["enabled"] = json!(false);
        request.operation = "admit".into();
        request.revision = Some(revision.clone());
        request.workflows = vec![path.into()];
        request.required_checks = inspected.requirements;
        request.plan = Some(crate::project::CiUpdatePlan::new(
            vec![crate::project::GeneratedFile {
                relative_path: path.into(),
                content: workflow.into(),
            }],
            vec![],
            vec![],
        ));
        for branch in ["maintenance/candidate", "trunk"] {
            request.branch = Some(branch.into());
            let admitted = execute_with(&request, get, classic).unwrap();
            admitted.validate(&request).unwrap();
            assert_eq!(
                admitted.checks[0].run,
                crate::review::canonical_digest(&request).unwrap()
            );
        }
        assert_eq!(
            &*classic_calls.borrow(),
            &["maintenance/candidate", "trunk"]
        );
        let inventory_key = "repos/team/demo/actions/workflows?per_page=100&page=1";
        responses.borrow_mut().get_mut(inventory_key).unwrap()["workflows"][0]["state"] =
            json!("disabled_manually");
        assert!(execute_with(&request, get, classic).is_err());
        responses.borrow_mut().get_mut(inventory_key).unwrap()["workflows"][0]["state"] =
            json!("active");
        request.operation = "qualify".into();
        request.plan = None;
        for (branch, run_id) in [("maintenance/candidate", 43), ("trunk", 42)] {
            request.branch = Some(branch.into());
            let qualified = execute_with(&request, get, classic).unwrap();
            qualified.validate(&request).unwrap();
            assert_eq!(qualified.status, "success");
            assert_eq!(qualified.checks[0].revision, revision);
            assert_eq!(qualified.checks[0].run, run_id.to_string());
            let key = format!(
                "repos/team/demo/actions/runs?head_sha={revision}&event=push&branch={}&per_page=100&page=1",
                encoded(branch)
            );
            responses.borrow_mut().get_mut(&key).unwrap()["workflow_runs"][0]["conclusion"] =
                json!("cancelled");
            assert!(execute_with(&request, get, classic).is_err());
            responses.borrow_mut().get_mut(&key).unwrap()["workflow_runs"][0]["conclusion"] =
                json!("success");
        }
        // The generator takes the same production path with an inferred workflow
        // and the current default branch bound into the original-run query.
        request.branch = None;
        request.workflows.clear();
        assert_eq!(
            execute_with(&request, get, classic).unwrap().status,
            "success"
        );
        responses.borrow_mut().remove(inventory_key);
        assert!(execute_with(&request, get, classic).is_err());
    }

    #[test]
    fn signature_admission_requires_an_explicit_disabled_observation() {
        admit_signatures(&json!({"enabled":false})).unwrap();
        for invalid in [
            Value::Null,
            json!({}),
            json!({"enabled":true}),
            json!({"enabled":"false"}),
        ] {
            assert!(admit_signatures(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn workflow_state_admission_rejects_disabled_partial_and_duplicate_inventories() {
        let paths = BTreeSet::from([
            ".github/workflows/ci.yaml".into(),
            ".github/workflows/new.yaml".into(),
        ]);
        let active = json!({"id":1,"path":".github/workflows/ci.yaml","state":"active"});
        admit_workflow_states(&paths, &json!({"total_count":1,"workflows":[active]})).unwrap();
        // New paths are admitted only from a complete workflow registry.
        admit_workflow_states(&paths, &json!({"total_count":0,"workflows":[]})).unwrap();
        for state in [
            "disabled_manually",
            "disabled_inactivity",
            "deleted",
            "unknown",
        ] {
            let mut workflow = active.clone();
            workflow["state"] = json!(state);
            assert!(
                admit_workflow_states(&paths, &json!({"total_count":1,"workflows":[workflow]}))
                    .is_err()
            );
        }
        for invalid in [
            Value::Null,
            json!({"workflows":[active]}),
            json!({"total_count":2,"workflows":[active]}),
            json!({"total_count":2,"workflows":[active,active]}),
            json!({"total_count":1,"workflows":[{"id":1,"path":".github/workflows/ci.yaml"}]}),
        ] {
            assert!(admit_workflow_states(&paths, &invalid).is_err());
        }
    }

    fn generated_prebuild() -> crate::project::GeneratedFile {
        use crate::{
            config::{ArtifactsConfig, AtticConfig, PrebuildConfig},
            render::ci,
        };
        let attic: AtticConfig = serde_json::from_value(json!({
            "cache":"example", "url":"https://cache.example", "token_name":"example",
            "token_secret":"ATTIC_TOKEN"
        }))
        .unwrap();
        ci::github_prebuild_file(
            &std::collections::BTreeMap::from([
                ("x86_64-linux".into(), "ubuntu-24.04".into()),
                ("aarch64-darwin".into(), "macos-15".into()),
            ]),
            &[".#default".into()],
            &PrebuildConfig {
                release_archives: true,
                publish_attic: true,
                attic_app: Some(".#push-cache".into()),
                ..Default::default()
            },
            &ArtifactsConfig {
                prebuild_binaries: true,
                ..Default::default()
            },
            Some(&attic),
        )
        .unwrap()
    }

    #[test]
    fn generated_prebuild_push_is_admitted_with_release_and_publication_conditions() {
        let file = generated_prebuild();
        let path = file.relative_path.to_str().unwrap();
        let source = json!({"path":path,"type":"file","encoding":"base64",
            "content":base64::engine::general_purpose::STANDARD.encode(&file.content)});
        for branch in ["maintenance/candidate", "trunk"] {
            let result = push_workflow(path, &source, Some(branch));
            assert!(result.is_ok(), "{branch}: {result:?}");
        }
        for (from, to) in [
            ("github.workflow_ref", "github.workflow"),
            ("-prebuild", ""),
            ("!inputs.release &&", "inputs.release &&"),
            ("if: ${{ inputs.release }}", "if: ${{ inputs.unproved }}"),
            ("if: ${{ inputs.release }}", "if: true"),
            ("name: Validate tag", "name: '${{ inputs.name }}'"),
            ("runner: macos-15", "runner: '${{ inputs.runner }}'"),
            (
                "runs-on: ${{ matrix.runner }}",
                "name: '${{ matrix.system }}'\n    runs-on: ${{ matrix.runner }}",
            ),
            ("include:", "other: [x86_64-linux]\n        include:"),
            (
                "  build:\n",
                "  extra:\n    steps:\n      - name: Extra\n        run: true\n  build:\n",
            ),
        ] {
            let changed = json!({"path":path,"type":"file","encoding":"base64",
                "content":base64::engine::general_purpose::STANDARD.encode(file.content.replace(from, to))});
            assert!(
                push_workflow(path, &changed, Some("maintenance/candidate")).is_err(),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn production_prebuild_qualification_binds_every_source_conditional_outcome() {
        let file = generated_prebuild();
        let path = file.relative_path.to_str().unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&file.content).unwrap();
        let source_steps = yaml["jobs"]["build"]["steps"].as_sequence().unwrap();
        let revision = "a".repeat(40);
        for branch in ["maintenance/candidate", "trunk", "TRUNK"] {
            let mut request: AdapterRequest = serde_json::from_value(json!({
                "schemaVersion":1,"operation":"admit","coordinate":"forge.example/team/demo",
                "apiUrl":"https://forge.example/api","authority":"777","revision":revision,
                "branch":branch,"workflows":[path],"requiredChecks":[],
                "plan":{"schema":"simit.ci-update-plan.v1","files":[{"relative_path":path,"content":file.content}],"obsolete":[]}
            })).unwrap();
            let mut steps = vec![
                json!({"number":1,"name":"Set up job","status":"completed","conclusion":"success"}),
            ];
            for (index, step) in source_steps.iter().enumerate() {
                let name = step["name"].as_str().unwrap();
                let runs = match name {
                    "Validate tag"
                    | "Build unsigned release archives"
                    | "Upload unsigned release archives" => false,
                    "Publish flake inputs to Attic" | "Upload flake input manifest" => {
                        branch != "maintenance/candidate"
                    }
                    _ => true,
                };
                steps.push(json!({"number":index+2,"name":name,"status":"completed","conclusion":if runs {"success"} else {"skipped"}}));
            }
            steps.push(json!({"number":100,"name":"Post Checkout","status":"completed","conclusion":"success"}));
            let jobs = json!({"total_count":2,"jobs":[
                {"id":8,"run_id":42,"run_attempt":1,"head_sha":revision,"name":"build (aarch64-darwin, macos-15)","status":"completed","conclusion":"success","steps":steps},
                {"id":9,"run_id":42,"run_attempt":1,"head_sha":revision,"name":"build (x86_64-linux, ubuntu-24.04)","status":"completed","conclusion":"success","steps":steps}
            ]});
            let replay = |request: &AdapterRequest, jobs: &Value| {
                execute_with(
                    request,
                    |endpoint| {
                        Ok(match endpoint {
                            "repos/team/demo" => {
                                json!({"id":2,"full_name":"team/demo","archived":false,"disabled":false,"default_branch":"trunk","clone_url":"https://forge.example/team/demo.git"})
                            }
                            "repos/team/demo/actions/workflows?per_page=100&page=1" => {
                                json!({"total_count":1,"workflows":[{"id":10,"path":path,"state":"active"}]})
                            }
                            "repos/team/demo/actions/runs/42/attempts/1/jobs?per_page=100&page=1" => {
                                jobs.clone()
                            }
                            "repos/team/demo/check-suites/5" => {
                                json!({"id":5,"app":{"id":777},"head_sha":revision})
                            }
                            _ if endpoint
                                == format!(
                                    "repos/team/demo/rules/branches/{}?per_page=100&page=1",
                                    encoded(branch)
                                ) =>
                            {
                                json!([])
                            }
                            _ if endpoint
                                == format!("repos/team/demo/branches/{}", encoded(branch)) =>
                            {
                                json!({"commit":{"sha":revision}})
                            }
                            _ if endpoint
                                == format!("repos/team/demo/contents/{path}?ref={revision}") =>
                            {
                                json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(&file.content)})
                            }
                            _ if endpoint
                                == format!(
                                    "repos/team/demo/actions/runs?head_sha={revision}&event=push&branch={}&per_page=100&page=1",
                                    encoded(branch)
                                ) =>
                            {
                                json!({"total_count":1,"workflow_runs":[{"id":42,"repository":{"id":2},"head_repository":{"id":2},"head_sha":revision,"path":path,"event":"push","run_attempt":1,"head_branch":branch,"check_suite_id":5,"status":"completed","conclusion":"success"}]})
                            }
                            _ => anyhow::bail!("unexpected endpoint {endpoint}"),
                        })
                    },
                    |_, _| Ok(()),
                )
            };
            assert_eq!(replay(&request, &jobs).unwrap().status, "success");
            request.operation = "qualify".into();
            request.plan = None;
            assert_eq!(replay(&request, &jobs).unwrap().status, "success");
            let release = steps
                .iter()
                .position(|step| step["name"] == "Validate tag")
                .unwrap();
            let publish = steps
                .iter()
                .position(|step| step["name"] == "Publish flake inputs to Attic")
                .unwrap();
            let build = steps
                .iter()
                .position(|step| step["name"] == "Build native Nix outputs")
                .unwrap();
            for (index, key, value) in [
                (release, "conclusion", json!("success")),
                (release, "name", json!("Unrelated skipped step")),
                (release, "number", json!(99)),
                (release, "status", json!("in_progress")),
                (build, "conclusion", json!("skipped")),
                (
                    publish,
                    "conclusion",
                    json!(if branch == "maintenance/candidate" {
                        "success"
                    } else {
                        "skipped"
                    }),
                ),
                (publish, "conclusion", json!("failure")),
            ] {
                let mut changed = jobs.clone();
                changed["jobs"][0]["steps"][index][key] = value;
                assert!(
                    replay(&request, &changed).is_err(),
                    "{branch}: {index}/{key}"
                );
            }
            let mut missing = jobs.clone();
            missing["jobs"][0]["steps"]
                .as_array_mut()
                .unwrap()
                .remove(release);
            assert!(replay(&request, &missing).is_err());
            let mut missing = jobs.clone();
            missing["jobs"][0]["steps"]
                .as_array_mut()
                .unwrap()
                .remove(build);
            assert!(replay(&request, &missing).is_err());
            let mut missing = jobs.clone();
            missing["total_count"] = json!(1);
            missing["jobs"].as_array_mut().unwrap().pop();
            assert!(replay(&request, &missing).is_err());
            let mut duplicate = jobs.clone();
            duplicate["jobs"][0]["steps"]
                .as_array_mut()
                .unwrap()
                .push(steps[release].clone());
            assert!(replay(&request, &duplicate).is_err());
            let mut duplicate = jobs.clone();
            duplicate["jobs"][1]["name"] = duplicate["jobs"][0]["name"].clone();
            assert!(replay(&request, &duplicate).is_err());
        }
    }

    #[test]
    fn generated_rust_primary_concurrency_is_admitted_without_accepting_nearby_unsafe_groups() {
        use crate::{
            cli::{CiProvider, Platform, Runtime, WorkspaceStrategy},
            render::ci::{self, CiOptions, FilesRequest, SelfCheckOptions},
            user_config::{ResolvedCiRunners, ResolvedRunner},
        };
        let package = serde_json::from_value(json!({
            "id":"demo", "name":"demo", "version":"0.1.0", "manifest_path":"/demo/Cargo.toml"
        }))
        .unwrap();
        let runner = ResolvedRunner {
            name: None,
            labels: vec!["ubuntu-24.04".into()],
        };
        let runners = ResolvedCiRunners {
            ci: runner.clone(),
            release: runner,
            windows: None,
        };
        for runtime in [Runtime::Cargo, Runtime::Nix] {
            for (workspace_strategy, separate_events) in [
                (WorkspaceStrategy::Members, false),
                (WorkspaceStrategy::Aggregate, false),
                (WorkspaceStrategy::Aggregate, true),
            ] {
                let files = ci::primary_files(FilesRequest {
                    provider: CiProvider::Actions,
                    platform: Platform::Github,
                    crow: &Default::default(),
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
                        workspace_strategy,
                        separate_events,
                        ..Default::default()
                    },
                })
                .unwrap();
                let generated = &files[0];
                let path = generated.relative_path.to_str().unwrap();
                let source = |content: &str| {
                    json!({"path":path,"type":"file","encoding":"base64",
                    "content":base64::engine::general_purpose::STANDARD.encode(content)})
                };
                for branch in ["simit/update/0123456789abcdef", "trunk"] {
                    assert!(
                        push_workflow(path, &source(&generated.content), Some(branch)).unwrap(),
                        "{runtime:?} {workspace_strategy:?} separate_events={separate_events}"
                    );
                }
                if workspace_strategy == WorkspaceStrategy::Aggregate && !separate_events {
                    for (from, to) in [
                        (
                            "group: .github/workflows/ci.yaml-",
                            "group: .github/workflows/other.yaml-",
                        ),
                        (
                            "github.event.pull_request.head.repo.full_name || github.repository",
                            "github.repository",
                        ),
                        ("tags-ignore: [\"**\"]", "tags: [\"**\"]"),
                        (
                            "cancel-in-progress: true",
                            "cancel-in-progress: '${{ inputs.cancel }}'",
                        ),
                    ] {
                        let changed = generated.content.replace(from, to);
                        assert_ne!(changed, generated.content, "missing mutation {from}");
                        assert!(
                            push_workflow(path, &source(&changed), Some("trunk")).is_err(),
                            "{from}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn workflow_concurrency_admission_requires_path_and_ref_isolation() {
        let path = ".github/workflows/ci.yaml";
        for (concurrency, accepted) in [
            ("", true),
            ("concurrency: shared\n", false),
            (
                "concurrency:\n  group: shared\n  cancel-in-progress: true\n",
                false,
            ),
            (
                "concurrency:\n  group: shared\n  cancel-in-progress: false\n",
                false,
            ),
            (
                "concurrency:\n  group: '${{ github.workflow }}-${{ github.ref }}'\n  cancel-in-progress: true\n",
                false,
            ),
            (
                "concurrency:\n  group: '${{ github.workflow_ref }}'\n  cancel-in-progress: true\n",
                true,
            ),
            (
                "concurrency:\n  group: '${{ github.workflow_ref }}-${{ github.ref }}'\n  cancel-in-progress: false\n",
                true,
            ),
            (
                "concurrency:\n  group: '${{ github.workflow_ref }}-${{ github.event_name }}-${{ github.head_ref || github.ref_name }}'\n  cancel-in-progress: true\n",
                true,
            ),
            (
                "concurrency:\n  group: '${{ github.workflow_ref }}'\n  cancel-in-progress: '${{ inputs.cancel }}'\n",
                false,
            ),
        ] {
            let file = json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("name: CI\non: push\n{concurrency}jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n"))});
            assert_eq!(
                push_workflow(path, &file, Some("maintenance/candidate")).is_ok(),
                accepted,
                "{concurrency}"
            );
        }
        for cancel in ["true", "false"] {
            let file = json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on: push\njobs:\n  test:\n    concurrency:\n      group: shared\n      cancel-in-progress: {cancel}\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n"))});
            assert!(push_workflow(path, &file, Some("trunk")).is_err());
        }
        let paths = [".github/workflows/CI.yaml".into(), path.into()];
        assert!(requested_workflows(&paths, Some("trunk"), |path| Ok(json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode("on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n")}))).is_err());
    }

    #[test]
    fn inspection_rejects_external_unbound_and_incomplete_required_checks() {
        for app in [json!(42), json!(-1), Value::Null] {
            let policy = json!({"required_status_checks":{"contexts":["test"],"checks":[{"context":"test","app_id":app}]}});
            assert!(protection_requirements(&policy, 777).is_err());
        }
        let mut policy = json!({"required_status_checks":{"contexts":["test"],"checks":[{"context":"test","app_id":777}]}});
        let admitted = protection_requirements(&policy, 777).unwrap();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].name, "test");
        assert_eq!(admitted[0].authority, "777");
        policy["required_status_checks"]["contexts"] = json!(["missing"]);
        assert!(protection_requirements(&policy, 777).is_err());
        assert!(
            protection_requirements(&Value::Null, 777)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn inspection_rejects_push_restrictions_before_status_check_admission() {
        for status in [
            Value::Null,
            json!({"contexts":["test"],"checks":[{"context":"test","app_id":777}]}),
        ] {
            for restrictions in [
                json!({"users":[],"teams":[],"apps":[]}),
                json!({"users":[{"login":"operator"}]}),
                json!({"teams":[{"slug":"maintainers"}]}),
                json!({"apps":[{"id":777}]}),
            ] {
                let mut protection =
                    json!({"restrictions":restrictions,"required_status_checks":status});
                assert!(
                    protection_requirements(&protection, 777)
                        .unwrap_err()
                        .to_string()
                        .contains("push-restricted target")
                );
                protection["restrictions"] = Value::Null;
                protection_requirements(&protection, 777).unwrap();
            }
        }
    }

    #[test]
    fn inspection_rejects_locked_targets_with_or_without_required_checks() {
        for status in [
            Value::Null,
            json!({"contexts":["test"],"checks":[{"context":"test","app_id":777}]}),
        ] {
            let mut protection =
                json!({"lock_branch":{"enabled":true},"required_status_checks":status});
            assert!(
                protection_requirements(&protection, 777)
                    .unwrap_err()
                    .to_string()
                    .contains("target branch is locked")
            );
            protection["lock_branch"]["enabled"] = json!(false);
            protection_requirements(&protection, 777).unwrap();
            protection["lock_branch"]["enabled"] = json!("false");
            assert!(protection_requirements(&protection, 777).is_err());
        }
    }

    #[test]
    fn classic_protection_admission_covers_uncreated_refs_and_all_pages() {
        let page = |nodes: Value, more: bool, cursor: Value, total: u64| {
            json!({
                "data":{"repository":{"branchProtectionRules":{
                    "totalCount":total,"nodes":nodes,
                    "pageInfo":{"hasNextPage":more,"endCursor":cursor}
                }}}
            })
        };
        let rule = |pattern: &str, pushes: bool, creations: bool| {
            json!({
                "id":"rule-1", "pattern":pattern, "restrictsPushes":pushes, "blocksCreations":creations,
                "requiresCommitSignatures":false, "requiresApprovingReviews":false
            })
        };
        for field in ["requiresCommitSignatures", "requiresApprovingReviews"] {
            let mut protected = rule("maintenance/*", false, false);
            protected[field] = json!(true);
            assert!(
                admit_classic_rules("maintenance/uncreated", |_| Ok(page(
                    json!([protected]),
                    false,
                    Value::Null,
                    1
                )))
                .is_err()
            );
            let mut missing = rule("maintenance/*", false, false);
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                admit_classic_rules("maintenance/uncreated", |_| Ok(page(
                    json!([missing]),
                    false,
                    Value::Null,
                    1
                )))
                .is_err()
            );
        }
        for (pushes, creations) in [(true, true), (true, false), (false, true)] {
            for pattern in [
                "maintenance/*",
                "maintenance/**",
                "maintenance/0123456789abcdef",
            ] {
                assert!(
                    admit_classic_rules("maintenance/0123456789abcdef", |_| Ok(page(
                        json!([rule(pattern, pushes, creations)]),
                        false,
                        Value::Null,
                        1
                    )))
                    .unwrap_err()
                    .to_string()
                    .contains("restricted classic protection")
                );
            }
        }
        admit_classic_rules("maintenance/uncreated", |_| {
            Ok(page(
                json!([rule("trunk", true, true)]),
                false,
                Value::Null,
                1,
            ))
        })
        .unwrap();
        admit_classic_rules("maintenance/uncreated", |_| {
            Ok(page(
                json!([rule("maintenance/*", false, false)]),
                false,
                Value::Null,
                1,
            ))
        })
        .unwrap();
        admit_classic_rules("maintenance/uncreated", |_| {
            Ok(page(json!([]), false, Value::Null, 0))
        })
        .unwrap();
        let first = (0..100)
            .map(|id| {
                let mut entry = rule(&format!("other-{id}"), true, true);
                entry["id"] = json!(format!("rule-{id}"));
                entry
            })
            .collect::<Vec<_>>();
        assert!(
            admit_classic_rules("maintenance/uncreated", |cursor| Ok(if cursor.is_none() {
                page(json!(first), true, json!("next"), 101)
            } else {
                assert_eq!(cursor, Some("next"));
                let mut last = rule("maintenance/*", true, true);
                last["id"] = json!("last");
                page(json!([last]), false, Value::Null, 101)
            }))
            .is_err()
        );
        admit_classic_rules("maintenance/uncreated", |cursor| {
            Ok(if cursor.is_none() {
                page(json!(first), true, json!("next"), 101)
            } else {
                let mut last = rule("other/*", true, true);
                last["id"] = json!("last");
                page(json!([last]), false, Value::Null, 101)
            })
        })
        .unwrap();
        for invalid in [
            Value::Null,
            json!({"data":{"repository":null}}),
            json!({"errors":[{"message":"unavailable"}]}),
            page(json!([]), false, Value::Null, 1),
            page(json!([rule("other", true, true)]), true, Value::Null, 2),
            page(
                json!([rule("other", true, true), rule("other", true, true)]),
                false,
                Value::Null,
                2,
            ),
            page(
                json!([{"id":"bad","pattern":"maintenance/*","restrictsPushes":null,"blocksCreations":false}]),
                false,
                Value::Null,
                1,
            ),
            page(
                json!([rule("maintenance/[ab]", true, true)]),
                false,
                Value::Null,
                1,
            ),
        ] {
            assert!(admit_classic_rules("maintenance/uncreated", |_| Ok(invalid.clone())).is_err());
        }
        assert!(
            admit_classic_rules("maintenance/uncreated", |_| anyhow::bail!(
                "classic inventory unavailable"
            ))
            .is_err()
        );
    }

    #[test]
    fn ruleset_admission_checks_the_exact_uncreated_maintenance_branch() {
        let endpoint = "repos/team/demo";
        let rules = |path: &str| -> Result<Value> {
            Ok(match path {
                "repos/team/demo/rules/branches/custom-default" => json!([]),
                "repos/team/demo/rules/branches/maintenance%2F0123456789abcdef" => {
                    json!([{"type":"creation"}])
                }
                _ => panic!("unexpected branch rules request: {path}"),
            })
        };
        admit_rules(endpoint, "custom-default", rules).unwrap();
        assert!(
            admit_rules(endpoint, "maintenance/0123456789abcdef", rules)
                .unwrap_err()
                .to_string()
                .contains("active rulesets")
        );
        admit_rules(endpoint, "maintenance/uncreated", |path| {
            assert_eq!(
                path,
                "repos/team/demo/rules/branches/maintenance%2Funcreated"
            );
            Ok(json!([]))
        })
        .unwrap();
        for invalid in [Value::Null, json!({}), json!({"rules":[]})] {
            assert!(
                admit_rules(endpoint, "maintenance/uncreated", |_| Ok(invalid.clone())).is_err()
            );
        }
        assert!(
            admit_rules(endpoint, "maintenance/uncreated", |_| anyhow::bail!(
                "rules inventory unavailable"
            ))
            .is_err()
        );
    }

    #[test]
    fn paginated_job_inventory_preserves_and_enforces_the_reported_total() {
        assert!(collection("jobs", |_| Ok(json!({"total_count":2,"jobs":[{"id":1}]}))).is_err());
        assert!(collection("jobs", |_| Ok(json!({"jobs":[{"id":1}]}))).is_err());
        let first = (0..100).map(|id| json!({"id":id})).collect::<Vec<_>>();
        let complete = collection("jobs", |page| {
            Ok(if page == 1 {
                json!({"total_count":101,"jobs":first})
            } else {
                json!({"total_count":101,"jobs":[{"id":100}]})
            })
        })
        .unwrap();
        assert_eq!(complete["total_count"], 101);
        assert_eq!(complete["jobs"].as_array().unwrap().len(), 101);
        assert!(
            collection("jobs", |page| Ok(if page == 1 {
                json!({"total_count":101,"jobs":first})
            } else {
                json!({"total_count":100,"jobs":[]})
            }))
            .is_err()
        );
    }

    #[test]
    fn unrelated_failed_or_ambiguous_workflows_do_not_gate_generator_checks() {
        let runs = [
            json!({"id":1,"head_branch":"custom-default","path":".github/workflows/ci.yaml"}),
            json!({"id":2,"head_branch":"maintenance/candidate","path":".github/workflows/ci.yaml","conclusion":"failure"}),
            json!({"id":3,"head_branch":"custom-default","path":".github/workflows/docs.yaml","conclusion":"failure"}),
            json!({"id":4,"head_branch":"custom-default","path":".github/workflows/docs.yaml","conclusion":"failure"}),
        ];
        let paths = generator_workflow_paths(&runs, "custom-default", |run| {
            assert_ne!(run["id"], 2, "unrelated branch entered qualification");
            Ok(run["id"] == 1)
        })
        .unwrap();
        assert_eq!(paths, BTreeSet::from([".github/workflows/ci.yaml".into()]));
        assert!(
            generator_workflow_paths(&runs, "custom-default", |_| Ok(false))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn shared_job_names_qualify_only_after_each_workflow_succeeds() {
        let revision = "a".repeat(40);
        let request: AdapterRequest = serde_json::from_value(json!({
            "schemaVersion":1,"operation":"qualify","coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api","credentialFile":null,"authority":"777",
            "revision":revision,"branch":"maintenance/test","workflows":[],
            "requiredChecks":[{"name":"test","authority":"777"}]
        }))
        .unwrap();
        let mut response: AdapterResponse = serde_json::from_value(json!({
            "schemaVersion":1,"coordinate":request.coordinate,"repositoryId":"2",
            "defaultBranch":"trunk","revision":revision,"remote":"https://forge.example/team/demo.git",
            "policyDigest":"b".repeat(64),"requirements":[],"status":"success","checks":[]
        })).unwrap();
        for (id, path) in [
            (42, ".github/workflows/first.yaml"),
            (43, ".github/workflows/second.yaml"),
        ] {
            let run = json!({"id":id,"repository":{"id":2},"head_repository":{"id":2},"event":"push","head_sha":revision,"head_branch":"maintenance/test","path":path,"run_attempt":1,"check_suite_id":5,"status":"completed","conclusion":"success"});
            let suite = json!({"id":5,"app":{"id":777},"head_sha":revision});
            let jobs = json!({"total_count":1,"jobs":[{"id":8,"run_id":id,"run_attempt":1,"head_sha":revision,"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"status":"completed","conclusion":"success"}]}]});
            assert!(
                admit_run(
                    (2, 777),
                    (&revision, Some("maintenance/test"), path),
                    &run,
                    &jobs,
                    &suite,
                    &PushSteps::default(),
                )
                .unwrap()
            );
            record_checks(&mut response, jobs["jobs"].as_array().unwrap(), 777, id).unwrap();
            let mut failed_jobs = jobs.clone();
            failed_jobs["jobs"][0]["steps"][0]["conclusion"] = json!("failure");
            assert!(
                admit_run(
                    (2, 777),
                    (&revision, Some("maintenance/test"), path),
                    &run,
                    &failed_jobs,
                    &suite,
                    &PushSteps::default(),
                )
                .is_err()
            );
        }
        response.validate(&request).unwrap();
        assert_eq!(response.checks.len(), 1);
    }

    #[test]
    fn actions_adapter_selects_push_triggers_from_owned_source_inventory() {
        let paths = [
            "deny.toml",
            ".crow/build.yaml",
            ".github/workflows/ci.yaml",
            ".github/workflows/manual.yml",
        ]
        .map(str::to_owned);
        let selected = requested_workflows(&paths, None, |path| Ok(json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(if path.ends_with("manual.yml") {"on: workflow_dispatch\n"} else {"on: [push, pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n"})}))).unwrap();
        assert_eq!(
            selected,
            BTreeSet::from([".github/workflows/ci.yaml".into()])
        );
        for trigger in [
            "push",
            "[push, pull_request]",
            "\n  push:\n    branches: [trunk]",
        ] {
            let file = json!({"path":paths[2],"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on: {trigger}\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n"))});
            assert!(push_workflow(&paths[2], &file, Some("trunk")).unwrap());
        }
        assert!(requested_workflows(&paths, None, |_| Ok(Value::Null)).is_err());
    }

    #[test]
    fn admission_rejects_required_contexts_outside_owned_literal_jobs() {
        let path = ".github/workflows/ci.yaml";
        let mut request: AdapterRequest = serde_json::from_value(json!({
            "schemaVersion":1,"operation":"admit","coordinate":"forge.example/team/demo",
            "apiUrl":"https://forge.example/api","credentialFile":null,"authority":"777",
            "revision":"a".repeat(40),"branch":"maintenance/candidate","workflows":[path],
            "requiredChecks":[{"name":"test","authority":"777"}],
            "plan":{"schema":"simit.ci-update-plan.v1","files":[],"obsolete":[]}
        }))
        .unwrap();
        for (event, job, accepted) in [
            ("push", "runs-on: ubuntu-latest", true),
            ("push", "name: test\n    runs-on: ubuntu-latest", true),
            (
                "push",
                "name: hand-maintained-check\n    runs-on: ubuntu-latest",
                false,
            ),
            (
                "push",
                "name: '${{ github.job }}'\n    runs-on: ubuntu-latest",
                false,
            ),
            (
                "push",
                "strategy:\n      matrix:\n        os: [ubuntu-latest]\n    runs-on: '${{ matrix.os }}'",
                false,
            ),
            ("workflow_dispatch", "runs-on: ubuntu-latest", false),
        ] {
            request.plan.as_mut().unwrap().files = vec![crate::project::GeneratedFile {
                relative_path: path.into(),
                content: format!(
                    "on: {event}\njobs:\n  test:\n    {job}\n    steps:\n      - run: exit 0\n"
                ),
            }];
            request.validate().unwrap();
            let result = admit_workflows(&request);
            assert_eq!(result.is_ok(), accepted, "{event}: {job}: {result:?}");
        }
        let plan = request.plan.as_mut().unwrap();
        plan.files[0].content = "on: push\njobs:\n  owned:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n".into();
        plan.files.push(crate::project::GeneratedFile {
            relative_path: ".github/workflows/unrequested.yaml".into(),
            content: "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n".into(),
        });
        assert!(
            admit_workflows(&request)
                .unwrap_err()
                .to_string()
                .contains("required check test is not provably produced")
        );
        request.required_checks[0].name = "owned".into();
        admit_workflows(&request).unwrap();
    }

    #[test]
    fn workflow_admission_rejects_excluded_maintenance_refs_before_push() {
        let path = ".github/workflows/ci.yaml";
        let branch = "maintenance/0123456789abcdef";
        for (filters, admitted) in [
            ("branches: [trunk]", false),
            ("branches: ['**']", true),
            ("branches: ['*']", false),
            ("branches: ['maintenance/**']", true),
            ("branches: ['**', '!maintenance/**']", false),
            ("branches: ['**', '!maintenance/**', 'maintenance/*']", true),
            ("branches-ignore: ['maintenance/**']", false),
            ("branches-ignore: [docs]", true),
            ("tags: ['v*']", false),
            ("paths: ['src/**']", false),
            ("paths-ignore: ['.github/**']", false),
            ("branches: ['maintenance/?*']", false),
        ] {
            let file = json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on:\n  push:\n    {filters}\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n"))});
            assert_eq!(
                push_workflow(path, &file, Some(branch)).is_ok(),
                admitted,
                "{filters}"
            );
        }
    }
    #[test]
    fn workflow_admission_rejects_masked_and_opaque_step_outcomes() {
        let path = ".github/workflows/ci.yaml";
        for (job, admitted) in [
            (
                "runs-on: ubuntu-latest\n    steps:\n      - run: exit 0",
                true,
            ),
            (
                "runs-on: ubuntu-latest\n    continue-on-error: false\n    steps:\n      - run: exit 0\n        continue-on-error: false",
                true,
            ),
            (
                "runs-on: ubuntu-latest\n    continue-on-error: true\n    steps:\n      - run: exit 1",
                false,
            ),
            (
                "runs-on: ubuntu-latest\n    steps:\n      - run: exit 1\n        continue-on-error: true",
                false,
            ),
            (
                "runs-on: ubuntu-latest\n    steps:\n      - run: exit 1\n        continue-on-error: '${{ matrix.experimental }}'",
                false,
            ),
            ("uses: team/repo/.github/workflows/hidden.yml@trunk", false),
        ] {
            let file = json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on: push\njobs:\n  test:\n    {job}\n"))});
            assert_eq!(
                push_workflow(path, &file, Some("trunk")).is_ok(),
                admitted,
                "{job}"
            );
        }
    }

    #[test]
    fn workflow_admission_rejects_conditional_jobs_and_steps() {
        let path = ".github/workflows/ci.yaml";
        for job in [
            "if: false\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0",
            "if: github.ref == 'refs/heads/trunk'\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 0",
            "runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n        if: false",
            "runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n        if: matrix.system == 'x86_64-linux'",
            "runs-on: ubuntu-latest\n    steps:\n      - run: exit 0\n        if: true",
        ] {
            let file = json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on: push\njobs:\n  test:\n    {job}\n"))});
            assert!(
                push_workflow(path, &file, Some("maintenance/candidate"))
                    .unwrap_err()
                    .to_string()
                    .contains("conditional jobs or steps"),
                "{job}"
            );
        }
    }

    #[test]
    fn original_admission_requires_exact_source_app_attempt_and_complete_jobs() {
        let revision = "a".repeat(40);
        let run = json!({"id":42,"repository":{"id":2},"head_repository":{"id":2},"event":"push","head_sha":revision,"head_branch":"maintenance/test","path":".github/workflows/ci.yaml","run_attempt":1,"check_suite_id":5,"status":"completed","conclusion":"success"});
        let suite = json!({"id":5,"app":{"id":777},"head_sha":revision});
        let jobs = json!({"total_count":1,"jobs":[{"id":8,"run_id":42,"run_attempt":1,"head_sha":revision,"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"status":"completed","conclusion":"success"}]}]});
        let admit = |r: &Value, j: &Value, s: &Value| {
            admit_run(
                (2, 777),
                (
                    &revision,
                    Some("maintenance/test"),
                    ".github/workflows/ci.yaml",
                ),
                r,
                j,
                s,
                &PushSteps::default(),
            )
        };
        assert!(admit(&run, &jobs, &suite).unwrap());
        for steps in [Value::Null, json!({}), json!([])] {
            let mut changed = jobs.clone();
            changed["jobs"][0]["steps"] = steps;
            assert!(admit(&run, &changed, &suite).is_err());
        }
        let mut changed = jobs.clone();
        changed["jobs"][0].as_object_mut().unwrap().remove("steps");
        assert!(admit(&run, &changed, &suite).is_err());
        for conclusion in ["skipped", "cancelled", "failure", "neutral"] {
            let mut changed = jobs.clone();
            changed["jobs"][0]["steps"][0]["conclusion"] = json!(conclusion);
            assert!(admit(&run, &changed, &suite).is_err());
        }
        for (key, value) in [
            ("head_sha", json!("b".repeat(40))),
            ("run_attempt", json!(2)),
            ("head_branch", json!("trunk")),
            ("event", json!("pull_request")),
            ("conclusion", json!("cancelled")),
        ] {
            let mut changed = run.clone();
            changed[key] = value;
            assert!(admit(&changed, &jobs, &suite).is_err(), "{key}");
        }
        let mut changed = suite.clone();
        changed["app"]["id"] = json!(123);
        assert!(admit(&run, &jobs, &changed).is_err());
        for conclusion in ["skipped", "cancelled", "failure", "neutral"] {
            let mut changed = jobs.clone();
            changed["jobs"][0]["conclusion"] = json!(conclusion);
            assert!(admit(&run, &changed, &suite).is_err());
        }
        assert!(admit(&run, &json!({"total_count":0,"jobs":[]}), &suite).is_err());
        let mut pending = run.clone();
        pending["status"] = json!("in_progress");
        pending["conclusion"] = Value::Null;
        assert!(!admit(&pending, &jobs, &suite).unwrap());
    }
}
