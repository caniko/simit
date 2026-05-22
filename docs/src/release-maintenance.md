# Release Maintenance

Before publishing a release, add intended changes under `## [Unreleased]` in
`CHANGELOG.md` and validate the changelog:

```sh
simit changelog check
```

Run the local release flow:

```sh
simit release patch -m "release patch"
```

The crates.io publish workflow runs when an exact semver tag is pushed. The
workflow validates that the tag matches the Cargo package version, runs a
publish dry run, and requires `CRATES_IO_API_TOKEN` to publish.

Projects that publish Windows packages can add Chocolatey and Scoop jobs to
the release artifact workflow. See
[Windows Packaging](getting-started/windows-packaging.md) for the required
`simit.toml` sections, CI flags, and secrets.

Release candidate validation commands:

```sh
nix flake check --keep-going --print-build-logs
nix develop --command cargo publish --dry-run
```
