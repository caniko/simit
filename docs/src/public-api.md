# Public API

The Rust library exposes the modules used by the `simit` binary.

- `cargo` parses Cargo metadata, selects workspace packages, plans semantic
  version bumps, and updates manifests and lockfiles.
- `changelog` initializes, updates, validates, promotes, and displays Keep a
  Changelog files.
- `cli` defines the command-line parser structures.
- `commands` contains the command implementations used by the binary.
  Shared scaffolding (`run_git`, `shell_word`, `redact_url`,
  `prepare_target`, `bootstrap_repo`, `ArtifactCheck`, `WriteArtifact`,
  `CheckPrintMode`, `BumpFlow`, `print_next_steps`) lives in the
  `commands::scaffold` submodule; new `init`/`dist` subcommands should
  reuse it instead of re-deriving these helpers.
- `config` loads simit project config and resolves package release settings.
- `registry` persists the per-user project registry under
  `$XDG_DATA_HOME/simit/projects.toml`. Mutating commands should call
  `registry::touch_current_project_or_warn` (or
  `refresh_current_project_or_warn` for non-feature-changing actions
  such as `commit`/`release`) on their success path; registry IO must
  warn and continue rather than failing the primary command. The
  `SIMIT_NO_REGISTRY=1` env var fully suppresses registry IO and is the
  intended opt-out for sandboxed CI. Bulk onboarding uses
  `registry::discover_under` (and `discover_under_dry_run`), which walk
  a filesystem subtree, identify Cargo workspaces via a cheap top-level
  `Cargo.toml` table sniff (no `cargo metadata` per candidate), apply
  the documented deny list and hidden-dir skip rules, stop descent at
  the outermost workspace, and only register projects with at least
  one non-`Absent` simit feature (override with
  `DiscoverOptions::include_empty`).
- `git` performs release preflight checks, stages changed version files,
  delegates commits, and creates release tags.
- `project` detects repository languages and manages generated files.
- `release_trust` discovers and validates release maintainer trust roots.
- `render` renders generated CI, flake, diff, and Homebrew formula content.
- `sha256` computes release artifact hashes.
- `user_config` loads and validates user-scoped infrastructure defaults such
  as CI runner labels.

Full API documentation is published on docs.rs:

```text
https://docs.rs/simit
```
