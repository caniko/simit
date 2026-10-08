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
and secrets stay repository-scoped. Non-Cargo version mutation still requires
additional generator support.
