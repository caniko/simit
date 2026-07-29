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

Generate a compact, language-aware `.gitignore` for the current project:

```sh
simit init gitignore
simit init gitignore --check
simit init gitignore --print
```

The generated entries cover local build, cache, editor, environment, and
documentation outputs. Rust and uv/Python-specific entries are added when
those project types are detected. Review project-specific rules before
replacing an existing `.gitignore`.

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

For one aggregate GitHub workflow that runs workspace gates once, use:

```sh
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate
```

Aggregate mode is for workspace CI only. Crate publishing and package-specific
release workflows remain member-scoped.
