# simit vision

`simit` is a semver-aware commit and release helper for Rust projects. It bumps
selected Cargo package versions, updates `Cargo.lock`, stages only the version
files it touched, delegates the commit to git, and tags the release with the new
version. Around that core it generates and maintains the project infrastructure
a release needs: CI workflows, crane-based Nix flakes, keep-a-changelog files,
README badge blocks, `.gitignore` templates, release maintainer trust roots,
distribution-channel packaging, and a per-user project registry.

## Problem

Releasing a Rust crate by hand is a sequence of small, order-sensitive steps:
bump the version, update the lockfile, write the changelog section, commit only
the right files, sign and name the tag, then keep CI, the flake, and the
published channels in step with the project's configuration. Each project
repeats the sequence and drifts; a mistake is usually discovered after a bad tag
is public.

## Principles

- Cargo package metadata is the source of truth for versions. Simit acts on
  Cargo packages rather than on a separate version file.
- Version files only. A commit stages just the files the bump changed and leaves
  everything else in the worktree alone.
- Local release preparation and tagging only. `simit release` runs tests and
  clippy, promotes the changelog, commits, and tags locally; it does not push.
  A pushed tag that ran on a bad commit is repaired explicitly with `release
  sync-up`.
- Changelogs are strict Keep a Changelog: entries go under `[Unreleased]` with a
  section kind, and a release promotes that section with a date.
- Generated files are deterministic and re-runnable. `simit upgrade` applies
  deterministic upgrades to a workspace, explicit paths, or every registered
  project, with `--check` and `--dry-run` available.
- Test isolation is narrow by design. `--git-fixtures` scopes a temporary Git
  configuration to the fixture root and leaves identity, system configuration,
  the checkout's configuration, and hook files unchanged.

## Scope

- `simit commit` for patch/minor/major releases, prerelease suffixes, workspaces,
  and target packages, with `--dry-run`, `--no-tag`, and `--no-sign`.
- `simit release` as the local full flow, plus `release sync-up`, `release plan`,
  and release verification.
- Changelog initialisation, entry addition, promotion, and checking.
- `simit init` for CI (Forgejo, GitHub, Crow project config), crane flakes,
  language-aware `.gitignore`, and release maintainer setup.
- Project registry commands (`list`, `show`, `discover`, `scan`, `clear-state`)
  and `simit upgrade`.
- Generated CI and packaging for mixed-language monorepositories with
  independently versioned and tagged components, including Cargo, npm, and
  Python packages and native distribution channels.
- `simit test`, including the disposable Git fixture mode.
- Repository review workflow generation and verification.

## Non-goals

- It does not push release commits. Tag moves and channel pushes require
  explicit opt-in flags (`simit release sync-up --push`, `simit dist … publish
  --push`); routine publication is the job of the CI the generated workflows
  define.
- It does not invoke the Crow CLI or contact a Crow server. Crow output is
  project-side configuration only.
- It is not a general build system; builds and Nix realisation belong to the
  project's own flake and the canix cache wrapper.

## Direction

Recent history is mixed-language monorepository releases: independently tagged
Cargo, npm, and Python components, scoped release notes and verification, and
publication to native package channels (APT, COPR, AUR, Chocolatey) with
provenance checks. The checked-in planning dossier for a release-chaperone
fleet turns a readiness report into generator improvements and follow-up
cleanups in five target repositories.

## Place in the fleet

Canix consumes simit as a flake input and runs it as the atlas `simit-updater`,
which keeps generated CI and flake wiring current across the owned projects.
Most caniko repositories, including most of those in this batch, have
simit-generated CI workflows and `simit.toml` configuration.
