"""Run the generated publisher against real archives and fixture registry data."""
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("publisher", sys.argv.pop())
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class NativeRelease(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.out = Path(self.temp.name)
        self.identity = dict(schemaVersion=1, registry="python", name="engine", version="1.2.3", namespace="engine-python", tag="engine-python/v1.2.3", source="f" * 40)

    def tar(self, name, member, content):
        path = self.out / name
        with tarfile.open(path, "w:gz") as archive:
            entry = tarfile.TarInfo(member)
            entry.size = len(content)
            archive.addfile(entry, io.BytesIO(content))
        return path

    def python_artifacts(self):
        metadata = b"Name: Engine\nVersion: 1.2.3\n"
        self.tar("engine-1.2.3.tar.gz", "engine-1.2.3/PKG-INFO", metadata)
        with zipfile.ZipFile(self.out / "engine-1.2.3-py3-none-any.whl", "w") as archive:
            archive.writestr("engine-1.2.3.dist-info/METADATA", metadata)
        return self.receipt()

    def receipt(self):
        files = {path.name: publisher.digest(path).hex() for path in self.out.iterdir() if path.name != "receipt.json"}
        (self.out / "receipt.json").write_text(json.dumps(self.identity | {"files": files}))
        return files

    def test_python_exact_files_partial_upload_and_checksum_conflicts(self):
        files = self.python_artifacts()
        for path in self.out.iterdir():
            if path.name != "receipt.json":
                publisher.archive_identity(path, self.identity)
        first = next(iter(files))
        metadata = dict(info=dict(name="Engine", version="1.2.3"), urls=[dict(filename=first, yanked=False, digests=dict(sha256=files[first]))])
        with patch.object(publisher, "registry_metadata", return_value=metadata):
            self.assertEqual(publisher.missing_files(self.out, self.identity, files), [name for name in files if name != first])
            metadata["urls"][0]["digests"]["sha256"] = "0" * 64
            with self.assertRaisesRegex(RuntimeError, "checksum conflict"):
                publisher.missing_files(self.out, self.identity, files)
            metadata["urls"][0]["digests"]["sha256"] = files[first]
            metadata["urls"][0]["yanked"] = True
            with self.assertRaisesRegex(RuntimeError, "yanked"):
                publisher.missing_files(self.out, self.identity, files)

    def test_downloaded_artifact_tampering_and_source_mismatch_fail_before_network(self):
        self.python_artifacts()
        with patch.object(publisher, "registry_metadata", side_effect=AssertionError("must not query")):
            identity = self.identity | {"source": "0" * 40}
            with self.assertRaisesRegex(RuntimeError, "signed source"):
                publisher.publish(self.out, identity)
            with (self.out / "engine-1.2.3.tar.gz").open("ab") as output:
                output.write(b"tampered")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                publisher.publish(self.out, self.identity)

    def test_npm_scoped_archive_and_registered_integrity(self):
        self.identity.update(registry="npm", name="@example/engine", namespace="engine-node", tag="engine-node/v1.2.3")
        path = self.tar("engine.tgz", "package/package.json", json.dumps(dict(name=self.identity["name"], version="1.2.3", private=False)).encode())
        publisher.archive_identity(path, self.identity)
        files = self.receipt()
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(path.read_bytes()).digest()).decode()
        release = dict(name=self.identity["name"], version="1.2.3", dist=dict(integrity=integrity))
        metadata = dict(name=self.identity["name"], versions={"1.2.3": release})
        with patch.object(publisher, "registry_metadata", return_value=metadata):
            publisher.publish(self.out, self.identity)
            release["dist"]["integrity"] = "sha512-wrong"
            with self.assertRaisesRegex(RuntimeError, "checksum conflict"):
                publisher.missing_files(self.out, self.identity, files)

    def test_missing_credential_and_failed_upload_cannot_establish_acceptance(self):
        self.python_artifacts()
        with patch.object(publisher, "registry_metadata", return_value=None), patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "missing UV_PUBLISH_TOKEN"):
                publisher.publish(self.out, self.identity)
            with patch.dict(os.environ, {"UV_PUBLISH_TOKEN": "fixture-token"}), patch.object(publisher.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)):
                with self.assertRaisesRegex(RuntimeError, "publication failed"):
                    publisher.publish(self.out, self.identity)

    def test_propagation_wait_is_bounded_and_rechecks_exact_artifacts(self):
        self.python_artifacts()
        with patch.dict(os.environ, {"UV_PUBLISH_TOKEN": "fixture-token"}), patch.object(publisher.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), patch.object(publisher.time, "sleep") as sleep:
            with patch.object(publisher, "missing_files", side_effect=[["engine-1.2.3.tar.gz"], ["engine-1.2.3.tar.gz"], []]):
                publisher.publish(self.out, self.identity)
            self.assertEqual(sleep.call_count, 1)
            with patch.object(publisher, "missing_files", return_value=["engine-1.2.3.tar.gz"]):
                with self.assertRaisesRegex(RuntimeError, "deadline exceeded"):
                    publisher.publish(self.out, self.identity)
            self.assertEqual(sleep.call_count, 20)

    def test_python_semver_prerelease_matches_native_metadata(self):
        self.assertEqual(publisher.python_version("1.2.3-rc.4"), "1.2.3rc4")
        self.assertEqual(publisher.python_version("1.2.3-alpha.1"), "1.2.3a1")
        with self.assertRaisesRegex(RuntimeError, "PyPI-compatible"):
            publisher.python_version("1.2.3-arbitrary.1")

    def test_two_wheels_cannot_replace_the_required_sdist(self):
        self.python_artifacts()
        paths = list(self.out.glob("*.whl"))
        second = self.out / "engine-1.2.3-other.whl"
        second.write_bytes(paths[0].read_bytes())
        with self.assertRaisesRegex(RuntimeError, "wheel and sdist"):
            publisher.inventory(paths + [second], self.identity)

    def test_packing_creates_source_receipt_without_publication_credentials(self):
        out = self.out / "build"

        def build(args, **kwargs):
            self.assertNotIn("UV_PUBLISH_TOKEN", kwargs["env"])
            self.assertNotIn("NODE_AUTH_TOKEN", kwargs["env"])
            metadata = b"Name: engine\nVersion: 1.2.3\n"
            (out / ".gitignore").write_text("*\n")
            with zipfile.ZipFile(out / "engine-1.2.3-py3-none-any.whl", "w") as archive:
                archive.writestr("engine-1.2.3.dist-info/METADATA", metadata)
            with tarfile.open(out / "engine-1.2.3.tar.gz", "w:gz") as archive:
                entry = tarfile.TarInfo("engine-1.2.3/PKG-INFO")
                entry.size = len(metadata)
                archive.addfile(entry, io.BytesIO(metadata))

        with patch.dict(os.environ, {"UV_PUBLISH_TOKEN": "fixture-token", "NODE_AUTH_TOKEN": "fixture-token"}), patch.object(publisher.subprocess, "check_output", return_value="123\n"), patch.object(publisher, "command", side_effect=build):
            publisher.pack(self.out, out, self.identity)
        receipt = json.loads((out / "receipt.json").read_text())
        self.assertEqual(receipt["source"], self.identity["source"])
        self.assertEqual(len(receipt["files"]), 2)
        self.assertFalse((out / ".gitignore").exists())


unittest.main()
