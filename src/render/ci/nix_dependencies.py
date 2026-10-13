"""Export/import only exact-source build inputs within one Actions run."""
import gzip
import hashlib
import json
import os
import re
import struct
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def compressed_budget(maximum):
    # zlib's default windowBits=15/memLevel=8 deflateBound, plus ample room
    # for the fixed closure.nar.gz gzip header/trailer. GzipFile.write uses
    # Z_NO_FLUSH until close, so chunked writes do not add sync-flush overhead.
    # https://github.com/madler/zlib/blob/v1.3.1/deflate.c#L825-L894
    require(isinstance(maximum, int) and maximum > 0, 'Invalid transport budget')
    return maximum + (maximum >> 12) + (maximum >> 14) + (maximum >> 25) + 1024 * 1024


def evaluate(installable, expression):
    return json.loads(subprocess.check_output(['nix', 'eval', '--json', '--no-update-lock-file',
                                               installable, '--apply', expression], text=True))


def canonical_store_path(raw):
    # Nix StorePath::checkName, with the literal canonical store root only.
    return (isinstance(raw, bytes) and len(raw) <= 255
            and re.fullmatch(rb'/nix/store/[0123456789abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+._?=-]{1,211}', raw)
             and raw[44:] not in [b'.', b'..'] and not raw[44:].startswith((b'.-', b'..-')))


def store_paths_command(operation, paths):
    """Admit the complete argv before export or any subsequent import effect."""
    require(isinstance(paths, list) and 0 < len(paths) <= ExportInventory.MAX_PATHS
            and all(isinstance(path, str) and path.isascii()
                    and canonical_store_path(path.encode()) for path in paths),
            'Invalid dependency command inventory')
    command = ['nix-store', operation, *paths]
    # execve shares ARG_MAX between argument strings, the inherited environment
    # and both pointer arrays. Leave headroom for executable/platform overhead;
    # never split or concatenate independently terminated Nix export streams.
    limit = os.sysconf('SC_ARG_MAX')
    strings = sum(len(os.fsencode(value)) + 1 for value in command)
    strings += sum(len(os.fsencode(key)) + len(os.fsencode(value)) + 2
                   for key, value in os.environ.items())
    pointers = (len(command) + len(os.environ) + 2) * struct.calcsize('P')
    require(limit > 0 and strings + pointers <= limit - min(65536, limit // 4),
            'Dependency store command exceeds worker argument budget')
    return command


class ExportInventory:
    """Inspect the complete Nix writer grammar without importing or extracting it.

    Protocol: Nix adbf23e8184bbbe8d786aad1f362443b6542e0a4,
    src/libstore/export-import.cc and doc/manual/src/protocols/nix-archive.md.
    Importer limits: https://github.com/NixOS/nix/blob/2.34.8/src/libutil/archive.cc
    """
    MAX_PATHS = 100000
    MAX_NODES = 1000000
    MAX_REFERENCES = 1000000
    MAX_DEPTH = 64

    def __init__(self, stream, size):
        self.stream = stream
        self.remaining = size
        self.nodes = 0
        self.references = 0

    def exact(self, size):
        require(0 <= size <= self.remaining, 'Truncated export stream')
        raw = self.stream.read(size)
        require(len(raw) == size, 'Truncated export stream')
        self.remaining -= size
        return raw

    def number(self):
        return int.from_bytes(self.exact(8), 'little')

    def padding(self, size):
        length = -size % 8
        require(self.exact(length) == b'\0' * length, 'Nonzero export padding')

    def string(self, maximum):
        size = self.number()
        require(size <= maximum, 'Export string exceeds its bound')
        raw = self.exact(size)
        self.padding(size)
        return raw

    def token(self):
        return self.string(32)

    def expect(self, token):
        require(self.token() == token, 'Invalid NAR grammar token')

    def contents(self):
        size = self.number()
        require(size <= self.remaining, 'Truncated NAR contents')
        remaining = size
        while remaining:
            chunk = min(remaining, 65536)
            self.exact(chunk)
            remaining -= chunk
        self.padding(size)

    def node(self, depth=0):
        self.nodes += 1
        require(depth < self.MAX_DEPTH and self.nodes <= self.MAX_NODES, 'NAR tree exceeds its bound')
        self.expect(b'(')
        self.expect(b'type')
        kind = self.token()
        if kind == b'regular':
            field = self.token()
            if field == b'executable':
                self.expect(b'')
                field = self.token()
            require(field == b'contents', 'Invalid regular NAR fields')
            self.contents()
        elif kind == b'symlink':
            self.expect(b'target')
            target = self.string(4095)
            require(target and b'\0' not in target, 'Invalid NAR symlink target')
        elif kind == b'directory':
            previous = b''
            folded = set()
            while True:
                field = self.token()
                if field == b')':
                    return
                require(field == b'entry', 'Invalid directory NAR fields')
                self.expect(b'(')
                self.expect(b'name')
                name = self.string(255)
                require(name > previous and name not in [b'.', b'..'] and b'/' not in name
                        and b'\0' not in name, 'Invalid or unordered NAR entry name')
                # Match native admission: Darwin's case hack can rewrite aliases
                # into an existing suffixed entry. Deny ASCII case aliases on
                # every platform before importing any record in the stream.
                require(name.lower() not in folded, 'NAR entry names alias under case hacking')
                folded.add(name.lower())
                previous = name
                self.expect(b'node')
                self.node(depth + 1)
                self.expect(b')')
        else:
            raise RuntimeError('Unsupported NAR object type')
        self.expect(b')')

    def path(self):
        raw = self.string(255)
        require(canonical_store_path(raw), 'Noncanonical export store path')
        return raw

    def validate(self, closure, outputs, forbidden):
        require(isinstance(closure, list) and 0 < len(closure) <= self.MAX_PATHS,
                'Invalid admitted export inventory')
        require(all(isinstance(path, str) and path.isascii() and canonical_store_path(path.encode())
                    for path in closure), 'Invalid admitted export paths')
        admitted = {path.encode() for path in closure}
        require(len(admitted) == len(closure), 'Duplicate admitted export paths')
        require(set(outputs) <= set(closure) and not set(closure) & forbidden,
                'Export closure omits evaluated roots or includes qualification outputs')
        seen = set()
        while True:
            marker = self.number()
            if marker == 0:
                break
            require(marker == 1 and len(seen) < len(admitted), 'Invalid or excessive export records')
            self.expect(b'nix-archive-1')
            self.node()
            require(self.number() == 0x4558494e, 'Invalid export magic')
            path = self.path()
            require(path in admitted and path not in seen, 'Export path is unadmitted or duplicated')
            seen.add(path)
            count = self.number()
            self.references += count
            require(count <= len(admitted) and self.references <= self.MAX_REFERENCES,
                    'Export reference inventory exceeds its bound')
            previous = b''
            for _ in range(count):
                reference = self.path()
                require(reference in admitted and reference > previous,
                        'Export reference escapes closure or is duplicated/unordered')
                previous = reference
            deriver = self.string(255)
            require(not deriver or canonical_store_path(deriver), 'Invalid export deriver metadata')
            # Our enrolled Nix writer emits zero; legacy signatures establish no authority.
            require(self.number() == 0, 'Unsupported export signature framing')
        require(seen == admitted, 'Export stream inventory differs from admitted closure')
        require(self.remaining == 0 and self.stream.read(1) == b'', 'Trailing export data')


def discover():
    check_drvs = set()
    qualification_outputs = set()
    expression = 'd: { drvPath = d.drvPath; outputs = builtins.listToAttrs (map (name: { inherit name; value = d.${name}.outPath; }) d.outputs); }'
    for system in systems:
        checks = evaluate(f'.#checks.{system}', 'checks: builtins.mapAttrs (_: d: { drvPath = d.drvPath; outputs = builtins.listToAttrs (map (name: { inherit name; value = d.${name}.outPath; }) d.outputs); }) checks')
        require(isinstance(checks, dict) and checks, 'Invalid discovered check inventory')
        check_drvs.update(definition['drvPath'] for definition in checks.values())
        qualification_outputs.update(path for definition in checks.values() for path in definition['outputs'].values())
    definitions = {installable: evaluate(installable, expression) for installable in dependencies}
    qualification = {installable: evaluate(installable, expression) for installable in plan}
    qualified_drvs = {definition['drvPath'] for definition in qualification.values()}
    qualification_outputs.update(path for definition in qualification.values() for path in definition['outputs'].values())
    for definition in definitions.values():
        require(definition['drvPath'] not in check_drvs | qualified_drvs, 'A qualification derivation was enrolled as a build input')
        require(definition['outputs'] and all(re.fullmatch(r'/nix/store/[a-z0-9]{32}-[^/\s]+', path)
                                             for path in definition['outputs'].values()), 'Invalid dependency output paths')
    graph = json.loads(subprocess.check_output(['nix', 'derivation', 'show', '--recursive',
                                               *sorted(definition['drvPath'] for definition in definitions.values())], text=True))
    require(isinstance(graph, dict) and graph, 'Invalid build-input graph')
    require(not set(graph) & (check_drvs | qualified_drvs), 'Build-input graph contains a qualification derivation')
    return definitions, qualification_outputs


@contextmanager
def preflight_import(manifest_path, archive, identity, maximum, compressed_maximum):
    # Forcing drvPath can instantiate store objects or trigger IFD. Admit both
    # byte budgets before any Nix evaluation, keeping the same private spool
    # through later source/graph admission and the eventual import effect.
    require(manifest_path.is_file() and not manifest_path.is_symlink(), 'Invalid dependency manifest file')
    manifest = json.loads(manifest_path.read_text())
    require(all(manifest.get(key) == value for key, value in identity.items()), 'Dependency provenance differs from this source/run')
    require(manifest['complete'] is True, 'Dependency manifest is incomplete')
    require(type(manifest['uncompressed_bytes']) is int and 0 <= manifest['uncompressed_bytes'] <= maximum,
            'Declared dependency transport exceeds budget')
    verify_command = store_paths_command('--verify-path', manifest['closure'])
    require(archive.is_file() and not archive.is_symlink() and archive.stat().st_size == manifest['archive_bytes'], 'Invalid dependency archive file or size')
    require(archive.stat().st_size <= compressed_maximum, 'Compressed dependency archive exceeds budget')
    # Snapshot compressed bytes too: a checksum of a replaceable downloaded
    # inode must never authorize a later reopen with different NAR contents.
    # Both TemporaryFiles are unlinked and mode 0600.
    with tempfile.TemporaryFile(dir=archive.parent) as snapshot, tempfile.TemporaryFile(dir=archive.parent) as stream:
        copied = 0
        with archive.open('rb') as downloaded:
            while chunk := downloaded.read(1024 * 1024):
                copied += len(chunk)
                require(copied <= compressed_maximum, 'Compressed dependency archive exceeds budget')
                snapshot.write(chunk)
        snapshot.seek(0)
        require(hashlib.file_digest(snapshot, 'sha256').hexdigest() == manifest['archive_sha256'], 'Dependency archive checksum mismatch')
        # The verified private snapshot owns the compressed bytes now. Release
        # the download before expansion so disk use never retains three copies.
        archive.unlink()
        snapshot.seek(0)
        size = 0
        with gzip.GzipFile(fileobj=snapshot, mode='rb') as compressed:
            while chunk := compressed.read(1024 * 1024):
                size += len(chunk)
                require(size <= maximum, 'Decompressed dependency transport exceeds budget')
                stream.write(chunk)
        require(size == manifest['uncompressed_bytes'], 'Dependency transport was truncated')
        stream.seek(0)
        yield manifest, stream, size, verify_command


root = Path(os.environ['SIMIT_DEPENDENCY_ROOT'])
root.mkdir(exist_ok=True)
mode = os.environ['SIMIT_DEPENDENCY_MODE']
require(mode in ['export', 'import'], 'Unknown dependency transport operation')
maximum = int(os.environ['SIMIT_DEPENDENCY_MAX_BYTES'])
compressed_maximum = compressed_budget(maximum)
dependencies = json.loads(os.environ['SIMIT_NIX_DEPENDENCIES'])
plan = json.loads(os.environ['SIMIT_NIX_PLAN'])
systems = json.loads(os.environ['SIMIT_NIX_CHECK_SYSTEMS'])
limits = json.loads(os.environ['SIMIT_NIX_LIMITS'])
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], text=True).strip()
expected_head = os.environ['GITHUB_SHA']
if os.environ['GITHUB_EVENT_NAME'] == 'pull_request':
    expected_head = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['pull_request']['head']['sha']
require(head == expected_head, 'Dependency source does not match the event head')
subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--'], check=True)
identity = {'schema': 'simit.nix-dependencies.v1', 'head': head, 'tree': tree,
            'repository': os.environ['GITHUB_REPOSITORY'], 'run': os.environ['GITHUB_RUN_ID'],
            'attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
            'flake_lock_sha256': digest(Path('flake.lock')) if Path('flake.lock').is_file() else None}
require(dependencies and systems and len(set(dependencies)) == len(dependencies), 'Invalid dependency enrollment')
require(not set(dependencies) & set(plan), 'Dependency enrollment overlaps qualification plan')
manifest_path = root / 'manifest.json'
archive = root / 'closure.nar.gz'
if mode == 'export':
    definitions, qualification_outputs = discover()
    manifest = {**identity, 'definitions': definitions, 'complete': False}
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    outputs = set()
    for index, (installable, definition) in enumerate(definitions.items()):
        with (root / f'build-{index}.log').open('wb') as log:
            result = subprocess.run(['nix', 'build', '--no-link', '--no-update-lock-file',
                                     '--print-build-logs', '--json', *limits, installable + '^*'],
                                    stdout=subprocess.PIPE, stderr=log)
        (root / f'result-{index}.json').write_bytes(result.stdout)
        result.check_returncode()
        realized = json.loads(result.stdout)
        require(len(realized) == 1 and realized[0]['drvPath'] == definition['drvPath'], 'Dependency derivation differs from its evaluated identity')
        require(realized[0]['outputs'] == definition['outputs'], 'Dependency outputs differ from their evaluated identity')
        outputs.update(definition['outputs'].values())
    closure_data = json.loads(subprocess.check_output(['nix', 'path-info', '--json', '--recursive', *sorted(outputs)], text=True))
    if isinstance(closure_data, list):
        closure_data = {row['path']: row for row in closure_data}
    require(isinstance(closure_data, dict) and closure_data, 'Invalid dependency closure inventory')
    require(sum(row['narSize'] for row in closure_data.values()) <= maximum, 'Dependency closure exceeds its transport budget')
    closure = sorted(closure_data)
    require(all(re.fullmatch(r'/nix/store/[a-z0-9]{32}-[^/\s]+', path) for path in closure), 'Invalid dependency closure paths')
    require(not set(closure) & qualification_outputs, 'Dependency closure contains qualification outputs')
    export_command = store_paths_command('--export', closure)
    with (root / 'export.log').open('wb') as log, gzip.open(archive, 'wb', compresslevel=1) as compressed:
        with subprocess.Popen(export_command, stdout=subprocess.PIPE, stderr=log) as producer:
            size = 0
            while chunk := producer.stdout.read(1024 * 1024):
                size += len(chunk)
                if size > maximum:
                    producer.terminate()
                    producer.wait(timeout=30)
                    raise RuntimeError('Export stream exceeds transport budget')
                compressed.write(chunk)
            require(producer.wait() == 0, 'Dependency export failed')
    require(archive.stat().st_size <= compressed_maximum, 'Compressed dependency archive exceeds budget')
    manifest.update(complete=True, closure=closure, uncompressed_bytes=size,
                    archive_sha256=digest(archive), archive_bytes=archive.stat().st_size)
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
elif mode == 'import':
    with preflight_import(manifest_path, archive, identity, maximum, compressed_maximum) as (manifest, stream, size, verify_command):
        definitions, qualification_outputs = discover()
        require(manifest['definitions'] == definitions, 'Dependency manifest definitions differ')
        require(not set(manifest['closure']) & qualification_outputs, 'Dependency closure contains qualification outputs')
        paths = sorted(path for definition in definitions.values() for path in definition['outputs'].values())
        ExportInventory(stream, size).validate(manifest['closure'], paths, qualification_outputs)
        stream.seek(0)
        with (root / 'import.log').open('wb') as log:
            subprocess.run(['nix-store', '--import'], stdin=stream, stdout=log, stderr=subprocess.STDOUT, check=True)
    subprocess.run(verify_command, check=True)
    subprocess.run(['nix', 'path-info', *paths], check=True)
    (root / 'import-receipt.json').write_text(json.dumps({**identity, 'archive_sha256': manifest['archive_sha256'],
                                                       'restored': paths, 'qualification_execution': False}, indent=2) + '\n')
