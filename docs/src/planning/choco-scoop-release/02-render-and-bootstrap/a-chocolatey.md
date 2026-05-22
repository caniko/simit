# Phase 2a — Chocolatey renderer, bump command, tap bootstrap

> **Recommended Codex model: GPT 5.5 medium**
>
> Renderer is a deterministic template (nuspec XML + `chocolateyInstall.ps1`),
> but the install script must compute sha256 correctly for the downloaded
> archive and branch on architecture — these are non-mechanical details that
> reward careful attention. The bump command pushes to the Chocolatey
> community feed via `choco push`, which is a one-shot non-git flow distinct
> from the Homebrew tap clone-commit-push pattern, so blind copy from
> `src/commands/homebrew.rs` would be wrong. Medium effort fits the
> "moderate, with one real design call" shape.

## Working tree

Starts from phase 1 merged into `trunk`. If sub-layers 2a/2b run truly in parallel, rebase against 2b before opening a PR.

## Goal

Render Chocolatey package files (`<id>.nuspec`, `tools/chocolateyInstall.ps1`, `tools/chocolateyUninstall.ps1`) deterministically, expose `simit chocolatey render` / `simit chocolatey bump` analogous to the Homebrew counterparts, and provide `simit init-chocolatey` to bootstrap a fresh package directory.

## Why

The Homebrew formula is a single `.rb` file written into a tap repo; Chocolatey packages are a directory tree pushed to a centralized feed via `choco push --source=...`. We need our own renderer because the official `choco new` template carries a lot of noise and is not deterministic.

## Out of scope

- CI workflow integration (phase 3).
- Scoop (phase 2b).

## Plan

1. **Renderer** at `src/render/chocolatey_nuspec.rs`:
   - `pub fn render_nuspec(opts: &ChocolateyRenderOptions) -> String` emitting `<package><metadata>…</metadata><files>…</files></package>`. Required metadata fields: `id`, `version`, `title`, `authors`, `description`, `projectUrl`, `licenseUrl` (optional), `tags`, `releaseNotes` (optional).
   - `pub fn render_install_script(opts: &ChocolateyRenderOptions, checksums: &Sha256Set) -> String` emitting PowerShell that calls `Install-ChocolateyZipPackage` with per-arch `url64`/`url`/`checksum64`/`checksum`. Reuse the existing `src/sha256.rs` helper.
   - `ChocolateyRenderOptions` carries the resolved fields plus per-arch URLs derived from `archive_pattern` and `download_repo`.
   - Add a uninstall script renderer only if the install script writes outside the package directory; for the default zip flow, ship a no-op `chocolateyUninstall.ps1` so users see the file shape they'd amend.

2. **`simit chocolatey` subcommand** in [src/cli.rs](../../../../src/cli.rs) and `src/commands/chocolatey.rs`:
   - `Render { version, output_dir, ChocolateyOverridesArgs }` — write to stdout (zip the in-memory tree) or to `--output-dir`.
   - `Bump { version, package_dir, archive: Vec<String>, push: bool, push_source: Option<String>, api_key_env: Option<String>, ChocolateyOverridesArgs }`. The `--push` path runs `choco pack` then `choco push --source=$source --api-key=$env:$api_key_env`. On systems without `choco` (Linux dev box), surface a clear error and exit non-zero.
   - Reuse `crate::sha256::sha256_hex` and `crate::cargo::metadata_for_current_dir` exactly as `src/commands/homebrew.rs` does.

3. **`simit init-chocolatey`** in `src/commands/init_chocolatey.rs`:
   - Args: `--target <DIR>`, `--check`, `--diff`, `--print`, plus `ChocolateyOverridesArgs`.
   - Behaviour mirrors `init_homebrew_tap`: render a skeleton nuspec + install script into `<target>/`, optionally diff or print, no git wiring (Chocolatey packages aren't tap-style git repos; the publish flow is `choco push` from CI). Skip the `--no-git` flag entirely.

4. **Wiring**:
   - Register the subcommand in `src/cli.rs::Commands` and dispatch in `src/main.rs`.
   - Add module declarations to `src/commands/mod.rs` and `src/render/mod.rs`.

## Acceptance criteria

- [ ] `cargo check --all-targets` passes.
- [ ] `cargo test --test chocolatey` (new file) passes with: deterministic nuspec snapshot, deterministic install-script snapshot, sha256 wiring round-trip, missing-archive error path.
- [ ] `simit chocolatey render --version 0.1.0 --output-dir /tmp/pkg` writes a working `<id>.nuspec` + `tools/chocolateyInstall.ps1` that passes `choco pack` (skip the `choco pack` assertion if `choco` is not available in CI; assert file existence and content).
- [ ] `simit init-chocolatey --target /tmp/skel --print` prints the same content without writing.
- [ ] `simit chocolatey bump --version 0.1.0 --package-dir /tmp/pkg --archive x64=path/to/archive.zip` updates checksums and exits 0; with `--push` but no API key env, it errors clearly.
- [ ] Phase 1's `--with-chocolatey` still parses; no CI YAML drift on the simit repo.

## Files likely touched

- `src/render/chocolatey_nuspec.rs` (new)
- `src/render/mod.rs`
- `src/commands/chocolatey.rs` (new)
- `src/commands/init_chocolatey.rs` (new)
- `src/commands/mod.rs`
- `src/cli.rs`
- `src/main.rs`
- `tests/chocolatey.rs` (new)

## Pitfalls

- **XML escaping**: nuspec is XML — escape `&`, `<`, `>` in description/title. Don't reuse `shell_quote` patterns.
- **PowerShell here-strings**: install script will contain `$` and backticks; emit it as a raw string in Rust and escape inside via `\` only where PowerShell needs it.
- **sha256 timing**: Chocolatey expects checksums baked into the install script at pack time, not resolved at install time. `bump` must compute and inline them before `choco pack`.
- **Architecture mapping**: `Install-ChocolateyZipPackage -Url64`/`-Url` corresponds to x64/x86. arm64 needs a custom branch using `$env:PROCESSOR_ARCHITECTURE -eq 'ARM64'`. Decide whether to ship arm64 in v1 or defer; default to x64-only and add a TODO if arm64 is deferred.
- **`choco push` is non-idempotent**: the Chocolatey moderation queue will reject a re-push of the same version. Bump must check `choco search` or accept that re-pushes are an error case the user resolves manually.

## Reference

- nuspec schema: <https://docs.chocolatey.org/en-us/create/create-packages>.
- Install-ChocolateyZipPackage: <https://docs.chocolatey.org/en-us/create/functions/install-chocolateyzippackage>.
- Prior art in this repo: [src/commands/homebrew.rs](../../../../src/commands/homebrew.rs), [src/render/homebrew_formula.rs](../../../../src/render/homebrew_formula.rs).
