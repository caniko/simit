# Windows Packaging

For tagged Windows releases, `simit` renders deterministic Chocolatey package
skeletons and Scoop bucket manifests from project config. Both can be enabled in
`init ci`.

## Configuration

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
```

Keep exactly one project config source per repo: `simit.toml`, root Cargo
metadata under `[workspace.metadata.simit]` or `[package.metadata.simit]`, or
flake `outputs.simitConfig`. All use the same schema.

Chocolatey requires `download_repo` plus package metadata. Scoop requires
`bucket_url` and `download_repo`. Other fields usually fall back to Cargo
package metadata.

For a GitHub Scoop bucket, set `SCOOP_BUCKET_TOKEN` to a token with push access
to the target bucket repository.
The automatic `GITHUB_TOKEN` cannot push to another repository. For a
`github.com` bucket URL, Simit maps legacy
`bucket_token_secret = "CODEBERG_TOKEN"` to `SCOOP_BUCKET_TOKEN`. Codeberg
settings and custom secret names stay unchanged.

Chocolatey dependencies become nuspec `<dependencies>`; `version` is optional
(a NuGet version or range) and ids must be non-empty without whitespace:

```toml
[[chocolatey.dependencies]]
id = "uv"
# version = "[0.5,)"
```

## Bootstrap

```sh
simit init chocolatey --target packaging/chocolatey
simit init scoop-bucket --target ../scoop-foo
```

Both commands support `--check`, `--diff`, and `--print`. With `--no-git`,
`init scoop-bucket` writes only `bucket/<name>.json`.

## Release CI

```sh
simit init ci --platform github --with-chocolatey --with-scoop
simit init ci --platform forgejo --with-chocolatey --with-scoop
```

`--with-chocolatey` and `--with-scoop` imply `--with-artifacts`.
`--windows-runner` overrides the defaults: `windows-latest` on GitHub and
`windows` from simit user config on Forgejo.

Generated workflows read these secrets:

- `MINISIGN_SECRET_KEY` and `MINISIGN_PASSWORD` for the signed checksum manifest.
- `COSIGN_PRIVATE_KEY` and `COSIGN_PASSWORD` as the optional Sigstore fallback
  when keyless OIDC is unavailable.
- `chocolatey_api_key` for Chocolatey package pushes, unless
  `[chocolatey].api_key_secret` changes it.
- `SCOOP_BUCKET_TOKEN` for authenticated Scoop bucket pushes, unless
  `[scoop].bucket_token_secret` changes it.

The artifact workflow verifies signed release tags and emits signed
checksums plus SLSA provenance. See [Release Integrity](release-integrity.md)
for the required `keys/maintainers.gpg` and `keys/minisign.pub` trust roots.

## Free Authenticode signing with SignPath Foundation

`[release.windows_signing]` supports two providers: the default
`provider = "osslsigncode"`, which signs loose `.exe`s with a PFX secret, and
`provider = "signpath"`, which sends finished archives such as the Nix-built
`sorrel-<v>-x86_64-windows.zip` to [SignPath Foundation](https://signpath.org/).
The foundation is the certificate holder and signs OSI-licensed open source
projects for free. SignPath signs PE files in place and returns a signed zip, so
the release job need not repack it.

```toml
[release.windows_signing]
provider = "signpath"
organization_id = "<SignPath organization ID>"
project_slug = "sorrel"
signing_policy_slug = "release-signing"
# artifact_configuration_slug = "windows-zip"   # project default when unset
# api_token_secret = "SIGNPATH_API_TOKEN"       # the default
# wait_timeout_seconds = 3600                   # per archive, includes manual approval
archives = ["sorrel-{version}-x86_64-windows.zip"]
pe_files = ["sorrel.exe", "sorrelw.exe"]       # members that must end up signed
```

GitHub Actions only: `simit init release` rejects the provider on Forgejo,
GitLab and Crow, and rejects `dir`, `binaries`, `sign_*`, `*_archive` and the
`pfx`/`pass`/`subject` secrets alongside it (and SignPath fields with
osslsigncode). The release job also gets `actions: read`.

### Rendered steps

All SignPath steps precede `[[release.artifacts.extract]]` promotion and
checksums, so promoted exes, `SHA256SUMS`, cosign/minisign signatures and winget
and Chocolatey hashes reflect the signed bytes.

1. **Check SignPath signing availability** always runs and checks for a
   non-empty API-token secret. If empty, it prints
   `::warning::SignPath signing skipped: SIGNPATH_API_TOKEN is not set` and
   the release continues unsigned. Every later SignPath step is conditional on
   its `enabled` output.
2. `actions/upload-artifact` uploads each zip as-is (`archive: false`).
3. `signpath/github-action-submit-signing-request` (pinned to a commit)
   submits the zip with `wait-for-completion: true` and a bounded timeout, then
   saves the signed zip (`skip-decompress: true`) in a temporary directory.
4. The job replaces `release/<archive>` only if the signed zip has the original
   file list and `osslsigncode verify` accepts every `pe_files` entry. Any
   mismatch fails the release.

Every signing request submitted to SignPath Foundation requires manual approval
by a team member in SignPath. The job waits up to `wait_timeout_seconds`;
timeout or rejection fails the release before publication.

### One-time setup

1. Check eligibility and apply at <https://signpath.org/apply>. You need an
   OSI-approved license, no proprietary components, a public repository and
   a released, maintained app with some verifiable reputation. See the
   [OSS program terms](https://signpath.org/terms).
2. After acceptance, add the **GitHub.com** trusted build system to the
   SignPath organization and link it to the project. Create the artifact
   configuration below, a signing policy such as `release-signing` with manual
   approval, and a CI user with an API token allowed to submit to that policy.
3. Create the GitHub Actions repository **secret** `SIGNPATH_API_TOKEN` (or the
   name configured by `api_token_secret`). Add the config above with SignPath's
   organization ID, project slug and policy slug.
4. Run `simit init release` and commit the workflow.

Without the secret, the availability check warns, subsequent SignPath steps
skip, and releases publish unsigned.

### Artifact configuration

For a zip with `sorrel.exe` and `sorrelw.exe` at its root, create this
configuration in SignPath (make it the project default or set
`artifact_configuration_slug`):

```xml
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <pe-file path="sorrel.exe">
      <authenticode-sign/>
    </pe-file>
    <pe-file path="sorrelw.exe">
      <authenticode-sign/>
    </pe-file>
  </zip-file>
</artifact-configuration>
```

SignPath receives the archive directly, so the root is `<zip-file>`.
`pe_files` must contain the exact `path`s listed here.

### Program obligations

The project must meet these requirements beyond the generated workflow:

- Build provenance: SignPath verifies that a GitHub-hosted runner built the
  artifact from the public repository. Set `[release.artifacts].runner` (or
  `[ci].runner`) to a GitHub-hosted label such as `ubuntu-24.04`, not a
  self-hosted runner. Build the archives in that same workflow or in the
  `prebuild` workflow it calls. Sign only artifacts built from the project's
  own source.
- A code signing policy page on the project home and download pages: the phrase
  "Code signing policy", the line "Free code signing provided by SignPath.io,
  certificate by SignPath Foundation", and the team roles with their members.
- A privacy policy (or a statement that the program does not transfer
  information to other systems unless the user asks).
- Multi-factor authentication for every team member on both SignPath and the
  source repository.
- Product name and version metadata on the signed binaries, and manual
  approval of each release signing request.

Sources: [SignPath OSS terms](https://signpath.org/terms),
[GitHub Actions trusted build system](https://docs.signpath.io/trusted-build-systems/github),
[artifact configuration syntax](https://docs.signpath.io/artifact-configuration/syntax),
[`signpath/github-action-submit-signing-request`](https://github.com/SignPath/github-action-submit-signing-request).

## Local Render and Bump

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
```

For x64-only Windows archives, use `--scoop-no-arch arm64` or
`[scoop.architectures] arm64 = false`. If release archive names differ from the
default `{name}-{version}-{arch}-windows.zip`, override them with
`--choco-archive-pattern` and `--scoop-archive-pattern`.

## winget

With `[winget]`, the release workflow submits the zip to `microsoft/winget-pkgs`.
Set `package_id`, `download_repo`, and `zip_archive` (with a literal `{version}`).
The zip must contain the executable for portable installation.

```toml
[winget]
package_id = "Caniko.Sorrel"
download_repo = "caniko/sorrel"
zip_archive = "sorrel-{version}-x86_64-windows.zip"
# arm64_zip_archive = "sorrel-{version}-aarch64-windows.zip"
license_url = "https://github.com/caniko/sorrel/blob/trunk/LICENSE-MIT"
```

First-submission options and defaults:

| Field               | Default                                                       |
| ------------------- | ------------------------------------------------------------- |
| `publisher`         | `package_id` before the first `.`                             |
| `package_name`      | `package_id` after the first `.`                              |
| `license`           | Cargo `license`                                               |
| `license_url`       | none (omitted)                                                |
| `short_description` | Cargo `description`                                           |
| `homepage`          | Cargo `homepage` (becomes `PackageUrl`; else the repo URL)    |
| `binary`            | `<cargo name>.exe`, the path of the executable inside the zip |
| `command`           | Cargo package name, the portable command alias                |
| `dependencies`      | none; winget `PackageIdentifier`s, see below                  |
| `arm64_zip_archive` | none; set it to add a native `arm64` installer (same layout)  |

The installer manifest maps `dependencies` to winget `PackageDependencies`
entries. When the list is empty, `PackageDependencies` is omitted:

```toml
[winget]
dependencies = ["astral-sh.uv"]
```

```yaml
Dependencies:
  PackageDependencies:
    - PackageIdentifier: "astral-sh.uv"
```

The release step uses the GitHub REST API as the `WINGET_PAT` user via
`curl` and `jq`, not wine or `wingetcreate`. For a new package or version, it
writes three ManifestVersion 1.12.0 files (`<id>.yaml`, `<id>.installer.yaml`,
`<id>.locale.en-US.yaml`) under `manifests/<x>/<Publisher>/<Name>/<version>/`.
The installer uses `InstallerType: zip`, `NestedInstallerType: portable` and
the released zip's uppercase SHA-256. The release step ensures the token owner
has a fork of `microsoft/winget-pkgs`, commits to `<id>-<version>` (blobs, tree
on upstream head, commit, ref), and opens a PR titled
`New package: <id> version <v>` or `New version: <id> version <v>`.

On rerun, the step keeps an open PR from that branch on the token owner's fork
and force-moves the branch to a fresh commit with regenerated manifests, unless
the blobs already match. It only prints other open PRs. An upstream version
counts as "already published". curl reads the token from a 0600 header file,
never the command line. API failures print the HTTP status and GitHub's message.

The package directory determines whether this is a new package or version.
Under `[release.publish].enforcement = "activated-remote"`, the publisher probe
sets `PUBLISH_WINGET_STATE`: only winget-pkgs responses of 200 or 404 count;
anything else fails the release, and a failed submission only warns while the
package is inactive. In other enforcement modes, the step probes
`api.github.com/repos/microsoft/winget-pkgs/contents/manifests/...` itself and
fails the release if the probe or submission fails. Full manifests require a
license and a short description (3-256 characters), from config or Cargo
metadata; if required metadata is missing or the description is outside 3-256
characters, the step fails and names the offending fields. `WINGET_PAT` must be
a classic token with `public_repo`.

`simit dist winget submit --version X` runs the same script locally with
`bash`, `curl`, and `jq`: it downloads the zip, checksums it and submits.
`--wine`, `--wingetcreate` and `--wingetcreate-url` were removed.

### Multiple files

By default the manifest makes one `NestedInstallerFiles` entry from `binary`
and `command`. List multiple portable executables with `[[winget.files]]`:

```toml
[winget]
package_id = "Caniko.Sorrel"
download_repo = "caniko/sorrel"
zip_archive = "sorrel-{version}-x86_64-windows.zip"

[[winget.files]]
path = "sorrel.exe"
alias = "sorrel"

[[winget.files]]
path = "sorrelw.exe"
```

Every entry emits one item; `PortableCommandAlias` is written only when `alias`
is set:

```yaml
Installers:
  - Architecture: x64
    NestedInstallerFiles:
      - RelativeFilePath: "sorrel.exe"
        PortableCommandAlias: "sorrel"
      - RelativeFilePath: "sorrelw.exe"
```

Nonempty `files` replaces `binary` and `command`; empty `files` keeps the default
entry unchanged. Paths must be unique and relative, with no `..`, drive letter
or empty segment. Aliases must be nonempty, at most 40 characters (the schema
limit), free of path separators and unique ignoring case.

**Schema 1.12 findings.** In `manifest.installer.1.12.0.schema.json`,
`NestedInstallerFiles` items require only `RelativeFilePath`
(1-512 characters); `PortableCommandAlias` is optional (string or null,
1-40 characters). The list allows up to 1024 items. Without an alias, the
winget client (`PortableFlow.cpp`) uses the filename as the command: it creates
a `sorrelw.exe` symlink in the links directory, putting `sorrelw` on `PATH`.
If the alias differs from the filename, the client hard-links the exe under
the alias and links to that instead.

## Chocolatey Shortcuts and GUI Shims

For a zip with a console exe and GUI launcher, add a Start menu entry and GUI
shim for the launcher. The default package provides neither.

```toml
[chocolatey]
gui_exes = ["sorrelw.exe"]

[[chocolatey.shortcuts]]
name = "Sorrel"
target = "sorrelw.exe"
# arguments = "--some-flag"
# icon = "sorrelw.exe,0"
```

| Field                   | Meaning                                                                     |
| ----------------------- | --------------------------------------------------------------------------- |
| `shortcuts[].name`      | Shortcut name (`<name>.lnk`); a file name, no path separators or `:*?"<>\|` |
| `shortcuts[].target`    | Path inside the zip, relative to the tools directory                        |
| `shortcuts[].arguments` | Optional single-line argument string                                        |
| `shortcuts[].icon`      | Optional path inside the zip, optionally `,<index>` (e.g. `app.exe,0`)      |
| `gui_exes`              | Executables inside the zip that get a GUI shim                              |

Targets, icons and `gui_exes` must be relative, with no `..`, drive letter or
empty segment. Names must be unique ignoring case. Simit emits every value as a
PowerShell single-quoted string with `'` doubled and joins paths to `$toolsDir`
without concatenating them into a quoted path.

After `Install-ChocolateyZipPackage`, `chocolateyInstall.ps1` creates an empty
`<exe>.gui` marker for each `gui_exes` entry and calls
`Install-ChocolateyShortcut` per shortcut; `chocolateyUninstall.ps1` removes
the `.lnk` files with `Remove-Item -Force -ErrorAction SilentlyContinue`. With
neither option set, the scripts stay unchanged. On 32-bit Windows (if the project
publishes `x86` archives), these steps run before the early return.

**Programs folder.** Shortcuts go to
`[Environment]::GetFolderPath('CommonPrograms')`, the all-users Start menu.
Chocolatey normally installs from an elevated shell to
`C:\ProgramData\chocolatey\lib\<id>\tools`. An all-users shortcut matches that
machine-wide scope and resolves to the same folder at install and uninstall.
Per-user `Programs` would use the elevated account's profile, which may not
belong to the user launching the app. Chocolatey's `Install-ChocolateyShortcut`
docs name no folder and say shortcut failures (for example under SYSTEM)
warn rather than failing installation.

**GUI shim or `.ignore`.** Chocolatey shims every `.exe` in the package. A
console shim for a GUI exe blocks the shell until the window closes. Use `.gui`
to return immediately or `.ignore` for no shim. Shimgen detection isn't always
accurate; simit writes `.gui` so `sorrelw` works from a terminal or Win+R, like
alias-less winget files (see [above](#multiple-files)). Match case exactly:
`BOB.exe.gui` may not work for `bob.exe`. To keep the launcher off `PATH`, drop
it from `gui_exes` and ship its `<exe>.ignore` marker in the zip. The Start menu
shortcut still targets the real exe in the tools directory, not the shim.
