# Quick Start

Create a patch release commit and signed release tag:

```sh
simit commit patch -m "fix admission edge case"
```

Create a release commit without creating a tag:

```sh
simit commit --no-tag patch -m "prepare unreleased patch"
```

Preview a release commit without modifying the worktree:

```sh
simit commit --dry-run patch -m "preview patch release"
```

Run the local release flow with checks and changelog promotion:

```sh
simit release patch -m "release patch"
```

`simit release` runs Cargo tests, Clippy with warnings denied, promotes
`CHANGELOG.md` when present, commits, and tags locally.

Generate CI workflows for a single-package project:

```sh
simit init ci --platform forgejo
```

Generic Rust CI writes test, lint, and optional quality-gate workflows only. It
does not create crates.io publish workflows unless release publishing is
explicitly enabled:

```sh
simit init ci --platform forgejo --publish-crates
```

For Cargo workspaces, select the generated workflow set explicitly:

```sh
simit init ci --platform forgejo --workspace
simit init ci --platform forgejo --package my-crate
```

Workspace CI is rendered per package as `ci-<crate>.yaml`; single-package
projects keep the stable `ci.yaml` path. With `--publish-crates`, publishable
workspace members also get `publish-crate-<crate>.yaml`; single-package release
projects get `publish-crate.yaml`.
