# simit

`simit` is a semver-aware commit helper for Rust projects.

```sh
simit commit patch -m "fix admission edge case"
simit commit minor -m "add weighted provider API"
simit commit major -m "remove deprecated API"
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

For workspaces with more than one package, choose the target package:

```sh
simit commit --package memory-admission patch -m "release memory-admission"
```

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
