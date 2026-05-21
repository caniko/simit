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
