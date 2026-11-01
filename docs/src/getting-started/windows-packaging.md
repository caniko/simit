# Windows Packaging

`simit` supports Chocolatey packages and Scoop bucket manifests for tagged
Windows releases. Both flows use simit project config as the source of package
metadata, render deterministic skeleton files, and can be wired into `init ci`.

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

The same schema can live in `simit.toml`, in root Cargo metadata under
`[workspace.metadata.simit]` or `[package.metadata.simit]`, or in flake
`outputs.simitConfig`. Keep exactly one simit project config source in a repo.

Chocolatey requires `download_repo` plus package metadata. Scoop requires
`bucket_url` and `download_repo`. Fields not listed here can usually fall back
to Cargo package metadata.

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

Both commands support `--check`, `--diff`, and `--print`. `init scoop-bucket`
also supports `--no-git` for writing only `bucket/<name>.json`.

## Release CI

```sh
simit init ci --platform github --with-chocolatey --with-scoop
simit init ci --platform forgejo --with-chocolatey --with-scoop
```

`--with-chocolatey` and `--with-scoop` imply `--with-artifacts`. GitHub uses
`windows-latest` unless `--windows-runner` overrides it. Forgejo uses the
`windows` default from simit user config unless `--windows-runner` overrides it.

Generated workflows read these secrets:

- `MINISIGN_SECRET_KEY` and `MINISIGN_PASSWORD` for the signed checksum manifest.
- `COSIGN_PRIVATE_KEY` and `COSIGN_PASSWORD` as the optional Sigstore fallback
  when keyless OIDC is unavailable.
- `chocolatey_api_key` for Chocolatey package pushes, unless
  `[chocolatey].api_key_secret` changes it.
- `SCOOP_BUCKET_TOKEN` for authenticated Scoop bucket pushes, unless
  `[scoop].bucket_token_secret` changes it.

The same artifact workflow verifies signed release tags and emits signed
checksums plus SLSA provenance. See [Release Integrity](release-integrity.md)
for the required `keys/maintainers.gpg` and `keys/minisign.pub` trust roots.

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

Use `--scoop-no-arch arm64` or `[scoop.architectures] arm64 = false` when a
project only publishes x64 Windows archives. Use `--choco-archive-pattern` and
`--scoop-archive-pattern` when release archive names do not match the default
`{name}-{version}-{arch}-windows.zip` shape.

## winget

`[winget]` submits the release zip to `microsoft/winget-pkgs` from the release
workflow. Required: `package_id`, `download_repo`, and `zip_archive` (with a
literal `{version}`). The zip must hold the executable; it is installed as a
portable package.

```toml
[winget]
package_id = "Caniko.Sorrel"
download_repo = "caniko/sorrel"
zip_archive = "sorrel-{version}-x86_64-windows.zip"
license_url = "https://github.com/caniko/sorrel/blob/trunk/LICENSE-MIT"
```

Optional fields used for the first submission, with their defaults:

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

`dependencies` lists winget packages the installer needs. Each becomes a
`PackageDependencies` entry in the installer manifest (nothing is written when
the list is empty):

```toml
[winget]
dependencies = ["astral-sh.uv"]
```

```yaml
Dependencies:
  PackageDependencies:
    - PackageIdentifier: "astral-sh.uv"
```

The release step submits through the GitHub REST API as the `WINGET_PAT` user
(`curl` and `jq`; no wine, no `wingetcreate`). New packages and new versions
take one path: it writes the three ManifestVersion 1.12.0 files
(`<id>.yaml`, `<id>.installer.yaml`, `<id>.locale.en-US.yaml`) with an
`InstallerType: zip` / `NestedInstallerType: portable` installer and the
uppercase SHA-256 of the released zip under
`manifests/<x>/<Publisher>/<Name>/<version>/`, ensures the token owner's fork
of `microsoft/winget-pkgs` exists, commits the files to a branch
`<id>-<version>` on it (blobs, tree on the upstream head, commit, ref) and opens
the PR with the winget-pkgs title `New package: <id> version <v>` or
`New version: <id> version <v>`. Reruns are safe: when a PR from the branch is open on the token
user's fork, its branch is force-moved to a fresh commit with the regenerated
manifests and the same PR is kept (nothing changes when the blobs already
match); an open PR from anywhere else is only printed. A version that already
exists upstream is "already published". The token is passed to curl through a 0600
header file, never on the command line, and API failures print the HTTP status
and GitHub's message.

The package directory decides new package versus new version. Under
`[release.publish].enforcement = "activated-remote"` that comes from the
publisher probe (`PUBLISH_WINGET_STATE`; only a 200 or 404 from winget-pkgs
counts, any other response fails the release rather than guessing) and a
failed submission only warns until the package is active; otherwise the step
probes `api.github.com/repos/microsoft/winget-pkgs/contents/manifests/...`
itself and a failure fails the release. Every submission now writes the full
manifests, so it needs a license and a short description (set them or fill the
Cargo metadata; 3-256 characters), otherwise the step fails with a message
naming the fields. `WINGET_PAT` is still a classic token with `public_repo`.

`simit dist winget submit --version X` runs the same script locally (needs
`bash`, `curl`, `jq`): it downloads the zip for its checksum and submits.
`--wine`, `--wingetcreate` and `--wingetcreate-url` were removed.

### Multiple files

By default the installer manifest has one `NestedInstallerFiles` entry from
`binary` and `command`. A zip that holds more than one portable executable
lists them with `[[winget.files]]`:

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

Each entry becomes one item, and `PortableCommandAlias` is written only when
`alias` is set:

```yaml
Installers:
  - Architecture: x64
    NestedInstallerFiles:
      - RelativeFilePath: "sorrel.exe"
        PortableCommandAlias: "sorrel"
      - RelativeFilePath: "sorrelw.exe"
```

When `files` is set it replaces `binary` and `command`; when empty, the single
entry is rendered exactly as before. Paths are relative, with no `..`, drive
letter or empty segment, and unique; aliases are non-empty, at most 40
characters (the schema limit), free of path separators and unique ignoring case.

**Schema 1.12 findings.** In `manifest.installer.1.12.0.schema.json`,
`NestedInstallerFiles` items require only `RelativeFilePath`
(1-512 characters); `PortableCommandAlias` is optional (string or null,
1-40 characters), up to 1024 items. A portable zip may therefore contain files
without an alias. The winget client (`PortableFlow.cpp`) then uses the file's
own name as the command: a symlink `sorrelw.exe` is placed in the links
directory, so `sorrelw` is on `PATH`. With an alias that differs from the file
name, the client hard-links the exe under the alias and links that instead.

## Chocolatey Shortcuts and GUI Shims

A zip that ships a console exe and a GUI launcher needs two things the default
package does not do: a Start menu entry and a GUI shim for the launcher.

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

Targets, icons and `gui_exes` are relative paths with no `..`, drive letter or
empty segment; names are unique case-insensitively. Every value is emitted as a
PowerShell single-quoted string with `'` doubled, and paths are joined to
`$toolsDir`, never concatenated into a quoted path.

After `Install-ChocolateyZipPackage`, `chocolateyInstall.ps1` creates an empty
`<exe>.gui` marker for each `gui_exes` entry and calls
`Install-ChocolateyShortcut` per shortcut; `chocolateyUninstall.ps1` removes
the `.lnk` files with `Remove-Item -Force -ErrorAction SilentlyContinue`. With
neither option set the scripts are unchanged. On 32-bit Windows (when
`x86` archives are published) the same steps run before the early return.

**Programs folder.** Shortcuts go to
`[Environment]::GetFolderPath('CommonPrograms')`, the all-users Start menu.
Chocolatey installs machine-wide, normally from an elevated shell, into
`C:\ProgramData\chocolatey\lib\<id>\tools`, so an all-users entry matches the
install scope, and install and uninstall resolve the same folder. The per-user
`Programs` folder would resolve to the elevated account's profile, which can
differ from the user who launches the app. The Chocolatey docs for
`Install-ChocolateyShortcut` do not name a folder; they only say that failures
(for instance under the SYSTEM account) are warnings, not install failures.

**GUI shim or `.ignore`.** Chocolatey shims every `.exe` in the package. A
console shim of a GUI exe makes a shell wait until the window closes, so such
an exe needs either a `.gui` marker (a shim that returns at once) or an
`.ignore` marker (no shim). simit writes `.gui`: it keeps `sorrelw` available
from a terminal or Win+R, consistent with how winget links alias-less portable
files (see [above](#multiple-files)), and shimgen's own detection is documented
as not always accurate. Match the exe's case exactly; the docs warn that
`BOB.exe.gui` may not work for `bob.exe`. If a project would rather not expose
the launcher on `PATH`, drop it from `gui_exes` and ship the `.ignore` marker in
the zip. The Start menu shortcut targets the real exe in the tools directory,
not the shim, so it is unaffected by this choice.
