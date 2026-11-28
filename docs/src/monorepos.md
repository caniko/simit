# Mixed-language monorepositories

A monorepository has one root Simit configuration and a project-owned Nix
`ci` development shell. Components describe source ownership, dependencies,
and required qualification commands. Package versions remain independent.

```toml
[ci]
platform = "github"
provider = "actions"
runtime = "nix"

[monorepo]
schema_version = 1
formatter_modules = ["nix/formatters/python.nix"]

[[monorepo.components]]
id = "core"
paths = ["nix/core"]
checks = [{ id = "evaluate", run = "nix eval .#lib.core.contractVersion" }]

[[monorepo.components]]
id = "rust"
paths = ["nix/rust"]
cargo_packages = ["engine"]
depends_on = ["core"]
checks = [{ id = "test", run = "cargo test --locked -p engine" }]

[[monorepo.components]]
id = "python"
paths = ["python"]
depends_on = ["core"]
checks = [{ id = "test", run = "python3 -m unittest discover -s python/tests" }]
```

Configuration comes from `simit.toml`, Cargo `[workspace.metadata.simit]` or
`[package.metadata.simit]`, or flake `outputs.simitConfig` (use one source).
Member invocations discover the root per source, stopping at Git checkout
boundaries.

## Qualification planning

```console
simit monorepo plan --json
simit monorepo plan --base origin/trunk --json
simit monorepo plan --changed-path nix/core/default.nix --json
```

The JSON contract has `schemaVersion`, `changedPaths`, `full`, `selected`, and
`reasons`. Selection includes changed owners, their transitive dependents, then
every selected component's prerequisites in deterministic dependency order.
Cargo metadata supplies member source directories and local dependency edges
(including optional, build, dev, and target-specific dependencies); every Cargo
member needs exactly one component owner. For a root `Cargo.toml` package,
declare its existing source paths explicitly (for example
`paths = ["src", "tests", "build.rs"]`); Simit adds `Cargo.toml` to that
owner's paths without claiming nested workspace members.

Unknown source paths select all components. Default shared paths are
`simit.toml`, `flake.nix`, `flake.lock`, `Cargo.toml`, `Cargo.lock`,
`rust-toolchain.toml`, `deny.toml`, and `.github` (replaceable via
`shared_paths`). Changes under `ignore_paths` (e.g. `["README.md", "docs"]`)
select nothing; configured ignored paths must not overlap owned or shared paths.
Git selection includes committed, staged, unstaged, and untracked changes
relative to the base; renames keep both old and new paths, deletions keep old
owners, and missing/invalid base revisions fail locally.

Paths must be normalized and root-relative; overlapping ownership, unknown
dependencies, cycles, duplicate package ownership, and symlink traversal are
rejected. Components may own multiple paths and Cargo packages.

A component may own a separate Cargo workspace (such as a submodule with its own
lockfile) excluded from the root workspace:

```toml
[[monorepo.components]]
id = "rust"
paths = ["components/rust"]
cargo_manifest = "components/rust/Cargo.toml"
cargo_packages = ["harbor-cache"]
```

Its `cargo_packages` resolve from that workspace. The manifest must be a
workspace root or standalone package inside the component's paths with no member
shared with the root workspace, and every member of every workspace needs an
owner. Cross-workspace path dependencies still add `depends_on` edges. Generated
crate publication workflows and trusted publishers pass that manifest to Cargo
via `--manifest-path` and `--cargo-manifest`. Local `release plan`, `verify`,
and `sync-up` target the component workspace, while version bumps belong to that
workspace's repository (`simit release <bump> --component` refuses them).

## Generated qualification CI

Monorepo workflows pin Nix **2.35.2** explicitly (including native prebuild and
crate archive prerequisites) for the upstream
[lost build-slot wakeup fix](https://github.com/NixOS/nix/commit/fba3f6e49bea5f6b7b5b75ab794e32afaabd5d26)
for the `!awake.empty()` assertion abort observed with the install action's
2.35.1 default; both the version pin and installer action remain pinned when
regenerating workflows.

`simit init ci` and `simit init ci --check` operate on the full root workflow
set from root or member directories. GitHub Actions runs the planner at the
checked-out revision and qualifies the selected components (missing push-base
objects force full qualification). Every component needs explicit checks; failed
or cancelled selected jobs fail the aggregate `qualified` gate.
`[ci].push_branches` filters branch pushes while keeping PR and manual
qualification (empty list keeps all branch pushes).

Static policy (`simit.toml` or Cargo metadata) verifies generated workflows once
in the prerequisite `plan` job before selection; flake `outputs.simitConfig`
keeps verification on each component worker. Native runner-system guards and
component checks remain on their selected workers; release qualification
retains the same static-versus-flake verification ownership.

Root `devShells.<system>.ci` must supply Simit (with component support), Cargo,
and check tools; commands run there with declared env and timeouts. Component
check `runtime` must be omitted or `"nix"`. `systems` selects native runners
via `[ci.nix_system_runners]`, and handwritten supplementary workflows are
preserved.

By default each component gets `component-<id>`; components sharing `job` run in
one `component-<job>` job using one checkout, Nix install, and store, with each
member's checks running in dependency order and guarded by that member's
selection so dependents reuse what prerequisites built. Shared-job members must
declare identical `systems` and share the 360-minute budget.

```toml
[[monorepo.components]]
id = "fleetix"
job = "libs"

[[monorepo.components]]
id = "secret-manager"
job = "libs"
```

`[ci.cache]` substituters and trusted keys apply to planner and component jobs,
as in single-project CI, and are omitted from publication workflows.

`simit init flake` resolves member invocations to the root; optional
`formatter_modules` compose into the root treefmt module while custom flakes
keep their ownership contract. Do not use language or publication overrides to
replace component qualification commands.

Set `submodules = true` when components are Git submodules so every generated
GitHub `actions/checkout` step in qualification and source publication
workflows uses `submodules: recursive` (flakes copy submodule content only when
`flake.nix` sets `inputs.self.submodules = true`). Trusted publishers then pass
`--source-submodules`, and signed-source package verification fetches
submodules under this trust model:

- The superproject's signed tag is the sole trust root, pinning every gitlink to
  an exact commit ID (submodules need no signatures of their own).
- `.gitmodules` is read from the signed tag's tree, never the working tree;
  gitlinks and declarations must match 1:1 with only `path`, `url`, and
  (ignored) `branch` keys.
- URLs must be `https://github.com/<owner>/<repository>[.git]` under the
  superproject's owner (relative, SSH, `git`, `file`, and other forms are
  rejected).
- The verifier fetches each submodule at its pinned commit, checks its `HEAD`
  equals the gitlink, and rejects nested submodules without recursing.
- Submodule trees follow superproject entry rules: regular files, executables,
  and inert symlink blobs only.

Without `submodules = true`, signed sources containing gitlinks are still
rejected.

### Components qualified by a superproject

A superproject-qualified component keeps only its publication workflows by
declaring its qualifier in the component's own `simit.toml`:

```toml
[ci]
qualified_by = "caniko/fleet"   # GitHub owner/repo of the superproject

[ci.pages]
repo = "caniko/component"
```

With `qualified_by`, `simit init ci` emits only the Pages workflow (like
`--pages-only`); `--check` flags simit-generated qualification workflows
(`ci.yaml`, `ci-<crate>.yaml`, `build*`, `nix-builds*`, `prebuild*`, `qualify*`)
as obsolete drift, and `simit init ci` deletes them while preserving handwritten
workflows. `simit upgrade` points the README CI badge at
`https://github.com/<qualified_by>/actions`, and platform defaults to GitHub.

The slug must be `owner/repo`. Because crate publishers key on qualification
workflows, `qualified_by` rejects `[monorepo]`, `[ci].nix_builds`,
`[[ci.required_gates]]`, `[ci].publish_crates`, non-GitHub platforms/providers,
and CLI `--primary-only`, `--plan-json`, or `--publish-crates`. `--review-only`
and `--prebuild-only` still run their own generators unchanged.

### Running a declared check

```console
simit monorepo check --component core --check evaluate --nix-diagnostics
```

Local checks apply configured env literals even if the calling shell defines
them, and reject provider expressions (`${{ ... }}`) before running without
printing values. Generated Actions callers use `--gate-env provider-staged`
(every gate variable must exist under its inert step alias, empty allowed, and
passes verbatim without shell expansion); direct `--gate-env provider-resolved`
reads declared names. Checks run once from project root with Bash `-euo pipefail`;
failure diagnostics never retry or mask failure.

The native runner resolves root config, validates the component/check (unknown
checks fail before running), and executes the project-owned command with
`bash -euo pipefail` and declared env. Generated qualification invokes this
runner inside the project's `ci` shell. Both local and generated checks enforce
timeouts, stopping the gate process group before diagnostics. Evaluated step
values override devShell variables; local release qualification rejects
unresolved provider expressions before running any gate.

Generated checks reserve two minutes for diagnostics after the gate deadline,
captured before `nix develop` (so provisioning counts) and capped by a job
deadline captured before checkout/install. GitHub steps/jobs cap at 360 minutes
(a 360-minute gate gets up to 358 minutes in CI; local runs without `--deadline`
keep the full timeout). Expired CI budgets collect diagnostics without starting
the gate.

Passing setup gates may keep background services for later gates; failed,
timed-out, or cancelled gates stop their process group before diagnostics. The
local CLI handles SIGINT, SIGTERM, and SIGHUP (during synchronous process spawn,
signals keep default termination until a child exists; generated callers
supervise startup with their outer deadline). If Nix provisioning fails before
native startup, the caller collects the same bounded diagnostics and returns
the provisioning status. Separate private startup and diagnostic-completion
markers allow fallback capture after abrupt termination without repeating a
completed capture; if their private directory cannot be created, the caller
stops before provisioning or traps.

Generated checks stage provider values under inert names before Bash starts,
passing them via `--gate-env provider-staged` and removing consumed staging
aliases from the gate environment. The native runner assigns declared data
after gate Bash starts, preserving `IFS` and `PWD` while scoping gate `PATH` so
Simit/Bash tool paths survive Nix sandboxes without `/bin/bash`. Graph
validation rejects read-only and dynamically managed Bash names before
execution or writes—including integer/computed names (`RANDOM`, `SRANDOM`,
`OPTIND`, `HISTCMD`, `SECONDS`, `LINENO`, `BASH_ARGV0`) and internal arrays—plus
`BASH_ENV` (gates must source setup files explicitly in their declared
command); mutable scalars like `IFS` and `PWD` remain supported (see
[Bash variable reference](https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html)).
Job Bash supervises asynchronous provisioning (including lookup/startup) in an
isolated process group on hosted and custom Nix runners, enforcing the remaining
deadline across Nix provisioning, native init (including Cargo metadata), and
gate spawn, stopping the group on timeout and reserving diagnostic time. Native
Simit claims deadline/cleanup ownership only after validation, env capture,
timer/cancellation setup, and spawn attempt; after that handoff, even spawn
failures attempt diagnostics, and the completion marker writes only if the
diagnostic script succeeds. Failed gates return their original exit code (or
`128 + signal`) through CLI and supervisor regardless of diagnostic outcome.

On Linux, `--nix-diagnostics` writes `/proc/meminfo`, memory pressure, Nix
daemon status, and bounded daemon/kernel journals (from check start, up to 150
entries each, non-interactive sudo with 15-second timeout and five-second
termination grace per command) to stderr, retained in the provider job log,
after a failed check without retrying, restarting the daemon, or dumping env;
missing tools or permissions are reported without altering the failed status.

## Independent Cargo releases

```console
simit release plan --component rust --json
simit release patch --component rust --package engine -m 'release engine'
```

Component release plans list dependency-ordered publishable packages with their
components, versions, and package-qualified tags (`publish = false` members stay
non-publishable). Bumps target one owned package and create a signed tag like
`engine/v0.2.1` (versions stay plain SemVer, using the package's adjacent
`CHANGELOG.md`; workspace-inherited versions reject for independent bumps). With
`[release.changelog].auto_draft = true`, drafting scans only component-owned
paths since the latest reachable package tag. Changelog compare links use the
same package namespace. Mutations require a clean checkout.

`[ci].publish_crates = true` emits one independently tagged workflow per
publishable Cargo member. Each `<package>/v<version>` release qualifies the
whole component graph on declared native runners, verifies the signed tag
against `keys/maintainers.gpg`, checks its exact checkout and package version,
publishes only the tagged package (requiring local deps already published), and
runs package/archive checksum and registry propagation checks. Each
credential-free, tag-only builder has a separate default-branch publisher
enrolled via `SIMIT_PUBLISHER_REVISION` that binds the complete native tag,
package namespace, original provider run, and archive checksum before upload;
registry credentials come only from the protected `simit-crates-io` environment
restricted to the exact default branch.

`simit release verify --component <id> --package <name>` checks the adjacent
changelog and namespaced tag; `simit release sync-up` takes the same selector
and keeps clean-checkout and lease-protected tag push guarantees. Shared tag
verification and sync-up require explicit component selection in
monorepositories. With `[release.notes].source = "git"`, verification scans
commits touching component-owned paths between the previous package tag and the
exact release tag (excluding unrelated components and later commits). Trust and
secrets remain repository-scoped.

## Independent Python and npm versions

Declare each static version owner explicitly (template and test-fixture
manifests are never inferred as release packages):

```toml
[[monorepo.components]]
id = "python"
paths = ["components/python"]
releases = [
  { manifest = "components/python/pyproject.toml", namespace = "py-engine", publish = false },
]
checks = [{ id = "test", run = "uv run --project components/python pytest" }]
```

`release plan`, `patch`, `minor`, `major`, `verify`, and `sync-up` all support
these owners. Pass `--package` for multi-package components (including mixed
Cargo/Python). Explicit namespaces support scoped npm names like
`@example/engine` with safe tags; versions must be static SemVer (dynamic Python
versions reject).

Package names are unique per registry (so Python and npm may both have
`shared`), while release namespaces are globally unique and Python names follow
registry case/`-`/`_`/`.` normalization for uniqueness. `--package` matches
exact release namespaces first, falling back to a native package name only when
unambiguous in that component (use distinct namespaces like `shared-python` and
`shared-node`; ambiguous selections fail before mutation).

Python bumps preserve TOML comments and update the local package in the nearest
`uv.lock`; npm bumps preserve metadata, dependencies, and `private` while
updating adjacent `package-lock.json`/`npm-shrinkwrap.json` root records. Stale
lock identities fail before source mutation. Release checks run the affected
component graph in dependency order with configured timeouts (requiring Bash and
GNU `timeout`). Changelogs stay adjacent to manifests; dirty checkouts or
failing prerequisites block the version commit and tag.

`publish` controls registry eligibility (npm `private = true` always disables
it). Eligible owners get credential-free `publish-python-<namespace>.yaml` or
`publish-npm-<namespace>.yaml` plus a separate default-branch
`trusted-publish-<registry>-<namespace>.yaml`. Each package-qualified signed
tag qualifies the full graph on declared native runners, validates
tag/event/HEAD on fixed GitHub-hosted `ubuntu-24.04`, builds just that owner's
wheel+sdist or npm tarball, and stores a source-bound checksum receipt in an
attempt-qualified immutable artifact. The publisher always runs on
`ubuntu-24.04`, taking its engine, keyring, and package/namespace enrollment
from its protected revision. Before upload it re-admits the full signed tag,
source SHA, native provider workflow ID, original run/attempt, and archive
ID/digest, rejecting metadata/checksum conflicts and non-hosted archive-builder
enrollment, uploading only missing exact artifacts, and polling up to twenty
registry reads (thirty-second intervals) for checksum-matched propagation
(failed uploads pass only if exact artifacts already exist). `PYPI_API_TOKEN`
and `NPM_TOKEN` belong only to the protected `simit-pypi`/`simit-npm`
environments restricted to the default branch; the helper uploads admitted
bytes directly to the fixed registry endpoint without running candidate Nix,
scripts, hooks, or config.

Recovery reruns the original signed-tag run and reuses its first retained
archive (including partial uploads); only archives from passing original
attempts with a passing hosted `validate` builder job qualify (failed attempts
and duplicate attempt artifact names reject, and the publisher never rebuilds).
Native builds use root `ci` shell (Python 3.11+, GnuPG, `uv`/npm). Prepared npm
files must exist after qualification; `publishConfig` access/dist-tag settings
carry through (custom registries need a separate backend), prereleases require
`publishConfig.tag`, and `publishConfig.provenance = true` grants OIDC only to
the protected publisher, where an engine-owned signer uses the worker's global
npm Sigstore library to attest the admitted tarball digest, signed source, and
protected publisher revision (its child environment receives OIDC access, with
no registry token or candidate Node/npm config). Python keeps SemVer in
source/tags, mapping stable and `alpha`/`beta`/`rc`/`dev` numeric prereleases
to PyPI normalized versions; SemVer build metadata is unsupported for public
native releases. All native publication requires `keys/maintainers.gpg`,
namespaces are limited to 64 ASCII bytes at configuration admission, and npm
lifecycle scripts stay in registry metadata without ever running in the
publisher.

Public Python verification requires a complete unyanked wheel/sdist pair with
valid SHA-256 entries; npm provenance policy must be boolean or unset during
enrollment and credential-free packing, and public npm name admission follows
npm's legacy-compatible publish validation. Recovery of an already-published
exact npm archive also verifies its signed distribution tag and repairs that
registry mapping without rebuilding. Undecodable Git byte paths force full
qualification while known UTF-8 paths stay in the plan, and Cargo metadata
paths normalize to `/` on Windows before matching. Independent crates.io
workflows honor Cargo's registry allowlist (unpublished and alternate-only
members stay qualified without crates.io workflows; unrestricted members and
allowlists containing `crates-io` keep both). Native retries verify the
original hosted upload step's artifact ID and digest from GitHub's
attempt-specific step logs (same-name replacements do not inherit original
success; missing/ambiguous logs reject). The Python uploader consumes the
engine's already-normalized registry version (including prereleases), public
Python names follow Core Metadata grammar, and npm `private` must be absent or
boolean.

VS Code and JetBrains extension publishers stay independently managed: full
monorepo CI generation rejects those configuration sections before reconciling
workflows, so use `simit init ci --primary-only` to regenerate qualification
while keeping extension publishers.

Component gates share `bash -euo pipefail -c` semantics (quoting, env, timeouts)
locally and in CI. The packaged test suite's Python stdlib and `actionlint`
dependencies live in `nix/review-tools.nix`, independent of devShell tools.

`release verify` checks eligible packages against public registries using
read-only queries (each request has a ten-second deadline and a 4 MiB metadata
limit): PyPI exact release metadata must include a non-yanked file with a
SHA-256 digest, and npm abbreviated metadata must match package/version and
integrity. Missing, mismatched, or yanked releases fail; transport, HTTP, and
malformed-metadata errors report blocked; private packages make no registry
request. Cargo publication stays governed by `[ci].publish_crates`.
