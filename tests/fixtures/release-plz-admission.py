"""Execute release selection and the Cargo boundary against real Git histories."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

helper = Path(sys.argv[1]).resolve()
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("admission", helper)
admission = importlib.util.module_from_spec(spec)
spec.loader.exec_module(admission)


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


with tempfile.TemporaryDirectory() as directory:
    os.chdir(directory)
    git("init", "-b", "trunk")
    tree = subprocess.check_output(["git", "mktree"], input="", text=True).strip()

    def commit(message, *parents):
        # Disposable object fixtures need no operator identity, hooks or signer.
        text = f"tree {tree}\n" + "".join(f"parent {parent}\n" for parent in parents)
        text += "author Fixture <fixture@example.invalid> 1700000000 +0000\n"
        text += "committer Fixture <fixture@example.invalid> 1700000000 +0000\n\n"
        return subprocess.check_output(
            ["git", "hash-object", "-t", "commit", "--stdin", "-w"],
            input=text + message + "\n",
            text=True,
        ).strip()

    base = commit("initial")
    prepared = commit("release", base)
    merged = commit("merge", base, prepared)
    git("checkout", "--detach", merged)
    os.environ["GITHUB_REPOSITORY"] = "owner/repo"
    pr = {
        "number": 17,
        "head": {"ref": "release-plz-test"},
        "merged_at": "2026-10-19T00:00:00Z",
        "base": {"repo": {"full_name": "owner/repo"}},
    }
    commits = lambda number: [{"sha": prepared}]
    assert admission.release_sha(merged, [pr], commits) == prepared
    assert admission.release_sha(merged, [], commits) == ""
    try:
        admission.release_sha(merged, [{**pr, "merged_at": None}], commits)
        raise AssertionError("accepted an unmerged release PR")
    except RuntimeError:
        pass

    def guard(expected, *args):
        if args and args[0] == "publish":
            args = (*args, "--package", "demo")
        return subprocess.run(
            [sys.executable, str(helper), "cargo", *args],
            env={
                **os.environ,
                "SIMIT_EXPECTED_RELEASE_SHA": expected,
                "SIMIT_REAL_CARGO": shutil.which("true"),
                "SIMIT_RELEASE_PLZ_TOOL_PATH": os.environ["PATH"],
                "SIMIT_RELEASE_PLZ_PUBLISHER": shutil.which("true"),
                "RUNNER_TEMP": directory,
            },
            capture_output=True,
        )

    # Qualification of the branch tip is insufficient for a merge release.
    assert guard(prepared, "publish").returncode != 0
    assert guard(prepared, "publish", "--dry-run").returncode != 0
    assert guard(prepared, "metadata").returncode == 0
    assert guard(prepared, "build").returncode != 0
    assert guard(prepared, "metadata", "--config", "build.rustc-wrapper=bad").returncode != 0
    assert guard(prepared, "metadata", "--config=build.rustc-wrapper=bad").returncode != 0
    probe = Path(directory) / "data-cargo"
    probe.write_text(f"#!{sys.executable}\nimport os\nassert os.getcwd() == '/'\nassert all(name not in os.environ for name in ('GIT_TOKEN', 'CARGO_REGISTRY_TOKEN', 'GNUPGHOME', 'RUSTC_WRAPPER'))\n")
    probe.chmod(0o700)
    result = subprocess.run(
        [sys.executable, str(helper), "cargo", "metadata"],
        env={**os.environ, "SIMIT_REAL_CARGO": str(probe),
             "SIMIT_RELEASE_PLZ_TOOL_PATH": os.environ["PATH"],
             "GIT_TOKEN": "fixture-write-secret", "CARGO_REGISTRY_TOKEN": "fixture-registry-secret",
             "GNUPGHOME": "fixture-private-directory", "RUSTC_WRAPPER": "untrusted"},
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr.decode()
    git("checkout", "--detach", prepared)
    published = guard(prepared, "publish")
    assert published.returncode == 0 and b"Uploading demo" in published.stderr
    assert guard("", "publish").returncode != 0

    squashed = commit("squash", base)
    git("checkout", "--detach", squashed)
    assert admission.release_sha(squashed, [pr], commits) == squashed
    assert guard(squashed, "publish").returncode == 0
    assert guard(prepared, "publish").returncode != 0
    # An API result changing between admission and publication fails closed.
    os.environ["GITHUB_SHA"] = squashed
    os.environ["GITHUB_OUTPUT"] = str(Path(directory) / "output")
    os.environ["SIMIT_EXPECTED_RELEASE_SHA"] = prepared
    admission.api = lambda path: [pr] if "/commits/" in path else commits(17)
    try:
        admission.select()
        raise AssertionError("accepted a changed release selection")
    except RuntimeError:
        pass

    # The workflow must sign with a key authorized by the committed trust root.
    home = Path(directory) / "gnupg"
    home.mkdir(mode=0o700)
    os.environ["GNUPGHOME"] = str(home)

    def key(home, uid):
        subprocess.run(
            [
                "gpg", "--homedir", str(home), "--batch", "--pinentry-mode", "loopback",
                "--passphrase", "", "--quick-generate-key", uid, "ed25519", "sign", "0",
            ],
            check=True,
            capture_output=True,
        )
        return subprocess.check_output(
            ["gpg", "--homedir", str(home), "--armor", "--export"], text=True
        )

    trust = Path(directory) / "trust.asc"
    uid_name = "Release %20 Fixture: Δ"
    trust.write_text(key(home, f"{uid_name} <fixture@example.invalid>"))
    git("add", "trust.asc")
    tree = git("write-tree")
    git("checkout", "--detach", commit("trust root", squashed))
    admission.signing_identity("trust.asc")
    assert git("config", "user.email") == "fixture@example.invalid"
    assert git("config", "user.name") == uid_name
    git("config", "tag.gpgSign", "true")
    git("tag", "v0.2.0", "-m", "release")
    git("verify-tag", "v0.2.0")
    # The project's actual trust root is a binary keyring, not armored UTF-8.
    trust.write_bytes(subprocess.check_output(["gpg", "--homedir", str(home), "--export"]))
    git("add", "trust.asc")
    tree = git("write-tree")
    git("checkout", "--detach", commit("binary trust root", git("rev-parse", "HEAD")))
    admission.signing_identity("trust.asc")
    other_home = Path(directory) / "other-gnupg"
    other_home.mkdir(mode=0o700)
    trust.write_text(key(other_home, "Other Fixture <other@example.invalid>"))
    git("add", "trust.asc")
    tree = git("write-tree")
    git("checkout", "--detach", commit("unauthorized trust root", git("rev-parse", "HEAD")))
    try:
        admission.signing_identity("trust.asc")
        raise AssertionError("accepted a private key outside the trust root")
    except RuntimeError:
        pass
    for home in [home, other_home]:
        subprocess.run(
            ["gpgconf", "--homedir", str(home), "--kill", "gpg-agent"], check=True
        )

    # release-plz copies/clones repositories; local checkout config is not inherited.
    runtime = Path(directory) / "runtime"
    binary = runtime / "release-plz/bin/release-plz"
    binary.parent.mkdir(parents=True)
    binary.write_text("""#!/usr/bin/env python3
import os, pathlib, subprocess, tempfile
assert not pathlib.Path('.cargo/config.toml').exists()
assert not pathlib.Path('crate/.cargo/config.toml').exists()
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == os.environ['SIMIT_EXPECTED_RELEASE_SHA']
with tempfile.TemporaryDirectory() as clone:
    subprocess.run(["git", "init", clone], check=True, capture_output=True)
    result = subprocess.run(["git", "-C", clone, "credential", "fill"],
        input="protocol=https\\nhost=github.com\\npath=owner/repo.git\\n\\n",
        text=True, check=True, capture_output=True)
    assert "password=fixture-token" in result.stdout
""")
    binary.chmod(0o700)
    credentials = runtime / "release-plz-credentials"
    credentials.write_text('#!/bin/sh\nprintf "username=x-access-token\\npassword=%s\\n" "$GIT_TOKEN"\n')
    credentials.chmod(0o700)
    git("config", "--local", "credential.helper", str(credentials))
    git("config", "--local", "gpg.program", "gpg")
    for cargo_dir in [Path(".cargo"), Path("crate/.cargo")]:
        cargo_dir.mkdir(parents=True)
        (cargo_dir / "config.toml").write_text('[registry]\nglobal-credential-providers=["malicious"]\n')
    git("add", ".cargo", "crate")
    tree = git("write-tree")
    event_sha = commit("project Cargo config", git("rev-parse", "HEAD"))
    git("checkout", "--detach", event_sha)
    git("clone", "--no-hardlinks", "--no-checkout", ".", str(runtime / "release-plz-event-source"))
    result = subprocess.run(
        [sys.executable, str(helper), "run", "release"],
        env={
            **os.environ,
            "RUNNER_TEMP": str(runtime),
            "GIT_TOKEN": "fixture-token",
            "SIMIT_EXPECTED_RELEASE_SHA": event_sha,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0",
        },
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr.decode()

print("release-plz merge, squash, publication and authorized tag-signing checks passed")
