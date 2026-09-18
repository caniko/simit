# Release Plan

`simit release plan` prints the publish order for the current workspace by
reading `cargo metadata --no-deps` and sorting local path dependencies before
their dependents. Only publish-ordering kinds create edges: normal, build,
optional, and target-specific path dependencies. Dev-dependencies are ignored
by `cargo publish` and by the plan, so a publishable crate may dev-depend on a
`publish = false` helper (e.g. `xtask`) and still publish. Workspace-inherited
versions, renamed dependencies (`package = "real"`), optional, and
target-specific dependencies are resolved through directory mapping to real
package names; ties between independent crates break alphabetically.

```sh
simit release plan
simit release plan --json
simit release plan --dry-run-package
simit release plan --package sorrel-ui
```

The default text output is stable across runs. Ties between independent crates
are broken alphabetically by package name.

```text
publish order (3 crates):
  1. a 0.1.0
  2. b 0.1.0
  3. c 0.1.0
non-publishable members skipped: xtask
```

Non-publishable workspace members (`publish = false`) are never emitted in the
publish order. They are listed separately in text output so the chaperone can
see exactly what was skipped.

## Package Selection

Without `--package`, simit plans every publishable workspace member. When
`--package <name>` is present, simit treats the named crate as a publish root
and includes any publishable local path dependencies needed to reach it.

If a selected publishable crate depends on a non-publishable workspace member,
the command fails instead of silently omitting the dependency.

## Dry-Run Packaging

`--dry-run-package` executes:

```sh
cargo package -p <crate> --allow-dirty --no-verify
```

for each crate in publish order. This is archive construction only
(`--no-verify`): it proves the `.crate` file builds, not that packaged
contents verify, not a registry dry-run, and not publication. Current Cargo
does not accept `cargo package --dry-run`; simit never introduces it.
simit stops on the first packaging failure and returns exit code `1`.

Four stages, honestly separated:

1. Archive construction: `cargo package --allow-dirty --no-verify`.
2. Verification build from packaged contents: `cargo package --allow-dirty`
   (no `--no-verify`). For dependents this succeeds only after prerequisites
   are live on the registry (staged verification).
3. Registry publication dry-run: `cargo publish --dry-run`.
4. Actual publication: `cargo publish`, prerequisites before dependents with
   bounded propagation waits.

For a new workspace whose internal versions are not yet on crates.io, stage 2
initially fails for dependents (sibling source paths do not prove published
crates will build). Use staged verification: publish prerequisites first in
plan order (or to a disposable registry for pre-publish checks with a temp
`source` replacement that is never committed), then re-run stage 2 for
dependents. Never commit local registry overrides or path patches into
publishable manifests. See `docs/integrations/chaosbox-v1.md` for the tested
procedure.

## JSON Schema

`--json` prints one ordered array. Each item has this shape:

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

The array order is the publish order. `depends_on` lists only local publishable
workspace dependencies that must be published first.

`--json` and `--dry-run-package` are mutually exclusive so the JSON stream stays
machine-readable.
