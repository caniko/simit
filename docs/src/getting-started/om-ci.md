# Omnix CI

`simit init ci --runtime nix --with-om-ci` generates Nix CI that uses
[Omnix `om ci`](https://omnix.page/om/ci.html). Use it when the flake's
`checks.*` covers the tests, Clippy, formatting, and docs that CI must enforce.

`--with-om-ci` defaults to replace mode: one `om ci run` step replaces `nix
flake check` plus `nix develop -c cargo ...`. This is fast with simit's
generated crane flake, whose checks define the CI contract.

Replace mode assumes `checks.*` covers every required gate. Audit custom flake
checks before enabling replace mode; if the project relies on a cargo dev-shell
step outside `checks.*`, use augment mode or keep the legacy Nix runtime
workflow.

## Enable Replace Mode

```sh
simit init ci --platform forgejo --runtime nix --with-om-ci
```

For Forgejo, replace mode emits this CI workflow:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow_ref }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1

      - name: Run om ci
        env:
          OMNIX_REF: "github:juspay/omnix/v1.3.2"
        run: nix run "$OMNIX_REF" -- ci run
```

Forgejo + Nix workflows run on branch pushes and skip `pull_request` events.

## Enable Augment Mode

```sh
simit init ci --platform forgejo --runtime nix --om-ci-augment
```

Augment mode runs `om ci` first, then keeps the legacy Nix-runtime cargo steps:

```yaml
name: CI

on:
  push:
    branches: ["**"]

concurrency:
  group: ${{ github.workflow_ref }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1

      - name: Run om ci
        env:
          OMNIX_REF: "github:juspay/omnix/v1.3.2"
        run: nix run "$OMNIX_REF" -- ci run

      - name: Check flake
        run: nix flake check

      - name: Test
        run: nix develop -c cargo test

      - name: Clippy
        run: nix develop -c cargo clippy --all-targets -- --deny warnings

      - name: Package crate
        run: nix develop -c cargo package --allow-dirty
```

## Pin Omnix

The Omnix flakeref resolves in this order:

1. CLI `--omnix-ref`
2. Project config `[ci].omnix_ref`
3. User config `[ci.tools.omnix].ref`
4. simit's pinned default

```sh
simit init ci --platform forgejo --runtime nix --with-om-ci \
  --omnix-ref github:juspay/omnix/v1.3.2
```

```toml
[ci]
om_ci = true
omnix_ref = "github:juspay/omnix/v1.3.2"
```

```toml
[ci.tools.omnix]
ref = "github:juspay/omnix/v1.3.2"
```

## Caveats

`--with-om-ci` requires `--runtime nix`.

Replace mode assumes `checks.*` covers every required gate. For custom flakes,
audit `[flake.expected_outputs].checks` before replacing the generated cargo
dev-shell steps.

Requested `cargo-audit` and `cargo-deny` steps stay separate, including in
replace mode. If `checks.*` also runs those tools, they run twice.
