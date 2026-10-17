#!/usr/bin/env python3
"""Exercise the packaged updater/generator against disposable, local-only repos."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from contextlib import ExitStack


def run(*args, **kwargs):
    result = subprocess.run(args, text=True, capture_output=True, **kwargs)
    if result.returncode:
        diagnostics = f"{result.stdout}\n{result.stderr}"
        # Tool logs belong to this synthetic case's private TMPDIR. Include them
        # before TemporaryDirectory removes the only useful failure evidence.
        temporary = Path(kwargs.get("env", {}).get("TMPDIR", tempfile.gettempdir())).resolve()
        seen = set()
        for _ in range(16):
            paths = set(re.findall(r"tool stderr retained at ([^\s\"']+)", diagnostics)) - seen
            if not paths:
                break
            for value in paths:
                seen.add(value)
                path = Path(value.rstrip(":"))
                if path.resolve().is_relative_to(temporary) and path.is_file():
                    diagnostics += f"\n--- {path.name} ---\n" + path.read_text(errors="replace")
        raise AssertionError(f"{args}: {diagnostics}")
    return result.stdout.strip()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def stop_oracle(process):
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


modes = ["store-wrapper", "digested-store-wrapper", "no-input", "locked-input"]
if len(sys.argv) == 2:
    failed = []
    for mode in modes:
        print(f"BEGIN {mode}", flush=True)
        result = subprocess.run([sys.executable, __file__, sys.argv[1], mode])
        if result.returncode:
            failed.append(mode)
    print(f"Packaged updater qualification: {len(modes) - len(failed)}/{len(modes)} passed; failed: {failed}", flush=True)
    raise SystemExit(bool(failed))

assert len(sys.argv) == 3 and sys.argv[2] in modes
generator = Path(sys.argv[1]).resolve(strict=True)
assert str(generator).startswith("/nix/store/")
repository = Path(__file__).resolve().parents[1]
revision = run("git", "rev-parse", "HEAD", cwd=repository)
python = Path(sys.executable).resolve(strict=True)
assert str(python).startswith("/nix/store/")

with tempfile.TemporaryDirectory(prefix="simit-packaged-updater-") as temporary, ExitStack() as cleanups:
    root = Path(temporary)
    # A digest-enrolled store script must keep its sibling resources too.
    wrapper = root / "updater-wrapper"
    wrapper.mkdir()
    (wrapper / "marker").write_text("sibling-resource\n")
    script = wrapper / "generator"
    script.write_text(f"#!{python} -I\nimport os, pathlib, sys\n"
                      "assert pathlib.Path(__file__).with_name('marker').read_text() == 'sibling-resource\\n'\n"
                      f"os.execv({str(generator)!r}, [{str(generator)!r}, *sys.argv[1:]])\n")
    script.chmod(0o500)
    stored_wrapper = Path(run("nix-store", "--add", str(wrapper))) / "generator"
    for mode in [sys.argv[2]]:
        case = root / mode
        case.mkdir()
        source = case / "source"
        source.mkdir()
        fixture_git = case / "gitconfig"
        fixture_git.write_text('[user]\n name = Updater qualification fixture\n email = updater@example.invalid\n'
                               '[commit]\n gpgsign = false\n'
                               f'[url "{(case / "remote.git").as_uri()}"]\n insteadOf = https://fixture.example/team/demo.git\n')
        tool_logs = case / "tool-logs"
        tool_logs.mkdir()
        env = dict(os.environ, GIT_CONFIG_GLOBAL=str(fixture_git), GIT_CONFIG_NOSYSTEM="1", TMPDIR=str(tool_logs))
        (source / "Cargo.toml").write_text('[package]\nname = "updater-fixture"\nversion = "0.1.0"\nedition = "2024"\n')
        (source / "src").mkdir()
        (source / "src/main.rs").write_text("fn main() {}\n")
        marker = "no-input-marker"
        if mode == "locked-input":
            seed = case / "seed/source"
            seed.mkdir(parents=True)
            (seed / "flake.nix").write_text('{ outputs = _: { marker = "locked-input-marker"; }; }\n')
            run("nix-store", "--add", str(seed))
            nar_hash = run("nix", "hash", "path", "--sri", str(seed))
            save(source / "flake.lock", {"version": 7, "root": "root", "nodes": {
                "root": {"inputs": {"cached": "cached"}},
                "cached": {"locked": {"type": "github", "owner": "fixture", "repo": "offline", "rev": "a" * 40, "narHash": nar_hash},
                           "original": {"type": "github", "owner": "fixture", "repo": "offline"}}}})
            (source / "flake.nix").write_text('{ inputs.cached.url = "github:fixture/offline"; outputs = { cached, ... }: { simitConfig.ci.extra_setup = [ cached.marker ]; }; }\n')
            marker = "locked-input-marker"
        else:
            (source / "flake.nix").write_text('{ outputs = _: { simitConfig.ci.extra_setup = [ "no-input-marker" ]; }; }\n')
        run("git", "init", "--quiet", "--initial-branch=trunk", cwd=source, env=env)
        run("git", "add", "--all", cwd=source, env=env)
        run("git", "commit", "--quiet", "-m", "disposable fixture", cwd=source, env=env)
        remote = case / "remote.git"
        run("git", "clone", "--quiet", "--bare", str(source), str(remote), env=env)
        control = case / "control.json"
        save(control, {"remotes": {f"fixture.example/team/{name}": f"https://fixture.example/team/{name}.git" for name in ["demo", "generator"]}})
        ready = case / "oracle-port"
        oracle = subprocess.Popen([str(python), "-I", str(repository / "tests/fixtures/updater-adapter.py"),
                                   str(control), str(remote), str(ready)], env=env, stdout=subprocess.DEVNULL)
        cleanups.callback(stop_oracle, oracle)
        deadline = time.monotonic() + 10
        while not ready.exists() or not ready.read_text().isdigit():
            assert oracle.poll() is None, "oracle exited before readiness"
            assert time.monotonic() < deadline, "oracle did not become ready"
            time.sleep(0.01)
        adapter = case / "adapter"
        adapter.write_text(f"#!{python} -I\noracle_url = 'http://127.0.0.1:{int(ready.read_text())}'\n" +
                            (repository / "tests/fixtures/updater-adapter-client.py").read_text())
        adapter.chmod(0o500)
        inventory = case / "inventory.json"
        save(inventory, {"schemaVersion": 1, "root": str(case / "projects"),
                         "observedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                         "producer": "packaged-updater-qualification", "policyDigest": "b" * 64,
                         "projects": [{"coordinate": "fixture.example/team/demo", "forge": "fixture", "relativePath": "arbitrary/demo",
                                       "kind": "owned-repo", "lifecycle": "active", "access": "normal",
                                       "checkoutRole": "canonical", "simitCi": "auto", "mirrors": []}]})
        executable = stored_wrapper if "wrapper" in mode else generator
        enrollment = {"executable": str(executable), "sourceRevision": revision,
                      "sourceCoordinate": "fixture.example/team/generator", "forge": "fixture",
                      "requiredChecks": [{"name": "generator", "authority": "777"}]}
        if mode == "digested-store-wrapper":
            enrollment["executableSha256"] = hashlib.sha256(executable.read_bytes()).hexdigest()
        config = case / "updates.json"
        save(config, {"schemaVersion": 1,
                      "workspaces": {"projects": {"root": str(case / "projects"), "stateDirectory": str(case / "state"), "inventory": {"file": str(inventory)}}},
                      "forges": {"fixture": {"host": "fixture.example", "apiUrl": "https://fixture.example/api", "adapter": {"program": str(adapter)},
                                             "adapterSha256": hashlib.sha256(adapter.read_bytes()).hexdigest(), "checkAuthority": "777"}},
                      "profiles": {"fleet": {"workspace": "projects", "scopes": ["ci"], "select": {"coordinates": ["fixture.example/team/demo"]},
                                             "enrollCurrent": True, "generator": enrollment}}})
        output = run(str(generator), "projects", "update", "--config", str(config), "--profile", "fleet", "--wait-seconds", "0", "--json", env=env)
        reports = json.loads(output)
        assert len(reports) == 1 and reports[0]["phase"] == "preview", output
        assert marker in reports[0]["diff"], output
        assert run("git", "rev-parse", "trunk", cwd=remote, env=env) == run("git", "rev-parse", "HEAD", cwd=source, env=env)
        print(f"PASS {mode}: packaged updater -> verified generator -> sandbox -> offline plan", flush=True)
