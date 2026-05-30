# Project Registry

Simit keeps a per-user project registry at `$XDG_DATA_HOME/simit/projects.toml`,
or the platform default data directory when `XDG_DATA_HOME` is unset. Mutating
commands update this registry after their primary work succeeds, so registry
failures warn but do not fail the command that generated project files.

The registry records each project path, package name, first and last time simit
saw it, and feature status for known surfaces such as `flake`, `ci`,
`homebrew`, `chocolatey`, `scoop`, `changelog`, and `hooks`.

For hooks, the registry distinguishes `configured`, `installed`,
`conflicted`, and `absent`. `configured` means hook config exists but Git would
not currently execute the project's managed wrappers. `installed` means the
resolved hook path would execute the managed wrappers. `conflicted` means hook
config exists, but the effective or repo-local `core.hooksPath` bypasses the
managed route and needs attention.

For CI, the registry distinguishes `managed`, `managed+extra`, `hand-rolled`,
`drift`, and `absent`. `hand-rolled` means workflow YAML exists under
`.forgejo/workflows/` or `.github/workflows/` but does not carry simit's
generated-workflow marker. `managed+extra` means simit's generated workflows are
current and supplementary non-generated workflow files are also present.
`drift` means a marked generated workflow no longer matches what simit would
render for the project.

CI drift detection uses the same option resolution order as
`simit init ci --check --diff`:

1. CLI flags, when running `simit init ci`.
2. Explicit fields in `simit.toml` `[ci]`.
3. Inference from existing generated workflow content.
4. Generator defaults.

That means a project with persisted `[ci]` settings should report the same
result from `simit projects list` and from a bare
`simit init ci --platform <forgejo|github> --check --diff`. Repositories that
have not adopted `simit.toml` still get best-effort zero-config drift detection
from the generated workflow content, but persisted `[ci]` fields win when they
disagree with inference.

## Onboarding existing projects

Run discovery once per machine to populate the registry from projects simit
already manages:

```sh
# one-shot per machine
simit projects discover ~/Projects --dry-run
simit projects discover ~/Projects
```

`discover` walks a filesystem subtree and adds new registry entries it finds.
`scan` refreshes feature status for projects that are already registered; use
`simit projects scan --prune` afterwards if you also want stale paths removed.
Pass `--include-empty` when you want to register Cargo workspaces that do not
currently use simit; human output marks those entries with `[no simit features]`
and `projects list` shows them with a `-` indicator. Default human discovery
output prints only the skipped count; use `--json` when you need the full list
of skipped paths.

When a registered project path no longer exists, `scan` reports it instead of
silently leaving stale feature state behind. Use `--prune` to delete those
entries after reviewing them.

List registered projects:

```sh
simit projects list
```

By default, `projects list` skips ephemeral `/tmp/...` registry entries left
behind by scratch test runs. Pass `--include-ephemeral` when you need to see
them again.

Filter by feature status:

```sh
simit projects list --feature flake=managed
simit projects list --feature ci=drift
simit projects list --feature ci=hand-rolled
```

Omitting `=STATUS` matches any non-absent status:

```sh
simit projects list --feature homebrew
```

Use JSON output when another tool or an AI agent needs structured state:

```sh
simit projects list --json --feature ci=drift
simit projects list --json --feature ci=hand-rolled
simit projects show --json .
```

Human `simit projects show <path>` output also includes a `regen:` line for
generated CI projects. When `simit.toml [ci]` already captures the current CI
shape, the hint stays minimal:

```sh
regen: simit init ci --platform forgejo
```

Legacy projects without persisted `[ci]` options print the inferred flag set
instead so the command still reproduces the current workflows.

Refresh feature detection across registered projects:

```sh
simit projects scan
simit projects scan --prune
```

Use `--dry-run` to inspect a scan or prune without changing the registry:

```sh
simit projects scan --dry-run --prune
simit projects prune --dry-run
```

Clear all registry state when you want to rebuild it from discovery:

```sh
simit projects clear-state --dry-run
simit projects clear-state
```

Remove one entry when you no longer want simit to track it:

```sh
simit projects forget /absolute/project/path
```

## Disabling the registry

Set `SIMIT_NO_REGISTRY=1` to skip all registry IO. Sandboxed CI runs
that should not leak per-project state into a shared home directory
should export this. Registry IO failures already warn-and-continue, so
this env var is only needed when you want zero filesystem touches.

## Writing integration tests

Integration tests under `tests/` route through the shared
`tests/common/` harness, which points `XDG_DATA_HOME` at a per-test
temporary directory so the developer's real registry is never touched.
New tests that exercise mutating commands should use the same harness
and avoid spawning `simit` with the inherited environment.
