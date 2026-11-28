# Distribution Channels

`simit` can keep distribution packaging files and tag-triggered release
workflows in sync with project config. It also generates editor publisher and
Crow workflows:

- AUR: `simit init aur` writes `dist/aur/<pkg>/PKGBUILD` files for source,
  `-bin`, and `-git` flavors.
- COPR: `simit init copr` writes the RPM spec and `.copr/Makefile`.
- apt: `simit init apt` writes `dist/apt/conf/distributions` for reprepro.
- Release workflow: `simit init release` writes `.forgejo/workflows/release.yml`
  by default or `.github/workflows/release.yml` for GitHub. The generated
  workflow builds artifacts, uploads the hosted release, and publishes
  configured channels.
- Editor marketplaces: `simit init ci --with-vscode` and
  `simit init ci --with-jetbrains` write the VS Code Marketplace/Open VSX and
  JetBrains publisher workflows (see [Editor Marketplaces](#editor-marketplaces)).
- Crow: `simit init ci --ci-provider crow` and
  `simit init release --ci-provider crow` write project-side Crow v6.1
  workflows under `.crow` without a Crow CLI or server. YAML is the default;
  set `[ci.crow].format = "jsonnet"` for Jsonnet.

All four init commands support `--check`, `--diff`, and `--print`.

```sh
simit init aur
simit init aur --check --diff
simit init copr
simit init apt
simit init release
simit init ci --ci-provider crow --runtime nix
simit init release --ci-provider crow
```

Render the same channel templates to stdout:

```sh
simit dist aur render
simit dist copr render
simit dist apt render
```

## Configuration

Keep exactly one Simit project config source per repo: `simit.toml`, root Cargo
metadata under `[workspace.metadata.simit]` or `[package.metadata.simit]`, or
flake `outputs.simitConfig`. All use the same schema.

```toml
[ci]
provider = "crow"

[ci.crow]
format = "yaml"
nix_image = "ghcr.io/cachix/devenv:latest"

[release.codeberg]
repo = "example/foo"

# Use this instead for GitHub releases:
# [release.github]
# repo = "example/foo"

[release.artifacts]
runner = "atlas"
build_commands = [
  "mkdir -p release",
  "cargo build --release --locked",
]

[aur]
download_repo = "example/foo"

[copr]
download_repo = "example/foo"
project = "example/foo"
# Optional COPR SCM entrypoint; defaults to ".copr/Makefile".
makefile_path = "dist/copr/Makefile"
# Shared release-workflow build/publish directory; defaults to "srpms".
srpm_dir = "target/release-artifacts/srpms"

[apt]
repo_url = "ssh://git@codeberg.org/example/foo-apt.git"
public_url = "https://apt.example.com/"
```

`copr.makefile_path` is a workspace-relative path without parent traversal.
Render, init, drift checks, next-step instructions, and the managed COPR badge
use it. The managed apt badge links to `apt.public_url` if set, otherwise to
`dist/apt/conf/distributions`.

The COPR release workflow creates the `repo-vVERSION.tar.gz` basename expected
by the generated RPM spec. `copr.srpm_dir` sets a shared workspace-relative
staging directory for build and publication. Publication requires exactly one
nonempty `.src.rpm` there.

AUR metadata generation works on unprivileged and root release runners. Root
runners run `makepkg` via `setpriv` with an unprivileged UID/GID, then restore
the clone's ownership on success or failure. Only successful generation replaces
metadata and allows publication.

Chocolatey publishes from the project's declared Nix development environment
using the external packager in `chocolatey.nix_tool`. Keep
`<package-dir>/.simit-candidates/` for retries: it holds the exact packed
candidate and source/content hashes. A retry skips publication only if the
downloaded registry package is byte-identical. Different bytes, failed lookups
and unverified push failures are errors. When pushing to the public Chocolatey
service, verification uses its community NuGet v2 feed. For custom HTTP(S) push
sources, provide the NuGet v2 base URL or NuGet v3 service index. Packing and
lookups never receive the configured API-key environment variable.

Use `simit dist chocolatey bump --pack` with the same package/archive options
to keep a candidate without registry access or an API key. Actions release
workflows restore and explicitly save the source-bound candidate before
publication, then require a successful restore so fresh hosted retries use the
original bytes. The cache key includes repository, ref, commit and generated
payload hashes. Packing finds the single nonempty package and keeps its actual
filename, including Chocolatey CLI 2.x version normalization.

To verify bytes, Simit discovers NuGet v3 `PackageBaseAddress/3.0.0` resources
from the configured service index and uses normalized, lowercase package
IDs/versions in content URLs. NuGet v2 uses its package download route.
Normalized filenames and lookup versions omit build metadata. Unavailable or
malformed indexes fail verification. Discovery and downloads require anonymous
reads, even when publishing needs an API key. They receive no publication
credentials: HTTP 401/403 fails before upload, with no credentialed retry or
credential forwarding through redirects.

If the harbor-rs flake exposes `packages.<system>.release-bundle`, opt in with:

```toml
[release.artifacts]
prebuild_binaries = true

[release.changelog]
auto_draft = true
```

An explicit `nix_bundle_attrs` list still takes precedence when a project
publishes multiple bundles.

### cargo-binstall

`cargo binstall` reads `[package.metadata.binstall]` from the crate manifest.
Harbor-rs derives it from the release bundle and simit keeps the two equal.
Declare the check next to the bundle:

```nix
packages.release-bundle = binaryRelease.releaseBundle;

checks.binstall-metadata = harbor.lib.rust.mkBinstallCheck {
  inherit pkgs;
  bundle = binaryRelease.releaseBundle;
  cargoToml = ./Cargo.toml;
};
```

`mkBinstallRelease` exposes archives per system. Use `binstall-metadata` for
one binary crate or `binstall-metadata-<crate>` for a workspace with several.

Build the bundles on native runners and require the metadata in the manifest:

```toml
[release.artifacts]
prebuild_binaries = true
binstall = true
system_runners = { x86_64-linux = "ubuntu-24.04", aarch64-darwin = "macos-15" }
```

With `system_runners`, one job per system builds `packages.<system>.<bundle>`
on the specified runner and uploads its archives. The workflow merges them
into one checksummed, signed release. Tag validation precedes all builds;
duplicate archive names across systems fail publication. Without
`system_runners`, the release job builds the bundle inline.

`simit binstall sync` writes evaluated `checks.<system>.binstall-metadata*.toml`
into each publishable binary crate's `Cargo.toml`, replacing only
`[package.metadata.binstall*]` tables. `--check` prints a unified diff for each
drifted package and exits nonzero. `--package NAME` limits selection;
`--system SYS` overrides the host system. For any crate with binaries but no
check, it reports `missing` and suggests exposing `mkBinstallRelease` checks.
With `binstall = true`, `simit release verify` runs the same check fatally.

### Release platforms

`platforms` lists release operating systems without repeating the Nix system
table. `linux` expands to `x86_64-linux` on
`ubuntu-24.04`, `macos` to `aarch64-darwin` on `macos-15`, and `windows` adds
no runner: the `x86_64-linux` job cross-builds `x86_64-pc-windows-gnu` with
MinGW, so `windows` requires `linux`. An explicit `system_runners` entry wins
for the same system.

```toml
[release.artifacts]
binstall = true
platforms = ["linux", "macos", "windows"]
```

A nonempty `platforms` selects the conventional `packages.<system>.release-bundle`
output, so `prebuild_binaries = true` is not required; an explicit
`nix_bundle_attrs` list still wins.

`simit release verify` fails if evaluated bundle metadata lacks an archive for
a requested platform, or reports `blocked` when the flake exposes no
`binstall-metadata` check from which to read archive targets.

### Desktop binaries

A webview app cannot use the static musl archive: a Linux build links
webkit2gtk/GTK from glibc, macOS links system frameworks, and Windows links
WebView2, which the `x86_64-linux` job cross-builds against MinGW. Keep the
release config. Ship portable nix-bundle Linux archives, the Windows cross
archive, and the native macOS archive in
`packages.<system>.release-bundle`:

```toml
[release.artifacts]
platforms = ["linux", "macos", "windows"]
```

```nix
# glibc desktop build, made portable with nix-bundle.
desktop = harbor.lib.mkPortableBinaryRelease {
  inherit pkgs pname version;
  artifacts.x86_64-linux.entries.${pname}.package = desktopPackage;
};

packages.release-bundle = desktop.releaseBundle;
```

`mkPortableBinaryRelease` produces the Linux archives. Add the MinGW Windows
`.zip` and macOS archive to that bundle. Expose its `binstall` metadata as a
`checks.<system>.binstall-metadata*` check so `simit binstall sync` and
`simit release verify` can verify every requested platform ships.

With `auto_draft`, `simit release` runs `simit changelog draft` before
promoting `[Unreleased]`. Drafting uses the latest reachable semver tag (the
root commit for a first release), accepts guidance from
`.skills/release-changelog/SKILL.md` if present, and fails closed if Codex
returns no valid notable entries. Use `--no-changelog` only as an explicit
release changelog escape hatch.

The public distribution and release sections are `[aur]`, `[copr]`, `[apt]`,
`[homebrew]`, `[chocolatey]`, `[scoop]`, `[flatpak]`, `[winget]`,
`[release.codeberg]`, `[release.github]`, `[release.artifacts]`, `[release.attic]`,
`[release.changelog]`, `[release.announce]`, `[release.windows_signing]`, and
`[release.pypi]`.

For new packages and versions, `[winget]` uses the GitHub API as the
`WINGET_PAT` user (no wine or `wingetcreate`). The release step writes and
commits version, installer and default-locale manifests to a branch on the
token owner's fork and opens a PR to `microsoft/winget-pkgs`. Reruns reuse an
open PR. The optional `publisher`, `package_name`, `license`, `license_url`,
`short_description`, `homepage`, `binary` and `command` fields and their
Cargo-metadata defaults are listed in
[Windows Packaging](windows-packaging.md#winget).

## Release Secrets

### Binary-only releases and tag conventions

Keep package versions unprefixed; configure Git tags separately:

```toml
[ci]
publish_crates = false
required_gates = [
  { id = "player-tests", run = "nix develop -c scripts/ci.sh", timeout_minutes = 150, env = { WOW_DATA = "" } },
]

[release]
tag_prefix = "v" # "" (the default) or "v"

[release.notes]
source = "git" # "changelog" (default), "git", or "none"

[release.codeberg]
repo = "example/player"

[release.artifacts]
prebuild_binaries = true
```

`simit release` creates `v0.2.0` for package version `0.2.0`; `sync-up`
and `verify` use the same convention. Hosted release URLs use the tag,
while archive names and artifact manifests use the semantic version.
Required gates run after signature validation and checkout of the tagged
revision, before artifact publication.

Git notes list chronological commit subjects from the closest reachable
matching release tag through the exact release tag. They exclude same-commit
and unreachable tags. The first release lists all reachable history. Git
notes need no `CHANGELOG.md`. Explicit notes configuration overrides legacy
hosted `body_from_changelog`; without an explicit notes source,
`body_from_changelog = false` disables notes and the changelog precondition.

For hosted-only projects, `simit release verify` checks the release workflow,
artifact producer, configured credentials, trust root and tag without
checking crates.io publication. Credential presence remains a remote check;
inspect the contract with `simit release secrets contract --json`.

Generated release workflow headers list required and optional secrets. Configure
their names as follows:

- AUR: `[aur].ssh_key_secret` defaults to `AUR_SSH_KEY`.
- COPR: `[copr].login_secret`, `[copr].username_secret`, and
  `[copr].token_secret` default to `copr_login`, `copr_username`, and
  `copr_token`.
- apt: `[apt].gpg_key_secret`, `[apt].gpg_key_id_secret`,
  `[apt].gpg_passphrase_secret`, and `[apt].ssh_key_secret` default to
  `apt_repo_gpg_key`, `apt_repo_gpg_key_id`, `apt_repo_gpg_passphrase`, and
  `apt_repo_ssh_key`.
- Codeberg release upload: `[release.codeberg].token_secret` defaults to
  `codeberg_token`.
- GitHub release upload: `[release.github].token_secret` defaults to
  `GITHUB_TOKEN`.
- Homebrew: `[homebrew].tap_token_secret` defaults to `homebrew_tap_token`.
- Scoop: `[scoop].bucket_token_secret` defaults to `SCOOP_BUCKET_TOKEN`.
- Chocolatey: `[chocolatey].api_key_secret` defaults to `chocolatey_api_key`;
  `[chocolatey].api_key_env` defaults to `CHOCOLATEY_API_KEY`; set
  `[chocolatey].api_key_from_runner = true` when the runner already exposes
  that environment variable.
- Flatpak and winget: `[flatpak].token_secret` and `[winget].token_secret`
  default to `FLATHUB_TOKEN` and `WINGET_PAT`.
- Announcements: `[release.announce].mastodon_token_secret`,
  `mastodon_base_url_secret`, `matrix_token_secret`,
  `matrix_homeserver_secret`, and `matrix_room_secret` default to their
  matching uppercase environment-style names.
- Windows signing: `[release.windows_signing].pfx_secret`, `pass_secret`, and
  `subject_secret` default to `WINDOWS_SIGNING_PFX`, `WINDOWS_SIGNING_PASS`,
  and `WINDOWS_SIGNING_SUBJECT`.
- PyPI: `[release.pypi].token_secret` defaults to `PYPI_TOKEN`; it is unused
  with `trusted_publishing = true`.

## Editor Marketplaces

`[vscode]` publishes extensions to VS Code Marketplace and Open VSX;
`[jetbrains]` signs and publishes an IntelliJ Platform plugin. With either
section or its corresponding `--with-vscode` / `--with-jetbrains` flag,
`simit init ci` writes `publish-vscode-extension.yaml` or
`publish-jetbrains-plugin.yaml` to the configured platform's workflow directory.
Both require a `push` of an exact semver tag with `[release].tag_prefix`, except
that JetBrains also accepts a `workflow_dispatch` `version`. Before publishing,
the publish job rechecks the GPG signature against the default branch's
`keys/maintainers.gpg`, the tag object name, and that checkout matches the tag's
commit. They check the version in `Cargo.toml` (`cargo_package`) and in the
extension's `package.json` or the plugin's `gradle.properties`
(`pluginVersion`), then run `prepublish_commands`. Every platform installs Nix
with the same pinned `cachix/install-nix-action`.

Without `runner`, the GitHub publish job defaults to hosted `ubuntu-24.04`;
Forgejo keeps the project's release runner.

```toml
[ci]
platform = "github"

[vscode]
extension_dir = "pkl-lsp-vscode"
pat_source = "actions-secret"
# environment = "simit-marketplace" # the default
vsce_pat_secret = "VSCE_PAT" # default
ovsx_pat_secret = "OVSX_PAT" # default
cargo_package = "pkl-lsp-server"
package_command = "nix run .#package-release-assets -- \"$VERSION\""
prepublish_commands = ["nix flake check --no-build"]

[jetbrains]
plugin_dir = "pkl-lsp-jetbrains"
plugin_xml_id = "com.tartanoglu.pkl-lsp"
package_installable = ".#jetbrains-plugin"
credential_source = "actions-secret"
# environment = "simit-marketplace" # the default
cargo_package = "pkl-lsp-server"
prepublish_commands = ["nix flake check --no-build"]
```

With `pat_source`/`credential_source` set to `actions-secret` or `both`, the
job declares `environment: <name>` and reads environment secrets. Create that
environment (default `simit-marketplace`) and allow release tags in its
deployment policy. Rejected tags fail before any steps run or secrets arrive.
Only tag pushes and dispatches run the workflow; pull requests never receive
these secrets. In that GitHub environment, provide:

- VS Code/Open VSX: `VSCE_PAT` and `OVSX_PAT` (`vsce_pat_secret`,
  `ovsx_pat_secret`).
- JetBrains: `JETBRAINS_MARKETPLACE_TOKEN`, `JETBRAINS_CERTIFICATE_CHAIN`,
  `JETBRAINS_PRIVATE_KEY`, and `JETBRAINS_PRIVATE_KEY_PASSWORD`
  (`marketplace_token_secret`, `certificate_chain_secret`,
  `private_key_secret`, `private_key_password_secret`).

The VS Code workflow packages `release/*` with `package_command` and attaches
the resulting assets to the pushed tag's release via
`gh release upload <tag> ... --clobber` using `GITHUB_TOKEN`
(`permissions: contents: write`). It never creates a
release. If the release workflow hasn't created one after two minutes, the
publish job fails and tells you to rerun after the release run finishes. A
pinned self-hosted `runner` needs `gh` on `PATH`. JetBrains uploads the signed
archive only as a workflow artifact and keeps `contents: read`.

On Forgejo, the Codeberg API uploads assets to the required `codeberg_repo`
using `codeberg_token` (`codeberg_token_secret`). `file-env` PATs still work
from the runner.

## Release Workflow

`simit init release` uses `[release.codeberg]` for Forgejo/Codeberg or
`[release.github]` for GitHub, selected with `--platform` (or `[ci].platform`).
`[release.artifacts]` controls the runner, Nix
substituters, artifact build commands, SBOM commands, checksum globs, signing,
and the committed minisign public key path. Optional `[release.attic]`,
`[release.announce]`, and `[release.windows_signing]` sections add cache push,
stable-release announcements, and Authenticode signing steps.

Optional `[release.pypi]` uploads the wheels the release build left in
`release/` (for example from a Nix `release-bundle`) after the hosted release,
including prereleases. `uv publish --check-url` skips files already on PyPI on
rerun. This step builds nothing; add `"*.whl"` to `checksum_globs` to attach the
same wheels to the hosted release. `trusted_publishing = true` uses the job's
GitHub OIDC token: register `release.yml` as the trusted publisher. This
requires GitHub Actions unless the `pypi` channel is disabled. Without trusted
publishing, the workflow checks the token secret before any publication. Under
`[release.publish].enforcement = "activated-remote"`, the channel stays soft
until the project exists on PyPI, and a `staged` channel warns and skips if the
token or wheels are missing:

```toml
[release.pypi]
trusted_publishing = true # or: token_secret = "PYPI_TOKEN"
```

This differs from `[ci].pypi_*`, which builds and publishes a Python project
in a separate `publish-pypi.yaml` workflow.

Only channels whose config sections are present publish. AUR, apt, Homebrew,
Scoop, Chocolatey, Flatpak, and winget publish stable releases only. COPR uses
its testing project for prerelease tags if configured.

### Republishing channels for an existing release

If a downstream channel fails after the GitHub release (for example,
Chocolatey rejects a push during moderation of an earlier version, or winget
rejects a PR on validation), fix the cause and rerun only those channels.
Reuse the existing release instead of cutting a tag:

```sh
gh workflow run release.yml -f tag=0.2.1 -f channels=winget
gh workflow run release.yml -f tag=0.2.1 -f channels=winget,chocolatey
```

Dispatch from the default branch to use the current `republish` job and fixed
manifests or scripts. Without `tag`, dispatch behaves as before. The `tag`
and `channels` inputs and `republish` job require GitHub, `sign = true`, and
at least one republishable channel: `aur`, `flathub`, `winget`, `homebrew`,
`scoop`, `pypi` and `chocolatey`. `apt` and `copr` cannot be republished
because the release job builds their inputs. Set `channels` to a comma-separated
subset of configured channels. Empty, unknown, unconfigured or non-republishable
names fail before checkout.

The job:

- checks out `refs/tags/<tag>` and validates it as for a push: verifies the
  signature against the default branch's `keys/maintainers.gpg`, checks the
  tag label, version and changelog, and requires checkout at the signed tag's
  commit;
- skips the build, signing, attestation, smoke checks, release publication,
  Attic push and announcements. It downloads `SHA256SUMS.txt` and its `.minisig`
  from the existing hosted release, verifies them with minisign and committed
  `keys/minisign.pub`, downloads only the assets the selected channels read,
  and runs `sha256sum -c --strict` on those selected downloaded assets. Missing
  assets, bad signatures or checksum mismatches fail the job before any channel
  step;
- runs the publisher probe (under `activated-remote`) and then only the selected
  channel steps, with the same soft-skip semantics as a release run.

The job has `contents: read` (plus `id-token: write` only for PyPI trusted
publishing). Its shared per-tag concurrency group queues it behind an active
release of that tag.

Channel steps are idempotent. winget reuses an open PR and skips versions
already upstream. If the PR head is `<id>-<version>` on the token user's fork,
winget keeps the PR and force-moves that branch to a commit with regenerated
manifests unless the blobs match. Chocolatey compares registry bytes with the
packed candidate. The tag and commit key the cache as in the push run, so
repeated republishes of a tag reuse one candidate. Only a dispatch on the tag
ref can read its push cache (`--ref <tag>`, once that tag's `release.yml` has
the inputs). The default branch's first republish packs a fresh candidate and
fails closed if the registry already has different bytes for that version.
PyPI uses `uv publish --check-url`; Homebrew, Scoop, AUR and Flathub skip
unchanged trees.

Hosted assets are primary: the workflow fails early if hosted-release or signing
credentials are missing. Cache and downstream packaging are secondary. Attic
pushes warn and skip without a runner token. The workflow tries to build `.deb`
assets before checksums and hosted-release upload. If the runner container
lacks a mount-capable debootstrap/chroot environment, it warns and publishes
the artifacts it built. Without `.deb` files, apt publication skips.

If every release needs `.deb` assets, validate the runner before tagging:

```sh
simit init release --check --diff
test -r "$ATTIC_TOKENS_DIR/<project>" || true
nix develop -c debootstrap --variant=minbase bookworm "$(mktemp -d)" \
  http://deb.debian.org/debian
```

Run the debootstrap check under Forgejo Actions' runner image/container policy.
Passing in a host shell does not prove the release job can mount or chroot in
the container.

Keep `release.yml` as the only automatic signed-tag workflow. Ordinary
Simit-managed CI qualification workflows explicitly ignore tags so per-crate
checks don't queue ahead of the artifact publisher on small trusted runner
pools. Generated release scripts avoid implicit shell state such as `OLDPWD`:
with `set -u`, unset variables in Forgejo runner shells can interrupt a release
between artifact build and upload.
