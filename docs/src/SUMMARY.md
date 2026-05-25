# Summary

- [Introduction](introduction.md)
- [Installation](getting-started/installation.md)
- [Quick Start](getting-started/quick-start.md)
- [Project Registry](getting-started/project-registry.md)
- [Release Integrity](getting-started/release-integrity.md)
- [CI Caching](getting-started/ci-caching.md)
- [Windows Packaging](getting-started/windows-packaging.md)
- [Omnix CI](getting-started/om-ci.md)
- [Public API](public-api.md)
- [Release Maintenance](release-maintenance.md)

# Plan: publish-version-extractor-fix

- [Overview](planning/publish-version-extractor-fix/README.md)
- [01 — Commit and release simit 0.15.1](planning/publish-version-extractor-fix/01-simit-commit-and-release.md)
- [02 — Package-scope the extractor](planning/publish-version-extractor-fix/02-simit-package-scope-extractor.md)
- [03 — Detritus: commit regen and retry publish](planning/publish-version-extractor-fix/03-detritus-commit-and-retry-publish.md)
- [04 — Sweep latent dependents](planning/publish-version-extractor-fix/04-sweep-latent-dependents.md)

# Plan: hooks-enforcement

- [Overview](planning/hooks-enforcement/README.md)
- [Research dossier](planning/hooks-enforcement/hooks-enforcement-research.md)
- [01 — Simit: reality-based hook detection](planning/hooks-enforcement/01-simit-detect-hooks-status.md)
- [02 — Simit: `simit hooks install` subcommand](planning/hooks-enforcement/02-simit-hooks-install-subcommand.md)
- [03 — Canix: dispatcher hooks directory](planning/hooks-enforcement/03-canix-dispatcher-hooks.md)
- [04 — Fleet sweep: install hooks across projects](planning/hooks-enforcement/04-fleet-sweep-install-hooks.md)
- [05 — Simit: surface conflicted hook state](planning/hooks-enforcement/05-simit-surface-conflicted-state.md)
