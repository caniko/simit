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
    // The relative path is validated before inclusion in shell syntax.
    let policy = config.policy_path.as_ref().map_or(String::new(), |path| {
        format!(" --policy \"$GITHUB_WORKSPACE/{path}\"")
    });
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
  cancel-in-progress: true
jobs:
  resolve:
    # Dispatch on a source branch must not access the policy App credential.
    if: ${{ github.ref == format('refs/heads/{0}', github.event.repository.default_branch) && (github.event_name != 'issue_comment' || github.event.issue.pull_request) }}
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    outputs:
      prs: ${{ steps.prs.outputs.prs }}
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
              const response = await fetch(`https://api.github.com/repos/${repo}/pulls?state=open&per_page=100`, {
                headers: {Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json'}
              });
              if (!response.ok) throw new Error(`PR enumeration failed: ${response.status}`);
              const items = await response.json();
              if (items.length >= 100) throw new Error('PR sweep exceeds bound; dispatch individual PRs');
              prs = items.map(pr => pr.number);
            } else {
              prs = [Number(event.pull_request?.number || event.issue?.number || event.inputs?.pr_number)];
            }
            if (!prs.every(n => Number.isSafeInteger(n) && n > 0)) throw new Error('invalid PR identity');
            fs.appendFileSync(process.env.GITHUB_OUTPUT, `prs=${JSON.stringify(prs)}\n`);
          })().catch(error => { console.error(error.message); process.exit(1); });
          NODE
  evaluate:
    needs: resolve
    if: ${{ needs.resolve.outputs.prs != '[]' }}
    strategy:
      fail-fast: false
      max-parallel: 2
      matrix:
        pr: ${{ fromJSON(needs.resolve.outputs.prs) }}
    concurrency:
      group: review-policy-pr-${{ matrix.pr }}
      cancel-in-progress: true
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - name: Resolve authoritative PR identity
        id: candidate
        env:
          GH_TOKEN: ${{ github.token }}
          PR_NUMBER: ${{ matrix.pr }}
        run: |
          node <<'NODE'
          const fs = require('node:fs');
          (async () => {
            const event = JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'));
            const number = Number(process.env.PR_NUMBER);
            const repo = process.env.GITHUB_REPOSITORY;
            if (!Number.isSafeInteger(number) || number <= 0 || !/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('invalid PR identity');
            const response = await fetch(`https://api.github.com/repos/${repo}/pulls/${number}`, {
              headers: {Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json'}
            });
            if (!response.ok) throw new Error(`PR lookup failed: ${response.status}`);
            const pr = await response.json();
            if (!/^[a-f0-9]{40}$/.test(pr.head.sha)) throw new Error('invalid candidate SHA');
            fs.appendFileSync(process.env.GITHUB_OUTPUT, `url=https://github.com/${repo}/pull/${number}\nhead=${pr.head.sha}\n`);
          })().catch(error => { console.error(error.message); process.exit(1); });
          NODE
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
          PR_URL: ${{ steps.candidate.outputs.url }}
          EXPECTED_HEAD: ${{ steps.candidate.outputs.head }}
          RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}
        run: |
          "$RUNNER_TEMP/toolbelt/bin/canix-toolbelt" review gate --pr "$PR_URL" --expected-head "$EXPECTED_HEAD" --timeout-seconds 600 --publish-check --details-url "$RUN_URL"%POLICY%
"#;
