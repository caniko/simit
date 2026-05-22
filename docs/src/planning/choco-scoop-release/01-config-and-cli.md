# Phase 1 — Config schema and CLI surface for Chocolatey + Scoop

> **Recommended Codex model: GPT 5.5 medium**
>
> Moderate complexity: mirrors the existing `[homebrew]` config and
> `HomebrewOverridesArgs` patterns in [src/config.rs](../../../../src/config.rs)
> and [src/cli.rs](../../../../src/cli.rs). The shape is well-defined by the
> Homebrew prior art; the design call is mostly about which knobs each
> packager genuinely needs (e.g. Scoop bucket URL vs. Chocolatey API key,
> per-arch archives vs. a single MSI/zip). Low cargo (5.5 low) would
> miscopy field-by-field and not catch the schema asymmetries; max is
> overkill for what is structurally a port.

## Working tree

`trunk` at HEAD `602f7a2`. No outstanding plan branches. Work directly on `trunk` or a `choco-scoop/phase-1` topic branch — both are fine for this repo's solo workflow.

## Goal

Add `[chocolatey]` and `[scoop]` sections to `simit.toml`, plus matching CLI override arg groups and `init-ci` flags, so phases 2–3 can consume a `ResolvedChocolatey` / `ResolvedScoop` without further config plumbing.

## Why

The Homebrew flow (commit `e1b06bd`, `8456773`) established the contract: `simit.toml` is the source of truth, CLI flags override per-invocation, Cargo metadata supplies last-resort fallbacks. Choco and Scoop must follow the same contract so users can declare a Windows release surface once and have `init-ci`, `simit chocolatey bump`, and `simit scoop bump` all read the same fields.

## Out of scope

- Rendering nuspec / scoop manifests (phase 2).
- Bootstrap commands `simit init-chocolatey` / `simit init-scoop-bucket` (phase 2).
- CI workflow YAML changes (phase 3).
- Tests beyond the config-resolution unit level (phase 4).

## Plan

1. **Config structs** in [src/config.rs](../../../../src/config.rs):
   - `ChocolateyConfig` with: `name: Option<String>`, `id: Option<String>` (defaults to package name), `title: Option<String>`, `authors: Option<String>`, `description`, `project_url`, `license_url: Option<String>`, `tags: Option<String>`, `release_notes_url: Option<String>`, `download_repo: String`, `archive_pattern` (default `{name}-{version}-{arch}-windows.zip`), `push: ChocolateyPushConfig { source: String /* default https://push.chocolatey.org/ */ }`.
   - `ScoopConfig` with: `name: Option<String>`, `bucket_url: String`, `description`, `homepage`, `license`, `download_repo: String`, `archive_pattern` (default `{name}-{version}-{arch}-windows.zip`), `binaries: Vec<String>`, `architectures: ScoopArchSet { x64: bool=true, arm64: bool=true }`.
   - `ResolvedChocolatey` / `ResolvedScoop` parallel to `ResolvedHomebrew`.
   - `ChocolateyOverrides<'a>` / `ScoopOverrides<'a>` parallel to `HomebrewOverrides<'a>`.
   - `resolve_chocolatey` / `resolve_scoop` methods on `ProjectConfig` using the same `merge` helper.
   - Extend `ProjectConfig` with `chocolatey: Option<ChocolateyConfig>` and `scoop: Option<ScoopConfig>`.
   - Add `validate_chocolatey` / `validate_scoop` mirroring `validate_homebrew` (reject embedded credentials in URLs, length-cap descriptions where the spec requires it — Chocolatey description is 4000 chars, tags ≤ 4000 chars; do not validate beyond what packagers actually reject).

2. **CLI arg groups** in [src/cli.rs](../../../../src/cli.rs):
   - `ChocolateyOverridesArgs` with `--choco-name`, `--choco-id`, `--choco-title`, `--choco-authors`, `--choco-description`, `--choco-project-url`, `--choco-license-url`, `--choco-tags`, `--choco-release-notes-url`, `--choco-download-repo`, `--choco-archive-pattern`, `--choco-push-source`.
   - `ScoopOverridesArgs` with `--scoop-name`, `--scoop-bucket`, `--scoop-description`, `--scoop-homepage`, `--scoop-license`, `--scoop-download-repo`, `--scoop-archive-pattern`, `--scoop-binary` (repeatable), `--scoop-no-arch <x64|arm64>` (repeatable).
   - Extend `InitCiCommand` with `--with-chocolatey`, `--with-scoop`, plus `#[command(flatten)] pub chocolatey: ChocolateyOverridesArgs` and `pub scoop: ScoopOverridesArgs`.
   - Reuse the `as_overrides()` pattern from `HomebrewOverridesArgs`.

3. **Wire validation** in [src/commands/init_ci.rs](../../../../src/commands/init_ci.rs):
   - Allow `--with-chocolatey` and `--with-scoop` on both `Platform::Forgejo` and `Platform::Github` (Windows packagers are not forgejo-only).
   - Both flags imply `--with-artifacts` (emit the same notice as `--with-homebrew`).
   - Both flags require `Runtime::Nix` initially OR `Runtime::Cargo` — decide based on whether nix builds Windows cross-targets in this repo today. Default: allow both runtimes, since the Windows artifact build runs on a Windows runner in phase 3 regardless of the Linux job's runtime. Document the choice in a comment.
   - Build `ChocolateyOptions` / `ScoopOptions` (mirroring `homebrew_options`) but do **not** thread them into `CiOptions` yet — leave a `TODO(phase-3)` next to the call site or stash them in a `let _ = …` so the code still compiles. The render-side struct lands in phase 3; phase 1 only proves config + CLI parse and resolve cleanly.

4. **Stubs**: re-export the new config types from `src/config.rs` and ensure `cargo check --all-targets` is clean. Do not add CLI subcommands yet (they land in phase 2).

## Acceptance criteria

- [ ] `cargo check --all-targets` passes.
- [ ] `cargo test --test config` passes with at least one new test per packager covering: required-field-missing error, CLI override beats config, config beats Cargo metadata, all-architectures-disabled error (Scoop only).
- [ ] `simit init-ci --platform github --with-chocolatey` parses without error when `[chocolatey]` is present in `simit.toml`; emits a clear error when it is absent.
- [ ] `simit init-ci --platform github --with-scoop` parses analogously.
- [ ] `simit init-ci --platform github --with-chocolatey --with-artifacts=false` still implies `--with-artifacts` and prints the implication notice.
- [ ] No changes to generated CI YAML yet — `simit init-ci --platform forgejo --check` on the simit repo itself still passes (i.e. the new flags don't accidentally alter existing output when unset).

## Files likely touched

- `src/config.rs`
- `src/cli.rs`
- `src/commands/init_ci.rs`
- `tests/config.rs`
- `Cargo.toml` (no new deps expected; only if a packager-specific URL parse helper is needed)

## Pitfalls

- **Chocolatey `id` vs `name`**: nuspec `<id>` is the package identifier (lowercase, no spaces, dashes ok); `<title>` is the human name. Mirror that distinction in the config struct, don't conflate.
- **Scoop bucket URL**: like a Homebrew tap, it must be a clonable git URL. Apply the same `reject_basic_auth_url` guard.
- **Architecture sets differ**: Homebrew has 4 platforms; Chocolatey ships a single multi-arch nupkg in most flows (no per-arch field in the basic schema, though `chocolateyInstall.ps1` branches on `$env:PROCESSOR_ARCHITECTURE`); Scoop has explicit per-arch URLs. Don't paste the Homebrew `HomebrewPlatformSet` blindly — model what each packager actually needs.
- **`deny_unknown_fields`**: keep it on all new structs; this is the convention.
- **CLI ID collisions**: clap will reject duplicate long names. The `--homebrew-*` namespace already exists; use `--choco-*` and `--scoop-*` to keep things short and unambiguous.

## Reference

- Prior art: commit `8456773` (introduce simit.toml), commit `e1b06bd` (add `--with-homebrew` to `init-ci`), commit `6126b81` (Homebrew render/bump CLI).
- Chocolatey nuspec schema: <https://docs.chocolatey.org/en-us/create/create-packages>.
- Scoop manifest schema: <https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests>.
