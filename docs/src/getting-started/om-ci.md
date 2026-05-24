# Omnix CI

`simit init ci --runtime nix --with-om-ci` wires generated Nix CI through
[Omnix `om ci`](https://omnix.page/om/ci.html). Use it when the project has a
flake and `checks.*` already covers test, clippy, formatting, and documentation
work that CI must enforce.

Replace mode is the default. It trades the legacy generated sequence of `nix
flake check` plus `nix develop -c cargo ...` steps for one `om ci run` step.
That is fast for projects using simit's generated crane flake because the flake
checks are the CI contract.

Do not enable replace mode for a custom flake until its checks are audited. If
the project relies on the cargo dev-shell steps to exercise behavior that is
not represented in `checks.*`, use augment mode or keep the legacy Nix runtime
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
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@v4

      - name: Run om ci
        env:
          OMNIX_REF: "github:juspay/omnix/v1.3.2"
        run: nix run "$OMNIX_REF" -- ci run
```

Forgejo + Nix workflows keep the existing trigger rule: they run on branch
pushes and skip `pull_request` events.

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
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: atlas
    steps:
      - name: Checkout
        uses: https://code.forgejo.org/actions/checkout@v4

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

Replace mode assumes the flake checks are complete. For custom flakes, audit
`[flake.expected_outputs].checks` before replacing the generated cargo
dev-shell steps.

`cargo-audit` and `cargo-deny` still render as discrete generated steps when
requested, even under replace mode. Projects that also run those tools from
`checks.*` will run them twice.
