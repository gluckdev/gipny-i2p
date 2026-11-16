#!/usr/bin/env python3
"""Bounded PE architecture receipt for native app/agent payloads, not NSIS stubs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct

MACHINES = {'x64': 0x8664, 'arm64': 0xAA64}
MAX_FILE = 512 * 1024 * 1024


def inspect_pe(path, architecture):
    path = Path(path)
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'not a regular PE payload: {path.name}')
    size = path.stat().st_size
    if not 64 <= size <= MAX_FILE:
        raise ValueError(f'PE size outside limit: {path.name}')
    with path.open('rb') as stream:
        dos = stream.read(64)
        if dos[:2] != b'MZ':
            raise ValueError('missing DOS signature')
        offset = struct.unpack_from('<I', dos, 60)[0]
        if not 64 <= offset <= min(1024 * 1024, size - 24):
            raise ValueError('invalid PE header offset')
        stream.seek(offset)
        header = stream.read(24)
        if header[:4] != b'PE\0\0':
            raise ValueError('missing PE signature')
        machine, sections, _, _, _, optional_size, flags = struct.unpack('<HHIIIHH', header[4:])
        if machine != MACHINES[architecture]:
            raise ValueError(f'wrong PE machine: {machine:#x}, expected {architecture}')
        if not flags & 0x0002 or not 1 <= sections <= 96:
            raise ValueError('invalid executable flag or section count')
        if not 112 <= optional_size <= 4096 or offset + 24 + optional_size + sections * 40 > size:
            raise ValueError('invalid optional/section header bounds')
        optional = stream.read(optional_size)
        if struct.unpack_from('<H', optional)[0] != 0x20B:
            raise ValueError('native 64-bit payload must be PE32+')
        directory_count = struct.unpack_from('<I', optional, 108)[0]
        if directory_count > 16 or 112 + directory_count * 8 > optional_size:
            raise ValueError('invalid PE data directory count')
        headers_size = struct.unpack_from('<I', optional, 60)[0]
        if not offset + 24 + optional_size + sections * 40 <= headers_size <= size:
            raise ValueError('invalid SizeOfHeaders')
        for _ in range(sections):
            section = stream.read(40)
            raw_size, raw_offset = struct.unpack_from('<II', section, 16)
            if raw_size and (raw_offset < headers_size or raw_offset + raw_size > size):
                raise ValueError('PE section outside file')
        stream.seek(0)
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    return {'name': path.name, 'architecture': architecture, 'machine': f'0x{machine:04x}',
            'size': size, 'sha256': digest, 'dll': bool(flags & 0x2000)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--architecture', choices=MACHINES, required=True)
    parser.add_argument('--file', action='append', required=True)
    parser.add_argument('--receipt', type=Path, required=True)
    args = parser.parse_args()
    if len(args.file) > 128 or len(set(args.file)) != len(args.file):
        parser.error('payload list must be unique and at most 128 files')
    commit = os.environ.get('GITHUB_SHA')
    if commit is not None and not re.fullmatch('[0-9a-f]{40}', commit):
        parser.error('invalid source SHA')
    receipt = {'schema_version': 1, 'source_sha': commit, 'architecture': args.architecture,
               'files': [inspect_pe(path, args.architecture) for path in args.file],
               'runtime_tested': False}
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
