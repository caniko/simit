# simit

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.github/workflows/ci.yaml) [![Nix](https://img.shields.io/badge/Nix-managed-5277c3)](flake.nix) [![docs](https://img.shields.io/badge/docs-enabled-6f42c1)](docs) [![crates.io](https://img.shields.io/badge/crates.io-publishable-f46623)](https://crates.io/crates/simit) [![artifacts](https://img.shields.io/badge/artifacts-configured-2ea44f)](.github/workflows/release.yml)

<!-- simit:badges:end -->

`simit` is a semver-aware commit helper for Rust projects.

Building Simit requires Rust 1.88 or newer. Generated projects keep their
declared compiler requirements.

```sh
simit commit patch -m "fix admission edge case"
simit commit minor -m "add weighted provider API"
simit commit major -m "remove deprecated API"
simit commit patch --pre rc.1 -m "prepare release candidate"
```

Simit bumps the selected package's version in `Cargo.toml`, updates `Cargo.lock`
if present, stages only the version files Simit changed, and calls `git commit`.
It creates a signed tag named exactly like the new version with the message
`Release <version>`.

Use `--no-tag` to skip tag creation:

```sh
simit commit --no-tag patch -m "prepare unreleased patch"
```

If GPG signing is unavailable, use `--no-sign` for an unsigned lightweight tag:

```sh
simit commit --no-sign patch -m "release without tag signing"
```

Preview a release commit without writing to the worktree:

```sh
simit commit --dry-run patch -m "preview patch release"
```

In multi-package workspaces, select one package or all:

```sh
simit commit --package memory-admission patch -m "release memory-admission"
simit commit --workspace patch -m "release all workspace crates"
```

Check and release locally with a strict Keep a Changelog update:

```sh
simit release --no-sign patch -m "release patch"
```

`simit release` runs `cargo test`, `cargo clippy --all-targets --all-features
-- --deny warnings`, promotes `[Unreleased]` if `CHANGELOG.md` exists,
then commits and tags without pushing. `--no-changelog` skips promotion for
one release.

If a pushed release tag runs CI on a bad commit, commit the fix and retag
the current version at `HEAD`:

```sh
simit release sync-up --push
```

Omit `--push` to move only the local tag.

## Running tests with Git fixtures

Run a project's test command directly, preserving arguments and exit status:

```sh
simit test -- cargo test
simit test --git-fixtures -- pnpm test:run
```

On Unix, `--git-fixtures` sets `TMPDIR`, `TMP`, and `TEMP` for the test subprocess
to a disposable directory. Temporary Git config includes the operator's global
config, then uses `includeIf gitdir:` to disable inherited hooks and automatic
commit/tag signing **only for Git directories under that root**. Identity, system
and checkout config, and hook files stay unchanged. The original checkout and
linked worktrees with Git metadata outside the root keep their normal policy.
Explicit local and command-scope Git settings take precedence.

Tests must use that directory. Hard-coded `/tmp` fixtures or a replacement
`GIT_CONFIG_GLOBAL` bypass isolation. Set `TMPDIR` before invoking Simit to
choose the parent; use a short path for Unix sockets. Git must support
`git var GIT_CONFIG_GLOBAL`. Simit runs the command without a shell, streams
standard I/O, and removes the directory after the test command exits, even if
the command failed. It neither installs dependencies nor provides an OS sandbox.
Without `--git-fixtures`, the environment is unchanged.

## Project registry

Inspect the per-user registry of projects Simit has acted on with `simit projects`:

```sh
simit projects list
simit projects list --json --feature flake=managed
simit projects show
simit projects discover ~/Projects --dry-run
simit projects scan --prune
simit projects clear-state --dry-run
```

JSON lets scripts and AI agents find all projects with a Simit-managed feature,
such as flake, CI, or packaging. Register existing Simit-managed projects once
per machine with `simit projects discover <ROOT>`.

## Project upgrades

Apply deterministic upgrades to this workspace, explicit paths, or all
registered Simit-managed projects:

```sh
simit upgrade
simit upgrade --path ../project-a --path ../project-b
simit upgrade --all
simit upgrade --dry-run --diff
simit upgrade --check
```

The upgrade maintains Simit's README badge block directly below the top-level heading.
Badges reflect initialized features and project config, including
CI, Nix, docs, crates.io readiness, release artifacts, and configured distribution channels.

## Changelog management

Initialize a canonical Keep a Changelog file:

```sh
simit changelog init
```

Add entries under `[Unreleased]` with one standard section kind per entry:

```sh
simit changelog add added "support artifact workflows"
simit changelog add fixed "avoid detached-head release failures"
```

Promote `[Unreleased]` into a dated release section:

```sh
simit changelog release 0.4.0
simit changelog release 0.4.0 --date 2026-05-20 --repo-url https://github.com/caniko/simit
```

Validate or inspect a changelog section:

```sh
simit changelog check
simit changelog show
simit changelog show 0.3.1
```

`simit release` runs the same promotion logic if `CHANGELOG.md` exists:

```sh
simit changelog add added "describe the release"
simit release patch -m "release patch"
```

## Project hygiene

Run `simit init gitignore` to generate a compact, language-aware `.gitignore`
for the current project. Use `--check` in CI to verify it, or `--print` to
preview without writing.

The template covers common local outputs and adds Rust or uv/Python entries
when it detects those project types. Review custom rules before replacing an
existing .gitignore.

## CI wiring

Generate lightweight Rust CI and crates.io publish workflows:

```sh
simit init ci --platform forgejo
simit init ci --platform github
simit init ci --ci-provider crow
```

Simit writes `.github/workflows` with pinned native GitHub Actions and defaults
to GitHub-hosted `ubuntu-latest` and `windows-latest`. `.forgejo/workflows`
keeps Forgejo action references and runner-fleet defaults.

See the [Chaosbox v1 integration](docs/integrations/chaosbox-v1.md) for required
gates and dependency-ordered workspace publication on GitHub Actions.

Simit writes only project config under `.crow`; it never calls the Crow CLI
or server. Crow workflows default to v6.1 YAML, with optional Jsonnet, and Simit
renders the same CI, publishing, Pages, extension, plugin, artifact, and release
options into Crow workflows:

```sh
simit init ci --ci-provider crow --runtime cargo
simit init ci --ci-provider crow --crow-format jsonnet --runtime nix
simit init release --ci-provider crow
```

Configure Crow under `[ci.crow]`:

```toml
[ci]
provider = "crow"

[ci.crow]
format = "yaml" # or "jsonnet"
image = "rust:bookworm"
nix_image = "ghcr.io/cachix/devenv:latest"
# Optional: replace the generated check steps with a project-owned gate.
check_command = "nix develop -c cargo run --locked -p my-ci -- ci"
# Optional: the push copy alone receives this Crow repo secret.
check_push_secret = "ATTIC_TOKEN"
platform = "linux/amd64"
labels = { location = "europe" }
workspace_base = "/crow"
skip_clone = false

[ci.crow.variables.DEPLOY_ENV]
description = "Target environment"
options = ["staging", "production"]
default = "staging"
required = true
```

Crow workflows use native `labels`, `platform`, `when`, `workspace`, `variables`,
`skip_clone`, step `commands`, and `from_secret` bindings. Simit escapes shell
variables as `$${NAME}` for Crow interpolation. `check_command` keeps setup
and self-check but replaces the default flake/Cargo checks with one `project-check`
step. `check_push_secret` runs the same complete command in separate event-gated
push and pull-request steps. Only the push step gets a `from_secret` binding. Never list the
secret in `[ci].required_secrets`, which binds it to every step. For Nix, use a
`nix_image` whose container can write to `/nix`; the default may need
runner-specific permissions.

Forgejo defaults to direct Rust container jobs, even with `flake.nix`. Simit
chooses the default image from
`package.rust-version`: `rust:<version>-bookworm` for current MSRVs, or
`rust:<version>-trixie` once the matching official tag exists. Without
`rust-version`, Simit uses `rust:bookworm`. Only `--with-msrv` checks MSRV.

Set Forgejo runner labels in Simit's user config before generating workflows:

```sh
simit config init
$EDITOR "$(simit config path)"
simit config check
```

The config lives at `$XDG_CONFIG_HOME/simit/config.toml`, or
`~/.config/simit/config.toml` when `XDG_CONFIG_HOME` is unset. `init ci`
selects Forgejo runners from `[ci.defaults.forgejo]` and fails without a match
rather than guessing a non-portable label. Use `--runner` only for a one-off
explicit label override:

```sh
simit init ci --platform forgejo --runner custom-runner
```

Use `--runtime nix` only for workflows that need flake outputs, such as
cross-platform binaries or release artifacts. Forgejo Nix workflows use the
configured `nix` runner default and skip pull-request events:

```sh
simit init ci --platform forgejo --runtime nix
```

With a generated `.#site`, opt into a repository-local Pages workflow:

```sh
simit init ci --platform forgejo --runtime nix \
  --with-pages --pages-repo caniko/my-project
```

Simit writes `.forgejo/workflows/pages.yaml`. The workflow reads
`secrets.codeberg_token`, sets the `forgejo-actions` Git identity, adds an
authenticated `pages-origin` remote for the same Codeberg repository, and runs
`DEPLOY_REMOTE=pages-origin nix run .#deploy-pages`.

For GitHub, use the same Pages flag with `--platform github`. Simit writes
`.github/workflows/pages.yaml` to build the site, upload via pinned
`actions/upload-pages-artifact`, and deploy via the `github-pages` environment
using `actions/deploy-pages` with `pages: write` and OIDC permissions.

To move or check Pages without changing an existing Crow or Forgejo CI setup:

```sh
simit init ci --pages-only --platform github
simit init ci --pages-only --platform github --check --diff
```

You can also store Pages settings in project config:

```toml
[ci.pages]
repo = "caniko/my-project"
```

### Omnix CI (`--with-om-ci`)

With `--runtime nix`, `--with-om-ci` replaces generated flake checks and Cargo
dev-shell tests with [Omnix `om ci`](https://omnix.page/om/ci.html):

```sh
simit init ci --platform forgejo --runtime nix --with-om-ci
```

`--with-om-ci` defaults to replace mode, which trusts `checks.*` to cover tests,
clippy, fmt, and docs. Simit's generated crane flake meets that contract. Before
enabling replace mode on a custom flake (`[flake].mode = "custom"`), audit
`[flake.expected_outputs].checks` first; otherwise CI can pass while skipping
work from `nix develop -c cargo ...`.

Forgejo CI in replace mode:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow_ref }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1

      - name: Run om ci
        env:
          OMNIX_REF: "github:juspay/omnix/v1.3.2"
        run: nix run "$OMNIX_REF" -- ci run
```

For incomplete flake checks, `--om-ci-augment` runs `om ci` before legacy
Nix-runtime Cargo steps:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow_ref }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1

      - name: Run om ci
        env:
          OMNIX_REF: "github:juspay/omnix/v1.3.2"
        run: nix run "$OMNIX_REF" -- ci run

      - name: Check flake
        run: nix flake check

      - name: Test
        run: nix develop -c cargo test

      - name: Clippy
        run: nix develop -c cargo clippy --all-targets -- --deny warnings

      - name: Package crate
        run: nix develop -c cargo package --allow-dirty
```

Pin the Omnix flakeref via `--omnix-ref` or config.
Precedence: CLI `--omnix-ref` > project `[ci].omnix_ref` > user
`[ci.tools.omnix].ref` > simit's pinned default.

Use `--check` in CI to compare committed workflows with `simit`'s output:

```sh
simit init ci --platform forgejo --check
```

Drift errors print the regeneration command with runtime, runner, workspace,
and optional gate flags. Migration and runner debugging:
`docs/src/getting-started/ci-adoption.md`.

Simit renders project settings `[ci].extra_setup`, `[ci].extra_env`, and
`[ci].required_secrets` into every generated CI, publish, and artifact workflow.
Extra setup runs after checkout/toolchain setup and before tests or builds.
Extra env is job-scoped. Simit lists required secrets in comments without
reading them locally.
Set `[ci].pypi_token_secret` to override the default `PYPI_TOKEN` secret name,
or set `[ci].pypi_trusted_publishing = true` to use GitHub OIDC instead.

Generic `init ci` generates test, lint, and optional quality gates without
crates.io publishing. `--publish-crates` adds `publish-crate.yaml`, which runs
only on exact semver tag pushes (e.g. `0.9.0`). It verifies that the tag equals
the selected Cargo package's version, verifies its signature against
`keys/maintainers.gpg`, runs a publish dry run, then publishes with
`CRATES_IO_API_TOKEN`. Members with `publish = false` keep tests and clippy CI
but skip `cargo package` and `publish-crate` gates. During release-enabled
`init ci` generation, `init ci` finds the signing key via
`[release.signing].key`, `git config user.signingkey`, or `--maintainer-key`
and writes the maintainer public keyring.

Manage the trust root explicitly:

```sh
simit release trust status
simit release trust init
simit release trust check
```

The default trust root is `keys/maintainers.gpg`; override it with
`[release.signing].trust_root` or `--maintainers-gpg`.

Release workflows that sign checksums need matching minisign secrets in the
remote Actions secret store. `simit release secrets` verifies the pair before
upload and never prints secret values:

```sh
simit release secrets init --repo owner/repo \
  --minisign-secret-key-file /path/to/minisign.sec \
  --minisign-password-file /path/to/minisign.password
simit release secrets check --repo owner/repo \
  --assume-account-secret codeberg_token
```

For GitHub, `--platform github` uses the Actions store and local `gh` CLI
for secret encryption and listing:

```sh
simit release secrets init --platform github --repo owner/repo \
  --minisign-secret-key-file /path/to/minisign.sec \
  --minisign-password-file /path/to/minisign.password
```

Use `--rotate-minisign` only for a new release line: it generates a fresh
encrypted minisign key pair, uploads `MINISIGN_SECRET_KEY` and
`MINISIGN_PASSWORD`, and replaces `keys/minisign.pub`.

Add optional CI jobs and checks:

```sh
simit init ci --platform github --with-nextest --with-msrv --with-docs
simit init ci --platform forgejo --with-audit --with-deny --with-artifacts
simit init ci --platform forgejo --check --diff
```

`--with-msrv` requires `package.rust-version`.

Cargo-runtime workflows cache `~/.cargo/bin` by generated workflow files and use
`Swatinem/rust-cache@v2` for registry and target state by default. Optional
tools use `command -v ... ||
cargo install ... --locked` to skip downloads on hits and install on misses.
Nix-runtime workflows skip Cargo caches and rely on the runner's Nix store and
configured binary-cache behavior.

Forgejo or GitHub + Nix artifact workflows can also publish a Homebrew tap:

```sh
simit init ci --platform forgejo --runtime nix --with-artifacts --with-homebrew \
  --homebrew-tap https://codeberg.org/caniko/homebrew-demo.git \
  --homebrew-description "demo binary" \
  --homebrew-homepage https://example.com \
  --homebrew-download-repo caniko/demo \
  --homebrew-binary demo
```

`--with-homebrew` requires Nix and implies `--with-artifacts`. The project
workflow must stage each enabled platform archive at
`release/{name}-{version}-{arch}-{os}.tar.gz` before the Homebrew step, which
checks for these tarballs but doesn't build them.

## Homebrew automation

`simit` stores tap metadata in project config, bootstraps the Homebrew tap once,
and generates the project's Forgejo release step from that same config.

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

Bootstrap the tap with a placeholder formula:

```sh
simit init homebrew-tap --target ../homebrew-foo
# prints the next-step git commit and push hints
```

Wire the release workflow from the project repo:

```sh
simit init ci --platform forgejo --runtime nix \
  --with-artifacts --with-homebrew
```

Locally, render the formula or bump a checked-out tap from release archives:

```sh
simit dist homebrew render
simit dist homebrew bump \
  --version 0.3.1 \
  --tap ../homebrew-foo \
  --archive darwin_arm=release/foo-0.3.1-aarch64-darwin.tar.gz \
  --archive darwin_intel=release/foo-0.3.1-x86_64-darwin.tar.gz \
  --archive linux_arm=release/foo-0.3.1-aarch64-linux.tar.gz \
  --archive linux_intel=release/foo-0.3.1-x86_64-linux.tar.gz \
  --push
```

rs-modde's generated Homebrew release step publishes `modde` and
`modde-ui` to `caniko/homebrew-modde`.

## Windows packaging

Declare Chocolatey or Scoop packaging in project config; `simit` can then render
and publish them using the release-artifact metadata:

```toml
[chocolatey]
id = "foo"
title = "Foo"
authors = "Example Maintainers"
description = "Cross-platform foo manager"
summary = "Cross-platform foo manager for Windows users"
project_url = "https://foo.example.com"
icon_url = "https://foo.example.com/icon.png"
package_source_url = "https://codeberg.org/example/foo-package"
docs_url = "https://foo.example.com/docs"
bug_tracker_url = "https://codeberg.org/example/foo/issues"
project_source_url = "https://codeberg.org/example/foo"
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
simit init chocolatey --target packaging/chocolatey
simit init scoop-bucket --target ../scoop-foo
```

Wire Windows publishing into tagged release CI:

```sh
simit init ci --platform github --with-chocolatey --with-scoop
simit init ci --platform forgejo --with-chocolatey --with-scoop
```

`--with-chocolatey` and `--with-scoop` imply `--with-artifacts`. GitHub uses
`windows-latest` by default. Forgejo uses user config `[ci.defaults.forgejo].windows`
unless `--windows-runner` overrides it. Workflows read
`secrets.chocolatey_api_key` for Chocolatey pushes,
`secrets.homebrew_tap_token` for Homebrew tap pushes, and
`secrets.SCOOP_BUCKET_TOKEN` for Scoop bucket pushes unless project config
overrides those secret names.

Render, bump, or publish locally:

```sh
simit dist chocolatey render --output-dir packaging/chocolatey
simit dist chocolatey bump \
  --version 0.3.1 \
  --package-dir packaging/chocolatey \
  --archive x64=release/foo-0.3.1-x86_64-windows.zip

simit dist scoop render --output packaging/scoop/foo.json
simit dist scoop bump \
  --version 0.3.1 \
  --bucket ../scoop-foo \
  --archive x64=release/foo-0.3.1-x86_64-windows.zip \
  --archive arm64=release/foo-0.3.1-aarch64-windows.zip

simit dist windows publish \
  --version 0.3.1 \
  --archive x64=release/foo-0.3.1-x86_64-windows.zip \
  --work-dir target/simit-windows \
  --chocolatey --scoop --winget
```

## Distribution channels

Simit renders Linux packaging deterministically from `simit.toml`, Cargo
metadata, or `outputs.simitConfig`. Bootstrap channel files for committing:

```sh
simit init aur
simit init copr
simit init apt
simit init apt-repo --target ../apt-foo
simit init release
```

Each command supports `--check`, `--diff`, and `--print`. `init aur` writes
`dist/aur/<pkg>/PKGBUILD` for source, `-bin`, and `-git` flavors. `init copr`
writes the RPM spec and `.copr/Makefile`. `init apt` writes
`dist/apt/conf/distributions`. `init apt-repo` requires `[apt].public_url` and
`dist/apt/key.gpg.asc`. It bootstraps a separate repository-backed Codeberg Pages
site with `conf/distributions`, the public signing key, a Pages workflow, and a
README. Run it from the package project, then push the target repo's configured
Pages branch once. `init release` writes `.forgejo/workflows/release.yml` by
default or `.github/workflows/release.yml` with `--platform github`. The workflow
uploads artifacts to Codeberg with `[release.codeberg]` or GitHub with
`[release.github]`, then publishes all configured channels.

Cargo release artifacts, promotion, and republish use the runner's `git` and
`sha256sum`, plus `curl`, `jq`, and `tar` as needed. Signing uses checksum-pinned
minisign and Cosign on Linux x86-64 or arm64; of the signing tools, republish
needs only minisign.
Only jobs that need Nix set it up (e.g. flake builds and Nix-based publishers).
If a custom shell command needs Nix, invoke it explicitly (e.g.
`nix develop -c ./scripts/smoke`) so Simit detects it.

Draft changelog candidates separately with
`simit changelog draft [--base-ref REF] [--dry-run]`. Set
`[release.changelog] auto_draft = true` to run the same fail-closed drafting
step before `simit release` promotes `[Unreleased]`. Optionally add project
guidance in `.skills/release-changelog/SKILL.md`. Codex stays read-only; Simit
validates and inserts its entries.

Preview the same templates without writing files, or run APT steps locally:

```sh
simit dist aur render
simit dist copr render
simit dist apt render
simit dist apt build --version 0.3.1 --release-dir release
simit dist apt publish --version 0.3.1 --release-dir release --push
simit dist apt verify --repo ../apt-foo --version 0.3.1
```

Project config sections: `[aur]`, `[copr]`, `[apt]`,
`[homebrew]`, `[chocolatey]`, `[scoop]`, `[flatpak]`, `[winget]`,
`[release.codeberg]`, `[release.github]`, `[release.artifacts]`, `[release.attic]`,
`[release.changelog]`, `[release.announce]`, `[release.windows_signing]`, and `[release.pypi]`
(prebuilt `release/*.whl` to PyPI; see the distribution-channels guide).
Override publish workflow secret names with `[aur].ssh_key_secret`,
`[copr].login_secret`, `[copr].username_secret`, `[copr].token_secret`,
`[apt].gpg_key_secret`, `[apt].gpg_key_id_secret`,
`[apt].gpg_passphrase_secret`, `[apt].ssh_key_secret`,
`[homebrew].tap_token_secret`, `[scoop].bucket_token_secret`,
`[chocolatey].api_key_secret`, `[chocolatey].api_key_env`,
`[chocolatey].api_key_from_runner`, `[release.codeberg].token_secret`,
`[release.github].token_secret`,
`[flatpak].token_secret`, `[winget].token_secret`,
`[release.announce].*_secret`,
`[release.windows_signing].*_secret` (including SignPath `api_token_secret`), and `[release.pypi].token_secret`.

APT reuses a repository-backed Pages branch across releases:

```toml
[apt]
repo_url = "ssh://git@codeberg.org/example/apt-foo.git"
public_url = "https://apt.foo.example/"
branch = "pages"
packages = ["foo=foo"]

[apt.pages]
provider = "codeberg-git-pages"
runner = "atlas-nix-trusted"

[release.publish]
enforcement = "activated-remote"

[release.publish.channels]
apt = "required"
scoop = "required"
homebrew = "staged"
```

The template builds `.deb` files with `simit dist apt build`, then uses
`simit dist apt publish` to update the existing reprepro database and push
a commit that preserves history. It publishes Scoop with `simit dist scoop bump`.
Channel policies can require APT/Scoop while another channel stays staged
during rollout.

For harbor-rs-compatible Nix projects, list flake attributes producing flat
release bundles in `[release.artifacts].nix_bundle_attrs`. Include exactly one
versioned `*-release-manifest.json` across the bundles. Simit validates it against
the tag before checksums and hosted-release upload. Existing `build_commands`
still add installers and other project-specific formats.

For the conventional harbor-rs output, `[release.artifacts].prebuild_binaries = true`
means `nix_bundle_attrs = ["release-bundle"]` unless you provide an explicit list.

When a bundle needs paths from a private substituter, set
`[release.artifacts].netrc_secret` to the name of a GitHub Actions secret
holding netrc content, e.g. `machine attic.example.com password
<token>`. Every GitHub release and prebuild job that installs Nix with
`[release.artifacts]` substituters first writes that content to a mode-0600 file
under `$RUNNER_TEMP` and points `netrc-file` at it. Use a pull-only token:
same-repository pull-request prebuilds get it too. Fork pull requests get an
empty netrc and substitute anonymously. Other release backends reject this key.

GitHub flake-input publication supplements the primary CI provider. Point
`attic_app` at an app built with `rs-harbor.lib.mkAtticPush { flake = "."; }`:

```toml
[prebuild]
publish_attic = true
attic_app = ".#push-flake-inputs"

[prebuild.system_runners]
"x86_64-linux" = "ubuntu-24.04"
```

Simit invokes the app only on the repository's default branch or a trusted
release call; pull requests and feature branches never receive the token.

By default, prebuild realizes `[ci].nix_builds` without a separate
`nix-builds.yaml`. With GitHub Actions as primary CI, set `[prebuild].nix_builds`
(the published subset, e.g. packages) or configure `[ci.nix_build]`
(e.g. `groups` with `check_systems`) for a separate `[ci].nix_builds`
qualification matrix. `simit init ci --prebuild-only` keeps that matrix. With
Forgejo, GitLab, or Crow as primary CI, there is no separate `[ci].nix_builds`
matrix and `--prebuild-only` removes a stale one. `[ci.nix_build].only` is
incompatible with `[prebuild]`.

## Project config

To persist project settings, choose exactly one config source:

- `simit.toml` at the Cargo workspace root.
- `[workspace.metadata.simit]` or `[package.metadata.simit]` in root
  `Cargo.toml`.
- `outputs.simitConfig` in `flake.nix`.

All sources share section names: `[flake]`, `[ci]`, `[homebrew]`,
`[chocolatey]`, `[scoop]`, `[aur]`, `[copr]`, `[apt]`, `[flatpak]`,
`[winget]`, and `[release.*]`.

Homebrew precedence per setting: CLI flag, project config, Cargo package
metadata, then an error. Supply `tap_url` and `download_repo` via flag or
project config; neither falls back to Cargo metadata.

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

With `[flake].mode = "custom"`, projects can keep their own Nix inputs and
outputs while letting Simit manage formatter/pre-commit hook definitions;
`simit init flake --check` validates Simit's required wiring semantically rather
than against the canonical template:

```toml
[flake]
mode = "custom"
toolchain_binding = "toolchain.rustToolchain"
crane_lib_binding = "craneLib"
package_binding = "package"

[flake.expected_outputs]
packages = ["default", "docs", "site"]
checks = ["default", "formatting", "clippy", "fmt", "nextest", "doc", "audit", "deny", "hm-module"]
top_level = ["hmModules"]
```

Configure persistent CI additions without editing generated YAML:

```toml
[ci]
all_features = false  # keep optional platform features in dedicated checks
unit_tests_only = true # leave service-backed integration tests to their workflow
extra_setup = [
  "apt-get update && apt-get install -y --no-install-recommends postgresql-client",
]
extra_env = { SKILLNET_TEST_PG_URL = "${{ secrets.SKILLNET_TEST_PG_URL }}" }
required_secrets = ["SKILLNET_TEST_PG_URL", "CRATES_IO_API_TOKEN"]
```

## User config

User config keeps local infrastructure, such as runner labels, out of project
config so repositories stay portable.

```toml
[ci.runners.atlas]
platform = "forgejo"
labels = ["atlas"]
os = "linux"
arch = "x86_64"
runtimes = ["cargo", "nix"]
trusted = true

[ci.runners.windows_atlas]
platform = "forgejo"
labels = ["windows-atlas"]
os = "windows"
arch = "x86_64"
runtimes = ["cargo"]

[ci.defaults.forgejo]
cargo = "atlas"
nix = "atlas"
release = "atlas"
windows = "windows_atlas"
```

Each default names a runner from `[ci.runners]`. Simit writes a workflow only
if the runner matches the platform, operating system, and runtime. GitHub
falls back to built-in `ubuntu-latest` and `windows-latest` unless overridden
in user config.

In Cargo metadata, nest the project schema under `metadata.simit`:

```toml
[workspace.metadata.simit.homebrew]
tap_url       = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
```

In a flake, export the same schema as JSON-compatible Nix data:

```nix
{
  outputs = {self, simit, ...}: {
    simitConfig = simit.lib.mkSimitConfig {
      homebrew = {
        tap_url = "https://codeberg.org/caniko/homebrew-mythos.git";
        download_repo = "caniko/mythos";
      };
      flake = {
        mode = "custom";
        toolchain_binding = "toolchain.rustToolchain";
        expected_outputs.checks = ["hm-module"];
        expected_outputs.top_level = ["hmModules"];
      };
      ci = {
        extra_setup = ["echo project setup"];
        extra_env.SKILLNET_TEST_PG_URL = "\${{ secrets.SKILLNET_TEST_PG_URL }}";
      };
    };
  };
}
```

## Flake and hook wiring

Generate the canonical Rust crane flake, formatter, and pre-commit hook
definitions:

```sh
simit init flake
```

Simit writes `flake.nix`, `nix/treefmt.nix`, and `nix/pre-commit.nix`, detects
Rust, Nix, uv-based Python, TOML, YAML, and Markdown files, and wires
`treefmt-nix` and `cachix/git-hooks.nix`. It patches an existing `flake.nix`
only if it finds safe anchors; otherwise use `--print` and apply the wiring
manually.

In `[flake].mode = "custom"`, `simit init flake` writes only `nix/treefmt.nix`
and `nix/pre-commit.nix`, leaving `flake.nix` intact. `simit init flake --check`
requires treefmt/git-hooks inputs, `treefmtEval` and `pre-commit-check`
bindings, formatter/check/dev-shell hook wiring, and configured expected
outputs. Helpers such as `harbor-rs` can supply project-specific outputs without
Simit rendering them.

Preview the generated files without writing them:

```sh
simit init flake --print
```

Check committed flake and hook files in CI:

```sh
simit init flake --check
simit init flake --check --diff
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

Before publishing, confirm `CHANGELOG.md` has the intended `## [Unreleased]`
entries. Optionally run `simit changelog check`, then release:

```sh
simit release patch -m "release patch"
```

Only an exact semver release tag push triggers the publish workflow, which
requires `CRATES_IO_API_TOKEN` and `keys/maintainers.gpg`. `simit init ci --publish-crates`
and `simit release trust init` generate the keyring from the configured release
signing key.

If the pushed tag's workflow fails, commit the fix and rerun the release pipeline:

```sh
simit release sync-up --push
```

## License

`simit` uses the MIT License. See `LICENSE`.
