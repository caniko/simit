//! Crow (Woodpecker-compatible) API collection.
//!
//! Crow replaces GitHub's run -> job -> step model with
//! pipeline -> workflow -> step. One pathway row is one step execution, with
//! the workflow name retained for the per-workflow rollup.

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::ci_usage::{
    Forge, RepoSpec, Sample, UnitKind, Window,
    http::JsonApi,
};

const PER_PAGE: usize = 50;
const MAX_PIPELINE_PAGES: u32 = 20;

pub(super) fn collect(spec: &RepoSpec, window: &Window) -> Result<(Vec<Sample>, Vec<String>)> {
    let server = std::env::var("CROW_SERVER").context(
        "CROW_SERVER is required for crow: repositories (for example https://ci.example.com)",
    )?;
    let token = std::env::var("CROW_TOKEN")
        .context("CROW_TOKEN is required for crow: repositories")?;
    let base = format!("{}/api/v1", server.trim_end_matches('/'));
    let api = JsonApi::new(&base, token, "Crow", "application/json", false)?;
    let owner = &spec.owner;
    let name = &spec.name;
    let repo = format!("{owner}/{name}");
    let lookup = api.get(&format!("repos/lookup/{owner}/{name}"))?;
    let id = lookup["id"]
        .as_u64()
        .with_context(|| format!("Crow lookup for {repo} is missing the repository id"))?;
    let mut samples = Vec::new();
    let mut page = 1;
    loop {
        ensure!(
            page <= MAX_PIPELINE_PAGES,
            "Crow pipeline pagination cap ({MAX_PIPELINE_PAGES} pages) reached for {repo}; narrow --since/--until"
        );
        let path = format!("repos/{id}/pipelines?page={page}&perPage={PER_PAGE}");
        let value = api.get(&path)?;
        let pipelines = value
            .as_array()
            .with_context(|| format!("Crow pipeline inventory for {repo} is not an array"))?;
        let mut page_entirely_before_window = !pipelines.is_empty();
        for pipeline in pipelines {
            let Some(created) = unix_time(&pipeline["created"]) else {
                continue;
            };
            if created < window.since {
                continue;
            }
            page_entirely_before_window = false;
            if created >= window.until {
                continue;
            }
            let number = pipeline["number"]
                .as_u64()
                .with_context(|| format!("Crow pipeline for {repo} is missing number"))?;
            let detail = api.get(&format!("repos/{id}/pipelines/{number}"))?;
            samples.extend(parse_pipeline(&repo, &detail)?);
        }
        if pipelines.len() < PER_PAGE || page_entirely_before_window {
            break;
        }
        if page == MAX_PIPELINE_PAGES {
            bail!(
                "Crow pipeline pagination cap ({MAX_PIPELINE_PAGES} pages) reached for {repo}; narrow --since/--until"
            );
        }
        page += 1;
    }
    Ok((samples, Vec::new()))
}

fn parse_pipeline(repo: &str, pipeline: &Value) -> Result<Vec<Sample>> {
    let workflows = pipeline["workflows"].as_array();
    let mut samples = Vec::new();
    if let Some(workflows) = workflows {
        for workflow in workflows {
            samples.extend(parse_workflow(repo, workflow)?);
        }
    }
    // Pipeline-level errors carry scheduling failures (typically no agent or a
    // quota denial) before any workflow exists. Workflow-level errors are
    // already represented in step rows, so only use these when nothing ran.
    if workflows.is_none_or(|workflows| workflows.is_empty()) {
        for error in pipeline["errors"].as_array().into_iter().flatten() {
            if error["is_warning"].as_bool() == Some(true) {
                continue;
            }
            let message = error["message"].as_str().unwrap_or_default();
            let workflow =
                workflow_from_error(message).unwrap_or_else(|| "(pipeline)".to_owned());
            samples.push(Sample {
                forge: Forge::Crow,
                repo: repo.to_owned(),
                workflow,
                unit: "(not started)".to_owned(),
                unit_kind: UnitKind::Pipeline,
                seconds: None,
                failed: !quota_denial(message),
                oom_killed: false,
                quota_denied: quota_denial(message),
                runner: None,
            });
        }
        if samples.is_empty()
            && matches!(
                pipeline["status"].as_str().unwrap_or_default(),
                "failure" | "error"
            )
        {
            samples.push(Sample {
                forge: Forge::Crow,
                repo: repo.to_owned(),
                workflow: "(pipeline)".to_owned(),
                unit: "(not started)".to_owned(),
                unit_kind: UnitKind::Pipeline,
                seconds: None,
                failed: true,
                oom_killed: false,
                quota_denied: false,
                runner: None,
            });
        }
    }
    Ok(samples)
}

fn parse_workflow(repo: &str, workflow: &Value) -> Result<Vec<Sample>> {
    let name = workflow["pipeline_name"]
        .as_str()
        .or_else(|| workflow["name"].as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("(unnamed workflow)")
        .to_owned();
    let error = workflow["error"].as_str().unwrap_or_default();
    let workflow_oom = oom_killed(error);
    let agent = workflow["agent_name"]
        .as_str()
        .filter(|agent| !agent.is_empty())
        .map(str::to_owned);
    let children = workflow["children"].as_array();
    let mut samples = Vec::new();
    if let Some(children) = children {
        for child in children {
            let (Some(started), Some(finished)) =
                (unix_time(&child["started"]), unix_time(&child["finished"]))
            else {
                continue;
            };
            let seconds = (finished - started).num_milliseconds() as f64 / 1000.0;
            if seconds < 0.0 {
                continue;
            }
            let state = child["state"].as_str().unwrap_or_default();
            let failed = matches!(state, "failure" | "error");
            let exit_code = child["exit_code"].as_i64();
            samples.push(Sample {
                forge: Forge::Crow,
                repo: repo.to_owned(),
                workflow: name.clone(),
                unit: child["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .unwrap_or("(unnamed step)")
                    .to_owned(),
                unit_kind: UnitKind::Step,
                seconds: Some(seconds),
                failed,
                // The OOM message lives on the workflow; attribute it to the
                // failed step (normally exit 137) that consumed the memory.
                oom_killed: workflow_oom && (failed || exit_code == Some(137)),
                quota_denied: false,
                runner: agent.clone(),
            });
        }
    }
    // A workflow refused by the quota has no timed steps but still names itself.
    if samples.is_empty() && quota_denial(error) {
        samples.push(Sample {
            forge: Forge::Crow,
            repo: repo.to_owned(),
            workflow: name,
            unit: "(not started)".to_owned(),
            unit_kind: UnitKind::Pipeline,
            seconds: None,
            failed: false,
            oom_killed: false,
            quota_denied: true,
            runner: None,
        });
        return Ok(samples);
    }
    if samples.is_empty()
        && let (Some(started), Some(finished)) =
            (unix_time(&workflow["started"]), unix_time(&workflow["finished"]))
    {
        let seconds = (finished - started).num_milliseconds() as f64 / 1000.0;
        if seconds >= 0.0 {
            samples.push(Sample {
                forge: Forge::Crow,
                repo: repo.to_owned(),
                workflow: name,
                unit: "(workflow)".to_owned(),
                unit_kind: UnitKind::Step,
                seconds: Some(seconds),
                failed: matches!(
                    workflow["state"].as_str().unwrap_or_default(),
                    "failure" | "error"
                ),
                oom_killed: workflow_oom,
                quota_denied: false,
                runner: agent,
            });
        }
    }
    Ok(samples)
}

fn unix_time(value: &Value) -> Option<DateTime<Utc>> {
    let seconds = value.as_i64()?;
    if seconds <= 0 {
        return None;
    }
    DateTime::from_timestamp(seconds, 0)
}

/// Crow reports quota denials before a workflow gets an agent. A missing
/// agent label is a scheduling failure, not a quota denial.
fn quota_denial(message: &str) -> bool {
    message.to_ascii_lowercase().contains("quota")
}

fn oom_killed(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("oom-kill")
        || message.contains("oomkill")
        || message.contains("out of memory")
        || (message.contains("memory") && message.contains("137"))
}

/// The denial names the workflow that never started:
/// "### Workflow `annotation-browser` cannot be started".
fn workflow_from_error(message: &str) -> Option<String> {
    let lowered = message.to_ascii_lowercase();
    let start = lowered.find("workflow `")? + "workflow `".len();
    let rest = &message[start..];
    let end = rest.find('`')?;
    let name = rest[..end].trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIPELINES: &str = include_str!("../../tests/fixtures/ci-usage/crow-pipelines.json");
    const PIPELINE: &str = include_str!("../../tests/fixtures/ci-usage/crow-pipeline.json");
    const QUOTA: &str = include_str!("../../tests/fixtures/ci-usage/crow-quota-pipeline.json");

    #[test]
    fn pipeline_list_parses_created_timestamps_and_numbers() {
        let value: Value = serde_json::from_str(PIPELINES).unwrap();
        let pipelines = value.as_array().unwrap();
        assert_eq!(pipelines.len(), 3);
        assert_eq!(pipelines[0]["number"], 38);
        assert_eq!(unix_time(&pipelines[0]["created"]).unwrap().timestamp(), 1_794_203_395);
        assert!(unix_time(&serde_json::json!(0)).is_none());
    }

    #[test]
    fn workflow_steps_become_timed_rows_and_attribute_oom() {
        let value: Value = serde_json::from_str(PIPELINE).unwrap();
        let samples = parse_pipeline("caniko/bekiper", &value).unwrap();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples[0].workflow, "qualify-flake-check");
        assert_eq!(samples[0].unit, "clone");
        assert_eq!(samples[0].seconds, Some(14.0));
        assert_eq!(samples[0].runner.as_deref(), Some("artemis"));
        assert!(!samples[0].failed);
        assert_eq!(samples[1].unit, "flake-check");
        assert_eq!(samples[1].seconds, Some(294.0));
        assert_eq!(samples[2].workflow, "qualify-registry-postgresql-systemd");
        assert_eq!(samples[2].unit, "clone");
        assert_eq!(samples[3].unit, "registry-postgresql-systemd");
        assert_eq!(samples[3].seconds, Some(1529.0));
        assert!(samples[3].failed);
        assert!(samples[3].oom_killed);
    }

    #[test]
    fn missing_agent_is_a_scheduling_failure_named_after_its_workflow() {
        let value: Value = serde_json::from_str(QUOTA).unwrap();
        let samples = parse_pipeline("caniko/bekiper", &value).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].workflow, "annotation-browser");
        assert_eq!(samples[0].unit, "(not started)");
        assert_eq!(samples[0].unit_kind, UnitKind::Pipeline);
        assert!(!samples[0].quota_denied);
        assert!(samples[0].failed);
        assert_eq!(samples[0].seconds, None);
        assert!(!quota_denial("No agent matching the required labels was found"));
        assert!(!oom_killed("no room left on the volume"));
    }

    #[test]
    fn workflow_level_quota_denial_is_counted_without_timed_steps() {
        let workflow = serde_json::json!({
            "name": "qualify-flake-check",
            "state": "failure",
            "error": "CI quota denied (linux): the CI time quota \"CI quota (all)\" is used up",
            "children": [{ "name": "clone", "state": "skipped", "started": 0, "finished": 0 }]
        });
        let samples = parse_workflow("caniko/bekiper", &workflow).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].workflow, "qualify-flake-check");
        assert!(samples[0].quota_denied);
        assert!(!samples[0].failed);
        assert_eq!(samples[0].seconds, None);
    }
}
