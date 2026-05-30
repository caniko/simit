# Phase 05 — canix: expose release secrets to Forgejo jobs

> **Recommended Codex model: GPT 5.5 high**
>
> NixOS host configuration touching the live atlas Forgejo runner: agenix
> secrets, container env/mount wiring, and a `canix deploy switch atlas` that can
> disrupt CI for every project on the runner if done wrong. The change is small
> but the blast radius and the "is it actually reaching the job?" verification
> need real care and rollback awareness. Complex × orchestrator → `high`.

## Working tree

`/data/nvme0/can/Projects/canix` (NixOS multi-host repo). Independent of the
simit/rs-modde phases — runs in parallel with Waves 0–3. Use the `canix-cli`
skill for deploy/secret operations.

## Goal

Every secret the rs-modde release workflow references is reachable by the atlas
Forgejo job at release time — either as a Codeberg Actions secret of the
expected name, or (preferred for runner-managed creds) exposed into the job
container the same way the Attic token is. The terminal validation (Phase 07)
must find each channel's credential present (publish) or absent (clean
soft-skip), never a half-configured failure.

## Why this matters now

`choco-api-key` is registered as a Forgejo **runner credential**
(`services.forgejo.runner.instances.codeberg.credentials.choco-api-key`,
file at `/run/credentials/forgejo-runner@codeberg.service/choco-api-key`), but
`root/hosts/atlas/server/forgejo-runners.nix` currently only mounts the **Attic**
token into job containers (`-v … -e ATTIC_TOKENS_DIR=…`). So the choco key — and
any other runner-managed cred — is not yet visible to jobs. The release workflow
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

1. Inventory what already exists under
   `canix/root/modules/server/forgejo-runner-secrets/` (choco-api-key,
   modde-apt-repo-_, modde-minisign-_, codeberg-base-token, …) and map each to
   the workflow reference it must satisfy.
2. Decide the exposure model per secret and apply consistently:
   - **Runner-credential → job env** (mirror Attic): for `choco-api-key`, add a
     mount + `-e` in `forgejo-runners.nix` container options so the job sees it
     as an env var, then set rs-modde's `[chocolatey].api_key_from_runner = true`
     and `api_key_env` to that var name (coordinate with Phase 06).
   - **Codeberg Actions secret**: for tokens the workflow reads as
     `${{ secrets.X }}` (codeberg*token, homebrew_tap_token, SCOOP_BUCKET_TOKEN,
     FLATHUB_TOKEN, WINGET_PAT, copr*_, AUR*SSH_KEY, modde_apt_repo*_,
     MINISIGN\_\*), ensure each is registered as a repo/org Actions secret on
     Codeberg with the matching name. (This step may be partly Codeberg-UI work,
     not canix — record which are UI-managed.)
3. For any newly wired runner cred, add its agenix secret module under
   `forgejo-runner-secrets/` following the existing `lib.nix` `secretMappings`
   pattern; `agenix rekey -a` as needed.
4. Deploy: `canix deploy switch atlas` (or the canix-cli equivalent). Confirm the
   runner restarts cleanly and stays online (atlas-runner online check).
5. Verify exposure without a real release: a trivial throwaway workflow / job (or
   inspect the running container) confirms the intended env var/file is present
   in the job environment. Record the exact env var name for Phase 06/07.

## Acceptance criteria

- [ ] A documented mapping exists: each release-workflow secret reference →
      how it's satisfied (Actions secret name, or runner-exposed env var name),
      or explicitly marked "intentionally absent → soft-skip".
- [ ] `choco-api-key` (or its chosen exposure) is reachable inside an atlas
      Forgejo job, verified by inspection or a probe job — not assumed.
- [ ] `canix deploy switch atlas` succeeds and the runner is back online with no
      failed systemd units.
- [ ] The exact env var name / secret name for chocolatey is handed to Phase 06
      (so `[chocolatey].api_key_from_runner`/`api_key_env` or `api_key_secret`
      match reality).

## Files likely touched

- `/data/nvme0/can/Projects/canix/root/hosts/atlas/server/forgejo-runners.nix`
- `/data/nvme0/can/Projects/canix/root/modules/server/forgejo-runner-secrets/*.nix`
  (+ `lib.nix` `secretMappings`)

## Pitfalls

- **Disrupting all runner jobs.** A bad `forgejo-runners.nix` edit can break CI
  for every Codeberg project on atlas. Symptom: runner offline / jobs fail to
  start after deploy. Recovery: `canix deploy switch atlas` to the prior
  generation (or `nixos-rebuild --rollback` on the host); validate online before
  walking away.
- **Credential present on host but not in container.** Systemd credentials live
  at `/run/credentials/...` on the host; jobs run in containers. Symptom: env var
  empty in job → workflow soft-skips even though the secret "exists." Recovery:
  add the `-v` mount + `-e` like the Attic token; verify inside the container.
- **Codeberg Actions secrets vs runner creds confusion.** `${{ secrets.X }}`
  resolves from Codeberg's Actions secret store, not from runner systemd creds.
  Symptom: a runner-only cred never appears as `secrets.X`. Recovery: either
  register the Actions secret on Codeberg, or switch that channel to the
  runner-env model (`*_from_runner` where simit supports it; chocolatey does).

## Reference

- Skills: `canix-cli`, `atlas-runner`, `forgejo-atlas-ci`.
- Attic exposure precedent to mirror: `forgejo-runners.nix` lines around
  `ATTIC_TOKENS_DIR`.
- Consumer: [07-rs-modde-validate-and-release.md](./07-rs-modde-validate-and-release.md).
