# Usage Facts

`simit projects facts` reports how each checkout consumes simit and which
upgrade path covers each usage. It prints one JSON document in the
`fleetix.supply-facts.v1` envelope with the `simit.usage-facts.v1`
vocabulary. Fleetix merges it with other producers' facts (such as a canix
overlay of updater enrollment) and computes the gaps.

```sh
simit projects facts PATH...            # defaults to the current directory
simit projects facts --eval-flake PATH... # also evaluate flake simitConfig
```

The command is read-only. It never writes files or the project registry, and
its `init ... --check` children run with `SIMIT_NO_REGISTRY=1`. A repository
that cannot be observed becomes an entry in `errors`; the exit code stays 0,
also when usages drift. Argument errors and failures to write the document
exit with 2.

Each path must be a checkout whose `origin` remote names `HOST/OWNER/REPO`;
that coordinate identifies the repository. Run it on exact default-branch
snapshots: local edits are reported as they are.

## Flake configuration

A flake `outputs.simitConfig` is evaluated only with `--eval-flake`. Without it
the repository reports `config-unresolved`, and nothing runs `nix`, including
the `init ... --check` children (they receive `SIMIT_NO_FLAKE_EVAL=1`, which
any simit process honours). A flake source always reports
`config-not-persistable`, because the generator cannot rewrite it.

## Nodes and edges

| Node            | Id                                      | Attributes                                                                                   |
| --------------- | --------------------------------------- | -------------------------------------------------------------------------------------------- |
| repository      | `repo:<coordinate>`                     | `forge` (`github`, `forgejo` or `gitlab`), `forgeHost`, `ciTarget` (`<provider>/<platform>`) |
| usage           | `usage:<coordinate>#<kind>[:<locator>]` | `usageKind`, `state`, `paths` or `path`, `revisions`, `tags`, `input`                        |
| pinned revision | `rev:github.com/caniko/simit@<sha>`     | `source`, `revision`                                                                         |
| runner          | `runner:<forgeHost>/<label>`            | `forgeHost`, `label`                                                                         |
| cache           | `cache:<name>`                          | `url`                                                                                        |
| workflow        | `workflow:<coordinate>#<path>`          | `path`, `generated`, `triggers`, `labels`, `steps`                                           |

Edges: repository `uses` usage, usage `pins` revision, usage and workflow
`runsOn` runner (Actions `runs-on` labels; `.github` workflows run on
`github.com`), usage `publishesTo` cache (`[release.attic]`), and usage
`contains` workflow (the repository when no usage owns the file).

Every workflow file under `.github/workflows`, `.forgejo/workflows`, `.crow`
and `.gitlab-ci.yml` becomes a workflow node from a static parse.
`triggers` is `{events, branches}`: events normalized to `push`,
`pull_request`, `tag`, `manual`, `cron`, `deployment` and `release` (other
Actions events keep their names), and `branches` the push branch filter as
written, empty when push runs on every branch. A Crow file without `when` runs
on every event. Jsonnet and GitLab files report `triggers: null`. `labels` are
Actions `runs-on` labels or Crow `labels` as `key=value`; `steps` are step
names. Per-file reference usages carry `generated`, true inside a
simit-generated workflow.

Generated usages expect a `generatorRevision` attribute that only a receipt
or pin can supply (`unknown-generator-revision`); repository-variable usages
expect the variable's `revision`.

### Usage kinds

| Kind                                                                                              | Source                                                                                                                   |
| ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `config.simit-toml`, `config.cargo-workspace`, `config.cargo-package`, `config.flake`             | config sources                                                                                                           |
| `ci.primary`, `ci.publish`, `ci.pages`, `ci.review`, `ci.custody`, `ci.template`, `ci.local-gate` | files `simit init ci` renders, compared as `simit projects audit` does                                                   |
| `release.workflow`, `dist.aur`, `dist.copr`, `dist.apt`                                           | `simit init release` / `init <channel> --check`                                                                          |
| `flake.wiring`, `hooks.config`                                                                    | `simit init flake` outputs                                                                                               |
| `readme.badges`                                                                                   | the README badge block `simit upgrade` maintains                                                                         |
| `keys.trust-root`                                                                                 | the release trust root                                                                                                   |
| `flake-input.direct`, `flake-input.transitive`, `flake-input.cut`                                 | `flake.lock` inputs locking `caniko/simit`; `cut` is a `simit` input that follows nothing                                |
| `runtime.publisher-var`, `runtime.ci-engine`                                                      | `SIMIT_PUBLISHER_REVISION` / `SIMIT_CI_ENGINE_REVISION` in workflows (a committed `\|\| '<sha>'` fallback is a revision) |
| `runtime.flake-ref`, `runtime.cargo-install`, `runtime.devshell`                                  | simit run from a flake reference, `cargo install` or `nix develop` in workflows and `scripts/`                           |
| `pin.config-command`                                                                              | flake references in `simit.toml` command strings                                                                         |
| `script.invocation`                                                                               | `simit` commands in `scripts/`                                                                                           |
| `cargo.dependency`                                                                                | a `simit` package in `Cargo.lock`                                                                                        |

States are `current`, `drift`, `hand-rolled`, `absent` and `unknown`. Pins and
runtime references report `unknown`: their freshness depends on the
orchestrator's generator revision, so compare their `pins` edges with it. A
floating reference adds the `unpinned-runtime` blocker.

### Repository blockers

| Blocker                  | Meaning                                                             |
| ------------------------ | ------------------------------------------------------------------- |
| `provider-mismatch`      | the CI target cannot run on the forge, such as Crow on `github.com` |
| `config-conflict`        | several config sources                                              |
| `config-not-persistable` | the config comes from a flake                                       |
| `config-unresolved`      | a flake config was not, or could not be, evaluated                  |

## Capability matrix

`capabilities` lists every upgrade path simit knows:
`{usageKind, match, path, via, lands, orchestrator?, status, blocker?, reason,
fallback?}`. `match` keys compare with repository attributes (`forge`,
`ciTarget`, and `landing` from an overlay); fleetix uses the most specific
matching row. A row covers a usage only when it is `implemented` and `lands`;
otherwise its `blocker` explains the gap (`no-orchestrator`,
`adapter-missing`, `scope-unsupported`, `pr-required-unsupported`,
`provider-mismatch`, `manual-pin`, `transitive-pin`).

Updater rows are `implemented` exactly when their scope, adapter and landing
appear in `SUPPORTED_SCOPES`, `UPDATER_ADAPTERS` and `SUPPORTED_LANDINGS`
(`src/commands/projects_update/schema.rs`); a unit test enforces this. A change
that adds updater support therefore flips its rows and refreshes the golden
document `tests/fixtures/supply-facts/simit-usage-facts.v1.json` with
`SIMIT_BLESS=1 cargo test --test projects_facts`.
