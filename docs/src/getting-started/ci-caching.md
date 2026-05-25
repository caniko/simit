# CI Caching

Generated Cargo-runtime workflows include two cache layers by default.

The first cache stores `~/.cargo/bin` so CI tools installed by the workflow can
be reused on later runs. The cache key includes the runner operating system and
the generated workflow files:

```text
cargo-bin-${{ runner.os }}-${{ hashFiles('.forgejo/workflows/*.yaml') }}
```

Forgejo workflows hash `.forgejo/workflows/*.yaml`; GitHub workflows hash
`.github/workflows/*.yaml`. Changing generated workflows for the active
platform invalidates the tool cache. Ordinary source changes do not.

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

## Measured speedup

Reference numbers from
[`rs-memory-admission`](https://codeberg.org/caniko/rs-memory-admission) on
its self-hosted `atlas` runner, after adopting simit-managed CI in commit
`d40f905`:

| Run                                 | Cache state             | Wall-time             |
| ----------------------------------- | ----------------------- | --------------------- |
| First seeding runs (`run#57`–`#59`) | writes only, no restore | failed early, ≤ 2 min |
| First green warm run (`run#60`)     | warm restore            | **2m03s**             |
| Subsequent warm run (`run#61`)      | warm restore            | **3m18s**             |

The warm runs include `cargo nextest run --all-features`, both clippy
profiles, `cargo doc`, `cargo audit`, `cargo deny`, and `cargo package` —
i.e. the full quality-gate suite, not a stripped-down subset.

Your numbers will vary with crate-graph size and how often `Cargo.lock`
invalidates the rust-cache key. Expect the first run after a lockfile bump
to recompile from scratch; subsequent runs at the same lock should restore
in under a minute.
