# Phase 2b — Scoop manifest renderer, bump command, bucket bootstrap

> **Recommended Codex model: GPT 5.5 medium**
>
> Scoop manifests are deterministic JSON with a documented schema. The render
> is mechanical, but the bucket-publish flow (clone bucket repo, edit JSON,
> commit, push) is structurally a closer cousin of the Homebrew tap flow,
> meaning the heavy lifting is "adapt the Homebrew bump logic to a JSON file
> at `bucket/<name>.json` instead of `Formula/<name>.rb`". Medium routing
> matches: more than mechanical, but the pattern is already in-tree.

## Working tree

Starts from phase 1 merged into `trunk`. Safe to run in parallel with 2a — disjoint files except `src/cli.rs::Commands`, `src/commands/mod.rs`, `src/render/mod.rs`, `src/main.rs`.

## Goal

Render Scoop JSON manifests, expose `simit scoop {render,bump}` and `simit init-scoop-bucket`, matching the Homebrew tap publish flow's git-based contract.

## Out of scope

- CI workflow integration (phase 3).
- Chocolatey (phase 2a).

## Plan

1. **Renderer** at `src/render/scoop_manifest.rs`:
   - `pub fn render(opts: &ScoopRenderOptions, checksums: &ScoopChecksums) -> String` emitting canonical Scoop JSON: `version`, `description`, `homepage`, `license`, `architecture: { "64bit": { url, hash, bin }, "arm64": { … } }`, optional `checkver`/`autoupdate` blocks left out for v1.
   - JSON output must be stable: use `serde_json::to_string_pretty` with sorted keys via `serde_json::Map`'s insertion order, or hand-write with a deterministic field order. Snapshot tests will catch drift.
   - `ScoopRenderOptions` derived from `ResolvedScoop` plus per-arch URLs computed from `archive_pattern` + `download_repo`.

2. **`simit scoop` subcommand**:
   - `Render { version, output: Option<Path>, ScoopOverridesArgs }` — stdout or file.
   - `Bump { version, bucket: Utf8PathBuf, archive: Vec<String>, push: bool, commit_message: Option<String>, ScoopOverridesArgs }` — mirror `HomebrewBumpArgs` field-for-field. Writes `bucket/<name>.json`, optional `git add/commit/push` exactly like Homebrew bump.
   - Reuse `crate::sha256::sha256_hex`.

3. **`simit init-scoop-bucket`**:
   - Mirror `init_homebrew_tap` precisely: `--target`, `--check`, `--diff`, `--print`, `--no-git`. The bucket repo layout is a flat directory of JSON files at the root (or under `bucket/`); pick `bucket/` for consistency with the official Scoop pattern.
   - Render `bucket/<name>.json` skeleton with placeholder hashes (zeros) so `simit scoop bump` overwrites them on first release.

4. **Wiring**:
   - Add `Commands::Scoop`, `Commands::InitScoopBucket` to [src/cli.rs](../../../../src/cli.rs).
   - Register modules and dispatch in `src/commands/mod.rs` and `src/main.rs`.

## Acceptance criteria

- [ ] `cargo check --all-targets` passes.
- [ ] `cargo test --test scoop` (new) passes with: snapshot of rendered manifest, per-arch hash injection, missing-arch error, `--no-arch arm64` produces a manifest without the arm64 block.
- [ ] `simit scoop render --version 0.1.0` prints JSON that `jq -e .` parses.
- [ ] `simit scoop bump --version 0.1.0 --bucket /tmp/bucket --archive x64=/tmp/a.zip --archive arm64=/tmp/b.zip` writes the manifest and (without `--push`) leaves the bucket dir as a clean git diff.
- [ ] `simit init-scoop-bucket --target /tmp/bucket --print` matches what `simit scoop render` would emit for an unreleased version (with placeholder hashes).
- [ ] Phase 1's `--with-scoop` still parses; no CI YAML drift.

## Files likely touched

- `src/render/scoop_manifest.rs` (new)
- `src/render/mod.rs`
- `src/commands/scoop.rs` (new)
- `src/commands/init_scoop_bucket.rs` (new)
- `src/commands/mod.rs`
- `src/cli.rs`
- `src/main.rs`
- `tests/scoop.rs` (new)

## Pitfalls

- **JSON determinism**: any HashMap iteration in the renderer will produce non-deterministic output. Use `BTreeMap` or hand-emit.
- **Hash format**: Scoop accepts `sha256:<hex>` or bare `<hex>`; pick one and stick with it. Bare hex is shorter and is what `scoop` itself emits.
- **Architecture keys**: the manifest uses `"64bit"` (not `"x64"`), `"32bit"`, `"arm64"`. Don't accidentally write `"x64"`.
- **`bin` field**: must be a string or array of strings; if the package ships a single binary, use a string for cleaner diffs.
- **Bucket URL credentials**: same `reject_basic_auth_url` guard as Homebrew.

## Reference

- Scoop manifest schema: <https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests>.
- Reference JSON schema file: <https://raw.githubusercontent.com/ScoopInstaller/Scoop/master/schema.json>.
- Prior art: [src/commands/homebrew.rs](../../../../src/commands/homebrew.rs), [src/commands/init_homebrew_tap.rs](../../../../src/commands/init_homebrew_tap.rs).
