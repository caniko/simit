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

## Installing Hooks

Use `simit hooks install` to write project-local pre-commit wrappers even when
Git is configured with a system `core.hooksPath` dispatcher:

```sh
simit hooks install
simit hooks install --check
simit hooks install --diff
```

`--check` exits non-zero when the installed hooks drift from simit's expected
wrapper content. `--diff` prints the wrapper diff without rewriting files.

If a repo-local `core.hooksPath` such as `.git/hooks` shadows a friendly system
dispatcher, simit warns because project hooks would run while dispatcher-owned
system hooks are bypassed. Use `--fix` to unset the rogue local override when a
dispatcher is available:

```sh
simit hooks install --fix
```

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
