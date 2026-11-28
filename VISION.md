# simit vision

`simit` is a semver-aware commit and release helper for Rust projects. It bumps
selected Cargo package versions, updates `Cargo.lock`, stages only the version
files it changes, commits via Git, and tags the new version. It generates and
maintains CI workflows, crane-based Nix flakes, Keep a Changelog files, README
badge blocks, `.gitignore` templates, release maintainer trust roots,
distribution packaging, and a per-user project registry.

## Problem

Manual Rust releases depend on order: bump the version, update the lockfile,
write the release's changelog section, commit only the intended files, sign and
name the tag, then align CI, the flake, and distribution channels with project
config. Projects repeat these steps and drift. Mistakes usually surface after a
bad tag goes public.

## Principles

- Simit takes versions from Cargo package metadata, not a separate version file.
- Commits stage only the version files changed by the bump, leaving the rest of
  the worktree alone.
- `simit release` runs tests and clippy, promotes the changelog, commits, and
  tags locally without pushing. Repair a pushed tag that ran on a bad commit
  explicitly with `release
sync-up`.
- Strict Keep a Changelog: add entries under `[Unreleased]` by section kind;
  releases promote the section with a date.
- Generation and upgrades are deterministic and repeatable. `simit upgrade`
  targets a workspace, explicit paths, or every registered project. It supports
  `--check` and `--dry-run`.
- `--git-fixtures` scopes temporary Git config to the fixture root without
  changing identity, system or checkout config, or hook files.

## Scope

- `simit commit` for patch/minor/major releases, prerelease suffixes, workspaces,
  and target packages, with `--dry-run`, `--no-tag`, and `--no-sign`.
- `simit release` for the full local flow, plus `release sync-up`, `release plan`,
  and release verification.
- Changelog setup, entries, promotion, and checks.
- `simit init` for CI (Forgejo, GitHub, Crow project config), crane flakes,
  language-aware `.gitignore`, and release maintainer setup.
- Project registry commands (`list`, `show`, `discover`, `scan`, `clear-state`)
  and `simit upgrade`.
- Generated CI and packaging for mixed-language monorepos, including
  independently versioned and tagged Cargo, npm, and Python components, plus
  native distribution channels.
- `simit test` with optional disposable Git fixtures.
- Repository review workflow generation and verification.

## Non-goals

- Simit does not push release commits. Tag moves and channel pushes require
  explicit flags (`simit release sync-up --push`, `simit dist … publish
--push`). Generated CI handles routine publication.
- Crow workflow generation never invokes the Crow CLI or contacts a Crow server.
- Builds and Nix realisation belong to the project's flake and the canix cache
  wrapper, not Simit.

## Direction

Recent work covers mixed-language monorepo releases: independently tagged
Cargo, npm, and Python components, scoped notes and verification, and native
package publication (APT, COPR, AUR, Chocolatey) with provenance checks,
alongside release verification, workspace release planning, and hooks-only
flake ownership across adopting repositories.

## Place in the fleet

Canix uses Simit as a flake input and runs it as the atlas `simit-updater` to
keep generated CI and flake wiring current across owned projects. Most caniko
repositories, including most in this batch, use Simit-generated CI and
`simit.toml`.
