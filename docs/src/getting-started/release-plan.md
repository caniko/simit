# Release Plan

`simit release plan` reads `cargo metadata --no-deps` and prints the current
workspace's publish order: local path dependencies before dependents. Only
normal, build, optional, and target-specific path dependencies create edges.
The plan and `cargo publish` ignore dev-dependencies for ordering, so crates
can dev-depend on a `publish = false` helper like `xtask` without blocking
publication of the dependent. The plan maps
directories to real package names for workspace-inherited versions, renamed
dependencies (`package = "real"`), and optional and target-specific dependencies.

```sh
simit release plan
simit release plan --json
simit release plan --dry-run-package
simit release plan --package sorrel-ui
```

The plan is stable across runs and breaks ties between independent crates
alphabetically by package name. Default text output looks like:

```text
publish order (3 crates):
  1. a 0.1.0
  2. b 0.1.0
  3. c 0.1.0
non-publishable members skipped: xtask
```

The publish order excludes non-publishable workspace members (`publish = false`).
Text output lists them separately.

## Package Selection

Without `--package`, simit plans every publishable workspace member.
`--package <name>` selects the named crate as the publish root and includes its
transitive publishable local path dependencies.

The command fails if that dependency closure requires a non-publishable
workspace member; dev-dependencies do not create edges.

## Dry-Run Packaging

`--dry-run-package` executes:

```sh
cargo package -p <crate> --allow-dirty --no-verify
```

for each crate in publish order. `--no-verify` tests only `.crate` creation:
no verification build, registry dry-run, or publication. Current Cargo does not
accept `cargo package --dry-run`, so simit never calls it. The first packaging
failure stops the command with exit code `1`.

Four separate stages:

1. Archive construction: `cargo package --allow-dirty --no-verify`.
2. Verification build from packaged contents: `cargo package --allow-dirty`
   (no `--no-verify`). For dependents this succeeds only after prerequisites
   are live on the registry (staged verification).
3. Registry publication dry-run: `cargo publish --dry-run`.
4. Actual publication: `cargo publish`, prerequisites before dependents with
   bounded propagation waits.

For a new workspace with internal versions not yet on crates.io, stage 2
fails for dependents: sibling source paths don't prove published crates build.
Publish prerequisites in plan order to crates.io (or to a disposable registry
with a temporary, uncommitted `source` replacement for pre-publication checks),
then rerun stage 2 for dependents. Never commit local registry overrides or path
patches to publishable manifests. See `docs/integrations/chaosbox-v1.md` for the
tested procedure.

## JSON Schema

`--json` prints one array in publish order:

```json
[
  {
    "name": "sorrel-ui",
    "version": "0.1.0",
    "manifest_path": "/abs/path/to/Cargo.toml",
    "publish": true,
    "depends_on": ["sorrel-render"]
  }
]
```

`depends_on` lists only local publishable workspace dependencies that must
publish first.

`--json` and `--dry-run-package` are mutually exclusive to keep JSON
machine-readable.
