#!/usr/bin/env python3
"""Exercise the packaged generator's plan-parser command with real Nix, no builds."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap


generator = Path(sys.argv[1]).resolve(strict=True)
with tempfile.TemporaryDirectory(prefix="simit-crow-generator-") as temporary:
    root = Path(temporary)
    (root / "flake.nix").write_text("{ outputs = { self }: {}; }\n")
    (root / "simit.toml").write_text('''[prebuild]
provider = "crow"
qualify = [".#checks.x86_64-linux.server^dev,out"]
nix_builds = [".#packages.x86_64-linux.tool"]
qualify_capture_results = true
qualify_split_evaluation = true
qualify_max_jobs = 1
qualify_cores = 2
qualify_timeout_minutes = 1

[ci]
provider = "actions"
platform = "github"
runtime = "nix"
''')
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "evidence": str(root)}
    subprocess.run([str(generator), "init", "ci", "--prebuild-only"], cwd=root, env=env, check=True)
    workflow, = (root / ".crow").glob("*server*.yaml")
    # Read the generated YAML literal, not a separately reconstructed invocation.
    block = workflow.read_text().split("  - |-\n", 1)[1].split("  environment:", 1)[0]
    script = textwrap.dedent(block).replace("$${", "${")
    start = script.rfind("\n", 0, script.index("nix eval ")) + 1
    command = script[start:script.index("\nroot_parent=", start)]
    assert command.count("nix eval ") == 1
    drv = "/nix/store/" + "0" * 32 + "-fixture.drv"
    plan = [{"drvPath": drv, "outputs": {"out": None, "dev": None}}]
    cases = [
        ("selected outputs", json.dumps(plan), drv + "^dev,out\n"),
        ("empty plan", "[]", None),
        ("opaque path", '["/nix/store/opaque"]', None),
        ("invalid path", json.dumps([{"drvPath": "invalid", "outputs": {"out": None}}]), None),
        ("dynamic derivation", json.dumps([{"drvPath": {}, "outputs": {"out": None}}]), None),
        ("no outputs", json.dumps([{"drvPath": drv, "outputs": {}}]), None),
        ("invalid JSON", "not JSON", None),
    ]
    print(subprocess.check_output(["nix", "--version"], text=True).strip(), flush=True)
    for name, value, expected in cases:
        (root / "plan.json").write_text(value)
        result = subprocess.run(["sh", "-eu", "-c", command], cwd=root, env=env,
                                text=True, capture_output=True, timeout=90)
        targets = (root / "targets.txt").read_text()
        if expected is not None:
            assert result.returncode == 0, f"{name}: {result.stderr}"
            assert targets == expected, (name, targets)
        else:
            assert result.returncode != 0, f"{name}: unsupported plan accepted"
            assert not targets, f"{name}: stale targets retained"
        print(f"PASS {name}: generated command through real Nix", flush=True)
