# Project Registry

On every supported platform, Simit stores the per-user registry at
`$XDG_DATA_HOME/simit/projects.toml` if `XDG_DATA_HOME` is absolute, or in the
platform's default data directory if unset, empty, or relative. Mutating commands
update the registry after their main work succeeds. A registry-update failure
warns without failing the file-generating command.

The registry records project path, package name, first and last sightings, and
feature status for surfaces such as `flake`, `ci`,
`homebrew`, `chocolatey`, `scoop`, `changelog`, and `hooks`.

Hook states are `configured`, `installed`, `conflicted`, and `absent`.
`configured` means hook config exists but Git wouldn't run the project's managed
wrappers. `installed` means the resolved hook path would run them. `conflicted`
means config exists but effective or repo-local `core.hooksPath` bypasses the
managed route and needs attention.

CI states are `managed`, `managed+extra`, `hand-rolled`, `drift`, and `absent`.
`hand-rolled` means workflow YAML in `.forgejo/workflows/` or `.github/workflows/`
lacks simit's generated-workflow marker. `managed+extra` means current generated
workflows coexist with non-generated ones. `drift` means a marked workflow no
longer matches simit's render for the project.

CI drift detection uses the same option resolution order as
`simit init ci --check --diff`:

1. CLI flags, when running `simit init ci`.
2. Explicit fields in `simit.toml` `[ci]`.
3. Inference from existing generated workflow content.
4. Generator defaults.

With persisted `[ci]` settings, `simit projects list` should agree with a bare
`simit init ci --platform <forgejo|github> --check --diff`. Without `simit.toml`,
best-effort drift detection infers settings from generated workflows. Persisted
`[ci]` fields override inference.

## Live fleet audits

`projects list`, `show`, and `scan` use cached registry state. Use
`projects audit` to compare the current checkout against the current generator:

```sh
simit projects audit /path/to/project
simit projects audit --json /path/to/project
```

The audit never reads or writes the registry. Its stable JSON report includes
generator version, CI provider, platform and status, changed/missing/extra
generated workflow paths, a regeneration command if inferable, and per-project
errors. `provider` (`actions` or `crow`) identifies the validated CI backend.
Crow's legacy `platform` conflates provider and platform; automation should use
`provider` instead. Exit `0` means clean, `1` means attention needed, and `2`
means the comparison couldn't finish for at least one project.

`simit projects facts` covers all simit usage, not just CI, and reports which
upgrade path covers each usage. See [Usage Facts](../projects-facts.md).

## Onboarding existing projects

Run discovery once per machine to register projects simit already manages:

```sh
# one-shot per machine
simit projects discover ~/Projects --dry-run
simit projects discover ~/Projects
```

`discover` walks a filesystem subtree and adds new entries. `scan` refreshes
registered projects' feature status. Run `simit projects scan --prune` afterward
to remove stale paths too. `--include-empty` registers Cargo workspaces without
simit usage: human discovery output marks them `[no simit features]`, and
`projects list` marks them `-`. By default, human discovery output gives only
the skipped count; `--json` lists every skipped path.

`scan` reports missing registered paths. Review them before deleting their
entries with `--prune`.

List registered projects:

```sh
simit projects list
```

`projects list` hides ephemeral `/tmp/...` entries from scratch tests by
default. Show them with `--include-ephemeral`.

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

Use JSON for tools and AI agents that need structured state:

```sh
simit projects list --json --feature ci=drift
simit projects list --json --feature ci=hand-rolled
simit projects show --json .
```

Human `simit projects show <path>` output includes a `regen:` line for
generated CI projects. If `simit.toml [ci]` captures the current CI settings,
the hint is minimal:

```sh
regen: simit init ci --platform forgejo
```

Without persisted `[ci]` options, simit prints inferred flags to reproduce
the current workflows.

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

Clear the registry to rebuild it from discovery:

```sh
simit projects clear-state --dry-run
simit projects clear-state
```

Stop tracking one project:

```sh
simit projects forget /absolute/project/path
```

## Disabling the registry

In sandboxed CI where project state must not leak into a shared home directory,
export `SIMIT_NO_REGISTRY=1` or set it in the command's environment to skip all
registry IO. Registry IO failures only warn; set this variable only to avoid all
registry filesystem access.

## Writing integration tests

Integration tests in `tests/` use the shared `tests/common/` harness, which
sets `XDG_DATA_HOME` to a per-test temporary directory, so tests never touch
the developer's registry. Use the harness for new tests of mutating commands;
don't spawn `simit` with the inherited environment.
