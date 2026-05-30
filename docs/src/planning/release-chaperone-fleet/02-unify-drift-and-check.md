# Phase 02 — Unify drift detection and `--check` rendering

> **Recommended Codex model: GPT 5.5 / high**
>
> Complex design call: the drift detector and the `--check` path
> currently render expected files through two different
> option-resolution paths. Picking the unified model (config-first vs
> inference-first vs both) materially affects every adopting repo's
> noise floor. Orchestrator role on complex work — 5.5 at `high`
> matches the routing matrix.

## Working tree

[/data/nvme0/can/Projects/simit](file:///data/nvme0/can/Projects/simit)
— branch off `trunk` _after_ phase 01 has landed.

## Goal

`simit projects list` and `simit init ci --platform <p> --check
--diff` agree on whether a repo has drifted. When they report
"clean", the chaperone can trust the result without re-running with
extra flags. When they report "drift", they describe the same
drift in the same terms.

Concretely: any repo whose generated CI was produced through current
simit (and whose options are persisted via phase 01) reports `ci:
managed` in `simit projects list` and a clean diff in
`simit init ci --check --diff` with no CLI flags.

## Why this matters now

The dossier documents skillnet as the canonical example: drift
detector flags `ci=drift`, but the bare `--check` only reports
drift because it does not restore `--with-audit/--with-deny/...`.
Add `--with-msrv --with-audit --with-deny --with-docs` and the diff
is clean. Two codepaths, two opinions; the chaperone has to know
which to trust.

This is the user-facing fix for the "spurious drift" pain.
Without it, phase 01's persistence work still requires the user to
re-run with flags to convince themselves the check is real.

## Out of scope

- Re-implementing the `cargo metadata`-based package selection.
- Changing the workflow rendering itself (`src/render/ci.rs`).
- Per-repo cleanup work (phase 07).
- Making `simit init flake --check` agree with anything in this
  phase. That is phase 03's territory.

## Why this matters now (continued — design choice)

There are two viable unification models. Decide which to ship based
on phase 01's behavior:

- **Config-first.** Always read `simit.toml`'s `[ci]` as the source
  of truth for option resolution in both paths. Drop
  `infer_ci_options` entirely. Cleaner but requires every adopting
  repo to have a populated `simit.toml`.
- **Inference fallback.** Read `simit.toml` first; fall back to
  inference from existing workflows if the config is absent or
  incomplete. Preserves zero-config adoption.

Default recommendation in this phase: **inference fallback**. It
keeps zero-config adoption working while making the persisted-config
path canonical. Mark this as a design call in the commit message.

## Plan

1. **Audit current option sources.** Read
   [src/registry.rs:541-789](../../../src/registry.rs#L541) and
   [src/commands/init_ci.rs:84-160](../../../src/commands/init_ci.rs#L84)
   side by side. List every option in `CiOptions` and document
   which path resolves it from which source. Use this table to
   verify nothing falls through the gap after refactoring.

2. **Extract a single `ResolvedCiInputs` builder** that the drift
   detector and the `--check`/regen path both call. The builder
   precedence:
   - CLI flags (when the call site provides them).
   - `simit.toml` `[ci]` (from phase 01).
   - Inference from existing marked workflows
     ([src/registry.rs:754](../../../src/registry.rs#L754)
     `infer_ci_options`).
   - Generator defaults.

   Drift detection call site passes no CLI flags; `--check`/regen
   passes whatever the user supplied. Both then route through the
   same builder.

3. **Migrate `infer_ci_options` callers** in
   `src/registry.rs::infer_expected_ci_files` to use the new builder
   (so the drift detector now sees config-first results too). Keep
   inference as the fallback layer in the builder, not as a separate
   codepath.

4. **Drop the divergence.** Once both paths use the same builder,
   remove the residual config reads scattered through
   `infer_ci_options` (e.g. `config.ci.extra_setup.clone()`
   duplications) in favor of single-source resolution.

5. **Tests.** Add coverage for the unification contract:
   - A repo with `simit.toml [ci] with_audit = true` and a generated
     workflow that includes the audit steps reports `managed` (not
     `drift`) in `simit projects show` and clean in
     `simit init ci --check --diff`.
   - A repo with no `simit.toml` but with audit steps in its
     workflow (inference fallback) also reports `managed`.
   - A repo whose `simit.toml` claims `with_audit = true` but whose
     workflow lacks audit steps reports `drift` with a diff that
     names the missing audit step.

6. **Docs.** Update
   [docs/src/getting-started/project-registry.md](../../getting-started/project-registry.md)
   to describe the new resolution order and the relationship between
   `simit.toml [ci]` and drift detection.

## Acceptance criteria

- [ ] `simit projects list` and `simit init ci --platform <p>
--check --diff` agree on `managed` vs `drift` for every repo
      in `simit projects list`.
- [ ] A single `ResolvedCiInputs` builder is the only path to
      `CiOptions` in both the drift detector and `init ci`.
- [ ] Inference from workflow content still works for repos with
      no `simit.toml`.
- [ ] CLI flags continue to override both config and inference.
- [ ] `cargo test` covers all three of the unification scenarios
      named in step 5.
- [ ] The simit repo itself passes `cargo run -- init ci
--platform forgejo --check --diff` with no flags.

## Files likely touched

- `src/registry.rs` (drift detector callers)
- `src/commands/init_ci.rs` (option builder)
- New module if extraction is non-trivial:
  `src/ci/resolved_inputs.rs`
- `tests/`
- `docs/src/getting-started/project-registry.md`

## Pitfalls

- **Do not let the unification change rendered workflow output.**
  This phase is purely about resolution, not generation. Snapshot
  test the simit repo's own generated workflows before and after to
  prove they match.
- **Inference is fuzzy.** `infer_ci_options` reads workflow text for
  string markers (`"cargo audit"`, `"cargo deny check"`, etc.). If
  the renderer ever changes those strings, inference silently
  breaks. Add a comment cross-linking the markers in
  `infer_ci_options` to the emitting site in `src/render/ci.rs`.
- **Beware the order trap.** If config says one thing and inference
  says another, config wins — but the user may be surprised. Log a
  one-line note when the drift detector falls back to inference
  because no config was present.

## Reference

- Research dossier: improvement (2).
- Phase 01 (must land first).
- [src/registry.rs:541](../../../src/registry.rs#L541) — drift detector.
- [src/registry.rs:754](../../../src/registry.rs#L754) — current inference.
- [src/commands/init_ci.rs:84](../../../src/commands/init_ci.rs#L84) — CLI option build.
