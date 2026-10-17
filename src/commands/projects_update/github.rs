//! GitHub Actions adapter. Provider identifiers and API location come from policy.
use super::schema::*;
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde_json::Value;
use std::{collections::BTreeSet, io::Read, time::Duration};

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
            .map(std::fs::read_to_string)
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
        let mut request = self
            .client
            .get(self.base.join(path)?)
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
    fn list(&self, path: &str, key: &str) -> Result<Value> {
        collection(key, |page| {
            let separator = if path.contains('?') { '&' } else { '?' };
            self.get(&format!("{path}{separator}per_page=100&page={page}"))
        })
    }
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

fn push_workflow(path: &str, file: &Value, branch: Option<&str>) -> Result<bool> {
    ensure!(
        file["path"] == path && file["type"] == "file" && file["encoding"] == "base64",
        "workflow source inventory is incomplete"
    );
    let content = text(file, "/content")?;
    let content =
        base64::engine::general_purpose::STANDARD.decode(content.replace(['\r', '\n'], ""))?;
    let yaml: serde_yaml::Value = serde_yaml::from_slice(&content)?;
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
    if trigger.is_some() {
        let jobs = yaml
            .get("jobs")
            .and_then(serde_yaml::Value::as_mapping)
            .context("missing workflow jobs")?;
        ensure!(!jobs.is_empty(), "empty workflow job inventory");
        for job in jobs.values() {
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
    }
    Ok(trigger.is_some())
}

fn requested_workflows(
    paths: &[String],
    branch: Option<&str>,
    mut source: impl FnMut(&str) -> Result<Value>,
) -> Result<BTreeSet<String>> {
    let mut workflows = BTreeSet::new();
    for path in paths {
        if std::path::Path::new(path).parent() == Some(std::path::Path::new(".github/workflows"))
            && (path.ends_with(".yaml") || path.ends_with(".yml"))
            && push_workflow(path, &source(path)?, branch)?
        {
            workflows.insert(path.clone());
        }
    }
    Ok(workflows)
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

pub fn execute(request: &AdapterRequest) -> Result<AdapterResponse> {
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
    let api = Api::new(request)?;
    let endpoint = format!("repos/{repository}");
    let repo = api.get(&endpoint)?;
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
        admit_rules(&endpoint, branch, |path| api.list(path, ""))?;
    }
    let info = if request.operation == "admit" {
        Value::Null
    } else {
        api.get(&format!("{endpoint}/branches/{}", encoded(branch)))?
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
        let protection = if info["protected"] == true {
            api.get(&format!(
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
        protection
    } else {
        Value::Null
    };
    let mut response = AdapterResponse {
        schema_version: 1,
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
    let runs = api.list(&query, "workflow_runs")?;
    let runs = runs["workflow_runs"]
        .as_array()
        .context("missing workflow runs")?;
    let mut generator_jobs = std::collections::BTreeMap::new();
    let source = |path: &str| {
        let path = path.split('/').map(encoded).collect::<Vec<_>>().join("/");
        api.get(&format!("{endpoint}/contents/{path}?ref={revision}"))
    };
    let paths = if request.workflows.is_empty() {
        generator_workflow_paths(runs, branch, |run| {
            let run_id = number(run, "/id")?;
            let jobs = api.list(
                &format!("{endpoint}/actions/runs/{run_id}/attempts/1/jobs"),
                "jobs",
            )?;
            let job_entries = jobs["jobs"].as_array().context("missing job inventory")?;
            let suite = api.get(&format!(
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
        requested_workflows(&request.workflows, Some(branch), source)?
    };
    if paths.is_empty() {
        response.status = "pending".into();
        return Ok(response);
    }
    for path in paths {
        ensure!(
            path.starts_with(".github/workflows/")
                && (path.ends_with(".yaml") || path.ends_with(".yml")),
            "GitHub adapter cannot qualify this workflow format"
        );
        if request.workflows.is_empty() {
            ensure!(
                push_workflow(&path, &source(&path)?, Some(branch))?,
                "generator qualification requires a push workflow"
            );
        }
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
                api.list(
                    &format!("{endpoint}/actions/runs/{run_id}/attempts/1/jobs"),
                    "jobs",
                )?,
                api.get(&format!(
                    "{endpoint}/check-suites/{}",
                    number(run, "/check_suite_id")?
                ))?,
            )
        };
        if !admit_run(
            (id, authority),
            &revision,
            Some(branch),
            &path,
            run,
            &jobs,
            &suite,
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
    revision: &str,
    branch: Option<&str>,
    workflow: &str,
    run: &Value,
    completed_jobs: &Value,
    suite: &Value,
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
    let mut seen = BTreeSet::new();
    for job in jobs {
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
        ensure!(
            !steps.is_empty()
                && steps
                    .iter()
                    .all(|s| s["status"] == "completed" && s["conclusion"] == "success"),
            "job has a non-success step"
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                    &revision,
                    Some("maintenance/test"),
                    path,
                    &run,
                    &jobs,
                    &suite
                )
                .unwrap()
            );
            record_checks(&mut response, jobs["jobs"].as_array().unwrap(), 777, id).unwrap();
            let mut failed_jobs = jobs.clone();
            failed_jobs["jobs"][0]["steps"][0]["conclusion"] = json!("failure");
            assert!(
                admit_run(
                    (2, 777),
                    &revision,
                    Some("maintenance/test"),
                    path,
                    &run,
                    &failed_jobs,
                    &suite
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
    fn original_admission_requires_exact_source_app_attempt_and_complete_jobs() {
        let revision = "a".repeat(40);
        let run = json!({"id":42,"repository":{"id":2},"head_repository":{"id":2},"event":"push","head_sha":revision,"head_branch":"maintenance/test","path":".github/workflows/ci.yaml","run_attempt":1,"check_suite_id":5,"status":"completed","conclusion":"success"});
        let suite = json!({"id":5,"app":{"id":777},"head_sha":revision});
        let jobs = json!({"total_count":1,"jobs":[{"id":8,"run_id":42,"run_attempt":1,"head_sha":revision,"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"status":"completed","conclusion":"success"}]}]});
        let admit = |r: &Value, j: &Value, s: &Value| {
            admit_run(
                (2, 777),
                &revision,
                Some("maintenance/test"),
                ".github/workflows/ci.yaml",
                r,
                j,
                s,
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
