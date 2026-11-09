# CI usage metrics

`simit ci usage` answers one question: which CI workflows and jobs/steps
consumed the most wall-clock time in a window? It only reads provider APIs; it
never changes a repository, workflow or run.

```sh
simit ci usage \
  --repo github:caniko/simit \
  --repo crow:caniko/bekiper \
  --since 30d
```

## Inputs

| Flag            | Meaning                                                                    |
| --------------- | -------------------------------------------------------------------------- |
| `--repo`        | `github:owner/name` or `crow:owner/name`; repeat for several repositories. |
| `--since`       | `YYYY-MM-DD` (UTC start of day) or `Nd`/`Nh`/`Nw`; default `30d`.          |
| `--until`       | `YYYY-MM-DD` (exclusive next midnight) or `Nd`/`Nh`/`Nw` before now; default now. |
| `--top`         | Pathway rows to report, default 20.                                        |
| `--json`        | Emit the stable machine-readable schema below.                             |

Credentials come from the environment and are never printed:

- **GitHub**: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token`. Set
  `GITHUB_API_URL` for GitHub Enterprise Server.
- **Crow**: `CROW_SERVER` (origin, for example `https://ci.codefloe.com`) and
  `CROW_TOKEN` (Bearer). The API base is `<CROW_SERVER>/api/v1`.

## What a pathway is

- **GitHub Actions**: one row per completed job. Duration is
  `completed_at - started_at`; the workflow name comes from the run. Runner
  labels and the conclusion are recorded. `failure`, `timed_out` and
  `startup_failure` count as failures. GitHub exposes no portable OOM marker, so
  `oom_kills` stays zero on GitHub; timeouts are visible through `failures`.
- **Crow**: one row per executed step, grouped by workflow name; a workflow
  with no timed children falls back to `(workflow)`. Step durations come from
  `finished - started`. OOM-killed containers are detected from the workflow
  error (and attributed to the failed step, normally exit 137). Workflows the CI
  quota refused count as `quota_denials`; other scheduling failures (for example
  no agent matching the labels) count as failures. Both appear as `(not started)`
  rows and do not count as timed runs.

Queued, running, skipped and cancelled records without both timestamps are
excluded from `runs` and duration statistics. `share` is the fraction of all
measured wall-clock time in the report. `p95` uses the nearest-rank method.

Rows are sorted by total wall-clock time descending. Per-repository and
per-workflow rollups (keyed by forge, repository and workflow) cover every
collected row; `--top` only truncates the
pathway table.

## Limits and errors

- GitHub caps a filtered workflow-run listing at 1000 results, so a busier range
  is split in half until each part fits. Jobs include every attempt of a
  re-run (`filter=all`). A range of an hour or less that still exceeds the cap is
  reported as a warning with the retrieved and matched counts.
- Pagination is bounded: GitHub runs 50 pages, GitHub jobs 10 pages per run,
  Crow pipelines 20 pages. Hitting a bound is an explicit error that names the
  repository.
- Transient timeouts, HTTP 429/5xx responses and short `Retry-After` values are
  retried at most twice. Exhausted rate limits fail immediately with the reset
  time; tokens are never included in messages.
- Authentication and 404 responses carry provider-specific hints (token source,
  repository access, `CROW_SERVER`).

## JSON schema

`schema_version` is `1`. Fields are stable; new optional fields may be added.

```json
{
  "schema_version": 1,
  "generated_at": "2026-11-09T06:00:00Z",
  "window": { "since": "2026-10-10T00:00:00Z", "until": "2026-11-09T06:00:00Z" },
  "top_limit": 20,
  "repos": ["github:caniko/simit", "crow:caniko/bekiper"],
  "total_runs": 1200,
  "total_seconds": 345600.0,
  "pathway_count": 84,
  "pathways": [
    {
      "forge": "github",
      "repo": "caniko/simit",
      "workflow": "CI",
      "unit": "test (ubuntu-24.04)",
      "unit_kind": "job",
      "runs": 42,
      "total_seconds": 75600.0,
      "mean_seconds": 1800.0,
      "p95_seconds": 2400.0,
      "max_seconds": 3600.0,
      "failures": 2,
      "oom_kills": 0,
      "quota_denials": 0,
      "share": 0.218,
      "runner_labels": ["ubuntu-24.04"]
    }
  ],
  "repo_rollup": [],
  "workflow_rollup": [],
  "warnings": []
}
```

`pathways` contains at most `top_limit` rows; `pathway_count` is the full row
count before truncation. `share` is a `0.0..=1.0` fraction. `unit_kind` is
`job` (GitHub), `step` (Crow) or `pipeline` (Crow record that never started).
