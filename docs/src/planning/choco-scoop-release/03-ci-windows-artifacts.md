# Phase 3 — CI workflow: Windows artifacts, Chocolatey push, Scoop publish

> **Recommended Codex model: GPT 5.5 high**
>
> This is the integration phase and it carries real design choices:
> (a) how to add a Windows job to `release-artifacts.yaml` without breaking
> the Linux-only Homebrew flow, (b) which runner label to default to on
> Forgejo vs. GitHub (no `windows-latest` exists on Codeberg/forgejo
> typically — likely need a user-supplied `--windows-runner` or a clear
> error), (c) how `choco push` authentication threads through CI secrets,
> (d) whether to push to the Scoop bucket from the existing Linux job or a
> Windows job. Mediocre output will ship a broken matrix or accidentally
> regress the existing Homebrew tap step. High effort earns its keep here.

## Working tree

Starts from phases 1 + 2 merged. The simit repo's own `.forgejo/workflows/` is the canary — `--check` must still pass on `trunk` after this phase, with `--with-chocolatey`/`--with-scoop` opt-in.

## Goal

Extend [src/render/ci.rs](../../../../src/render/ci.rs) `artifacts_workflow` so that:

- `--with-chocolatey` adds a Windows build matrix entry, computes archive sha256s, and runs `choco pack && choco push` from a Windows runner.
- `--with-scoop` reuses the Windows artifacts (or downloads released archives) and pushes a manifest to the bucket repo via the same clone-commit-push pattern as Homebrew tap.
- Both can coexist with `--with-homebrew` in a single workflow without duplicating Linux build steps.

## Why

CI is where the user-facing value lands. Phases 1 + 2 are plumbing; phase 3 is the deliverable. The artifacts workflow currently builds Linux only (`cargo build --release` or `nix build`) — Windows binaries don't exist yet, so neither packager has anything to ship.

## Out of scope

- New tests beyond CI YAML snapshot tests (phase 4 covers integration smoke).
- Replacing the existing Homebrew publish step (leave untouched; add alongside).

## Plan

1. **Extend `CiOptions`** with `chocolatey: Option<ChocolateyOptions>` and `scoop: Option<ScoopOptions>`, mirroring `HomebrewOptions`. Plumb through `init_ci::run`.

2. **Windows runner selection**:
   - Add `--windows-runner <LABEL>` to `InitCiCommand` with sensible defaults: `windows-latest` for GitHub, no default for Forgejo (require explicit override and error helpfully if `--with-chocolatey` or `--with-scoop` is set on Forgejo without it).
   - The Linux-runner `--runner` flag already exists; keep its semantics scoped to Linux jobs.

3. **`release-artifacts.yaml` matrix**:
   - Convert the single `build` job into a matrix when any Windows packager is enabled. Strategy: keep the existing Linux job as `build-linux` (unchanged for the Homebrew path), and add `build-windows` that runs on the Windows runner, uses `dtolnay/rust-toolchain@stable` (GitHub) or `rustup` (Forgejo Windows runner — usually pre-installed), runs `cargo build --release --locked`, and uploads `target/release/<binary>.exe`.
   - Pack the Windows binary into the archive shape declared by `archive_pattern` (default `<name>-<version>-x86_64-windows.zip`); use PowerShell `Compress-Archive` for portability.

4. **`publish-windows-packages` job** depending on `build-windows`:
   - Runs on the Windows runner.
   - If `chocolatey` opts are present: downloads the Windows artifact, runs `simit chocolatey bump --version $tag --package-dir … --archive x64=…/…zip --push --push-source $CHOCO_PUSH_SOURCE --api-key-env CHOCOLATEY_API_KEY`. Env: `CHOCOLATEY_API_KEY: ${{ secrets.chocolatey_api_key }}`.
   - If `scoop` opts are present: clones the bucket repo (same credential helper pattern as `push_homebrew_publish_step`), runs `simit scoop bump --version $tag --bucket … --archive x64=…/…zip --push`, env: `SCOOP_BUCKET_TOKEN: ${{ secrets.scoop_bucket_token }}`.
   - Use `nix run '.#rs-harbor'` only on Linux jobs; Windows must invoke `simit` from a pre-published binary or `cargo install simit` step. Pick `cargo install --locked simit` for simplicity; the Windows job already has `rustup`.

5. **Self-check**:
   - Extend `push_self_check_suffix` to emit `--with-chocolatey` / `--with-scoop` / `--windows-runner` flags when set, so simit's own CI verifies its generated output for these flags too once simit's `simit.toml` opts in (do not opt simit in by default in this phase — that's a config change for the user/maintainer).

6. **Forgejo specifics**:
   - Forgejo Actions doesn't natively run Windows runners on Codeberg's shared infra. Default behaviour with `--platform forgejo --with-chocolatey` should be: emit the workflow with a user-supplied `--windows-runner` label, **and** print a one-line stderr warning that the user must register a Windows runner in their Forgejo instance. This matches the spirit of the existing `--runner` flag's flexibility.

## Acceptance criteria

- [ ] `cargo test --test init_ci` passes; add snapshots for `release-artifacts.yaml` with each combination: `chocolatey`-only, `scoop`-only, `chocolatey+scoop`, `chocolatey+scoop+homebrew`.
- [ ] `simit init-ci --platform forgejo --runtime nix --with-homebrew --check` on the simit repo passes unchanged (no incidental drift).
- [ ] `simit init-ci --platform github --with-chocolatey --windows-runner windows-latest` produces a workflow whose YAML parses with `yq` (or `serde_yaml` in a unit test).
- [ ] `simit init-ci --platform forgejo --with-chocolatey` without `--windows-runner` errors clearly.
- [ ] Generated workflow is idempotent: running `init-ci` twice with the same flags produces byte-identical output.
- [ ] No regression in `simit init-ci --platform forgejo --with-homebrew --check` against the simit repo itself.

## Files likely touched

- `src/render/ci.rs`
- `src/commands/init_ci.rs`
- `src/cli.rs` (add `--windows-runner`)
- `tests/init_ci.rs` (new snapshots)

## Pitfalls

- **PowerShell line continuations**: backticks, not backslashes. Don't paste bash heredoc patterns into the Windows job verbatim.
- **`Compress-Archive` strips executable bit**: irrelevant for `.exe`, but watch out if the project later ships shell scripts.
- **Secret name casing**: forgejo/codeberg lowercases secret names in `${{ secrets.foo }}`; GitHub is case-sensitive. The existing Homebrew step uses `secrets.homebrew_tap_token` — keep the same lowercase convention.
- **`cargo install simit` on the Windows runner is slow**: ~3 minutes cold. Acceptable for a tag-push workflow; flag in a `# TODO(cache)` comment.
- **`choco` not installed everywhere**: GitHub `windows-latest` ships Chocolatey, Forgejo Windows runners typically don't. Emit an `Install Chocolatey` step that no-ops if `choco --version` succeeds.
- **`simit.toml` is the canonical source**: don't duplicate fields as workflow inputs the user must keep in sync; thread everything via the CLI flags simit already accepts.
- **Don't break the existing Homebrew publish step**: it lives inside the Linux `build` job today. Moving it into a renamed `build-linux` job will rename CI steps in everyone's workflow files — verify with `--diff` on a downstream consumer if possible, otherwise document the rename in CHANGELOG (phase 4).

## Reference

- Existing Homebrew publish: [src/render/ci.rs:271](../../../../src/render/ci.rs#L271).
- `choco push` docs: <https://docs.chocolatey.org/en-us/create/commands/push>.
- Scoop bucket publishing: <https://github.com/ScoopInstaller/Scoop/wiki/Buckets>.
