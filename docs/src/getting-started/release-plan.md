# Release Plan

`simit release plan` prints the publish order for the current workspace by
reading `cargo metadata --no-deps` and sorting local path dependencies before
their dependents.

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

for each crate in publish order. Current Cargo does not accept
`cargo package --dry-run`, so simit uses the local packaging command that checks
the package archive without publishing it. simit stops on the first packaging
failure and returns exit code `1`.

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
