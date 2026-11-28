# Phase 05 — canix: expose release secrets to Forgejo jobs

> **Recommended Codex model: GPT 5.5 high**
>
> NixOS host configuration touching the live Forgejo release runner: agenix
> secrets, container env/mount wiring, and a host switch that can
> disrupt CI for every project on the runner if done wrong. The change is small
> but the blast radius and the "is it actually reaching the job?" verification
> need real care and rollback awareness. Complex × orchestrator → `high`.

## Working tree

`<workspace>/canix` (NixOS multi-host repo). Independent of the
simit/rs-modde phases — runs in parallel with Waves 0–3.

## Goal

Every secret the rs-modde release workflow references is reachable by the release
Forgejo job at release time — either as a Codeberg Actions secret of the
expected name, or (preferred for runner-managed creds) exposed into the job
container like the existing cache credential. The terminal validation (Phase 07)
must find each channel's credential present (publish) or absent (clean
soft-skip), never a half-configured failure.

## Why this matters now

Runner credentials do not automatically reach job containers. The Chocolatey
API key needs an explicit injection path, like the cache credential. The release workflow
references `codeberg_token`, `chocolatey_api_key`/runner env, `homebrew_tap_token`,
`SCOOP_BUCKET_TOKEN`, `FLATHUB_TOKEN`, `WINGET_PAT`, `copr_login/username/token`,
`AUR_SSH_KEY`, `modde_apt_repo_gpg_key/_id/_passphrase/_ssh_key`,
`MINISIGN_SECRET_KEY/_PASSWORD`, optional `COSIGN_*`.

## Out of scope

- Do **not** invent secret values. Only wire exposure of existing creds /
  register the secret names; missing secrets are an external blocker to report,
  not to fabricate.
- Do **not** change simit or rs-modde here.
- Do **not** broaden the runner's mounted secrets beyond what the release needs.

## Plan

1. Inventory the runner's existing credentials and map each to the workflow
   reference it must satisfy.
2. Decide the exposure model per secret and apply consistently:
   - **Runner-credential → job env**: for the Chocolatey API key, add a
     mount and environment binding in the runner configuration so the job sees it
     as an env var, then set rs-modde's `[chocolatey].api_key_from_runner = true`
     and `api_key_env` to that var name (coordinate with Phase 06).
   - **Codeberg Actions secret**: for tokens the workflow reads as
     `${{ secrets.X }}` (codeberg*token, homebrew_tap_token, SCOOP_BUCKET_TOKEN,
     FLATHUB_TOKEN, WINGET_PAT, copr*_, AUR*SSH_KEY, modde_apt_repo*_,
     MINISIGN\_\*), ensure each is registered as a repo/org Actions secret on
     Codeberg with the matching name. (This step may be partly Codeberg-UI work,
     not canix — record which are UI-managed.)
3. Add any newly wired runner credential through the existing secret management
   configuration and rekey as needed.
4. Deploy to the runner host. Confirm the runner restarts cleanly and stays online.
5. Verify exposure without a real release: a trivial throwaway workflow / job (or
   inspect the running container) confirms the intended env var/file is present
   in the job environment. Record the exact env var name for Phase 06/07.

## Acceptance criteria

- [ ] A documented mapping exists: each release-workflow secret reference →
      how it's satisfied (Actions secret name, or runner-exposed env var name),
      or explicitly marked "intentionally absent → soft-skip".
- [ ] The Chocolatey API key is reachable inside a release
      Forgejo job, verified by inspection or a probe job — not assumed.
- [ ] The host switch succeeds and the runner is back online with no
      failed systemd units.
- [ ] The exact env var name / secret name for chocolatey is handed to Phase 06
      (so `[chocolatey].api_key_from_runner`/`api_key_env` or `api_key_secret`
      match reality).

## Files likely touched

- The runner host and secret management configurations.

## Pitfalls

- **Disrupting all runner jobs.** A bad runner configuration edit can break CI
  for every Codeberg project on the runner. Symptom: runner offline / jobs fail to
  start after deploy. Recovery: switch the host to the prior
  generation (or `nixos-rebuild --rollback` on the host); validate online before
  walking away.
- **Credential present on host but not in container.** Systemd credentials live
  at `/run/credentials/...` on the host; jobs run in containers. Symptom: env var
  empty in job → workflow soft-skips even though the secret "exists." Recovery:
  add the mount and environment binding; verify inside the container.
- **Codeberg Actions secrets vs runner creds confusion.** `${{ secrets.X }}`
  resolves from Codeberg's Actions secret store, not from runner systemd creds.
  Symptom: a runner-only cred never appears as `secrets.X`. Recovery: either
  register the Actions secret on Codeberg, or switch that channel to the
  runner-env model (`*_from_runner` where simit supports it; chocolatey does).

## Reference

- See the runner's deployment and CI documentation.
- Consumer: [07-rs-modde-validate-and-release.md](./07-rs-modde-validate-and-release.md).
