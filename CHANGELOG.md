# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://codeberg.org/caniko/simit/compare/0.7.0...HEAD
[0.7.0]: https://codeberg.org/caniko/simit/compare/0.3.1...0.7.0
