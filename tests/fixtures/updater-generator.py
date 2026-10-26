#!/usr/bin/env python3
"""Read-only generation oracle for disposable updater repository tests."""
import json
import sys
from pathlib import Path

scoped = Path("fixture-plans.json")
if scoped.exists():
    print(json.dumps(json.loads(scoped.read_text())[" ".join(sys.argv[1:])]))
    raise SystemExit
plan = Path("fixture-plan.json")
if plan.exists():
    print(plan.read_text())
    raise SystemExit
print(json.dumps(dict(
    schema="simit.ci-update-plan.v1",
    files=[dict(relative_path=(Path("fixture-backend").read_text().strip()
                              if Path("fixture-backend").exists()
                              else ".github/workflows/ci.yaml"),
                content="name: CI\non:\n  push:\njobs:\n  test:\n    steps: []\n"),
           dict(relative_path="new-managed.txt", content="new output\n")],
    obsolete=[],
)))
