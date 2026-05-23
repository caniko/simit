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
- `chocolatey_api_key` for Chocolatey package pushes.
- `scoop_bucket_token` for authenticated Scoop bucket pushes.

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
