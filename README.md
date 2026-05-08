# simit

`simit` is a semver-aware commit helper for Rust projects.

```sh
simit commit patch -m "fix admission edge case"
simit commit minor -m "add weighted provider API"
simit commit major -m "remove deprecated API"
```

The command bumps the selected package version in `Cargo.toml`, updates
`Cargo.lock` when present, stages only the version files it changed, delegates
to `git commit`, and creates a signed release tag named exactly like the new
version with the message `Release <version>`.

Use `--no-tag` to skip tag creation:

```sh
simit commit --no-tag patch -m "prepare unreleased patch"
```

Use `--no-sign` to create an unsigned lightweight tag in environments where
GPG signing is unavailable:

```sh
simit commit --no-sign patch -m "release without tag signing"
```

For workspaces with more than one package, choose the target package:

```sh
simit commit --package memory-admission patch -m "release memory-admission"
```
