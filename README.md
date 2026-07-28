# simit

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-drift-2088ff)](.forgejo/workflows/ci.yaml) [![Nix](https://img.shields.io/badge/Nix-drift-5277c3)](flake.nix) [![docs](https://img.shields.io/badge/docs-enabled-6f42c1)](docs) [![crates.io](https://img.shields.io/badge/crates.io-ready-f46623)](https://crates.io/crates/simit)

<!-- simit:badges:end -->

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

## Project registry

Simit records projects it has acted on in a per-user registry. Inspect that
state with `simit projects`:

```sh
simit projects list
simit projects list --json --feature flake=managed
simit projects show
simit projects discover ~/Projects --dry-run
simit projects scan --prune
simit projects clear-state --dry-run
```

The JSON output is intended for scripts and AI agents that need to find all
projects where simit manages a feature such as flake, CI, or packaging.
Run `simit projects discover <ROOT>` once per machine to add existing
simit-managed projects to the registry.

## Project upgrades

Apply deterministic simit upgrades to the current workspace, explicit paths, or
all registered simit-managed projects:

```sh
simit upgrade
simit upgrade --path ../project-a --path ../project-b
simit upgrade --all
simit upgrade --dry-run --diff
simit upgrade --check
```

The current upgrade maintains a simit-managed README badge block immediately
after the top-level heading. Badges are generated from actual initialized
features and project configuration, including CI, Nix, docs, crates.io readiness,
release artifacts, and configured distribution channels.

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

## Project hygiene

Generate a compact, language-aware .gitignore for the current project with
simit init gitignore. Use --check to verify it in CI and --print to inspect the
rendered template without writing it.

The template covers common local outputs and adds Rust or uv/Python entries
when those project types are detected. Existing project-specific ignore rules
should be reviewed before replacing an older .gitignore.

## CI wiring

Generate lightweight Rust CI and crates.io publish workflows for a repository:

```sh
simit init ci --platform forgejo
simit init ci --platform github
simit init ci --ci-provider crow
```

GitHub output is written under `.github/workflows`, uses native pinned GitHub
Actions, and defaults to GitHub-hosted runners (`ubuntu-latest` and
`windows-latest`). Forgejo output remains under `.forgejo/workflows` and keeps
its Forgejo action references and runner-fleet defaults.

Crow output is project-side configuration under `.crow`; simit does not invoke
the Crow CLI or contact a Crow server. It emits Crow v6.1 YAML by default, or
Jsonnet when selected, and renders the same CI, publishing, Pages, extension,
plugin, artifact, and release options into Crow workflows:

```sh
simit init ci --ci-provider crow --runtime cargo
simit init ci --ci-provider crow --crow-format jsonnet --runtime nix
simit init release --ci-provider crow
```

Crow-specific settings live below `[ci.crow]`:

```toml
[ci]
provider = "crow"

[ci.crow]
format = "yaml" # or "jsonnet"
image = "rust:bookworm"
nix_image = "ghcr.io/cachix/devenv:latest"
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

Generated Crow workflows use Crow's native `labels`, `platform`, `when`,
`workspace`, `variables`, `skip_clone`, step `commands`, and `from_secret`
bindings. Shell variables are escaped as `$${NAME}` for Crow interpolation.

Forgejo workflows use direct Rust container jobs by default, even when the
repository has a `flake.nix`. The default container is derived from
`package.rust-version`: `rust:<version>-bookworm` for current MSRVs, or
`rust:<version>-trixie` once the matching official tag exists. Without
`rust-version`, simit uses `rust:bookworm`. MSRV is only checked when
`--with-msrv` is requested.

Forgejo runner labels are user infrastructure. Before generating Forgejo
workflows, configure your local runner fleet in the simit user config:

```sh
simit config init
$EDITOR "$(simit config path)"
simit config check
```

The config lives at `$XDG_CONFIG_HOME/simit/config.toml`, or
`~/.config/simit/config.toml` when `XDG_CONFIG_HOME` is unset. `init ci`
selects Forgejo runners from `[ci.defaults.forgejo]`; if no matching default is
configured, it fails instead of guessing a non-portable label. Use `--runner`
only for a one-off explicit label override:

```sh
simit init ci --platform forgejo --runner custom-runner
```

Use `--runtime nix` only for workflows that intentionally need flake outputs,
such as cross-platform binary builds or release artifacts. Forgejo Nix
workflows use the configured `nix` runner default and do not run on
pull-request events:

```sh
simit init ci --platform forgejo --runtime nix
```

Projects that expose a generated `.#site` and `.#deploy-pages` app, such as
`plinth-project` Codeberg Pages sites, can opt into the repository-local Pages
deployment hook:

```sh
simit init ci --platform forgejo --runtime nix \
  --with-codeberg-pages --pages-repo caniko/my-project
```

This writes `.forgejo/workflows/pages.yaml`, reads
`secrets.codeberg_token`, configures the `forgejo-actions` git identity, adds an
authenticated `pages-origin` remote for the same Codeberg repository, and runs
`DEPLOY_REMOTE=pages-origin nix run .#deploy-pages`. The same settings can live
in project config:

On GitHub, use the same flag with `--platform github`. Simit writes
`.github/workflows/pages.yaml`, builds the site, uploads it with the pinned
`actions/upload-pages-artifact` action, and deploys it with
`actions/deploy-pages` using `pages: write` and OIDC permissions.

```toml
[ci.pages]
repo = "caniko/my-project"
```

### Omnix CI (`--with-om-ci`)

Use `--with-om-ci` with `--runtime nix` to replace the generated Nix flake
check and cargo dev-shell test steps with [Omnix `om ci`](https://omnix.page/om/ci.html):

```sh
simit init ci --platform forgejo --runtime nix --with-om-ci
```

Replace mode is the default when `--with-om-ci` is set. It trusts the flake's
`checks.*` to cover the project's test, clippy, fmt, and doc gates. Projects
using simit's generated crane flake satisfy that contract. Custom flakes
(`[flake].mode = "custom"`) must audit `[flake.expected_outputs].checks`
before enabling replace mode; otherwise CI can go green while skipping work the
legacy `nix develop -c cargo ...` steps used to run.

For Forgejo, replace mode emits this CI workflow:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
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

Use `--om-ci-augment` when the flake checks are not yet complete. Augment mode
runs `om ci` first, then keeps the legacy Nix-runtime cargo steps:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
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

Pin the Omnix flakeref with `--omnix-ref`, project config, or user config. The
resolution order is CLI `--omnix-ref` > project `[ci].omnix_ref` > user
`[ci.tools.omnix].ref` > simit's pinned default.

Use `--check` in CI to make sure committed workflows still match `simit`'s
generated output:

```sh
simit init ci --platform forgejo --check
```

When check mode reports drift, the error prints the effective regeneration
command, including runtime, runner, workspace, and optional gate flags. For
more migration rules and runner-debugging notes, see
`docs/src/getting-started/ci-adoption.md`.

When `[ci].extra_setup`, `[ci].extra_env`, or `[ci].required_secrets` are set
in project config, simit renders them into every generated CI, publish, and
artifact workflow. Extra setup runs after checkout/toolchain setup and before
tests or builds; extra env is job-level environment; required secrets are
documented as workflow comments but are not read locally.

Generic `init ci` runs render test, lint, and optional quality-gate workflows
without crates.io publishing. Add `--publish-crates` for release projects that
need a separate `publish-crate.yaml` workflow. That workflow runs only on exact
semver tag pushes such as `0.9.0`, verifies that the tag matches the Cargo
package version, verifies the signed tag against `keys/maintainers.gpg`, runs a
publish dry run, and publishes with the `CRATES_IO_API_TOKEN` secret. Workspace
members with `publish = false` keep test and clippy CI but do not get
`cargo package` or `publish-crate` gates. On release-enabled generation,
`init ci` discovers the release signing key from `[release.signing].key`,
`git config user.signingkey`, or `--maintainer-key`, then writes the maintainer
public keyring.

You can manage that trust root explicitly:

```sh
simit release trust status
simit release trust init
simit release trust check
```

The default trust root is `keys/maintainers.gpg`; override it with
`[release.signing].trust_root` or `--maintainers-gpg`.

Release workflows that sign checksums need matching minisign secrets in the
remote Actions secret store. `simit release secrets` verifies
the key pair before upload and never prints secret values:

```sh
simit release secrets init --repo owner/repo \
  --minisign-secret-key-file /path/to/minisign.sec \
  --minisign-password-file /path/to/minisign.password
simit release secrets check --repo owner/repo \
  --assume-account-secret codeberg_token
```

For GitHub, add `--platform github`; the command delegates secret encryption and
listing to the local `gh` CLI and uses the Actions secret store:

```sh
simit release secrets init --platform github --repo owner/repo \
  --minisign-secret-key-file /path/to/minisign.sec \
  --minisign-password-file /path/to/minisign.password
```

Use `--rotate-minisign` only for a new release line: it generates a fresh
encrypted minisign key pair, uploads `MINISIGN_SECRET_KEY` and
`MINISIGN_PASSWORD`, and replaces `keys/minisign.pub`.

Add optional CI jobs and checks when the project needs them:

```sh
simit init ci --platform github --with-nextest --with-msrv --with-docs
simit init ci --platform forgejo --with-audit --with-deny --with-artifacts
simit init ci --platform forgejo --check --diff
```

`--with-msrv` requires `package.rust-version`.

Cargo-runtime workflows include warm-cache steps by default: a `~/.cargo/bin`
cache keyed by the generated workflow files, plus `Swatinem/rust-cache@v2` for
Cargo registry and target state. Optional tool installs use `command -v ... ||
cargo install ... --locked` so a cache hit skips the download while a cache miss
still installs the required binary. Nix-runtime workflows skip these Cargo
caches and rely on the runner's Nix store and binary cache behavior.

Forgejo or GitHub + Nix artifact workflows can also publish a Homebrew tap:

```sh
simit init ci --platform forgejo --runtime nix --with-artifacts --with-homebrew \
  --homebrew-tap https://codeberg.org/caniko/homebrew-demo.git \
  --homebrew-description "demo binary" \
  --homebrew-homepage https://example.com \
  --homebrew-download-repo caniko/demo \
  --homebrew-binary demo
```

`--with-homebrew` requires Nix and implies `--with-artifacts`. The
project workflow must stage each enabled platform archive under
`release/{name}-{version}-{arch}-{os}.tar.gz` before the Homebrew step runs;
the generated step verifies those files exist but does not build project-shaped
tarballs itself.

## Homebrew automation

Homebrew tap publishing tends to grow a lot of release-CI boilerplate. `simit`
keeps the tap metadata in project config, bootstraps the tap once, and
generates the Forgejo release step from that same config.

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
simit init homebrew-tap --target ../homebrew-foo
# prints the next-step git commit and push hints
```

Wire the release workflow from the project repo:

```sh
simit init ci --platform forgejo --runtime nix \
  --with-artifacts --with-homebrew
```

For local inspection and iteration, render the formula or bump a checked-out
tap using release archives:

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

rs-modde is the worked example for this flow: its release CI publishes
`modde` and `modde-ui` to `caniko/homebrew-modde` from the generated Homebrew
step.

## Windows packaging

`simit` can render and publish Windows packages for Chocolatey and Scoop from
the same project metadata used by release artifacts. Declare the package
surface in project config:

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
`windows-latest` by default. Forgejo uses `[ci.defaults.forgejo].windows` from
the simit user config unless `--windows-runner` overrides it. Generated
workflows read `secrets.chocolatey_api_key` for Chocolatey pushes,
`secrets.homebrew_tap_token` for Homebrew tap pushes, and
`secrets.SCOOP_BUCKET_TOKEN` for Scoop bucket pushes unless project config
overrides those secret names.

For local inspection and iteration:

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

Linux distribution packaging is configured from `simit.toml`, Cargo metadata,
or `outputs.simitConfig` and rendered deterministically. Bootstrap committed
channel files with:

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
`dist/apt/conf/distributions`. `init apt-repo` bootstraps a separate
repository-backed Codeberg Pages site with `conf/distributions`, the public
signing key, a generated Pages workflow, and a README. It requires
`[apt].public_url` and `dist/apt/key.gpg.asc`; run it from the package project
and push the target repository's configured Pages branch once. `init release` writes
`.forgejo/workflows/release.yml` by default, or `.github/workflows/release.yml`
with `--platform github`. It uploads artifacts to Codeberg when
`[release.codeberg]` is present or to GitHub when `[release.github]` is present,
then publishes every configured channel.

For local inspection, render the same templates without writing files:

```sh
simit dist aur render
simit dist copr render
simit dist apt render
simit dist apt build --version 0.3.1 --release-dir release
simit dist apt publish --version 0.3.1 --release-dir release --push
simit dist apt verify --repo ../apt-foo --version 0.3.1
```

Relevant project config sections are `[aur]`, `[copr]`, `[apt]`,
`[homebrew]`, `[chocolatey]`, `[scoop]`, `[flatpak]`, `[winget]`,
`[release.codeberg]`, `[release.github]`, `[release.artifacts]`, `[release.attic]`,
`[release.announce]`, and `[release.windows_signing]`. The publish workflow
secret names are configurable with `[aur].ssh_key_secret`,
`[copr].login_secret`, `[copr].username_secret`, `[copr].token_secret`,
`[apt].gpg_key_secret`, `[apt].gpg_key_id_secret`,
`[apt].gpg_passphrase_secret`, `[apt].ssh_key_secret`,
`[homebrew].tap_token_secret`, `[scoop].bucket_token_secret`,
`[chocolatey].api_key_secret`, `[chocolatey].api_key_env`,
`[chocolatey].api_key_from_runner`, `[release.codeberg].token_secret`,
`[release.github].token_secret`,
`[flatpak].token_secret`, `[winget].token_secret`,
`[release.announce].*_secret`, and
`[release.windows_signing].*_secret`.

APT uses a repository-backed Pages branch rather than rebuilding a fresh
repository tree on every release:

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

The release template builds `.deb` files with `simit dist apt build`, then
uses `simit dist apt publish` to update the existing reprepro database and
push its history-preserving commit. Scoop publication is likewise delegated
to `simit dist scoop bump`; explicit channel policies allow a repository to
make APT/Scoop required while keeping another channel staged during rollout.

For rs-harbor-compatible Nix projects, set
`[release.artifacts].nix_bundle_attrs` to an explicit list of flake attributes
whose outputs are flat release bundles. The configured bundle set must contain
exactly one versioned `*-release-manifest.json`; Simit validates that manifest
against the tag before checksums and hosted-release upload. Existing
`build_commands` remain additive for installers and other project-specific
formats.

Projects using the conventional rs-harbor output can opt in with the shorter
`[release.artifacts].prebuild_binaries = true`; this is equivalent to
`nix_bundle_attrs = ["release-bundle"]` unless an explicit attribute list is
provided.

## Project config

Projects may opt in to stable simit settings with exactly one project config
source. Supported sources are:

- `simit.toml` at the Cargo workspace root.
- `[workspace.metadata.simit]` or `[package.metadata.simit]` in root
  `Cargo.toml`.
- `outputs.simitConfig` in `flake.nix`.

All sources use the same section names: `[flake]`, `[ci]`, `[homebrew]`,
`[chocolatey]`, `[scoop]`, `[aur]`, `[copr]`, `[apt]`, `[flatpak]`,
`[winget]`, and `[release.*]`.

For each Homebrew setting, resolution order is: CLI flag, simit project config,
Cargo package metadata, then an error. `tap_url` and `download_repo` have no
Cargo metadata fallback, so they must be set by a flag or in project config.

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

Projects with custom flakes can keep their own Nix inputs and outputs while
letting simit manage formatter/pre-commit hooks. Set `[flake].mode = "custom"`
to make `simit init flake --check` validate simit's required wiring
semantically instead of comparing against the canonical flake template:

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

Project CI can also declare stable workflow additions without hand-editing the
generated YAML:

```toml
[ci]
extra_setup = [
  "apt-get update && apt-get install -y --no-install-recommends postgresql-client",
]
extra_env = { SKILLNET_TEST_PG_URL = "${{ secrets.SKILLNET_TEST_PG_URL }}" }
required_secrets = ["SKILLNET_TEST_PG_URL", "CRATES_IO_API_TOKEN"]
```

## User config

User config is intentionally separate from project config. It stores local
infrastructure such as runner labels, while repositories stay portable.

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

Each default names a runner from `[ci.runners]`. The selected runner must match
the requested platform, operating system, and runtime before simit writes a
workflow. GitHub keeps portable built-in fallbacks (`ubuntu-latest` and
`windows-latest`) when user config does not override them.

Project config in Cargo metadata nests the same project schema under
`metadata.simit`:

```toml
[workspace.metadata.simit.homebrew]
tap_url       = "https://codeberg.org/caniko/homebrew-mythos.git"
download_repo = "caniko/mythos"
```

Flake config exports the same schema as JSON-compatible Nix data:

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

Generate a canonical Rust crane flake plus formatter and pre-commit hook definitions:

```sh
simit init flake
```

This writes `flake.nix`, `nix/treefmt.nix`, and `nix/pre-commit.nix`,
detects Rust, Nix, uv-based Python, TOML, YAML, and Markdown files, and wires
`treefmt-nix` and `cachix/git-hooks.nix`. Existing `flake.nix` files are
patched only when simit can find safe anchors; otherwise, use `--print` and
apply the generated wiring manually.

In custom flake mode, `simit init flake` writes only `nix/treefmt.nix` and
`nix/pre-commit.nix`; the repository-owned `flake.nix` is left intact. The
check mode requires treefmt/git-hooks inputs, the `treefmtEval` and
`pre-commit-check` bindings, formatter/check/dev-shell hook wiring, and any
configured expected outputs. This supports flakes based on helpers such as
`rs-harbor` without requiring simit to render every project-specific output.

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

Before publishing a release, make sure `CHANGELOG.md` has the intended
`## [Unreleased]` entries. You can validate them explicitly with
`simit changelog check`, then run:

```sh
simit release patch -m "release patch"
```

The crates.io publish workflow runs when the release tag is pushed and requires
`CRATES_IO_API_TOKEN`. It also requires `keys/maintainers.gpg`, which
`simit init ci --publish-crates` and `simit release trust init` generate from
the configured release signing key.

If that tag-triggered workflow fails after the tag has already been pushed,
commit the fix and rerun the release pipeline with:

```sh
simit release sync-up --push
```

## License

`simit` is licensed under the MIT License. See `LICENSE`.
