"""Exercise data-only PR admission, signed delivery, and manual-edit protection."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("admission", Path(sys.argv[1]).resolve())
admission = importlib.util.module_from_spec(spec)
spec.loader.exec_module(admission)


def git(*args, **kwargs):
    return subprocess.check_output(["git", *args], text=True, **kwargs).strip()


with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    os.chdir(root)
    git("init", "-b", "trunk")
    original = '[package]\nname="fixture"\nversion="0.1.0"\n'
    (root / "Cargo.toml").write_text(original)
    git("add", ".")
    tree = git("write-tree")
    commit = f"tree {tree}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\n\ninitial\n"
    sha = git("hash-object", "-t", "commit", "--stdin", "-w", input=commit)
    git("update-ref", "refs/heads/trunk", sha)
    runtime = root / "runtime"
    runtime.mkdir()
    (runtime / "release-plz-update.log").write_text("fixture 0.1.0 -> 0.2.0\n")
    os.environ.update({
        "RUNNER_TEMP": str(runtime), "GITHUB_SHA": sha, "GITHUB_REPOSITORY": "owner/repo",
        "GITHUB_REF_NAME": "trunk", "GITHUB_ENV": str(runtime / "env"),
    })
    (root / "Cargo.toml").write_text(original.replace("0.1.0", "0.2.0"))
    (root / "CHANGELOG.md").write_text("# Changelog\n\n## 0.2.0\n\nPrepared by release-plz.\n")
    admission.prepare_pr()
    patch = (runtime / "release-plz-pr/update.patch").read_bytes()
    assert b"CHANGELOG.md" in patch, "new changelogs must cross the data boundary"
    git("reset", "--hard", sha)
    admission.admit_pr()
    os.environ["SIMIT_RELEASE_PR_TREE"] = (runtime / "env").read_text().strip().split("=", 1)[1]
    assert (root / "Cargo.toml").read_text() == original, "admission must only change the index"
    assert not (root / "CHANGELOG.md").exists()
    assert git("show", ":Cargo.toml").find("0.2.0") != -1
    gpg = root / "gnupg"
    gpg.mkdir(mode=0o700)
    os.environ["GNUPGHOME"] = str(gpg)
    subprocess.run(["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", "", "--quick-generate-key", "Release Fixture <fixture@example.invalid>", "ed25519", "sign", "0"], check=True, capture_output=True)
    fingerprint = next(line.split(":")[9] for line in subprocess.check_output(["gpg", "--with-colons", "--list-secret-keys"], text=True).splitlines() if line.startswith("fpr:"))
    git("config", "user.name", "Release Fixture")
    git("config", "user.email", "fixture@example.invalid")
    git("config", "user.signingkey", fingerprint)
    remote = root / "remote.git"
    git("init", "--bare", str(remote))
    git("remote", "add", "origin", str(remote))
    os.environ["GIT_TOKEN"] = "fixture-write-token"
    original_check_output = subprocess.check_output
    remote_sha = sha

    def check_output(args, **kwargs):
        if args[:2] == ["gh", "api"]:
            return json.dumps({"object": {"sha": remote_sha}})
        return original_check_output(args, **kwargs)

    admission.subprocess.check_output = check_output
    prs = []
    admission.api = lambda path: prs
    requests = []
    admission.github = lambda method, path, payload: requests.append((method, path, payload)) or {"html_url": "https://github.com/owner/repo/pull/1"}
    admission.apply_pr()
    assert requests[0][0] == "POST"
    branch = requests[0][2]["head"]
    generated = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    git("verify-commit", generated)
    assert "0.2.0" in git("show", f"{generated}:Cargo.toml")
    assert "Prepared by release-plz" in git("show", f"{generated}:CHANGELOG.md")
    assert git("rev-parse", "HEAD") == sha
    prs.append({"number": 1, "body": requests[0][2]["body"], "head": {"ref": branch, "sha": generated, "repo": {"full_name": "owner/repo"}}})
    admission.apply_pr()
    assert requests[-1][0] == "PATCH", "automatic refresh should reuse the existing PR"
    generated = git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0]
    prs[0]["head"]["sha"] = generated
    prs[0]["body"] = requests[-1][2]["body"] + "\n\nOwner's manual notes."
    try:
        admission.apply_pr()
        raise AssertionError("overwrote manual PR body edits")
    except RuntimeError:
        pass
    assert len(requests) == 2
    prs[0]["body"] = requests[-1][2]["body"]
    prs[0]["head"]["sha"] = sha
    try:
        admission.apply_pr()
        raise AssertionError("overwrote a manually changed release PR")
    except RuntimeError:
        pass
    assert git("ls-remote", "--heads", "origin", f"refs/heads/{branch}").split()[0] == generated
    remote_sha = "f" * 40
    admission.apply_pr()
    assert len(requests) == 2, "stale source must not replace a newer release PR"
    admission.subprocess.check_output = original_check_output
    subprocess.run(["gpgconf", "--kill", "gpg-agent"], check=True)

    git("reset", "--hard", sha)
    # A valid no-op hunk can come from an untrusted worker even though git diff
    # would normally emit an empty patch. Do not sign an empty release commit.
    (runtime / "release-plz-pr/update.patch").write_text(
        'diff --git a/Cargo.toml b/Cargo.toml\n--- a/Cargo.toml\n+++ b/Cargo.toml\n'
        '@@ -1,3 +1,3 @@\n [package]\n-name="fixture"\n+name="fixture"\n version="0.1.0"\n'
    )
    (runtime / "env").write_text("")
    admission.admit_pr()
    assert (runtime / "env").read_text() == "SIMIT_RELEASE_PR_TREE=\n"

    # Reject workflow changes, links, and deleted manifests before importing keys.
    for kind in ("workflow", "link", "deletion"):
        git("reset", "--hard", sha)
        if kind == "workflow":
            path = root / ".github/workflows/evil.yaml"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("jobs: {}\n")
            git("add", "--intent-to-add", str(path))
        elif kind == "link":
            (root / "Cargo.lock").symlink_to("/proc/self/environ")
            git("add", "--intent-to-add", "Cargo.lock")
        else:
            (root / "Cargo.toml").unlink()
        bad = subprocess.check_output(["git", "diff", "--binary", "HEAD"])
        git("reset", "--hard", sha)
        (runtime / "release-plz-pr/update.patch").write_bytes(bad)
        try:
            admission.admit_pr()
            raise AssertionError(f"admitted {kind} patch")
        except RuntimeError:
            pass

print("release-plz PR data admission, signed delivery and manual-edit protection passed")
