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
-- --deny warnings`, promotes `CHANGELOG.md` from `[Unreleased]` when that file
exists, commits, and tags locally. It does not push. Use `--no-changelog` to
skip changelog promotion for one release.

If a pushed release tag runs CI on a bad commit, commit the fix and move the
current-version tag to `HEAD`:

```sh
simit release sync-up --push
```

Without `--push`, sync-up only moves the local tag.

## Changelog management

Initialize a canonical Keep a Changelog file:

```sh
simit changelog init
```

Add entries under `[Unreleased]` with one of the standard section kinds:

```sh
simit changelog add added "support artifact workflows"
simit changelog add fixed "avoid detached-head release failures"
```

Promote `[Unreleased]` into a dated release section:

```sh
simit changelog release 0.4.0
simit changelog release 0.4.0 --date 2026-05-20 --repo-url https://codeberg.org/caniko/simit
```

Validate or inspect a changelog section:

```sh
simit changelog check
simit changelog show
simit changelog show 0.3.1
```

`simit release` automatically runs the same promotion logic when `CHANGELOG.md`
is present, so the normal local release flow is:

```sh
simit changelog add added "describe the release"
simit release patch -m "release patch"
```

## CI wiring

Generate lightweight Rust CI and crates.io publish workflows for a repository:

```sh
simit init-ci --platform forgejo
simit init-ci --platform github
```

Forgejo workflows use direct Rust container jobs by default, even when the
repository has a `flake.nix`. The default container is derived from
`package.rust-version`: `rust:<version>-bookworm` for current MSRVs, or
`rust:<version>-trixie` once the matching official tag exists. Without
`rust-version`, simit uses `rust:bookworm`. MSRV is only checked when
`--with-msrv` is requested.

Forgejo workflows default to our self-hosted atlas runner (`runs-on: atlas`).
The runner bind-mounts Node, git, and other JavaScript-action runtime tools
into job containers, so generated workflows use the Forgejo checkout action
instead of manual git checkout. Use `--runner` only when a repository needs a
specific non-default runner:

```sh
simit init-ci --platform forgejo --runner custom-runner
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

Forgejo + Nix artifact workflows can also publish a Homebrew tap:

```sh
simit init-ci --platform forgejo --runtime nix --with-artifacts --with-homebrew \
  --homebrew-tap https://codeberg.org/caniko/homebrew-demo.git \
  --homebrew-description "demo binary" \
  --homebrew-homepage https://example.com \
  --homebrew-download-repo caniko/demo \
  --homebrew-binary demo
```

`--with-homebrew` is Forgejo + Nix only and implies `--with-artifacts`. The
project workflow must stage each enabled platform archive under
`release/{name}-{version}-{arch}-{os}.tar.gz` before the Homebrew step runs;
the generated step verifies those files exist but does not build project-shaped
tarballs itself.

## Homebrew automation

Homebrew tap publishing tends to grow a lot of release-CI boilerplate. `simit`
keeps the tap metadata in `simit.toml`, bootstraps the tap once, and generates
the Forgejo release step from that same config.

```toml
[homebrew]
tap_url       = "https://codeberg.org/caniko/homebrew-foo.git"
download_repo = "caniko/foo"
binaries      = ["foo", "foo-ui"]
description   = "Cross-platform foo manager"
homepage      = "https://foo.example.com"
license       = "GPL-3.0-only"
archive_pattern = "foo-{version}-{arch}-{os}.tar.gz"

[homebrew.platforms]
# All four platforms are enabled by default. Override here if needed:
# linux_arm = false
```

Bootstrap the tap repo once with a placeholder formula:

```sh
simit init-homebrew-tap --target ../homebrew-foo
# prints the next-step git commit and push hints
```

Wire the release workflow from the project repo:

```sh
simit init-ci --platform forgejo --runtime nix \
  --with-artifacts --with-homebrew
```

For local inspection and iteration, render the formula or bump a checked-out
tap using release archives:

```sh
simit homebrew render
simit homebrew bump \
  --version 0.3.1 \
  --tap ../homebrew-foo \
  --archive darwin_arm=release/foo-0.3.1-aarch64-darwin.tar.gz \
  --archive darwin_intel=release/foo-0.3.1-x86_64-darwin.tar.gz \
  --archive linux_arm=release/foo-0.3.1-aarch64-linux.tar.gz \
  --archive linux_intel=release/foo-0.3.1-x86_64-linux.tar.gz \
  --push
```

rs-modde is the worked example for this flow: its release CI publishes
`modde` and `modde-ui` to `caniko/homebrew-modde` from the generated Homebrew
step.

## Windows packaging

`simit` can render and publish Windows packages for Chocolatey and Scoop from
the same project metadata used by release artifacts. Declare the package
surface in `simit.toml`:

```toml
[chocolatey]
id = "foo"
title = "Foo"
authors = "Example Maintainers"
description = "Cross-platform foo manager"
project_url = "https://foo.example.com"
download_repo = "caniko/foo"
archive_pattern = "foo-{version}-{arch}-windows.zip"

[scoop]
name = "foo"
bucket_url = "https://codeberg.org/caniko/scoop-foo.git"
download_repo = "caniko/foo"
binaries = ["foo"]
archive_pattern = "foo-{version}-{arch}-windows.zip"

[scoop.architectures]
# x64 and arm64 are enabled by default. Override here if needed:
# arm64 = false
```

Bootstrap the package repositories once:

```sh
simit init-chocolatey --target packaging/chocolatey
simit init-scoop-bucket --target ../scoop-foo
```

Wire Windows publishing into tagged release CI:

```sh
simit init-ci --platform github --with-chocolatey --with-scoop
simit init-ci --platform forgejo --with-chocolatey --with-scoop \
  --windows-runner windows-atlas
```

`--with-chocolatey` and `--with-scoop` imply `--with-artifacts`. GitHub uses
`windows-latest` by default. Forgejo requires `--windows-runner` because
Codeberg's shared runners are Linux-only. Generated workflows read
`secrets.chocolatey_api_key` for Chocolatey pushes and
`secrets.scoop_bucket_token` for Scoop bucket pushes.

For local inspection and iteration:

```sh
simit chocolatey render --output-dir packaging/chocolatey
simit chocolatey bump \
  --version 0.3.1 \
  --package-dir packaging/chocolatey \
  --archive x64=release/foo-0.3.1-x86_64-windows.zip

simit scoop render --output packaging/scoop/foo.json
simit scoop bump \
  --version 0.3.1 \
  --bucket ../scoop-foo \
  --archive x64=release/foo-0.3.1-x86_64-windows.zip \
  --archive arm64=release/foo-0.3.1-aarch64-windows.zip
```

## Project config

Projects may opt in to stable simit settings with a `simit.toml` file at the
Cargo workspace root. The supported packaging sections are `[homebrew]`,
`[chocolatey]`, and `[scoop]`.

For each Homebrew setting, resolution order is: CLI flag, `simit.toml`, Cargo
package metadata, then an error. `tap_url` and `download_repo` have no Cargo
metadata fallback, so they must be set by a flag or in `[homebrew]`.

```toml
[homebrew]
tap_url       = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
binaries      = ["mythos", "mythos-ui"]
# description, homepage, license, and name default from Cargo.toml when unset.

[homebrew.platforms]
linux_arm = false  # Override: do not publish aarch64-linux.
```

Chocolatey and Scoop use the same resolution order. Their `download_repo`
fields must be `OWNER/REPO`, and Scoop also requires `bucket_url`.

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
`## [Unreleased]` entries. You can validate them explicitly with
`simit changelog check`, then run:

```sh
simit release patch -m "release patch"
```

The crates.io publish workflow runs when the release tag is pushed and requires
`CRATES_IO_API_TOKEN`.

If that tag-triggered workflow fails after the tag has already been pushed,
commit the fix and rerun the release pipeline with:

```sh
simit release sync-up --push
```

## License

`simit` is licensed under the MIT License. See `LICENSE`.
