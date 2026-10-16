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

Configuration can come from `simit.toml`, Cargo `[workspace.metadata.simit]` or
`[package.metadata.simit]`, or flake `outputs.simitConfig`. Use one configuration
source. Member invocations discover the root for each source, stopping at Git
checkout boundaries.

## Qualification planning

```console
simit monorepo plan --json
simit monorepo plan --base origin/trunk --json
simit monorepo plan --changed-path nix/core/default.nix --json
```

The JSON contract has `schemaVersion`, `changedPaths`, `full`, `selected`, and
`reasons`. Selection includes changed owners, their transitive dependents,
then every selected component's prerequisites, in deterministic dependency
order. Cargo metadata supplies member source directories and local dependency
edges, including optional, build, development, and target-specific dependencies.
All Cargo members must have exactly one component owner.
For a package in the root `Cargo.toml`, declare its source paths explicitly
(for example, `paths = ["src", "tests", "build.rs"]`, retaining only paths that
exist). Simit adds `Cargo.toml` to that owner's paths without claiming nested
workspace members.

An unknown source path selects all components. The default shared paths are
`simit.toml`, `flake.nix`, `flake.lock`, `Cargo.toml`, `Cargo.lock`,
`rust-toolchain.toml`, `deny.toml`, and `.github`; `shared_paths` can replace
that list. Git selection includes committed, staged, unstaged, and untracked
changes relative to the base. Renames retain both old and new paths; deletions
retain their old owners. Missing or invalid base revisions fail locally.

Paths must be normalized and root-relative. Overlapping component ownership,
unknown dependencies, cycles, duplicate package ownership, and symlink traversal
are rejected. A component may contain multiple paths and Cargo packages.

## Generated qualification CI

Monorepo workflows install Nix **2.35.2** explicitly, including their native
prebuild and crate archive prerequisites. This release contains the upstream
[lost build-slot wakeup fix](https://github.com/NixOS/nix/commit/fba3f6e49bea5f6b7b5b75ab794e32afaabd5d26)
for the `!awake.empty()` assertion abort observed with the install action's
2.35.1 default. The installer remains pinned when regenerating workflows.

`simit init ci` and `simit init ci --check` operate on the complete root workflow
set from both root and member directories. GitHub Actions runs the planner at the
checked-out revision and qualifies the selected components. Missing push-base
objects trigger full qualification. Every component requires explicit checks;
failed or cancelled selected jobs fail the aggregate `qualified` gate.

The root `devShells.<system>.ci` must supply Simit with this component support,
Cargo, and the tools used by checks. Each command runs in that shell with its
declared environment and timeout. `systems` can select native runners using
`[ci.nix_system_runners]`. Handwritten supplementary workflows are retained.

`simit init flake` also resolves member invocations to the root. Optional
`formatter_modules` are composed into the generated root treefmt module; custom
flakes retain their existing ownership contract. Language and publication
overrides must not be used to replace component qualification commands.

### Running a declared check

```console
simit monorepo check --component core --check evaluate --nix-diagnostics
```

Local checks use configured environment literals even when the calling shell
already defines those variables. Provider expressions (`${{ ... }}`) cannot be
evaluated locally and are rejected before execution without printing their values.
Generated Actions callers use `--gate-env provider-staged`: every configured
gate variable must be present under its inert evaluated step alias (an empty value
is valid), and its bytes are passed directly to the gate without shell expansion.
Direct `--gate-env provider-resolved` invocations read the declared variable names.
Checks run once from the project root with Bash `-euo pipefail`; failure diagnostics
never retry the check or replace its failure status.

The native runner resolves the root configuration, validates the component and
check, and executes its project-owned command with `bash -euo pipefail` and the declared
environment. Unknown checks fail before execution. Generated qualification uses
this runner inside the project's `ci` shell. Both local and generated checks enforce
the declared timeout; on expiry the runner stops the gate process group before
collecting diagnostics. Evaluated step values are passed through the Nix shell
even if its devShell defines the same variable. Local release qualification rejects
unresolved provider expressions before running any selected gate.

Generated checks reserve two minutes for bounded diagnostics after the gate's
deadline. The deadline is captured before `nix develop`, so provisioning counts
toward the step budget, and is capped by a job deadline captured before checkout
and installation. GitHub steps and jobs retain their 360-minute maximum; a gate
at that maximum receives up to 358 minutes in CI. Local invocations without the
generated `--deadline` retain the full configured timeout. An expired CI budget
collects diagnostics without starting the gate.

Successful setup gates may retain background services for later gates. Failed,
timed-out or cancelled gates stop their process group before diagnostics; the
local CLI handles SIGINT, SIGTERM and SIGHUP. If Nix provisioning fails before
the native runner starts, the caller collects the same bounded diagnostics and
returns the provisioning status. A private startup marker avoids collecting
diagnostics twice after a native gate failure.

Generated checks stage provider values under inert environment names before
Bash starts and use `--gate-env provider-staged` to pass them through native
initialization. The native runner assigns declared data after gate Bash startup,
preserving values such as `IFS` and `PWD` that Bash normally rewrites. A gate's
`PATH` applies only to the gate, preserving the tool paths needed to start Simit
and Bash even in Nix sandboxes without `/bin/bash`.
Read-only Bash names (`UID`, `EUID`, `PPID`, `BASHOPTS`, `SHELLOPTS`, and
`BASH_VERSINFO`) are rejected during graph validation, before execution or writes.
A pinned `actions/setup-python` step provisions Python 3.12 before component gates,
including on custom runners; it follows the job's Node 20/24 action selection.
A Python 3 standard-library supervisor enforces the remaining deadline during
Nix provisioning and native initialization (including Cargo metadata), stops its
process group on timeout, and leaves the reserved time for diagnostics. Only
after validation, environment capture, timer creation and cancellation setup
does native Simit mark its ownership of the gate deadline and cleanup.
Spawn failures still attempt diagnostics after that handoff. Failed gates return
their original exit code (or `128 + signal` for signal termination) through the
CLI and generated supervisor, independently of diagnostic success.

On Linux, `--nix-diagnostics` captures `/proc/meminfo`, memory pressure, Nix
daemon status, and bounded daemon/kernel journals after a failed check. Journal
queries start at the check's start time, retain at most 150 entries each, and
use non-interactive sudo with a 15-second timeout and five-second termination
grace per command. Missing tools or
permissions are reported without changing the failed result. The runner does
not retry the gate or restart the daemon, and does not dump its environment.
These diagnostics are written to stderr and retained in the provider job log.

## Independent Cargo releases

```console
simit release plan --component rust --json
simit release patch --component rust --package engine -m 'release engine'
```

Component release plans contain dependency-ordered publishable packages and
their owning components, versions, and package-qualified tags. `publish = false`
members remain non-publishable. A bump selects exactly one owned package and
creates a signed tag such as `engine/v0.2.1`; package versions stay plain SemVer.
The package's adjacent `CHANGELOG.md` is used. Workspace-inherited versions are
rejected for independent bumps. With `[release.changelog].auto_draft = true`,
drafting uses only component-owned paths since the latest reachable package tag.
Changelog compare links use the same package namespace. Component release mutation
requires a clean checkout.

Set `[ci].publish_crates = true` to generate one independently tagged publication
workflow per publishable Cargo member. Each `<package>/v<version>` release first
qualifies the entire component graph on its declared native runners, verifies the
signed tag against `keys/maintainers.gpg`, and checks its exact checkout and package
version. It publishes only the tagged package, retains the package/archive checksum
and registry propagation checks, and requires local dependencies to be published
already. Each credential-free, tag-only builder has a separate default-branch
publisher enrolled through `SIMIT_PUBLISHER_REVISION`. The trusted publisher binds
the complete native tag, package namespace, original provider run and archive
checksum before upload. Registry credentials come only from the protected
`simit-crates-io` environment, restricted to the exact default branch.

`simit release verify --component <id> --package <name>` checks the package's
adjacent changelog and namespaced tag. `simit release sync-up` accepts the same
selection and preserves its existing clean-checkout and lease-protected tag push
contracts. Shared tag verification and sync-up require explicit component
selection in monorepositories. With `[release.notes].source = "git"`, verification
uses commits touching component-owned paths between the previous package tag and
the exact release tag; unrelated components and later commits are excluded. Trust
and secrets stay repository-scoped.

## Independent Python and npm versions

Declare each static version owner explicitly; template and test-fixture manifests
are never inferred as release packages:

```toml
[[monorepo.components]]
id = "python"
paths = ["components/python"]
releases = [
  { manifest = "components/python/pyproject.toml", namespace = "py-engine", publish = false },
]
checks = [{ id = "test", run = "uv run --project components/python pytest" }]
```

The same `release plan`, `patch`, `minor`, `major`, `verify`, and `sync-up`
commands accept these owners. Select `--package` explicitly for components with
multiple release packages, including mixed Cargo/Python components. An explicit
namespace supports npm package names such as `@example/engine` while retaining
safe package-qualified tags. Versions must be static SemVer; dynamic Python
version providers are rejected.

Package names are unique within their native registry, so Python and npm may
both retain a package named `shared`. Release namespaces remain globally unique.
Python names use the registry's case and `-`/`_`/`.` normalization for uniqueness.
`--package` first selects an exact release namespace; a native package name is
accepted only when it identifies one owner in the selected component. Use
distinct namespaces such as `shared-python` and `shared-node` to choose between
matching registry names. Ambiguous name-only selections fail before mutation.

Python bumps preserve TOML comments and update the local package record in the
nearest `uv.lock`. npm bumps preserve package metadata, dependencies and `private`,
and update adjacent `package-lock.json`/`npm-shrinkwrap.json` root records. Stale
lock identities fail before source mutation. Release checks run the affected
component graph in dependency order with their configured timeouts; the execution
environment must supply Bash and GNU `timeout`. Changelogs remain adjacent to
their manifests. A dirty checkout or a failing prerequisite check prevents the
version commit and tag.

`publish` records registry eligibility (npm `private = true` always disables it).
Eligible owners generate credential-free `publish-python-<namespace>.yaml` or
`publish-npm-<namespace>.yaml` source workflows and a corresponding
`trusted-publish-<registry>-<namespace>.yaml` default-branch publisher.
Each package-qualified signed tag
qualifies the complete graph on every declared native runner, then validates
the tag/event/HEAD identity on a fixed GitHub-hosted `ubuntu-24.04` builder,
builds just that owner's wheel plus sdist or npm
tarball, and retains a source-bound checksum receipt in an attempt-qualified,
immutable provider artifact. The publisher always runs on `ubuntu-24.04`,
independently of the configured qualification capacity. Its engine, keyring and
package/namespace enrollment come from its protected workflow revision. Before
upload it re-admits the full signed tag, source SHA, native provider workflow ID,
original run/attempt and archive ID/digest. The publisher rejects metadata/checksum
conflicts and non-hosted archive-builder enrollment, uploads only missing exact
artifacts, and waits at most twenty
registry reads separated by thirty-second waits for checksum-matching
propagation. Failed uploads are accepted only if exact artifacts already exist.
Credentials (`PYPI_API_TOKEN` or `NPM_TOKEN`) belong only to the protected
`simit-pypi` or `simit-npm` environment, restricted to the default branch.
The protected helper uploads the admitted bytes directly to the fixed registry
endpoint. It never runs candidate Nix expressions, scripts, package hooks or
package configuration with those credentials. Recovery reruns the original
signed-tag run and reuses its first retained archive, including partial uploads;
only archives from successful original attempts with a successful hosted
`validate` builder job are eligible. Failed attempts cannot contribute retry
bytes, and duplicate attempt artifact names are rejected. It does not rebuild
a replacement archive in the publisher. Native build steps use
the project's root `ci` shell, which must provide Python 3.11+, GnuPG and `uv`
or npm. Prepared npm files must already be present after component qualification;
`publishConfig` access and distribution-tag settings are retained, while custom
registries require a separate backend. npm prereleases require an explicit
`publishConfig.tag`. npm `publishConfig.provenance = true` grants OIDC permission
only to the protected publisher. An engine-owned signer uses the hosted worker's
global npm Sigstore library to attest the admitted tarball digest, signed source
and protected publisher revision. Its child environment receives OIDC access,
but no registry token or candidate Node/npm configuration. Python
owners retain SemVer in source/tags; public Python releases
support stable versions and `alpha`, `beta`, `rc`, or `dev` numeric prereleases,
mapped to PyPI's normalized metadata version. SemVer build metadata is unsupported
for public native releases. All native publication requires `keys/maintainers.gpg`.
Native namespaces are limited to 64 ASCII bytes at configuration admission.
Registry metadata retains npm lifecycle scripts for consumers; the protected
publisher never executes those scripts.
Public Python verification requires the complete unyanked wheel/sdist pair with
valid SHA-256 entries. npm provenance policy must be boolean or unset during
enrollment and credential-free packing.
Public npm name admission follows npm's legacy-compatible publish validation.
Recovery of an already-published exact npm archive also verifies its signed
distribution tag and repairs that mapping without rebuilding the archive.
Undecodable Git byte paths force full component qualification; known UTF-8 paths
remain in the plan. Cargo metadata paths are normalized to repository-style `/`
separators on Windows before ownership matching.
Independent crates.io workflow generation honors Cargo's registry allowlist:
unpublished and alternate-only members remain qualified but receive no crates.io
builder or protected publisher. Unrestricted members and allowlists containing
`crates-io` retain both workflows.
Native archive retries require the original hosted upload step's artifact ID and
digest from GitHub's attempt-specific step logs. A later same-name replacement
does not inherit the original attempt's success. Missing or ambiguous upload
logs fail admission. The protected Python uploader consumes the engine's
already-normalized registry version, including prereleases. Public Python names
follow the Core Metadata grammar; npm `private` must be absent or boolean.

Configured VS Code and JetBrains extension publishers remain independently
managed. Full monorepo CI generation rejects those sections before reconciling
workflows; use `simit init ci --primary-only` to regenerate qualification while
preserving existing extension publishers.

Component gates use the same `bash -euo pipefail -c` command semantics locally
and in generated CI, including quoted commands, environment and timeouts. The
packaged test suite needs Python's standard library and `actionlint`; these are
declared in `nix/review-tools.nix`, independently of development-shell tools.

`release verify` checks eligible packages against the public native registry:
PyPI's exact release metadata must contain a non-yanked file with a SHA-256 digest,
and npm's abbreviated metadata must contain the matching package/version and
distribution integrity. A missing, mismatched, or yanked release fails; transport,
HTTP and malformed-metadata errors are reported as blocked. Private packages make
no registry request. These read-only queries use a ten-second request deadline
and a 4 MiB metadata limit. Cargo publication remains managed by
`[ci].publish_crates`.
