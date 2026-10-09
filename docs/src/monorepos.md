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
already. Qualification jobs have no registry credentials. Publication uses the
repository's existing `CRATES_IO_API_TOKEN` secret.

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
Eligible owners generate independent `publish-python-<namespace>.yaml` or
`publish-npm-<namespace>.yaml` workflows. Each package-qualified signed tag
qualifies the complete graph on every declared native runner, then validates
the tag/event/HEAD identity, builds just that owner's wheel plus sdist or npm
tarball, and transfers a source-bound checksum receipt to a separate publish
job. The publish job verifies the signed tag again, rejects metadata/checksum
conflicts, uploads only missing exact artifacts, and waits at most twenty
registry reads separated by thirty-second waits for checksum-matching
propagation. Failed uploads are accepted only if exact artifacts already exist.
Credentials (`PYPI_API_TOKEN` or `NPM_TOKEN`) are confined to the publish step;
npm packing and publishing disable lifecycle scripts. Native release steps use
the project's root `ci` shell, which must provide Python 3.11+, GnuPG and `uv`
or npm. Prepared npm files must already be present after component qualification;
`publishConfig` access and distribution-tag settings are retained, while custom
registries require a separate backend. npm prereleases require an explicit
`publishConfig.tag`. Python owners retain SemVer in source/tags; public releases
support stable versions and `alpha`, `beta`, `rc`, or `dev` numeric prereleases,
mapped to PyPI's normalized metadata version. SemVer build metadata is unsupported
for public native releases. All native publication requires `keys/maintainers.gpg`.

`release verify` checks eligible packages against the public native registry:
PyPI's exact release metadata must contain a non-yanked file with a SHA-256 digest,
and npm's abbreviated metadata must contain the matching package/version and
distribution integrity. A missing, mismatched, or yanked release fails; transport,
HTTP and malformed-metadata errors are reported as blocked. Private packages make
no registry request. These read-only queries use a ten-second request deadline
and a 4 MiB metadata limit. Cargo publication remains managed by
`[ci].publish_crates`.
