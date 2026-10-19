"""Execute release selection and the Cargo boundary against real Git histories."""

import importlib.util
import hashlib
import json
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


os.environ.update({"GITHUB_REPOSITORY": "owner/repo", "GITHUB_REF_NAME": "trunk", "GITHUB_RUN_ID": "20"})
current_run = {"id": 20, "workflow_id": 9, "run_number": 5}
predecessor = {"id": 19, "run_number": 4, "status": "completed", "conclusion": "success"}
newer = {"id": 21, "run_number": 6, "status": "queued", "conclusion": None}
original_api, original_github = admission.api, admission.github
original_sleep, original_monotonic = admission.time.sleep, admission.time.monotonic
runs = [newer, current_run, predecessor]
predecessor_jobs = [{"name": "select", "status": "completed", "conclusion": "success"}]
admission.github = lambda method, path, payload: current_run if path.endswith("/20") else predecessor
admission.api = lambda path, key=None: predecessor_jobs if key == "jobs" else runs
for conclusion in ["failure", "cancelled", "skipped", "timed_out"]:
    predecessor.update(status="completed", conclusion=conclusion)
    try:
        admission.prior_run()
        raise AssertionError(f"advanced past a {conclusion} predecessor")
    except RuntimeError:
        pass
for status in ["queued", "in_progress", "waiting"]:
    predecessor.update(status=status, conclusion=None)
    admission.time.sleep = lambda seconds: predecessor.update(status="completed", conclusion="success")
    admission.prior_run()  # No advancement until the predecessor succeeds.
    assert predecessor["status"] == "completed"
predecessor.update(status="in_progress", conclusion=None)
clock = iter([0, 350 * 60 + 1])
admission.time.monotonic = lambda: next(clock)
try:
    admission.prior_run()
    raise AssertionError("waited beyond the bounded predecessor budget")
except RuntimeError as error:
    assert "wait budget" in str(error)
admission.time.sleep, admission.time.monotonic = original_sleep, original_monotonic
predecessor.update(status="completed", conclusion="success")
admission.prior_run()  # A successful retry unblocks the next immutable event.
# Native queue admission excludes time spent behind the preceding gate. Model
# run 3 starting after run 2's 300-minute wait, then 300 minutes of real work.
predecessor.update(status="in_progress", conclusion=None)
clock = iter([300 * 60, 600 * 60])
admission.time.monotonic = lambda: next(clock)
admission.time.sleep = lambda seconds: predecessor.update(status="completed", conclusion="success")
admission.prior_run()
admission.time.sleep, admission.time.monotonic = original_sleep, original_monotonic
# Provider FIFO is admission-time, not run-number order. A later gate must not
# occupy the only admission slot while an older gate is queued behind it.
for status in ["queued", "in_progress"]:
    predecessor.update(status="in_progress", conclusion=None)
    predecessor_jobs[0].update(status=status, conclusion=None)
    admission.time.sleep = lambda seconds: predecessor.update(status="completed", conclusion="success")
    try:
        admission.prior_run()
        raise AssertionError("waited on a predecessor still needing the occupied admission slot")
    except RuntimeError as error:
        assert "admission" in str(error)
admission.time.sleep = original_sleep
predecessor.update(status="completed", conclusion="success")
predecessor_jobs[0].update(status="completed", conclusion="success")
for jobs in [[], [{"name": "select", "status": "completed", "conclusion": "failure"}]]:
    predecessor.update(status="in_progress", conclusion=None)
    admission.api = lambda path, key=None: jobs if key == "jobs" else runs
    try:
        admission.prior_run()
        raise AssertionError("accepted missing or unsuccessful predecessor admission")
    except RuntimeError as error:
        assert "admission" in str(error)
admission.api = lambda path, key=None: predecessor_jobs if key == "jobs" else runs
predecessor.update(status="completed", conclusion="success")
runs = [newer, current_run]
admission.prior_run()  # Initial enrollment has no earlier default-branch event.
runs = [newer]
try:
    admission.prior_run()
    raise AssertionError("accepted incomplete workflow-run history")
except RuntimeError:
    pass
runs = [current_run] + [newer] * 999
try:
    admission.prior_run()
    raise AssertionError("treated a truncated predecessor history as initial enrollment")
except RuntimeError:
    pass
admission.api, admission.github = original_api, original_github


with tempfile.TemporaryDirectory() as directory:
    os.chdir(directory)
    git("init", "-b", "trunk")
    home = Path(directory) / "gnupg"
    home.mkdir(mode=0o700)
    os.environ["GNUPGHOME"] = str(home)

    def key(home, uid):
        subprocess.run(
            ["gpg", "--homedir", str(home), "--batch", "--pinentry-mode", "loopback",
             "--passphrase", "", "--quick-generate-key", uid, "ed25519", "sign", "0"],
            check=True, capture_output=True,
        )
        return subprocess.check_output(
            ["gpg", "--homedir", str(home), "--armor", "--export"], text=True
        )

    trust = Path(directory) / "trust.asc"
    uid_name = "Release %20 Fixture: Δ"
    trust.write_text(key(home, f"{uid_name} <fixture@example.invalid>"))
    git("add", "trust.asc")
    tree = git("write-tree")
    os.environ["RELEASE_TRUST_ROOT"] = "trust.asc"
    git("config", "user.name", uid_name)
    git("config", "user.email", "fixture@example.invalid")
    fingerprint = next(line.split(":")[9] for line in subprocess.check_output(
        ["gpg", "--with-colons", "--list-secret-keys"], text=True,
    ).splitlines() if line.startswith("fpr:"))
    git("config", "user.signingkey", fingerprint)

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
    prepared = subprocess.check_output(["git", "commit-tree", "-S", tree, "-p", base], input="chore: release\n", text=True).strip()
    merged = commit("merge", base, prepared)
    git("checkout", "--detach", merged)
    os.environ["GITHUB_REPOSITORY"] = "owner/repo"
    os.environ["GITHUB_REF_NAME"] = "trunk"
    body = "Prepared by release-plz 0.3.172."

    def receipt(sha):
        return f"{body}\n\n<!-- simit-release-plz-head: {sha} {hashlib.sha256(body.encode()).hexdigest()} -->"

    pr = {
        "number": 17,
        "head": {"ref": f"release-plz-{base}", "sha": prepared, "repo": {"full_name": "owner/repo"}},
        "body": receipt(prepared),
        "merged_at": "2026-10-19T00:00:00Z",
        "merge_commit_sha": merged,
        "base": {"ref": "trunk", "repo": {"full_name": "owner/repo"}},
    }
    commits = lambda number: [{"sha": prepared}]
    assert admission.release_sha(merged, [pr], commits) == prepared
    # A push can contain a release merge followed by another ordinary commit.
    # Querying only its tip must not silently skip that release and advance PRs.
    followup = commit("after merged release", merged)
    event = Path(directory) / "event.json"
    output = Path(directory) / "selection-output"

    def select_push(before, after, prs, expected=None):
        event.write_text(json.dumps({"before": before, "after": after}))
        output.write_text("")
        os.environ.update(GITHUB_EVENT_PATH=str(event), GITHUB_EVENT_NAME="push",
                          GITHUB_SHA=after, GITHUB_OUTPUT=str(output))
        os.environ.pop("SIMIT_EXPECTED_RELEASE_SHA", None)
        if expected is not None:
            os.environ["SIMIT_EXPECTED_RELEASE_SHA"] = expected
        git("checkout", "--detach", after)
        # Even if every commit is associated with the PR, its exact merge must
        # be in this push range; neither its branch commits nor old merges count.
        admission.api = lambda path: prs if "/commits/" in path else [
            {"sha": next(pr["head"]["sha"] for pr in prs if f"/pulls/{pr['number']}/" in path)}
        ]
        admission.select()
        return output.read_text()

    assert select_push(base, merged, [pr]) == f"release_sha={prepared}\n"
    assert select_push(base, followup, [pr]) == f"release_sha={prepared}\n", "skipped a release merge before the push tip"
    assert select_push(merged, followup, [pr]) == "release_sha=\n", "reselected a release outside the push"
    assert select_push("0" * 40, merged, [pr]) == f"release_sha={prepared}\n"
    prepared2 = subprocess.check_output(["git", "commit-tree", "-S", tree, "-p", followup], input="chore: second release\n", text=True).strip()
    merged2 = commit("second release merge", followup, prepared2)
    pr2 = {**pr, "number": 18, "head": {**pr["head"], "sha": prepared2},
           "body": receipt(prepared2), "merge_commit_sha": merged2}
    try:
        select_push(base, merged2, [pr, pr2])
        raise AssertionError("advanced past multiple release merges in one push")
    except RuntimeError as error:
        assert "multiple release PRs" in str(error)
        assert output.read_text() == ""
    for before in [commit("unrelated history"), "a" * 40, "invalid"]:
        try:
            select_push(before, followup, [pr])
            raise AssertionError("accepted incomplete/non-fast-forward push history")
        except RuntimeError:
            assert output.read_text() == ""
    admission.api = original_api
    git("checkout", "--detach", merged)
    # The trusted-engine bootstrap checks out the event again at shallow depth.
    # Recheck in the full clone retained before that checkout.
    retained = Path(directory) / "retained-event-source"
    git("clone", "--no-hardlinks", "--no-checkout", ".", str(retained))
    shallow = Path(directory) / ".git/shallow"
    shallow.write_text(merged + "\n")
    assert admission.release_sha(merged, [pr], commits) == merged
    os.chdir(retained)
    assert admission.release_sha(merged, [pr], commits) == prepared
    os.chdir(directory)
    shallow.unlink()
    for bad_pr in (
        {**pr, "head": {**pr["head"], "repo": {"full_name": "fork/repo"}}},
        {**pr, "body": "no generated receipt"},
        {**pr, "head": {**pr["head"], "sha": base}, "body": receipt(base)},
        {**pr, "base": {**pr["base"], "ref": "other"}},
    ):
        try:
            admission.release_sha(merged, [bad_pr], lambda number: [{"sha": bad_pr["head"]["sha"]}])
            raise AssertionError("admitted an unauthenticated release PR")
        except (RuntimeError, subprocess.CalledProcessError):
            pass
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
    squash_pr = {**pr, "merge_commit_sha": squashed}
    after_squash = commit("after squash", squashed)
    assert select_push(base, after_squash, [squash_pr]) == f"release_sha={squashed}\n"
    assert select_push(squashed, after_squash, [squash_pr]) == "release_sha=\n"
    admission.api = original_api
    git("checkout", "--detach", squashed)
    # The original signed head of a squash may be missing from the event clone.
    # Exercise its immutable fetch through a disposable local transport.
    git("update-ref", "refs/heads/trunk", squashed)
    git("update-ref", f"refs/heads/{pr['head']['ref']}", prepared)
    missing = Path(directory) / "missing-pr-head"
    git("clone", "--no-local", "--depth", "1", "--branch", "trunk", Path(directory).as_uri(), str(missing))
    os.chdir(missing)
    git("config", f"url.{Path(directory).as_uri()}.insteadOf", "https://github.com/owner/repo.git")
    assert subprocess.run(["git", "cat-file", "-e", prepared], capture_output=True).returncode != 0
    assert admission.release_sha(squashed, [pr], commits) == squashed
    os.chdir(directory)
    assert guard(squashed, "publish").returncode == 0
    assert guard(prepared, "publish").returncode != 0
    # An API result changing between admission and publication fails closed.
    try:
        select_push(base, squashed, [squash_pr], expected=prepared)
        raise AssertionError("accepted a changed release selection")
    except RuntimeError as error:
        assert "changed after qualification" in str(error)
        assert output.read_text() == ""

    # The workflow must sign with a key authorized by the committed trust root.
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
    other_key = next(line.split(":")[9] for line in subprocess.check_output(
        ["gpg", "--homedir", str(other_home), "--with-colons", "--list-secret-keys"], text=True,
    ).splitlines() if line.startswith("fpr:"))
    impostor = subprocess.check_output(
        ["git", "-c", f"user.signingkey={other_key}", "commit-tree", "-S", tree, "-p", squashed],
        input="chore: release\n", text=True, env={**os.environ, "GNUPGHOME": str(other_home)},
    ).strip()
    fake_pr = {**pr, "head": {**pr["head"], "sha": impostor}, "body": receipt(impostor)}
    try:
        admission.release_sha(squashed, [fake_pr], lambda number: [{"sha": impostor}])
        raise AssertionError("accepted a head signed outside the committed trust root")
    except subprocess.CalledProcessError:
        pass
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
    binary.write_text(f"""#!{sys.executable}
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
