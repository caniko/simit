# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://codeberg.org/caniko/simit/compare/0.12.0...HEAD
[0.12.0]: https://codeberg.org/caniko/simit/compare/0.11.0...0.12.0
[0.11.0]: https://codeberg.org/caniko/simit/compare/0.10.0...0.11.0
[0.10.0]: https://codeberg.org/caniko/simit/compare/0.9.0...0.10.0
[0.9.0]: https://codeberg.org/caniko/simit/compare/0.8.0...0.9.0
[0.8.0]: https://codeberg.org/caniko/simit/compare/0.7.0...0.8.0
[0.7.0]: https://codeberg.org/caniko/simit/compare/0.3.1...0.7.0
