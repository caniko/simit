"""Pinned nixpkgs-review 3.7.0 API adapter; never resolves a PR/ref itself.

Selection uses upstream Review.build_commit/list_packages/differences/nix_eval.
Only the realization boundary is replaced so Rust can freeze the derivations
before building. No custom package change detector is maintained here.
"""
import json
import os
from importlib.metadata import version
from pathlib import Path
import subprocess
import sys
import tempfile

from nixpkgs_review.allow import AllowedFeatures
from nixpkgs_review.builddir import Builddir
from nixpkgs_review.buildenv import Buildenv
from nixpkgs_review.nix import nix_eval
from nixpkgs_review.review import CheckoutOption, Review


def scoped_nixpkgs_config(plan, system=None, warnings=None):
    if warnings is None:
        warnings = plan.get("request", {}).get("nixpkgs_broken_warnings", [])
    if not warnings:
        return "{  }"
    paths = " ".join(
        "[ " + " ".join(json.dumps(part) for part in attribute.split(".")) + " ]"
        for attribute in sorted(warnings)
    )
    native = json.dumps(system) if system else "builtins.currentSystem"
    # Policy handlers use lib.getName, not the requested attribute spelling.
    # Reading the name does not force the checked drvPath/outPath. No global
    # allowBroken or warning policy is used to discover the package name.
    return ("{ problems.handlers = let pkgs = import <nixpkgs> { config = {}; "
            f"system = {native}; }}; in builtins.listToAttrs (map (path: {{ "
            "name = pkgs.lib.getName (pkgs.lib.getAttrFromPath path pkgs); "
            f'value.broken = "warn"; }}) [ {paths} ]); }}')


class SelectOnly(Review):
    def __init__(self, plan, selected_system, output, **kwargs):
        super().__init__(**kwargs)
        self.plan = plan
        self.selected_system = selected_system
        self.output = output

    def build(self, packages_per_system, args):
        if set(packages_per_system) - {self.selected_system}:
            raise RuntimeError("changed-package detector returned unexpected systems")
        # Retain upstream discovery even when it produced no system entry. Never
        # substitute only_packages, which bypasses changed-package discovery.
        changed = set(packages_per_system.get(self.selected_system, set()))
        request = self.plan.get("request", {})
        packages = set(request.get("packages", []))
        checks = set(request.get("checks", []))
        additions = packages | checks
        selected = sorted(changed | additions)
        warnings = sorted(request.get("nixpkgs_broken_warnings", []))
        if warnings:
            # Evaluate each opted-in attribute subtree independently. A handler
            # keyed by package name must not admit another selected attribute
            # with the same pname, or a sibling of the requested attribute.
            pending = set(selected)
            attrs = []
            for warning in sorted(warnings, key=lambda name: (-len(name), name)):
                group = {name for name in pending if name == warning or name.startswith(warning + ".")}
                if group:
                    attrs.extend(self.evaluate(group, [warning]))
                    pending -= group
            if pending:
                attrs.extend(self.evaluate(pending, []))
            attrs.sort(key=lambda attr: attr.name)
        else:
            attrs = nix_eval(set(selected), self.selected_system, self.allow, self.builddir.nix_path)
        if any(a.broken or a.blacklisted or not a.exists or not a.drv_path for a in attrs):
            raise RuntimeError("selected package unavailable, broken, or blacklisted")
        represented = {name for a in attrs for name in [a.name, *a.aliases]}
        def covers(attribute, name):
            return name == attribute or name.startswith(attribute + ".")
        if any(not any(covers(attribute, name) for name in represented) for attribute in additions):
            raise RuntimeError("explicit Nixpkgs attribute selected no derivations")
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.worktree_dir(), text=True).strip()
        if actual != self.plan["target"]["commit"]:
            raise RuntimeError("nixpkgs-review checkout differs from frozen tested commit")
        self.output.write_text(json.dumps({
            "schema_version": 1,
            "backend_version": version("nixpkgs-review"),
            "tested_commit": actual,
            "base_commit": self.plan["pr"]["base"],
            "system": self.selected_system,
            "changed_attributes": sorted(changed),
            "additional_attributes": sorted(additions),
            "nixpkgs_broken_warnings": sorted(request.get("nixpkgs_broken_warnings", [])),
            "derivations": [{"attribute": a.name, "aliases": a.aliases, "derivation": a.drv_path,
                             "check": a.is_test() or any(covers(check, name) for check in checks for name in [a.name, *a.aliases])} for a in attrs],
        }, sort_keys=True, indent=2) + "\n")
        return {self.selected_system: attrs}

    def evaluate(self, selected, warnings):
        config = scoped_nixpkgs_config(self.plan, self.selected_system, warnings)
        previous = os.environ.get("NIXPKGS_CONFIG")
        with tempfile.NamedTemporaryFile(mode="w", suffix=".nix") as policy:
            # Retain upstream Buildenv's non-broken policy; nix_eval itself
            # enforces allowBroken=false. Only this group's named handler varies.
            policy.write("{ allowUnfree = true; allowAliases = false; checkMeta = true; } // " + config)
            policy.flush()
            os.environ["NIXPKGS_CONFIG"] = policy.name
            try:
                return nix_eval(selected, self.selected_system, self.allow, self.builddir.nix_path)
            finally:
                if previous is None:
                    os.environ.pop("NIXPKGS_CONFIG", None)
                else:
                    os.environ["NIXPKGS_CONFIG"] = previous


if version("nixpkgs-review") != "3.7.0":
    raise RuntimeError("unsupported nixpkgs-review API version; review adapter before updating tool lock")
def main():
    if sys.argv[1:] == ["--self-test"]:
        print(json.dumps({"backend_version": version("nixpkgs-review"), "imports": "passed"}))
        return
    plan = json.loads(Path(sys.argv[1]).read_text())
    selected_system = sys.argv[2]
    output = Path(sys.argv[3])
    # Fixed administrator-owned limits for upstream subprocesses, not request config.
    os.environ["NIX_CONFIG"] = "accept-flake-config = false\nallow-import-from-derivation = false\nbuilders =\nmax-jobs = 2\ncores = 2\ntimeout = 1800\nmax-silent-time = 600\n"
    allow = AllowedFeatures([])
    # Discovery compares both source revisions using upstream's normal policy.
    # Warning-name resolution belongs to final selection on the tested checkout:
    # an explicit addition need not exist in the base revision.
    extra_config = "{  }"
    with Buildenv(False, extra_config) as config, Builddir("exact-" + plan["digest"]) as builddir:
        review = SelectOnly(plan, selected_system, output, builddir=builddir, build_args="", no_shell=True, run="", remote="",
                            systems=[selected_system], allow=allow, build_graph="nix",
                            nixpkgs_config=config, extra_nixpkgs_config=extra_config, eval_type="local",
                            checkout=CheckoutOption.COMMIT, num_parallel_evals=1)
        # Passing the tested SHA in merge_commit bypasses git_merge in upstream.
        # Use that same SHA as head_commit so COMMIT cannot switch to another tree.
        review.build_commit(plan["pr"]["base"], plan["target"]["commit"], plan["target"]["commit"])


if __name__ == "__main__":
    main()
