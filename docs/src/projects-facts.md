# Usage Facts

`simit projects facts` reports each checkout's simit usage and corresponding
upgrade paths. It prints one JSON document using the `fleetix.supply-facts.v1`
envelope and `simit.usage-facts.v1` vocabulary. Fleetix merges it with other
producers' facts (e.g. a canix updater enrollment overlay) to find gaps.

```sh
simit projects facts PATH...            # defaults to the current directory
simit projects facts --eval-flake PATH... # also evaluate flake simitConfig
```

The command is read-only: it never writes files, and neither it nor its
`init ... --check` children (which receive `SIMIT_NO_REGISTRY=1`) update the
project registry. Unobservable repositories appear in `errors`. The command
exits with 0 even on drift or observation errors, and with 2 on argument errors
or document output failures.

Each input path must be a checkout whose `origin` names its repository as
`HOST/OWNER/REPO`; that coordinate identifies the repository. Use exact
default-branch snapshots; the report reflects local edits.

## Flake configuration

Simit evaluates flake `outputs.simitConfig` only with `--eval-flake`. Without
`--eval-flake`, it reports `config-unresolved`, no process runs `nix`, and the
`init ... --check` children receive `SIMIT_NO_FLAKE_EVAL=1`, which every simit
process honors. Flake sources always report `config-not-persistable`: the
generator can't rewrite them.

## Nodes and edges

| Node            | Id                                      | Attributes                                                                                                                                                            |
| --------------- | --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| repository      | `repo:<coordinate>`                     | `forge` (`github`, `forgejo`, `gitlab`, or `unknown` for a self-hosted forge whose committed CI names no platform), `forgeHost`, `ciTarget` (`<provider>/<platform>`) |
| usage           | `usage:<coordinate>#<kind>[:<locator>]` | `usageKind`, `state`, `paths` or `path`, `revisions`, `tags`, `input`                                                                                                 |
| pinned revision | `rev:github.com/caniko/simit@<sha>`     | `source`, `revision`                                                                                                                                                  |
| runner          | `runner:<forgeHost>/<label>`            | `forgeHost`, `label`                                                                                                                                                  |
| cache           | `cache:<name>@<url>`                    | `name`, `url`                                                                                                                                                         |
| workflow        | `workflow:<coordinate>#<path>`          | `path`, `generated`, `triggers`, `labels`, `steps`                                                                                                                    |

Edges: repository `uses` usage, usage `pins` revision, usage and workflow
`runsOn` runner (Actions `runs-on` labels; `.github` workflows run on
`github.com`), usage `publishesTo` cache (`[release.attic]`), and usage
`contains` workflow (the repository when no usage owns the file).

Static parsing creates one workflow node per workflow file in `.github/workflows`,
`.forgejo/workflows`, `.crow` and `.gitlab-ci.yml`. `triggers` is `{events, branches}`:
events normalize to `push`, `pull_request`, `tag`, `manual`, `cron`, `deployment`
and `release`; other Actions events keep their names. `branches` keeps the
written push filter (empty for all branches). `branchesIgnore` lists push
exclusions when present (`branches-ignore`, Crow `branch.exclude`). Crow files
without `when` run on every event. Jsonnet and GitLab report `triggers: null`.
`labels` holds Actions `runs-on` labels (runner groups as `group=<name>`) or
Crow `labels` as `key=value`. `steps` holds step names. Each per-file reference
usage carries `generated`, true inside simit-generated workflows.

Generated usages expect `generatorRevision` from a receipt or pin only
(`unknown-generator-revision`); repository-variable usages expect the
variable's `revision`.

### Usage kinds

| Kind                                                                                              | Source                                                                                                                                             |
| ------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `config.simit-toml`, `config.cargo-workspace`, `config.cargo-package`, `config.flake`             | config sources                                                                                                                                     |
| `ci.primary`, `ci.publish`, `ci.pages`, `ci.review`, `ci.custody`, `ci.template`, `ci.local-gate` | files `simit init ci` renders, compared as `simit projects audit` does                                                                             |
| `release.workflow`, `dist.aur`, `dist.copr`, `dist.apt`                                           | `simit init release` / `init <channel> --check`                                                                                                    |
| `flake.wiring`, `hooks.config`                                                                    | `simit init flake` outputs                                                                                                                         |
| `readme.badges`                                                                                   | the README badge block `simit upgrade` maintains                                                                                                   |
| `keys.trust-root`                                                                                 | the release trust root                                                                                                                             |
| `flake-input.direct`, `flake-input.transitive`, `flake-input.cut`                                 | `flake.lock` inputs locking `caniko/simit`; `cut` is a `simit` input that follows nothing                                                          |
| `runtime.publisher-var`, `runtime.ci-engine`                                                      | `SIMIT_PUBLISHER_REVISION` / `SIMIT_CI_ENGINE_REVISION` in workflows (a committed `\|\| '<sha>'` fallback is a revision)                           |
| `runtime.flake-ref`, `runtime.cargo-install`, `runtime.devshell`                                  | simit run from a flake reference, `cargo install` or `nix develop` in workflows and `scripts/`                                                     |
| `runtime.generated-ref`                                                                           | simit's own flake reference or `cargo install` in a simit-generated file; it follows that file, while commands copied from config stay `runtime.*` |
| `pin.config-command`                                                                              | flake references in `simit.toml` command strings                                                                                                   |
| `script.invocation`                                                                               | `simit` commands in `scripts/`                                                                                                                     |
| `cargo.dependency`                                                                                | a `simit` package in `Cargo.lock`                                                                                                                  |

A reference whose revision comes from another variable
(`github:caniko/simit/$VAR`) records that variable in `variables` and expects a
`revision`. Generated release workflows missing
their trust root report `keys.trust-root` as `drift` with `trust-root-missing`
as the blocker. Unreadable lock files and `scripts/` directories report `errors`,
not empty results.

States are `current`, `drift`, `hand-rolled`, `absent` and `unknown`. Pins and
runtime references report `unknown`; compare their `pins` edges against the
orchestrator's generator revision for freshness. Floating references add
the `unpinned-runtime` blocker.

### Repository blockers

| Blocker                  | Meaning                                                             |
| ------------------------ | ------------------------------------------------------------------- |
| `provider-mismatch`      | the CI target cannot run on the forge, such as Crow on `github.com` |
| `config-conflict`        | several config sources                                              |
| `config-not-persistable` | the config comes from a flake                                       |
| `config-unresolved`      | a flake config was not, or could not be, evaluated                  |

## Capability matrix

`capabilities` lists every upgrade path known to simit:
`{usageKind, match, path, via, lands, orchestrator?, status, blocker?, reason,
fallback?}`. Fleetix picks the most specific `match` against repository
attributes (`forge`, `ciTarget`, and overlay `landing`). A row covers a usage
only if it is `implemented` and `lands`; otherwise `blocker` explains the gap (`no-orchestrator`,
`adapter-missing`, `scope-unsupported`, `pr-required-unsupported`,
`provider-mismatch`, `manual-pin`, `transitive-pin`).

A unit test enforces that updater rows are `implemented` exactly when scope,
adapter and landing appear in `SUPPORTED_SCOPES`, `UPDATER_ADAPTERS` and
`SUPPORTED_LANDINGS` (`src/commands/projects_update/schema.rs`). A change that
adds updater support flips the corresponding rows to `implemented`; verify that
status and refresh `tests/fixtures/supply-facts/simit-usage-facts.v1.json` with
`SIMIT_BLESS=1 cargo test --test projects_facts`.
