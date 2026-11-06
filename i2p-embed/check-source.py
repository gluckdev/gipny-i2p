#!/usr/bin/env python3
"""Data-only check of the pinned miniupnpc import; never builds or uses network."""
import hashlib
import json
from pathlib import Path
import re

root = Path(__file__).resolve().parent
source = root / 'vendor' / 'miniupnpc'
manifest = json.loads((root / 'miniupnpc-source.json').read_text())
assert manifest['repository'] == 'https://github.com/miniupnp/miniupnp'
assert re.fullmatch(r'[0-9a-f]{40}', manifest['revision'])
assert manifest['api'] == 21
expected = manifest['vendored_sha256']
assert expected.keys() == manifest['upstream_sha256'].keys()
assert {path.relative_to(source).as_posix() for path in source.rglob('*') if path.is_file()} == set(expected)
changed = set()
for name, digest in expected.items():
    path = source / name
    assert path.is_file() and not path.is_symlink() and '..' not in Path(name).parts
    assert re.fullmatch(r'[0-9a-f]{64}', digest)
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, name
    if digest != manifest['upstream_sha256'][name]:
        changed.add(name)
assert changed == {'src/miniwget.c', 'src/minissdpc.c', 'src/receivedata.c', 'src/receivedata.h'}
print(f'miniupnpc API {manifest["api"]}: {len(expected)} pinned source files, {len(changed)} declared local changes')
