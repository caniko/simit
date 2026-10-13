"""No external services, credentials or packages: exercise protected upload logic."""
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
sys.dont_write_bytecode = True
import tarfile
import tempfile
import subprocess
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("native", sys.argv[1])
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def refuses(function):
    try:
        function()
    except (RuntimeError, KeyError):
        return
    raise AssertionError("tampering accepted")


with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    identity = {"schemaVersion": 2, "registry": "python", "name": "example", "version": "1.2.3", "namespace": "python-owner", "tag": "python-owner/v1.2.3", "source": "b" * 40, "repository": "owner/repo", "workflow": ".github/workflows/publish-python-python-owner.yaml", "runId": 42}
    wheel = root / "example-1.2.3-py3-none-any.whl"
    metadata = b"Metadata-Version: 2.1\nName: example\nVersion: 1.2.3\nSummary: test\n\nOriginal source\n"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("example-1.2.3.dist-info/METADATA", metadata)
    sdist = root / "example-1.2.3.tar.gz"
    with tarfile.open(sdist, "w:gz") as archive:
        info = tarfile.TarInfo("example-1.2.3/PKG-INFO")
        info.size = len(metadata)
        archive.addfile(info, io.BytesIO(metadata))
    files = {p.name: native.digest(p).hex() for p in (wheel, sdist)}
    receipt = identity | {"runAttempt": 1, "files": files}
    receipt_path = root / "receipt.json"
    receipt_path.write_text(json.dumps(receipt))
    assert native.validate_artifacts(root, identity, 1) == files
    for key, wrong in [("namespace", "other"), ("tag", "other/v1.2.3"), ("source", "c" * 40), ("repository", "other/repo"), ("workflow", ".github/workflows/other.yaml"), ("runId", 43), ("runAttempt", 2), ("name", "other"), ("version", "1.2.4"), ("files", {wheel.name: "0" * 64, sdist.name: files[sdist.name]})]:
        receipt_path.write_text(json.dumps(receipt | {key: wrong}))
        refuses(lambda: native.validate_artifacts(root, identity, 1))
    receipt_path.write_text(json.dumps(receipt))
    original = {p.name: p.read_bytes() for p in (wheel, sdist)}
    # Partial publication: only the original missing wheel is uploaded, then an
    # uncertain response reconciles matching immutable bytes without a rebuild.
    published = {sdist.name}
    def registry(_):
        return {"info": {"name": "example", "version": "1.2.3"}, "urls": [{"filename": name, "yanked": False, "digests": {"sha256": files[name]}} for name in published]}
    native.registry_metadata = registry
    calls = []
    def upload(out, selected, pending, manifest, token):
        assert pending == [wheel.name]
        assert token == "disposable-fixture-token"
        assert (out / wheel.name).read_bytes() == original[wheel.name]
        calls.append(pending)
        published.update(pending)
        raise RuntimeError("lost upload response")
    native.upload_native = upload
    os.environ["UV_PUBLISH_TOKEN"] = "disposable-fixture-token"
    native.publish(root, identity, 1, {})
    del os.environ["UV_PUBLISH_TOKEN"]
    native.publish(root, identity, 1, {})
    assert len(calls) == 1
    assert all((root / name).read_bytes() == data for name, data in original.items())
    native.registry_metadata = lambda _: {"info": {"name": "example", "version": "1.2.3"}, "urls": [{"filename": wheel.name, "yanked": False, "digests": {"sha256": "0" * 64}}]}
    refuses(lambda: native.publish(root, identity, 1, {}))
    # Upload transport parses artifacts only. Candidate config/hooks never run.
    native.subprocess.run = lambda *a, **kw: (_ for _ in ()).throw(AssertionError("candidate execution"))
    for name in ("keys/maintainers.gpg", "flake.nix", "package.json", "scripts/publish.py"):
        path = root / name
        path.parent.mkdir(exist_ok=True)
        path.write_text("raise RuntimeError('candidate tampering')")
    # Restore actual protected uploader, intercept only the HTTP boundary.
    actual = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(actual)
    requests = []
    actual.upload_request = requests.append
    actual.upload_native(root, identity, [wheel.name], {}, "disposable-fixture-token")
    assert requests[0].full_url == "https://upload.pypi.org/legacy/"
    assert original[wheel.name] in requests[0].data
    npm = {"name": "@example/native", "version": "1.2.3", "scripts": {"prepublishOnly": "exit 99"}, "publishConfig": {"registry": "https://attacker.invalid", "provenance": True}}
    tarball = root / "native-1.2.3.tgz"
    with tarfile.open(tarball, "w:gz") as archive:
        data = json.dumps(npm).encode()
        info = tarfile.TarInfo("package/package.json")
        info.size = len(data)
        archive.addfile(info, io.BytesIO(data))
    npm_identity = identity | {"registry": "npm", "name": npm["name"]}
    actual.upload_native(root, npm_identity, [tarball.name], {"publishConfig": {"access": "public", "tag": "stable"}}, "disposable-fixture-token")
    request = requests[-1]
    assert request.full_url == "https://registry.npmjs.org/%40example%2Fnative"
    body = json.loads(request.data)
    assert "scripts" not in body["versions"]["1.2.3"]
    assert "publishConfig" not in body["versions"]["1.2.3"]
    assert base64.b64decode(body["_attachments"][tarball.name]["data"]) == tarball.read_bytes()
    assert body["dist-tags"] == {"stable": "1.2.3"}
    refuses(lambda: actual.upload_native(root, npm_identity, [tarball.name], {"publishConfig": {"registry": "https://attacker.invalid"}}, "fixture"))
    # The provenance signer receives original source/bytes and protected workflow
    # identity, with OIDC in a separate child that cannot access registry tokens.
    provider = {"PATH": "/trusted/bin", "HOME": temp, "ACTIONS_ID_TOKEN_REQUEST_URL": "https://oidc.example/token", "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "fixture-oidc", "GITHUB_WORKFLOW_REF": "owner/repo/.github/workflows/trusted-publish-npm-node-engine.yaml@refs/heads/trunk", "GITHUB_WORKFLOW_SHA": "a" * 40, "GITHUB_RUN_ID": "99", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_REPOSITORY_ID": "101", "GITHUB_REPOSITORY_OWNER_ID": "102", "NODE_AUTH_TOKEN": "must-not-reach-signer", "NODE_OPTIONS": "--require candidate.js"}
    def sign(arguments, **kwargs):
        assert "NODE_AUTH_TOKEN" not in kwargs["env"]
        assert "NODE_OPTIONS" not in kwargs["env"]
        assert arguments[0] == "node"
        assert arguments[1].endswith("native-provenance.cjs")
        statement = json.loads(kwargs["input"])
        assert statement["subject"][0]["digest"]["sha512"] == hashlib.sha512(tarball.read_bytes()).hexdigest()
        dependencies = statement["predicate"]["buildDefinition"]["resolvedDependencies"]
        assert dependencies[0]["digest"]["gitCommit"] == npm_identity["source"]
        assert dependencies[0]["uri"].endswith("@refs/tags/" + npm_identity["tag"])
        assert dependencies[1]["digest"]["gitCommit"] == "a" * 40
        assert statement["predicate"]["runDetails"]["metadata"]["invocationId"].endswith("/99/attempts/2")
        bundle = {"mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json", "dsseEnvelope": {"payload": base64.b64encode(kwargs["input"].encode()).decode()}}
        return subprocess.CompletedProcess(arguments, 0, json.dumps(bundle), "")
    with patch.dict(os.environ, provider, clear=True), patch.object(actual.subprocess, "check_output", return_value="/trusted/npm\n") as npm_root, patch.object(actual.subprocess, "run", side_effect=sign):
        actual.upload_native(root, npm_identity, [tarball.name], {"publishConfig": {"access": "public", "provenance": True}}, "fixture")
        assert "NODE_AUTH_TOKEN" not in npm_root.call_args.kwargs["env"]
    attachments = json.loads(requests[-1].data)["_attachments"]
    assert base64.b64decode(attachments[tarball.name]["data"]) == tarball.read_bytes()
    assert npm_identity["name"] + "-1.2.3.sigstore" in attachments
print("native protected publisher contracts passed")
