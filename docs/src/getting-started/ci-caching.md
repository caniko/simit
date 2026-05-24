# CI Caching

Generated Cargo-runtime workflows include two cache layers by default.

The first cache stores `~/.cargo/bin` so CI tools installed by the workflow can
be reused on later runs. The cache key includes the runner operating system and
the generated workflow files:

```text
cargo-bin-${{ runner.os }}-${{ hashFiles('.forgejo/workflows/ci.yaml', '.github/workflows/ci.yaml') }}
```

Changing the generated workflow invalidates the tool cache. Ordinary source
changes do not.

The second cache uses `Swatinem/rust-cache@v2` for Cargo registry and target
state. Simit enables `cache-all-crates` and `cache-on-failure`, and generated
workflows only save the rust-cache entry from `refs/heads/trunk`. Pull requests
and short-lived branches can restore existing cache entries without becoming
cache writers.

When a cached CI tool is missing, generated workflows install it explicitly:

```sh
command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked
```

The same guard is used for optional `cargo-audit` and `cargo-deny` installs in
Cargo-runtime workflows.

Nix-runtime workflows do not emit these Cargo cache steps. They rely on the
runner's Nix store and binary cache behavior instead of caching Cargo build
directories from inside the workflow.
