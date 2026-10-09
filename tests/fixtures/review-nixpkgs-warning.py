"""Hosted native regression: real pinned Nixpkgs policy and upstream nix_eval.

This evaluates synthetic derivations without realizing a consumer package. The
only recorded I/O is checkout identity; policy/evaluator/Attr conversion is real.
"""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from nixpkgs_review.allow import AllowedFeatures
from nixpkgs_review.errors import NixpkgsReviewError
from nixpkgs_review.utils import ROOT

spec = importlib.util.spec_from_file_location("adapter", sys.argv[1])
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
nixpkgs = Path(sys.argv[2]).resolve(strict=True)
assert str(nixpkgs).startswith("/nix/store/")
system = "x86_64-linux"
revision = "a" * 40

# In upstream 3.7.0, broken describes failed evaluation, not retained meta.broken.
# A failed evaluation deliberately clears both path and derivation identity.
evaluator = (ROOT / "nix/evalAttrs.nix").read_text()
assert "broken = !exists || !maybePath.success;" in evaluator
assert "drvPath = if !broken then pkg.drvPath else null;" in evaluator

with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    fixture = root / "nixpkgs"
    fixture.mkdir()
    (fixture / "default.nix").write_text(
        "args: import " + json.dumps(str(nixpkgs)) + " (args // { "
        'system = "x86_64-linux"; overlays = [(final: previous: { '
        'vortex = final.runCommand "vortex-1" { meta.broken = true; '
        'passthru.tests.packaging = final.runCommand "vortex-packaging-1" {} "touch $out"; '
        '} "touch $out"; '
        'unrelated = final.runCommand "unrelated-1" { meta.broken = true; } "touch $out"; '
        'vortex-other = final.runCommand "vortex-other-1" { meta.broken = true; } "touch $out"; '
        'openssl_3 = final.runCommand "openssl-3.0" { meta.broken = true; '
        'passthru.tests.packaging = final.runCommand "openssl-packaging-3.0" {} "touch $out"; '
        '} "touch $out"; '
        'openssl_alias = final.runCommand "openssl-3.0" { meta.broken = true; } "touch $out"; '
        'darwin.builder = final.runCommand "blacklisted-fixture-1" {} "touch $out"; '
        '})]; })\n'
    )
    plan = {"pr": {"base": "b" * 40}, "target": {"commit": revision},
            "request": {"packages": ["vortex"], "checks": ["vortex.tests.packaging"],
                        "nixpkgs_broken_warnings": ["vortex"]}}
    config = root / "config.nix"
    output = root / "selection.json"

    def select(request, discovery):
        config.write_text(adapter.scoped_nixpkgs_config(request))
        review = adapter.SelectOnly(request, system, output,
            builddir=SimpleNamespace(nix_path="nixpkgs=" + str(fixture), worktree_dir=root),
            build_args="", no_shell=True, run="", remote="", systems=[system],
            allow=AllowedFeatures([]), build_graph="nix", nixpkgs_config=config,
            extra_nixpkgs_config=config.read_text(), eval_type="local",
            checkout=adapter.CheckoutOption.COMMIT)
        with patch.dict(os.environ, {"NIXPKGS_CONFIG": str(config)}), \
             patch.object(adapter.subprocess, "check_output", return_value=revision):
            review.build(discovery, None)
        return json.loads(output.read_text())

    accepted = select(plan, {})
    assert {item["attribute"] for item in accepted["derivations"]} == {"vortex", "vortex.tests.packaging"}
    assert {item["attribute"] for item in accepted["derivations"] if item["check"]} == {"vortex.tests.packaging"}
    assert all(item["derivation"] for item in accepted["derivations"])
    assert accepted["nixpkgs_broken_warnings"] == ["vortex"]
    print("Real pinned evaluator accepts usable meta.broken vortex with exact scoped warning")

    for attribute in ("unrelated", "vortex-other", "absent", "darwin.builder"):
        try:
            select(plan, {system: {attribute}})
        except (RuntimeError, NixpkgsReviewError) as error:
            print("Real pinned evaluator rejects", attribute, str(error))
        else:
            raise AssertionError("scoped warning admitted unrelated/missing/blacklisted attribute: " + attribute)
    ordinary = json.loads(json.dumps(plan))
    ordinary["request"]["nixpkgs_broken_warnings"] = []
    try:
        select(ordinary, {})
    except (RuntimeError, NixpkgsReviewError) as error:
        print("Real pinned evaluator rejects meta.broken vortex without warning", str(error))
    else:
        raise AssertionError("broken vortex was admitted without the scoped warning")

    # Nixpkgs problems.handlers is keyed by lib.getName, not the attribute path.
    # Preserve the request's attribute-scoped identity while testing a different
    # derivation name through the same actual pinned native policy/evaluator.
    renamed = json.loads(json.dumps(plan))
    renamed["request"] = {
        "packages": ["openssl_3"], "checks": ["openssl_3.tests.packaging"],
        "nixpkgs_broken_warnings": ["openssl_3"],
    }
    accepted = select(renamed, {})
    assert {item["attribute"] for item in accepted["derivations"]} == {"openssl_3", "openssl_3.tests.packaging"}
    assert {item["attribute"] for item in accepted["derivations"] if item["check"]} == {"openssl_3.tests.packaging"}
    assert all(item["derivation"] for item in accepted["derivations"])
    assert accepted["nixpkgs_broken_warnings"] == ["openssl_3"]
    print("Real pinned evaluator accepts attribute openssl_3 with package name openssl and exact scoped warning")
    for attribute in ("vortex", "unrelated", "absent", "darwin.builder", "openssl_alias"):
        try:
            select(renamed, {system: {attribute}})
        except (RuntimeError, NixpkgsReviewError) as error:
            print("Renamed scoped warning rejects", attribute, str(error))
        else:
            raise AssertionError("renamed scoped warning admitted unrelated/missing/blacklisted attribute: " + attribute)
    renamed["request"]["nixpkgs_broken_warnings"] = []
    try:
        select(renamed, {})
    except (RuntimeError, NixpkgsReviewError) as error:
        print("Real pinned evaluator rejects broken openssl_3 without warning", str(error))
    else:
        raise AssertionError("broken openssl_3 was admitted without the scoped warning")
