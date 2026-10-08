//! Trusted, provider-neutral PR policy check generation. Toolbelt owns the engine.
use crate::{project::GeneratedFile, render::ci};
use anyhow::{Result, ensure};
use serde::Deserialize;

pub const PATH: &str = ".github/workflows/review-policy.yaml";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub toolbelt_version: String,
    pub app_id_secret: String,
    pub app_private_key_secret: String,
    pub credential_environment: String,
    pub policy_path: Option<String>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        let version = semver::Version::parse(&self.toolbelt_version)?;
        ensure!(
            version.pre.is_empty()
                && version.build.is_empty()
                && version >= semver::Version::new(0, 2, 0),
            "review_policy requires an exact stable toolbelt release >= 0.2.0"
        );
        for secret in [&self.app_id_secret, &self.app_private_key_secret] {
            ensure!(
                !secret.is_empty()
                    && secret.len() <= 100
                    && !secret.starts_with("GITHUB_")
                    && secret
                        .as_bytes()
                        .first()
                        .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
                    && secret
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "invalid review_policy secret name"
            );
        }
        ensure!(
            self.app_id_secret != self.app_private_key_secret,
            "review_policy secrets must be distinct"
        );
        ensure!(
            !self.credential_environment.is_empty()
                && self.credential_environment.len() <= 100
                && self
                    .credential_environment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
            "review_policy requires a safe credential_environment with default-branch-only deployment rules and environment-only App secrets"
        );
        if let Some(path) = &self.policy_path {
            ensure!(
                !path.is_empty()
                    && path.len() <= 240
                    && path.split('/').all(|p| !p.is_empty()
                        && p != "."
                        && p != ".."
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))),
                "review_policy policy_path must be a safe relative path"
            );
        }
        Ok(())
    }
}

pub fn file(config: &Config) -> Result<GeneratedFile> {
    config.validate()?;
    // Validated paths are quoted as YAML values, never executable shell text.
    let policy = format!("\"{}\"", config.policy_path.as_deref().unwrap_or(""));
    let content = TEMPLATE
        .replace("%MARKER%", ci::GENERATED_WORKFLOW_MARKER)
        .replace(
            "%CHECKOUT%",
            &ci::github_action_ref("actions/checkout", "v4.3.1"),
        )
        .replace(
            "%APP_ACTION%",
            &ci::github_action_ref("actions/create-github-app-token", "v2"),
        )
        .replace("%APP_ID%", &config.app_id_secret)
        .replace("%APP_KEY%", &config.app_private_key_secret)
        .replace("%CREDENTIAL_ENVIRONMENT%", &config.credential_environment)
        .replace("%VERSION%", &config.toolbelt_version)
        .replace("%POLICY%", &policy);
    Ok(GeneratedFile {
        relative_path: PATH.into(),
        content,
    })
}

const TEMPLATE: &str = r#"%MARKER%
name: Review policy coordinator
on:
  schedule:
    - cron: '*/10 * * * *'
  pull_request_target:
    types: [opened, synchronize, reopened, ready_for_review, edited]
  issue_comment:
    types: [created, edited, deleted]
  workflow_dispatch:
    inputs:
      pr_number:
        description: Pull request to reconcile
        required: true
        type: string
permissions:
  contents: read
  pull-requests: read
concurrency:
  group: review-policy-${{ github.event_name }}-${{ github.event.pull_request.number || github.event.issue.number || inputs.pr_number || 'sweep' }}
  cancel-in-progress: ${{ github.event_name != 'schedule' }}
jobs:
  resolve:
    # Dispatch on a source branch must not access the policy App credential.
    if: ${{ github.ref == format('refs/heads/{0}', github.event.repository.default_branch) && (github.event_name != 'issue_comment' || github.event.issue.pull_request) }}
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    outputs:
      batches: ${{ steps.prs.outputs.batches }}
    steps:
      - id: prs
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          node <<'NODE'
          const fs = require('node:fs');
          (async () => {
            const event = JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'));
            const repo = process.env.GITHUB_REPOSITORY;
            if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('invalid repository');
            let prs;
            if (process.env.GITHUB_EVENT_NAME === 'schedule') {
              const base = `https://api.github.com/repos/${repo}/pulls?state=open&sort=created&direction=asc&per_page=100`;
              prs = [];
              // Every run restarts a complete bounded enumeration. Delayed or
              // skipped schedules cannot strand a page behind a clock cursor.
              for (let page = 1; page <= 50; page++) {
                const response = await fetch(`${base}&page=${page}`, {
                  headers: {Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json'}
                });
                if (!response.ok) throw new Error(`PR enumeration failed: ${response.status}`);
                const items = await response.json();
                if (!Array.isArray(items)) throw new Error('invalid PR collection');
                prs.push(...items.map(pr => pr.number));
                if (!response.headers.get('link')?.includes('rel="next"')) break;
                if (page === 50) throw new Error('sweep exceeds supported 5000-PR bound; no partial qualification');
              }
            } else {
              prs = [Number(event.pull_request?.number || event.issue?.number || event.inputs?.pr_number)];
            }
            if (!prs.every(n => Number.isSafeInteger(n) && n > 0)) throw new Error('invalid PR identity');
            prs = [...new Set(prs)];
            const batches = [];
            for (let start = 0; start < prs.length; start += 20) batches.push(prs.slice(start, start + 20));
            fs.appendFileSync(process.env.GITHUB_OUTPUT, `batches=${JSON.stringify(batches)}\n`);
          })().catch(error => { console.error(error.message); process.exit(1); });
          NODE
  evaluate:
    needs: resolve
    if: ${{ needs.resolve.outputs.batches != '[]' }}
    strategy:
      fail-fast: false
      max-parallel: 2
      matrix:
        batch: ${{ fromJSON(needs.resolve.outputs.batches) }}
    runs-on: ubuntu-24.04
    # App secrets exist only in this environment, whose platform deployment rules
    # must restrict access to the default branch. Repository-level copies are unsafe.
    environment: "%CREDENTIAL_ENVIRONMENT%"
    timeout-minutes: 45
    steps:
      - name: Checkout trusted policy only
        uses: %CHECKOUT%
        with:
          ref: ${{ github.sha }}
          persist-credentials: false
      - name: Acquire dedicated policy App credential
        id: policy-token
        uses: %APP_ACTION%
        with:
          app-id: ${{ secrets.%APP_ID% }}
          private-key: ${{ secrets.%APP_KEY% }}
          permission-checks: write
          permission-contents: read
          permission-pull-requests: read
          permission-issues: read
          permission-metadata: read
      - name: Install immutable registry engine
        run: cargo install canix-toolbelt --version =%VERSION% --locked --features cli --root "$RUNNER_TEMP/toolbelt"
      - name: Reconcile and publish revision-bound review evidence
        env:
          GH_TOKEN: ${{ steps.policy-token.outputs.token }}
          PR_BATCH: ${{ toJSON(matrix.batch) }}
          POLICY_PATH: %POLICY%
          RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}
        run: |
          node <<'NODE'
          const {spawnSync} = require('node:child_process');
          (async () => {
            const repo = process.env.GITHUB_REPOSITORY;
            const batch = JSON.parse(process.env.PR_BATCH);
            if (!/^[\w.-]+\/[\w.-]+$/.test(repo) || !Array.isArray(batch) || batch.length > 20 || !batch.every(n => Number.isSafeInteger(n) && n > 0)) throw new Error('invalid PR batch');
            let failed = false;
            for (const number of batch) {
              try {
                const response = await fetch(`https://api.github.com/repos/${repo}/pulls/${number}`, {
                  headers: {Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json'}
                });
                if (!response.ok) throw new Error(`PR lookup failed: ${response.status}`);
                const pr = await response.json();
                if (!/^[a-f0-9]{40}$/.test(pr.head.sha)) throw new Error('invalid candidate SHA');
                const args = ['review', 'gate', '--pr', `https://github.com/${repo}/pull/${number}`, '--expected-head', pr.head.sha,
                  '--timeout-seconds', process.env.GITHUB_EVENT_NAME === 'schedule' ? '0' : '600', '--publish-check', '--details-url', process.env.RUN_URL];
                if (process.env.POLICY_PATH) args.push('--policy', `${process.env.GITHUB_WORKSPACE}/${process.env.POLICY_PATH}`);
                const result = spawnSync(`${process.env.RUNNER_TEMP}/toolbelt/bin/canix-toolbelt`, args, {stdio:'inherit', timeout:660000});
                if (result.error || result.status !== 0) failed = true;
              } catch (error) { console.error(`PR ${number}: ${error.message}`); failed = true; }
            }
            if (failed) process.exitCode = 1;
          })().catch(error => { console.error(error.message); process.exit(1); });
          NODE
"#;
