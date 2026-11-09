//! Read-only CI resource usage metrics for GitHub Actions and Crow.
//!
//! `simit ci usage` reports which workflows and jobs/steps consumed the most
//! wall-clock time in a window. It never mutates a repository or CI state.

mod crow;
pub mod cli;
mod github;
mod http;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;

pub use cli::UsageArgs;

const SCHEMA_VERSION: u32 = 1;

/// Run one usage report for the already parsed command line.
pub fn run(args: UsageArgs) -> Result<()> {
    let window = Window::parse(&args.since, args.until.as_deref(), Utc::now())?;
    let specs = parse_repos(&args.repos)?;
    let mut builder = Builder::default();
    for spec in &specs {
        let (samples, warnings) = match spec.forge {
            Forge::Github => github::collect(spec, &window)?,
            Forge::Crow => crow::collect(spec, &window)?,
        };
        builder.warnings.extend(warnings);
        builder.extend(samples);
    }
    let report = builder.finish(
        window,
        specs.iter().map(RepoSpec::display).collect(),
        args.top,
    );
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

/// CI provider whose API supplied a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    Crow,
    Github,
}

impl Forge {
    fn as_str(self) -> &'static str {
        match self {
            Self::Crow => "crow",
            Self::Github => "github",
        }
    }
}

/// The schedulable unit a pathway row measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitKind {
    Job,
    Step,
    Pipeline,
}

/// A validated `forge:owner/name` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
    pub forge: Forge,
    pub owner: String,
    pub name: String,
}

impl RepoSpec {
    fn display(&self) -> String {
        format!("{}:{}/{}", self.forge.as_str(), self.owner, self.name)
    }
}

fn parse_repos(raw: &[String]) -> Result<Vec<RepoSpec>> {
    ensure!(!raw.is_empty(), "at least one --repo is required");
    let mut repos = Vec::new();
    for value in raw {
        let (forge, path) = value
            .split_once(':')
            .with_context(|| format!("invalid --repo {value:?}; use github:owner/name or crow:owner/name"))?;
        let forge = match forge {
            "github" => Forge::Github,
            "crow" => Forge::Crow,
            other => bail!(
                "unsupported --repo forge {other:?} in {value:?}; use github:owner/name or crow:owner/name"
            ),
        };
        let (owner, name) = path.split_once('/').with_context(|| {
            format!("invalid --repo {value:?}; use github:owner/name or crow:owner/name")
        })?;
        for (label, part) in [("owner", owner), ("name", name)] {
            ensure!(
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
                "invalid repository {label} in {value:?}"
            );
        }
        ensure!(name != ".", "invalid repository name in {value:?}");
        let repo = RepoSpec {
            forge,
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        ensure!(
            !repos.contains(&repo),
            "duplicate --repo {value:?}; paths are reported once"
        );
        repos.push(repo);
    }
    Ok(repos)
}

/// Half-open UTC window `[since, until)`.
#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl Window {
    fn parse(since: &str, until: Option<&str>, now: DateTime<Utc>) -> Result<Self> {
        let since = parse_bound(since, now, Bound::Start)?;
        let until = match until {
            Some(raw) => parse_bound(raw, now, Bound::End)?,
            None => now,
        };
        ensure!(
            since < until,
            "--since ({}) must be before --until ({})",
            since.to_rfc3339(),
            until.to_rfc3339()
        );
        Ok(Self { since, until })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    Start,
    End,
}

fn parse_bound(raw: &str, now: DateTime<Utc>, bound: Bound) -> Result<DateTime<Utc>> {
    if let Some(duration) = parse_relative(raw)? {
        return Ok(now - duration);
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").with_context(|| {
        format!("invalid window bound {raw:?}; use YYYY-MM-DD or Nd/Nh/Nw (for example 30d)")
    })?;
    let start = date
        .and_hms_opt(0, 0, 0)
        .context("invalid calendar date")?
        .and_utc();
    Ok(match bound {
        Bound::Start => start,
        Bound::End => start + Duration::days(1),
    })
}

fn parse_relative(raw: &str) -> Result<Option<Duration>> {
    let Some(unit) = raw.chars().last() else {
        return Ok(None);
    };
    let digits = &raw[..raw.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let amount: i64 = digits.parse().with_context(|| {
        format!("invalid window duration {raw:?}; use YYYY-MM-DD or Nd/Nh/Nw")
    })?;
    let multiplier = match unit {
        'd' => 86_400,
        'h' => 3_600,
        'w' => 604_800,
        _ => return Ok(None),
    };
    ensure!(
        (1..=36_500).contains(&amount),
        "invalid window duration {raw:?}; use a positive count with d, h or w"
    );
    Ok(Some(Duration::seconds(amount * multiplier)))
}

/// One measured execution of a job (GitHub) or step/workflow unit (Crow).
#[derive(Debug, Clone)]
pub struct Sample {
    pub forge: Forge,
    pub repo: String,
    pub workflow: String,
    pub unit: String,
    pub unit_kind: UnitKind,
    pub seconds: Option<f64>,
    pub failed: bool,
    pub oom_killed: bool,
    pub quota_denied: bool,
    pub runner: Option<String>,
}

#[derive(Default)]
struct Builder {
    rows: BTreeMap<(Forge, String, String, String), Accumulator>,
    warnings: Vec<String>,
}

impl Builder {
    fn extend(&mut self, samples: Vec<Sample>) {
        for sample in samples {
            let key = (
                sample.forge,
                sample.repo.clone(),
                sample.workflow.clone(),
                sample.unit.clone(),
            );
            let entry = self.rows.entry(key).or_default();
            entry.unit_kind = Some(sample.unit_kind);
            if let Some(seconds) = sample.seconds {
                entry.runs += 1;
                entry.durations.push(seconds);
                entry.total_seconds += seconds;
            }
            entry.failures += u64::from(sample.failed);
            entry.oom_kills += u64::from(sample.oom_killed);
            entry.quota_denials += u64::from(sample.quota_denied);
            if let Some(runner) = sample.runner {
                entry.runners.insert(runner);
            }
        }
    }

    fn finish(self, window: Window, repos: Vec<String>, top: usize) -> Report {
        let grand_total: f64 = self
            .rows
            .values()
            .map(|row| row.total_seconds)
            .sum::<f64>();
        let mut pathways: Vec<Pathway> = self
            .rows
            .into_iter()
            .map(|((forge, repo, workflow, unit), row)| {
                let runs = row.runs;
                let mut durations = row.durations;
                let mean_seconds = if runs == 0 {
                    0.0
                } else {
                    row.total_seconds / runs as f64
                };
                let max_seconds = durations
                    .iter()
                    .copied()
                    .fold(0.0_f64, f64::max);
                Pathway {
                    forge,
                    repo,
                    workflow,
                    unit,
                    unit_kind: row.unit_kind.unwrap_or(UnitKind::Job),
                    runs,
                    total_seconds: round(row.total_seconds),
                    mean_seconds: round(mean_seconds),
                    p95_seconds: round(percentile(&mut durations, 95)),
                    max_seconds: round(max_seconds),
                    failures: row.failures,
                    oom_kills: row.oom_kills,
                    quota_denials: row.quota_denials,
                    share: share(row.total_seconds, grand_total),
                    runner_labels: row.runners.into_iter().collect(),
                }
            })
            .collect();
        pathways.sort_by(|left, right| {
            right
                .total_seconds
                .total_cmp(&left.total_seconds)
                .then_with(|| right.runs.cmp(&left.runs))
                .then_with(|| left.forge.cmp(&right.forge))
                .then_with(|| left.repo.cmp(&right.repo))
                .then_with(|| left.workflow.cmp(&right.workflow))
                .then_with(|| left.unit.cmp(&right.unit))
        });

        let mut repo_rollup: Vec<RepoRollup> = Vec::new();
        let mut repo_totals: BTreeMap<(Forge, String), RollupTotals> = BTreeMap::new();
        let mut workflow_totals: BTreeMap<String, RollupTotals> = BTreeMap::new();
        for pathway in &pathways {
            repo_totals
                .entry((pathway.forge, pathway.repo.clone()))
                .or_default()
                .add(pathway);
            workflow_totals
                .entry(pathway.workflow.clone())
                .or_default()
                .add(pathway);
        }
        for ((forge, repo), totals) in repo_totals {
            repo_rollup.push(RepoRollup {
                forge,
                repo,
                runs: totals.runs,
                total_seconds: round(totals.total_seconds),
                failures: totals.failures,
                oom_kills: totals.oom_kills,
                quota_denials: totals.quota_denials,
                share: share(totals.total_seconds, grand_total),
            });
        }
        repo_rollup.sort_by(|left, right| {
            right
                .total_seconds
                .total_cmp(&left.total_seconds)
                .then_with(|| left.forge.cmp(&right.forge))
                .then_with(|| left.repo.cmp(&right.repo))
        });
        let mut workflow_rollup: Vec<WorkflowRollup> = workflow_totals
            .into_iter()
            .map(|(workflow, totals)| WorkflowRollup {
                workflow,
                runs: totals.runs,
                total_seconds: round(totals.total_seconds),
                failures: totals.failures,
                oom_kills: totals.oom_kills,
                quota_denials: totals.quota_denials,
                share: share(totals.total_seconds, grand_total),
            })
            .collect();
        workflow_rollup.sort_by(|left, right| {
            right
                .total_seconds
                .total_cmp(&left.total_seconds)
                .then_with(|| right.runs.cmp(&left.runs))
                .then_with(|| left.workflow.cmp(&right.workflow))
        });

        let pathway_count = pathways.len();
        let mut truncated = pathways;
        truncated.truncate(top);
        Report {
            schema_version: SCHEMA_VERSION,
            generated_at: Utc::now(),
            window,
            top_limit: top,
            repos,
            total_runs: repo_rollup.iter().map(|row| row.runs).sum(),
            total_seconds: round(grand_total),
            pathway_count,
            pathways: truncated,
            repo_rollup,
            workflow_rollup,
            warnings: self.warnings,
        }
    }
}

#[derive(Debug, Default)]
struct Accumulator {
    unit_kind: Option<UnitKind>,
    runs: u64,
    total_seconds: f64,
    durations: Vec<f64>,
    failures: u64,
    oom_kills: u64,
    quota_denials: u64,
    runners: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct RollupTotals {
    runs: u64,
    total_seconds: f64,
    failures: u64,
    oom_kills: u64,
    quota_denials: u64,
}

impl RollupTotals {
    fn add(&mut self, pathway: &Pathway) {
        self.runs += pathway.runs;
        self.total_seconds += pathway.total_seconds;
        self.failures += pathway.failures;
        self.oom_kills += pathway.oom_kills;
        self.quota_denials += pathway.quota_denials;
    }
}

fn share(part: f64, total: f64) -> f64 {
    if total <= 0.0 {
        0.0
    } else {
        round(part / total)
    }
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Nearest-rank percentile over already measured durations.
fn percentile(durations: &mut [f64], percent: usize) -> f64 {
    if durations.is_empty() {
        return 0.0;
    }
    durations.sort_by(f64::total_cmp);
    let rank = (durations.len() * percent).div_ceil(100);
    durations[rank.saturating_sub(1).min(durations.len() - 1)]
}

/// Stable machine output. `pathways` is truncated to `top_limit`.
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub window: Window,
    pub top_limit: usize,
    pub repos: Vec<String>,
    pub total_runs: u64,
    pub total_seconds: f64,
    pub pathway_count: usize,
    pub pathways: Vec<Pathway>,
    pub repo_rollup: Vec<RepoRollup>,
    pub workflow_rollup: Vec<WorkflowRollup>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Pathway {
    pub forge: Forge,
    pub repo: String,
    pub workflow: String,
    pub unit: String,
    pub unit_kind: UnitKind,
    pub runs: u64,
    pub total_seconds: f64,
    pub mean_seconds: f64,
    pub p95_seconds: f64,
    pub max_seconds: f64,
    pub failures: u64,
    pub oom_kills: u64,
    pub quota_denials: u64,
    /// Fraction of all measured wall-clock time, `0.0..=1.0`.
    pub share: f64,
    pub runner_labels: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoRollup {
    pub forge: Forge,
    pub repo: String,
    pub runs: u64,
    pub total_seconds: f64,
    pub failures: u64,
    pub oom_kills: u64,
    pub quota_denials: u64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRollup {
    pub workflow: String,
    pub runs: u64,
    pub total_seconds: f64,
    pub failures: u64,
    pub oom_kills: u64,
    pub quota_denials: u64,
    pub share: f64,
}

fn print_human(report: &Report) {
    println!(
        "window   {} .. {}",
        report.window.since.to_rfc3339(),
        report.window.until.to_rfc3339()
    );
    println!("repos    {}", report.repos.join(", "));
    println!(
        "total    {} measured runs, {:.1} h",
        report.total_runs,
        report.total_seconds / 3600.0
    );
    if report.pathway_count == 0 {
        println!("\nNo timed job/step records in this window.");
    } else {
        println!(
            "\npathways (top {} of {} by wall-clock time)",
            report.pathways.len(),
            report.pathway_count
        );
        print_table(
            &[
                "forge", "repo", "workflow", "job/step", "runs", "total_h", "mean_h", "p95_h",
                "max_h", "fail", "oom", "quota", "share",
            ],
            report
                .pathways
                .iter()
                .map(|row| {
                    vec![
                        row.forge.as_str().to_owned(),
                        row.repo.clone(),
                        row.workflow.clone(),
                        row.unit.clone(),
                        row.runs.to_string(),
                        hours(row.total_seconds),
                        hours(row.mean_seconds),
                        hours(row.p95_seconds),
                        hours(row.max_seconds),
                        row.failures.to_string(),
                        row.oom_kills.to_string(),
                        row.quota_denials.to_string(),
                        percent(row.share),
                    ]
                })
                .collect(),
            5,
        );
    }
    if !report.repo_rollup.is_empty() {
        println!("\nper repository");
        print_table(
            &[
                "forge", "repo", "runs", "total_h", "fail", "oom", "quota", "share",
            ],
            report
                .repo_rollup
                .iter()
                .map(|row| {
                    vec![
                        row.forge.as_str().to_owned(),
                        row.repo.clone(),
                        row.runs.to_string(),
                        hours(row.total_seconds),
                        row.failures.to_string(),
                        row.oom_kills.to_string(),
                        row.quota_denials.to_string(),
                        percent(row.share),
                    ]
                })
                .collect(),
            2,
        );
    }
    if !report.workflow_rollup.is_empty() {
        println!("\nper workflow");
        print_table(
            &["workflow", "runs", "total_h", "fail", "oom", "quota", "share"],
            report
                .workflow_rollup
                .iter()
                .map(|row| {
                    vec![
                        row.workflow.clone(),
                        row.runs.to_string(),
                        hours(row.total_seconds),
                        row.failures.to_string(),
                        row.oom_kills.to_string(),
                        row.quota_denials.to_string(),
                        percent(row.share),
                    ]
                })
                .collect(),
            1,
        );
    }
    if !report.warnings.is_empty() {
        println!();
        for warning in &report.warnings {
            println!("warning: {warning}");
        }
    }
}

fn hours(seconds: f64) -> String {
    format!("{:.1}", seconds / 3600.0)
}

fn percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

/// Print a compact left/right aligned table, truncating long names.
fn print_table(headers: &[&str], rows: Vec<Vec<String>>, right_from: usize) {
    // Only left-aligned name columns are capped; numbers are never cut.
    let max_width = |index: usize| if index < right_from { 38 } else { usize::MAX };
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            let max = max_width(index);
            widths[index] = widths[index].max(cell.chars().count().min(max));
        }
    }
    let render = |cells: &[String]| {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            let max = max_width(index);
            let cell = truncate(cell, max);
            if index > 0 {
                line.push_str("  ");
            }
            if index < right_from {
                line.push_str(&format!("{cell:<width$}", width = widths[index]));
            } else {
                line.push_str(&format!("{cell:>width$}", width = widths[index]));
            }
        }
        line.trim_end().to_owned()
    };
    let header_cells: Vec<String> = headers.iter().map(|header| (*header).to_owned()).collect();
    println!("{}", render(&header_cells));
    let separator: Vec<String> = widths
        .iter()
        .take(headers.len())
        .map(|width| "-".repeat(*width))
        .collect();
    println!("{}", render(&separator));
    for row in rows {
        println!("{}", render(&row));
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max.saturating_sub(2);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}..", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seconds: f64, workflow: &str, unit: &str) -> Sample {
        Sample {
            forge: Forge::Github,
            repo: "caniko/simit".into(),
            workflow: workflow.into(),
            unit: unit.into(),
            unit_kind: UnitKind::Job,
            seconds: Some(seconds),
            failed: false,
            oom_killed: false,
            quota_denied: false,
            runner: Some("ubuntu-24.04".into()),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[test]
    fn window_accepts_relative_and_calendar_bounds() {
        let window = Window::parse("30d", None, now()).unwrap();
        assert_eq!(window.until, now());
        assert_eq!((window.until - window.since).num_days(), 30);

        let window = Window::parse("2026-10-01", Some("2026-10-08"), now()).unwrap();
        assert_eq!(window.since.to_string(), "2026-10-01 00:00:00 UTC");
        assert_eq!(window.until.to_string(), "2026-10-09 00:00:00 UTC");
        let window = Window::parse("30d", Some("7d"), now()).unwrap();
        assert_eq!((window.until - window.since).num_days(), 23);
        assert!(Window::parse("2026-10-09", Some("2026-10-01"), now()).is_err());
        assert!(Window::parse("nope", None, now()).is_err());
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let mut durations = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&mut durations, 95), 10.0);
        let mut durations = vec![5.0];
        assert_eq!(percentile(&mut durations, 95), 5.0);
        assert_eq!(percentile(&mut [], 95), 0.0);
    }

    #[test]
    fn report_sorts_by_total_time_and_computes_rollups() {
        let mut builder = Builder::default();
        builder.extend(vec![
            sample(100.0, "CI", "test"),
            sample(300.0, "CI", "test"),
            sample(200.0, "CI", "clippy"),
            {
                let mut row = sample(50.0, "Release", "publish");
                row.failed = true;
                row
            },
        ]);
        let report = builder.finish(
            Window::parse("30d", None, now()).unwrap(),
            vec!["github:caniko/simit".into()],
            2,
        );
        assert_eq!(report.pathway_count, 3);
        assert_eq!(report.pathways.len(), 2);
        assert_eq!(report.pathways[0].unit, "test");
        assert_eq!(report.pathways[0].runs, 2);
        assert_eq!(report.pathways[0].total_seconds, 400.0);
        assert_eq!(report.pathways[0].mean_seconds, 200.0);
        assert_eq!(report.pathways[0].p95_seconds, 300.0);
        assert_eq!(report.pathways[0].max_seconds, 300.0);
        assert!((report.pathways[0].share - 400.0 / 650.0).abs() < 0.001);
        assert_eq!(report.total_seconds, 650.0);
        assert_eq!(report.total_runs, 4);
        assert_eq!(report.repo_rollup.len(), 1);
        assert_eq!(report.workflow_rollup[0].workflow, "CI");
        assert_eq!(report.workflow_rollup[0].total_seconds, 600.0);
    }

    #[test]
    fn quota_rows_do_not_count_as_timed_runs() {
        let mut builder = Builder::default();
        builder.extend(vec![Sample {
            forge: Forge::Crow,
            repo: "caniko/bekiper".into(),
            workflow: "qualify-flake-check".into(),
            unit: "(not started)".into(),
            unit_kind: UnitKind::Pipeline,
            seconds: None,
            failed: false,
            oom_killed: false,
            quota_denied: true,
            runner: None,
        }]);
        let report = builder.finish(
            Window::parse("30d", None, now()).unwrap(),
            vec!["crow:caniko/bekiper".into()],
            20,
        );
        assert_eq!(report.total_runs, 0);
        assert_eq!(report.pathways[0].runs, 0);
        assert_eq!(report.pathways[0].quota_denials, 1);
        assert_eq!(report.pathways[0].share, 0.0);
    }

    #[test]
    fn repo_inputs_are_validated() {
        assert_eq!(
            parse_repos(&["github:caniko/simit".into()]).unwrap()[0].display(),
            "github:caniko/simit"
        );
        assert!(parse_repos(&["gitlab:caniko/simit".into()]).is_err());
        assert!(parse_repos(&["github:../escape".into()]).is_err());
        assert!(parse_repos(&["github:caniko/simit".into(), "github:caniko/simit".into()]).is_err());
    }
}
