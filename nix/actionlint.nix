{
  actionlint,
  lib,
}:
# GitHub's native queue contract is newer than actionlint 1.7.12.
# Remove this parser patch once upstream accepts and validates it.
# https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency
assert lib.assertMsg (actionlint.version == "1.7.12") "requalify the native queue parser patch when updating actionlint";
  actionlint.overrideAttrs (old: {
    patches = (old.patches or []) ++ [./actionlint-queue.patch];
  })
