# Distribution Channels

`simit` can keep distribution packaging files and the tag-triggered release
workflow in sync with project config. The Linux channels are:

- AUR: `simit init aur` writes `dist/aur/<pkg>/PKGBUILD` files for source,
  `-bin`, and `-git` flavors.
- COPR: `simit init copr` writes the RPM spec and `.copr/Makefile`.
- apt: `simit init apt` writes `dist/apt/conf/distributions` for reprepro.
- Release workflow: `simit init release` writes
  `.forgejo/workflows/release.yml` by default, or
  `.github/workflows/release.yml` for GitHub, for artifact build, hosted release
  upload, and configured channel publish steps.
- Crow: `simit init ci --ci-provider crow` and
  `simit init release --ci-provider crow` write project-side Crow v6.1
  workflows under `.crow` without invoking a Crow CLI or server. YAML is the
  default; set `[ci.crow].format = "jsonnet"` for Jsonnet output.

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

Render the same channel templates to stdout for inspection:

```sh
simit dist aur render
simit dist copr render
simit dist apt render
```

## Configuration

The same schema can live in `simit.toml`, root Cargo metadata under
`[workspace.metadata.simit]` or `[package.metadata.simit]`, or flake
`outputs.simitConfig`. Keep exactly one simit project config source in a repo.

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

[apt]
repo_url = "ssh://git@codeberg.org/example/foo-apt.git"
```

For an rs-harbor project whose flake exposes the conventional
`packages.<system>.release-bundle`, the release artifact section can instead
use the simple opt-in:

```toml
[release.artifacts]
prebuild_binaries = true
```

An explicit `nix_bundle_attrs` list still takes precedence when a project
publishes multiple bundles.

The public distribution and release sections are `[aur]`, `[copr]`, `[apt]`,
`[homebrew]`, `[chocolatey]`, `[scoop]`, `[flatpak]`, `[winget]`,
`[release.codeberg]`, `[release.github]`, `[release.artifacts]`, `[release.attic]`,
`[release.announce]`, and `[release.windows_signing]`.

## Release Secrets

Generated release workflows name required or optional secrets in the workflow
header. These names are configurable:

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

## Release Workflow

`simit init release` uses `[release.codeberg]` for Forgejo/Codeberg or
`[release.github]` for GitHub, selected with `--platform` (or `[ci].platform`).
`[release.artifacts]` controls the runner, Nix
substituters, artifact build commands, SBOM commands, checksum globs, signing,
and the committed minisign public key path. Optional `[release.attic]`,
`[release.announce]`, and `[release.windows_signing]` sections add cache push,
stable-release announcements, and Authenticode signing steps.

The workflow publishes only channels whose config sections are present. AUR,
apt, Homebrew, Scoop, Chocolatey, Flatpak, and winget are stable-release-only
publish steps; COPR switches to its testing project for prerelease tags when
configured.

Hosted release assets are the primary release product. The generated workflow
therefore fails early for missing hosted-release and signing credentials, but treats
runner-side cache and downstream packaging as secondary. Attic cache pushes skip
with a warning when the runner token is unavailable. Debian package generation is
also best-effort: it attempts to build `.deb` assets before checksums and
hosted-release upload, but it warns and continues when the runner container cannot
provide a mount-capable debootstrap/chroot environment. In that case the
the hosted release still publishes the artifacts that were actually produced, and
the apt repository publish step skips because there are no `.deb` files.

For projects that require `.deb` assets on every release, validate the runner
contract before tagging:

```sh
simit init release --check --diff
test -r "$ATTIC_TOKENS_DIR/<project>" || true
nix develop -c debootstrap --variant=minbase bookworm "$(mktemp -d)" \
  http://deb.debian.org/debian
```

The debootstrap check must run in the same runner image/container policy used by
Forgejo Actions. A host shell passing the check does not prove the release job
can mount or chroot inside the runner container.

Keep `release.yml` as the only automatic signed-tag workflow. Normal
simit-managed CI workflows ignore tags explicitly so a release tag does not queue
per-crate checks ahead of the artifact publisher on small trusted runner pools.
Generated release scripts also avoid implicit shell state such as `OLDPWD`; under
`set -u`, those variables can be unset in Forgejo runner shells and must not sit
between successful artifact builds and hosted-release upload.
