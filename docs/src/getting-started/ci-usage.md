# CI usage metrics

`simit ci usage` ranks CI workflows and jobs/steps by wall-clock time in a
window. Its provider API calls are read-only.

```sh
simit ci usage \
  --repo github:caniko/simit \
  --repo crow:caniko/bekiper \
  --since 30d
```

## Inputs

| Flag      | Meaning                                                                           |
| --------- | --------------------------------------------------------------------------------- |
| `--repo`  | `github:owner/name` or `crow:owner/name`; repeat for several repositories.        |
| `--since` | `YYYY-MM-DD` (UTC start of day) or `Nd`/`Nh`/`Nw`; default `30d`.                 |
| `--until` | `YYYY-MM-DD` (exclusive next midnight) or `Nd`/`Nh`/`Nw` before now; default now. |
| `--top`   | Pathway rows to report, default 20.                                               |
| `--json`  | Emit the stable machine-readable schema below.                                    |

The command reads credentials as follows and never prints them:

- **GitHub**: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token`. Set
  `GITHUB_API_URL` for GitHub Enterprise Server.
- **Crow**: `CROW_SERVER` (origin, for example `https://ci.codefloe.com`) and
  `CROW_TOKEN` (Bearer). The API base is `<CROW_SERVER>/api/v1`.

## What a pathway is

- **GitHub Actions**: one row per completed job, with runner labels, conclusion,
  workflow name from the run, and `completed_at - started_at` duration.
  `failure`, `timed_out`, and `startup_failure` increase `failures`. Without a
  portable GitHub OOM marker, `oom_kills` stays zero.
- **Crow**: one row per executed step, grouped by workflow name, with duration
  `finished - started`. Workflows without timed children fall back to `(workflow)`. Workflow errors
  identify OOM kills, attributed to the failed step (normally exit 137). CI quota
  refusals count as `quota_denials`; other scheduling errors (such as unmatched
  agent labels) count as failures. Both appear as `(not started)` rows, not timed runs.

`runs` and duration statistics exclude queued, running, skipped, and cancelled
records without both timestamps. `share` is the row's fraction of all measured
wall-clock time in the report, before `--top` truncation. `p95` uses nearest rank.

Rows sort by descending total wall-clock time. Repository and workflow rollups
(keyed by forge, repository, and workflow) cover every row; `--top` truncates
only the pathway table.

## Limits and errors

- GitHub caps filtered workflow runs at 1000 results. Simit halves busy ranges
  until they fit and warns with retrieved and matched counts if a range of an
  hour or less still exceeds the cap. Jobs include every re-run attempt (`filter=all`).
- Pagination caps: GitHub runs 50 pages, GitHub jobs 10 pages per run, Crow
  pipelines 20 pages. Hitting a cap returns an error naming the repository.
- Simit retries transient timeouts, HTTP 429/5xx responses, and short
  `Retry-After` values at most twice. Exhausted rate limits fail immediately with
  the reset time. Messages never include tokens.
- Authentication and 404 responses carry provider-specific hints (token source,
  repository access, `CROW_SERVER`).

## JSON schema

`schema_version` is `1`. Fields are stable; new optional fields may be added.

```json
{
  "schema_version": 1,
  "generated_at": "2026-11-09T06:00:00Z",
  "window": {
    "since": "2026-10-10T00:00:00Z",
    "until": "2026-11-09T06:00:00Z"
  },
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

`pathways` has up to `top_limit` rows; `pathway_count` counts all rows before
truncation. `share` is a `0.0..=1.0` fraction. `unit_kind` is `job` (GitHub),
`step` (Crow), or `pipeline` (Crow record that never started).
