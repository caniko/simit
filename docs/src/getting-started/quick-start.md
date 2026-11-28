# Quick Start

Create a patch release commit and signed release tag:

```sh
simit commit patch -m "fix admission edge case"
```

Create a release commit without tagging:

```sh
simit commit --no-tag patch -m "prepare unreleased patch"
```

Preview a release commit without modifying the worktree:

```sh
simit commit --dry-run patch -m "preview patch release"
```

Run checks and promote the changelog in a local release:

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

Entries cover local build, cache, editor, environment, and documentation
outputs. Simit adds Rust and uv/Python-specific entries when it detects those
project types. Review project-specific rules before replacing `.gitignore`.

Generate CI workflows for a single-package project:

```sh
simit init ci --platform forgejo
```

Generic Rust CI generates only test, lint, and optional quality-gate workflows;
it does not create crates.io publish workflows unless you opt in:

```sh
simit init ci --platform forgejo --publish-crates
```

Select the workflow set explicitly for Cargo workspaces:

```sh
simit init ci --platform forgejo --workspace
simit init ci --platform forgejo --package my-crate
```

Workspace CI uses `ci-<crate>.yaml` per package; single-package projects keep
`ci.yaml`. With `--publish-crates`, publishable workspace members get
`publish-crate-<crate>.yaml`; single-package release projects get
`publish-crate.yaml`.

Run workspace gates once in an aggregate GitHub workflow:

```sh
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate
```

Aggregate mode applies only to workspace CI. With `--publish-crates`, Simit
generates member-scoped publish workflows (`publish-crate-<crate>.yaml`) by
default; it adds no cross-crate ordering.

Opt in to dependency-ordered workspace publishing on GitHub only:

```sh
simit init ci --platform github --runtime nix --workspace --workspace-strategy aggregate --publish-crates --coordinated-publish
```

This generates credential-free `publish-workspace.yaml` archive builds and an
independent `trusted-publish-workspace.yaml` publisher. Publication follows
`simit release plan` dependency order with signed-source, required-gate and
checksum-conflict checks. Complete the operator-owned enrollment in
[Release Integrity](release-integrity.md) before enabling uploads.

Declare an additional required integration gate (e.g. Gel) without hand-editing
YAML:

```toml
[[ci.required_gates]]
id = "gel-integration"
run = "nix run .#test-gel"
timeout_minutes = 30
```
