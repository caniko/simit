# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Make generated flake-wiring CI checks read-only so fetching Simit cannot
  mutate its lock file in the consumer workspace.

## [0.17.7] - 2026-07-11

### Fixed

- Restrict Codeberg release uploads to configured release artifacts and
  checksum/signature files instead of every file in `release/`.

## [0.17.6] - 2026-07-10

### Fixed

- Apply the list-only package gate to multi-job workflows created by
  per-step runner routing.

## [0.17.5] - 2026-07-10

### Fixed

- Keep generated CI package checks list-only so unpublished workspace
  dependencies do not block first publication.

## [0.17.4] - 2026-07-10

### Fixed

- Use executable version checks for cargo-audit and cargo-deny in Nix
  workflows instead of passing the shell builtin `command` to `nix develop`.

## [0.17.3] - 2026-07-10

### Fixed

- Accept both Cargo `#` and `@` package-id delimiters while filtering Nix
  development-shell hook output from publish workflow version checks.

## [0.17.2] - 2026-07-10

### Fixed

- Keep Codeberg/Gitea-compatible dispatch triggers scalar instead of emitting
  unsupported nested workflow inputs.
- Filter Nix development-shell hook output when extracting a package version
  from `cargo pkgid` in publish workflows, accepting both Cargo's `#` and
  `@` package-id delimiters.

## [0.17.1] - 2026-07-10

### Fixed

- Keep generated release workflows out of CI drift detection so projects with
  both managed CI and release automation report a managed CI status.

## [0.17.0] - 2026-07-10

### Added

- Add persisted per-step runner routing and multi-job Forgejo CI rendering,
  including crate publishing and required environment handling.
- Add enforced tag-driven release publisher workflows with cross-platform
  artifact and Windows package support.
- Add regression fixtures for flake-provided `simitConfig` and Sorrel-like
  `simit.toml` configurations.

### Changed

- Extend `init release` to render comprehensive publisher workflows and make
  configured publisher enforcement explicit.

## [0.16.1] - 2026-05-30

### Fixed

- Fix `simit init ci --check` so generated but project-owned release workflows
  are ignored instead of being misreported as CI drift.

## [0.16.0] - 2026-05-30

### Added

- Document and integration-test multichannel distribution packaging commands,
  including `simit init aur|copr|apt|release` and
  `simit dist aur|copr|apt render` flows.

### Changed

- `simit init flake` now defaults to hooks-only ownership of
  `nix/pre-commit.nix`; use `--scope full` or `[flake].scope = "full"` to keep
  simit managing `flake.nix` and formatter wiring. Existing full-scope adopters
  are preserved until they opt into hooks-only.

### Fixed

- `simit init flake --check --diff` now explains pre-commit hook removals when
  a generated hook has a known CI-side replacement.

## [0.15.4] - 2026-05-25

### Changed

- `simit projects scan` now reports registry entries whose paths no longer
  exist on disk, including broken symlinks (use `--prune` to remove).
- `simit projects list` flags missing-path entries in its attention footer
  alongside `drift` / `conflicted` features, while filtering ephemeral `/tmp/*`
  paths from that footer.

### Fixed

- `simit hooks install` warns when a repo-local `core.hooksPath` shadows a
  friendly system dispatcher. Pass `--fix` to unset the rogue value
  automatically.
- Make `tests/projects.rs::list_filters_tmp_missing_paths_from_attention_footer`
  independent of `$TMPDIR`, so the test suite passes inside the Nix build
  sandbox where `$TMPDIR` is the build-local directory rather than `/tmp`.

## [0.15.3] - 2026-05-25

### Added

- Add `simit hooks install` with `--check` and `--diff` modes for installing
  project pre-commit hook wrappers even when `core.hooksPath` is set.
- Surface drift and hook-conflict project registry issues in
  `simit projects list`/`show`, with `--no-issues` available for scripts that
  need the old human table shape.

### Fixed

- Detect dispatcher-bypassing hook paths as `hooks: conflicted`, and require
  all managed hook wrappers before reporting `hooks: installed`.

## [0.15.2] - 2026-05-24

### Fixed

- Scope generated crates.io publish workflow tag/version validation to the
  package being published, avoiding wrong-version checks in Cargo workspaces
  whose members have diverging versions.

## [0.15.1] - 2026-05-24

### Fixed

- Fix generated crates.io publish workflows so Cargo workspace metadata version
  extraction selects a single package version instead of failing tag validation
  with concatenated workspace versions.

## [0.15.0] - 2026-05-24

### Added

- Add Omnix `om ci` support for Nix-runtime generated CI through
  `--with-om-ci` and `[ci].om_ci`, augment mode through `--om-ci-augment` and
  `[ci].om_ci_augment`, and Omnix flakeref pinning through `--omnix-ref`,
  `[ci].omnix_ref`, and user config `[ci.tools.omnix].ref`.
- Document the default Cargo-runtime CI cache layout generated by `simit init
ci`, including the `~/.cargo/bin` key, rust-cache settings, and why Nix
  workflows skip the Cargo cache steps.

## [0.14.1] - 2026-05-24

### Fixed

- Skip crates.io publishing for already-visible package versions in generated
  publish workflows before requiring a registry token.

## [0.14.0] - 2026-05-23

### Added

- Support project-configured custom flake validation for repositories that keep project-owned flake outputs.
- Render project-configured CI setup steps, environment variables, and required-secret documentation into generated workflows.

## [0.13.0] - 2026-05-23

### Added

- Add `simit projects list|show|scan|forget|prune|clear-state` for inspecting and
  maintaining the per-user project registry.
- Add `simit projects discover <ROOT>` to walk a filesystem subtree, find Cargo
  workspaces with at least one simit feature in use, and register them in the
  per-user project registry. Supports `--dry-run`, `--json`, `--max-depth`,
  `--skip`, `--include-empty`, and `--follow-symlinks`.
- Mark `simit projects discover --include-empty` entries with no detected simit
  features in human output instead of treating generic project files as simit
  usage.
- Add `simit projects clear-state` to clear all per-user project registry state,
  with `--dry-run` support for previewing the target registry path.
- Keep default `simit projects discover` human output compact by summarizing
  skipped non-simit projects instead of listing every skipped path.

### Changed

- Breaking CLI restructure: grouped the former top-level `simit init-ci`,
  `simit init-flake`, `simit init-homebrew-tap`, `simit init-chocolatey`, and
  `simit init-scoop-bucket` commands under `simit init ...`, and grouped the
  former top-level `simit homebrew`, `simit chocolatey`, and `simit scoop`
  package-manager commands under `simit dist ...`.

## [0.12.0] - 2026-05-22

### Added

- Generate an MSRV pre-push hook from workspace rust-version metadata in init-flake output.
- Allow `simit init-flake --check` to accept equivalent generated treefmt and pre-commit hook wiring instead of requiring byte-for-byte generated files.

## [0.11.0] - 2026-05-22

### Added

- Add `simit release trust status|init|check` for managing the maintainer
  OpenPGP trust root used by generated publish workflows.
- `simit init-ci` now discovers the release signing key and generates
  `keys/maintainers.gpg` instead of requiring maintainers to export it by hand.

## [0.10.0] - 2026-05-22

### Added

- Generated release workflows now verify GPG-signed tags against
  `keys/maintainers.gpg`, sign `SHA256SUMS.txt` with minisign, and emit
  cosign/SLSA provenance bundles for release archives.
- Document release integrity trust roots, required signing secrets, and
  consumer verification commands.

### Fixed

- Lower simit's own Rust requirement back to 1.85 so a local simit checkout can
  run inside projects using the generated Rust 1.85 release workflows.

## [0.9.0] - 2026-05-22

### Added

- `simit release sync-up` for rerunning a failed tag-triggered release from a
  fixed commit.
- Simit project config can now live in `simit.toml`, Cargo metadata, or flake
  `outputs.simitConfig`, with a Nix helper exposed as `lib.mkSimitConfig`.

### Changed

- `simit init-flake` now renders `rustfmt` with the Rust edition declared by
  workspace packages instead of hard-coding edition 2021.
- `simit init-flake --check` accepts custom `rs-harbor` flakes that preserve
  the generated pre-commit hook wiring through equivalent local bindings.
- Document and regression-test that `simit init-ci --check` enforces the
  generated tag-triggered crates.io publish workflow.

## [0.8.0] - 2026-05-22

### Added

- Chocolatey package publishing via simit chocolatey and init-ci --with-chocolatey
- Scoop bucket publishing via simit scoop and init-ci --with-scoop

### Changed

- release-artifacts.yaml now uses a Linux + Windows matrix when Windows packagers are enabled

## [0.7.0] - 2026-05-21

### Added

- `simit changelog` subcommand for managing keep-a-changelog `CHANGELOG.md`
  files (init / add / release / check / show), with optional auto-promotion
  from `simit release`.
- Add Forgejo/Nix Homebrew tap publishing to init-ci.
- Add optional `simit.toml` project config with a `[homebrew]` section for
  upcoming Homebrew subcommands.
- Add `simit init-homebrew-tap` for bootstrapping Homebrew tap repos with a
  checked-in `Formula/<name>.rb` skeleton.
- Add `simit homebrew render` and `simit homebrew bump` for native Homebrew
  formula rendering, sha256 computation, and optional tap commits/pushes
  without shelling out to `rs-harbor`.

## [0.3.1] - 2026-05-11

- Adjust generated Rust CI defaults

## [0.3.0] - 2026-05-09

- Reorg and release to crates.io

## [0.2.0] - 2026-05-09

- Prepare the first public crates.io release.

[Unreleased]: https://codeberg.org/caniko/simit/compare/0.17.7...HEAD
[0.17.7]: https://codeberg.org/caniko/simit/compare/0.17.6...0.17.7
[0.17.6]: https://codeberg.org/caniko/simit/compare/0.17.5...0.17.6
[0.17.5]: https://codeberg.org/caniko/simit/compare/0.17.4...0.17.5
[0.17.4]: https://codeberg.org/caniko/simit/compare/0.17.3...0.17.4
[0.17.3]: https://codeberg.org/caniko/simit/compare/0.17.2...0.17.3
[0.17.2]: https://codeberg.org/caniko/simit/compare/0.17.1...0.17.2
[0.17.1]: https://codeberg.org/caniko/simit/compare/0.17.0...0.17.1
[0.17.0]: https://codeberg.org/caniko/simit/compare/0.16.1...0.17.0
[0.16.1]: https://codeberg.org/caniko/simit/compare/0.16.0...0.16.1
[0.16.0]: https://codeberg.org/caniko/simit/compare/0.15.4...0.16.0
[0.15.3]: https://codeberg.org/caniko/simit/compare/0.15.2...0.15.3
[0.15.2]: https://codeberg.org/caniko/simit/compare/0.15.1...0.15.2
[0.15.1]: https://codeberg.org/caniko/simit/compare/0.15.0...0.15.1
[0.15.0]: https://codeberg.org/caniko/simit/compare/0.14.1...0.15.0
[0.14.1]: https://codeberg.org/caniko/simit/compare/0.14.0...0.14.1
[0.14.0]: https://codeberg.org/caniko/simit/compare/0.13.0...0.14.0
[0.13.0]: https://codeberg.org/caniko/simit/compare/0.12.0...0.13.0
[0.12.0]: https://codeberg.org/caniko/simit/compare/0.11.0...0.12.0
[0.11.0]: https://codeberg.org/caniko/simit/compare/0.10.0...0.11.0
[0.10.0]: https://codeberg.org/caniko/simit/compare/0.9.0...0.10.0
[0.9.0]: https://codeberg.org/caniko/simit/compare/0.8.0...0.9.0
[0.8.0]: https://codeberg.org/caniko/simit/compare/0.7.0...0.8.0
[0.7.0]: https://codeberg.org/caniko/simit/compare/0.3.1...0.7.0
