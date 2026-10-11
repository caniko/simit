"""Export the real conventional fixture through the exact packaged review engine.

Hosted-only: the existing retrieval tests consume this sealed bundle, enforce
signatures in new stores, and compare the restored closure without building it.
"""

import argparse
import json
import os
import subprocess
from pathlib import Path


def run(engine, *arguments):
    return json.loads(
        subprocess.check_output([str(engine), *arguments], text=True, timeout=900)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--controller-root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    engine = args.engine.resolve(strict=True)
    controller = args.controller_root.resolve(strict=True)
    output = args.out.resolve()
    output.mkdir()

    identity = run(engine, "verify-engine", "--root", str(controller))
    assert identity["repository"] == os.environ["GITHUB_REPOSITORY"]
    assert identity["revision"] == os.environ["GITHUB_SHA"]
    request = run(engine, "example")
    request.update(
        repository=identity["repository"],
        pr=None,
        revision=identity["revision"],
        expected_head=identity["revision"],
        expected_base=None,
        directory="tests/fixtures/review/flake",
        systems=["x86_64-linux"],
        packages=["default"],
        checks=["smoke"],
        test_profile="checks-rebuild-v1",
        publication="none",
        post_result=False,
    )
    request_path = output / "request.json"
    request_path.write_text(json.dumps(request, indent=2) + "\n")
    plan = output / "flake-plan.json"
    run(
        engine,
        "plan",
        str(request_path),
        "--controller",
        identity["repository"],
        "--revision",
        identity["revision"],
        "--root",
        str(controller),
        "--output",
        str(plan),
    )
    bundle = output / "flake"
    result = run(
        engine,
        "build",
        "--plan",
        str(plan),
        "--system",
        "x86_64-linux",
        "--output",
        str(bundle),
    )
    for gate in ("build", "tests", "closure_export"):
        assert result[gate] == "passed", result
    assert result["closure"] and result["error"] is None
    assert result["publication"] == result["retrieval"] == "not_run"
    run(engine, "validate-report", "--plan", str(plan), "--bundle", str(bundle))
    print(
        json.dumps(
            {
                "fixture": str(output),
                "engine": identity,
                "closure_paths": len(result["closure"]),
            }
        )
    )


if __name__ == "__main__":
    main()
