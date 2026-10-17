#!/usr/bin/env python3
"""Read-only generation oracle for disposable updater repository tests."""
import json
import subprocess

# The fixture provisions only its disposable clone's identity, never operator
# globals or the real repository. Signing is exercised independently upstream.
for key, value in [("user.name", "Updater fixture"),
                   ("user.email", "updater@example.invalid"),
                   ("commit.gpgsign", "false")]:
    subprocess.run(["git", "config", "--local", key, value], check=True)
print(json.dumps(dict(
    schema="simit.ci-update-plan.v1",
    files=[dict(relative_path=".github/workflows/ci.yaml",
                content="name: CI\non:\n  push:\njobs:\n  test:\n    steps: []\n"),
           dict(relative_path="new-managed.txt", content="new output\n")],
    obsolete=[],
)))
