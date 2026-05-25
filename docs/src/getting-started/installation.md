# Installation

Install from crates.io:

```sh
cargo install simit
```

For source builds, clone the canonical repository and use Cargo:

```sh
git clone https://codeberg.org/caniko/simit.git
cd simit
cargo install --path .
```

The project is licensed under the MIT License. See the repository `LICENSE`
file for the full license text.

## Flake Ownership

`simit init flake` defaults to hooks-only ownership. In that mode simit writes
and checks `nix/pre-commit.nix` only; an existing `flake.nix` remains
project-owned and is ignored by `simit init flake --check --diff`.

Use full ownership only for repositories that want simit to manage the whole
canonical crane flake:

```sh
simit init flake --scope full
```

Persist the choice in project config when a repository should always use full
ownership:

```toml
[flake]
scope = "full"
```

Existing repositories that already have simit's full flake files keep full
behavior until maintainers opt into hooks-only. To migrate from full to
hooks-only, set `scope = "hooks-only"` or run `simit init flake --scope
hooks-only`; simit will not delete the existing `flake.nix`.
