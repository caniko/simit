# simit

`simit` is a semver-aware commit helper for Rust projects.

```sh
simit commit patch -m "fix admission edge case"
simit commit minor -m "add weighted provider API"
simit commit major -m "remove deprecated API"
simit commit patch --pre rc.1 -m "prepare release candidate"
```

The command bumps the selected package version in `Cargo.toml`, updates
`Cargo.lock` when present, stages only the version files it changed, delegates
to `git commit`, and creates a signed release tag named exactly like the new
version with the message `Release <version>`.

Use `--no-tag` to skip tag creation:

```sh
simit commit --no-tag patch -m "prepare unreleased patch"
```

Use `--no-sign` to create an unsigned lightweight tag in environments where
GPG signing is unavailable:

```sh
simit commit --no-sign patch -m "release without tag signing"
```

Preview a release commit without touching the worktree:

```sh
simit commit --dry-run patch -m "preview patch release"
```

For workspaces with more than one package, choose the target package:

```sh
simit commit --package memory-admission patch -m "release memory-admission"
simit commit --workspace patch -m "release all workspace crates"
```

Run the full local release flow, including checks and a strict Keep a Changelog
update:

```sh
simit release --no-sign patch -m "release patch"
```

`simit release` runs `cargo test`, `cargo clippy --all-targets --all-features
-- --deny warnings`, updates `CHANGELOG.md`, commits, and tags locally. It does
not push.

## CI wiring

Generate lightweight Rust CI and crates.io publish workflows for a repository:

```sh
simit init-ci --platform forgejo
simit init-ci --platform github
```

Forgejo workflows use direct Rust container jobs by default, even when the
repository has a `flake.nix`. The container tag is derived from
`package.rust-version`, for example `rust:1.85-alpine`.

Forgejo workflows are tuned for Codeberg's hosted runner limits. Plain Cargo
crate jobs use `codeberg-small` by default because test, clippy, and package
work usually exceed the tiny runner's two-minute budget. Use `--runner` when a
repository needs a specific hosted runner:

```sh
simit init-ci --platform forgejo --runner codeberg-medium
```

Use `--runtime nix` only for workflows that intentionally need flake outputs,
such as cross-platform binary builds or release artifacts:

```sh
simit init-ci --platform forgejo --runtime nix
```

Use `--check` in CI to make sure committed workflows still match `simit`'s
generated output:

```sh
simit init-ci --platform forgejo --check
```

Add optional CI jobs and checks when the project needs them:

```sh
simit init-ci --platform github --with-nextest --with-msrv --with-docs
simit init-ci --platform forgejo --with-audit --with-deny --with-artifacts
simit init-ci --platform forgejo --check --diff
```

`--with-msrv` requires `package.rust-version`.

## Flake and hook wiring

Generate a canonical Rust crane flake plus formatter and pre-commit hook definitions:

```sh
simit init-flake
```

This writes `flake.nix`, `nix/treefmt.nix`, and `nix/pre-commit.nix`,
detects Rust, Nix, uv-based Python, TOML, YAML, and Markdown files, and wires
`treefmt-nix` and `cachix/git-hooks.nix`. Existing `flake.nix` files are
patched only when simit can find safe anchors; otherwise, use `--print` and
apply the generated wiring manually.

Preview the generated files without writing them:

```sh
simit init-flake --print
```

Check committed flake and hook files in CI:

```sh
simit init-flake --check
simit init-flake --check --diff
```

## Shell integration

Generate shell completions or a manpage:

```sh
simit completions bash
simit completions zsh
simit completions fish
simit man
```

## Release checklist

Before publishing a release, make sure `CHANGELOG.md` has the intended
`## [Unreleased]` entries, then run:

```sh
simit release patch -m "release patch"
```

The crates.io publish workflow runs when the release tag is pushed and requires
`CRATES_IO_API_TOKEN`.

## License

`simit` is licensed under the MIT License. See `LICENSE`.
