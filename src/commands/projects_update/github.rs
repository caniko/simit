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

fn push_workflow(path: &str, file: &Value) -> Result<bool> {
    ensure!(
        file["path"] == path && file["type"] == "file" && file["encoding"] == "base64",
        "workflow source inventory is incomplete"
    );
    let content = text(file, "/content")?;
    let content =
        base64::engine::general_purpose::STANDARD.decode(content.replace(['\r', '\n'], ""))?;
    let yaml: serde_yaml::Value = serde_yaml::from_slice(&content)?;
    Ok(match yaml.get("on") {
        Some(serde_yaml::Value::String(event)) => event == "push",
        Some(serde_yaml::Value::Sequence(events)) => {
            events.iter().any(|e| e.as_str() == Some("push"))
        }
        Some(serde_yaml::Value::Mapping(events)) => {
            events.contains_key(serde_yaml::Value::String("push".into()))
        }
        _ => false,
    })
}

fn requested_workflows(
    paths: &[String],
    mut source: impl FnMut(&str) -> Result<Value>,
) -> Result<BTreeSet<String>> {
    let mut workflows = BTreeSet::new();
    for path in paths {
        if std::path::Path::new(path).parent() == Some(std::path::Path::new(".github/workflows"))
            && (path.ends_with(".yaml") || path.ends_with(".yml"))
            && push_workflow(path, &source(path)?)?
        {
            workflows.insert(path.clone());
        }
    }
    Ok(workflows)
}

fn generator_workflow_paths(
    runs: &[Value],
    mut required: impl FnMut(&Value) -> Result<bool>,
) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for run in runs {
        if required(run)? {
            paths.insert(text(run, "/path")?);
        }
    }
    Ok(paths)
}

pub fn execute(request: &AdapterRequest) -> Result<AdapterResponse> {
    request.validate()?;
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
    let info = api.get(&format!("{endpoint}/branches/{}", encoded(branch)))?;
    let revision = if request.operation == "qualify" {
        request
            .revision
            .clone()
            .context("qualification needs exact revision")?
    } else {
        text(&info, "/commit/sha")?
    };
    let mut requirements = Vec::new();
    let policy = if request.operation == "inspect" {
        let rules = api.list(
            &format!("{endpoint}/rules/branches/{}", encoded(branch)),
            "",
        )?;
        ensure!(
            rules.as_array().context("missing branch rules")?.is_empty(),
            "active rulesets require an adapter with ruleset-transition support"
        );
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
        if let Some(contexts) = protection["required_status_checks"]["contexts"].as_array() {
            let checks = protection["required_status_checks"]["checks"]
                .as_array()
                .context("required context authorities are unavailable")?;
            for name in contexts {
                let check = checks
                    .iter()
                    .find(|c| c["context"] == *name)
                    .context("required context lacks application binding")?;
                let authority = check["app_id"]
                    .as_u64()
                    .context("unbound status context cannot qualify automated updates")?;
                requirements.push(Requirement {
                    name: name.as_str().context("invalid required context")?.into(),
                    authority: authority.to_string(),
                });
            }
        }
        protection
    } else {
        Value::Null
    };
    let mut response = AdapterResponse {
        schema_version: 1,
        coordinate: request.coordinate.clone(),
        repository_id: id.to_string(),
        default_branch,
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
    // Only original push runs can qualify the exact candidate/default-branch bytes.
    let mut query = format!("{endpoint}/actions/runs?head_sha={revision}&event=push");
    if let Some(branch) = &request.branch {
        query.push_str(&format!("&branch={}", encoded(branch)));
    }
    let runs = api.list(&query, "workflow_runs")?;
    let runs = runs["workflow_runs"]
        .as_array()
        .context("missing workflow runs")?;
    let mut generator_jobs = std::collections::BTreeMap::new();
    let paths = if request.workflows.is_empty() {
        generator_workflow_paths(runs, |run| {
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
        requested_workflows(&request.workflows, |path| {
            let path = path.split('/').map(encoded).collect::<Vec<_>>().join("/");
            api.get(&format!("{endpoint}/contents/{path}?ref={revision}"))
        })?
    };
    if paths.is_empty() {
        response.status = "pending".into();
        return Ok(response);
    }
    let authority: u64 = request
        .authority
        .parse()
        .context("GitHub check authority must be a numeric application ID")?;
    for path in paths {
        ensure!(
            path.starts_with(".github/workflows/")
                && (path.ends_with(".yaml") || path.ends_with(".yml")),
            "GitHub adapter cannot qualify this workflow format"
        );
        let matching = runs
            .iter()
            .filter(|r| r["path"] == path)
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
            request.branch.as_deref(),
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
            json!({"id":1,"path":".github/workflows/ci.yaml"}),
            json!({"id":2,"path":".github/workflows/docs.yaml","conclusion":"failure"}),
            json!({"id":3,"path":".github/workflows/docs.yaml","conclusion":"failure"}),
        ];
        let paths = generator_workflow_paths(&runs, |run| Ok(run["id"] == 1)).unwrap();
        assert_eq!(paths, BTreeSet::from([".github/workflows/ci.yaml".into()]));
        assert!(
            generator_workflow_paths(&runs, |_| Ok(false))
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
        let selected = requested_workflows(&paths, |path| Ok(json!({"path":path,"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(if path.ends_with("manual.yml") {"on: workflow_dispatch\n"} else {"on: [push, pull_request]\n"})}))).unwrap();
        assert_eq!(
            selected,
            BTreeSet::from([".github/workflows/ci.yaml".into()])
        );
        for trigger in [
            "push",
            "[push, pull_request]",
            "\n  push:\n    branches: [trunk]",
        ] {
            let file = json!({"path":paths[2],"type":"file","encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(format!("on: {trigger}\n"))});
            assert!(push_workflow(&paths[2], &file).unwrap());
        }
        assert!(requested_workflows(&paths, |_| Ok(Value::Null)).is_err());
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
