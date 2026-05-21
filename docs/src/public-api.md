# Public API

The Rust library exposes the modules used by the `simit` binary.

- `cargo` parses Cargo metadata, selects workspace packages, plans semantic
  version bumps, and updates manifests and lockfiles.
- `changelog` initializes, updates, validates, promotes, and displays Keep a
  Changelog files.
- `cli` defines the command-line parser structures.
- `commands` contains the command implementations used by the binary.
- `config` loads `simit.toml` and resolves Homebrew release settings.
- `git` performs release preflight checks, stages changed version files,
  delegates commits, and creates release tags.
- `project` detects repository languages and manages generated files.
- `render` renders generated CI, flake, diff, and Homebrew formula content.
- `sha256` computes release artifact hashes.

Full API documentation is published on docs.rs:

```text
https://docs.rs/simit
```
