# Introduction

`simit` is a semver-aware commit helper for Rust projects.

It bumps selected Cargo package versions, updates `Cargo.lock` when present,
stages only the version files it changed, delegates commit creation to `git`,
and creates a release tag named exactly like the new version.

The tool also provides helpers for Keep a Changelog files, generated Rust CI,
crane-based Nix flakes, shell completions, manpages, and Homebrew tap release
automation.

## Scope

`simit` is intended for repositories that use Cargo package metadata as the
source of truth for crate versions. It performs local release preparation and
tagging; it does not push commits or tags to a remote.
