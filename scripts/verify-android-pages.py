#!/usr/bin/env python3
"""Check shipped ELF bytes and APK alignment; never executes native code.

Android 16 KiB devices are 64-bit. Preserve the 4 KiB policy for legacy
32-bit ABIs. This structural gate does not replace a 16 KiB runtime test.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import struct
import subprocess
import sys
import zipfile

ABIS = {'arm64-v8a': (2, 183, 16384), 'armeabi-v7a': (1, 40, 4096),
        'x86_64': (2, 62, 16384), 'x86': (1, 3, 4096)}
MAX_LIBRARY_BYTES = 256 * 1024 * 1024
MAX_APK_BYTES = 512 * 1024 * 1024
MAX_ENTRIES = 20000
MAX_DIRECTORY_BYTES = 16 * 1024 * 1024
MAX_LIBRARIES = 32
MAX_TOTAL_LIBRARY_BYTES = 512 * 1024 * 1024


def verify_elf(data, abi, name):
    expected_class, expected_machine, page = ABIS[abi]
    if len(data) > MAX_LIBRARY_BYTES or len(data) < 16:
        raise ValueError(f'{name}: invalid ELF size')
    if data[:4] != b'\x7fELF' or data[4:7] != bytes((expected_class, 1, 1)):
        raise ValueError(f'{name}: ELF class/endianness/version does not match {abi}')
    header = struct.Struct('<HHIQQQIHHHHHH' if expected_class == 2 else '<HHIIIIIHHHHHH')
    program = struct.Struct('<IIQQQQQQ' if expected_class == 2 else '<IIIIIIII')
    if len(data) < 16 + header.size:
        raise ValueError(f'{name}: truncated ELF header')
    fields = header.unpack_from(data, 16)
    kind, machine, version, _, phoff, _, _, ehsize, phentsize, phnum, *_ = fields
    if (kind != 3 or machine != expected_machine or version != 1
            or ehsize != 16 + header.size):
        raise ValueError(f'{name}: ELF type/machine/header does not match {abi}')
    if (not 0 < phnum <= 1024 or phentsize != program.size or phoff < ehsize
            or phoff + phnum * phentsize > len(data)):
        raise ValueError(f'{name}: invalid program header table')
    loads = []
    for index in range(phnum):
        values = program.unpack_from(data, phoff + index * phentsize)
        if expected_class == 2:
            ptype, _, offset, address, _, filesz, memsz, align = values
        else:
            ptype, offset, address, _, filesz, memsz, _, align = values
        if ptype != 1:
            continue
        if filesz > memsz or offset + filesz > len(data):
            raise ValueError(f'{name}: LOAD segment outside file')
        if align < page or align & (align - 1):
            raise ValueError(f'{name}: LOAD alignment {align} is below/invalid for {page}-byte policy')
        if offset % page != address % page or offset % align != address % align:
            raise ValueError(f'{name}: LOAD offset/address alignment mismatch')
        loads.append(align)
    if not loads:
        raise ValueError(f'{name}: no LOAD segments')
    return {'name': name, 'abi': abi, 'elf_class': expected_class,
            'elf_machine': machine, 'required_page_bytes': page,
            'load_alignments': loads, 'size': len(data),
            'sha256': hashlib.sha256(data).hexdigest()}


def apk_directory_entries(path):
    """Bound the central directory before ZipFile allocates its entry objects."""
    size = path.stat().st_size
    if size > MAX_APK_BYTES:
        raise ValueError('APK exceeds verifier size limit')
    with path.open('rb') as source:
        source.seek(max(0, size - 65557))  # EOCD plus the longest ZIP comment.
        tail = source.read()
    position = tail.rfind(b'PK\x05\x06')
    if position < 0 or len(tail) - position < 22:
        raise ValueError('APK has no complete ZIP directory record')
    _, disk, directory_disk, disk_count, count, length, offset, comment = struct.unpack_from('<4s4H2IH', tail, position)
    if (disk or directory_disk or disk_count != count or count == 65535
            or offset == 0xffffffff or length == 0xffffffff
            or position + 22 + comment != len(tail)
            or offset + length != size - len(tail) + position):
        raise ValueError('APK has an unsupported or inconsistent ZIP directory')
    if not 0 < count <= MAX_ENTRIES or length > MAX_DIRECTORY_BYTES:
        raise ValueError('APK exceeds entry limit or central directory size limit')
    return count


def verify_apk(path, abi, zipalign):
    count = apk_directory_entries(path)
    libraries = []
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if len(entries) != count:
            raise ValueError('APK directory entry count mismatch')
        names = set()
        total = 0
        for entry in entries:
            name = entry.filename
            parts = PurePosixPath(name).parts
            if (name in names or '\\' in name or '\x00' in name
                    or name.startswith('/') or any(p in ('', '.', '..') for p in name.rstrip('/').split('/'))):
                raise ValueError('APK contains duplicate or unsafe entry names')
            names.add(name)
            if not name.endswith('.so'):
                continue
            if len(parts) != 3 or parts[0] != 'lib' or parts[1] != abi:
                raise ValueError(f'{name}: native library outside expected lib/{abi}/ directory')
            total += entry.file_size
            if (not 0 < entry.file_size <= MAX_LIBRARY_BYTES
                    or total > MAX_TOTAL_LIBRARY_BYTES or len(libraries) >= MAX_LIBRARIES):
                raise ValueError('APK exceeds native library limits')
            libraries.append(verify_elf(archive.read(entry), abi, name))
        required = {f'lib/{abi}/libi2pd.so', f'lib/{abi}/libgipny_lib.so'}
        if not required.issubset(names):
            raise ValueError('APK must contain both the native router and application library')
    result = subprocess.run([str(zipalign), '-c', '-P', '16', '4', str(path)],
                            capture_output=True, text=True, timeout=120, check=False)
    if result.returncode:
        raise ValueError(f'APK zipalign check failed (exit {result.returncode})')
    return {'libraries': libraries, 'zipalign': {'status': 'passed', 'page_kib': 16,
                                               'alignment_bytes': 4}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--library', type=Path)
    source.add_argument('--apk', type=Path)
    parser.add_argument('--abi', choices=ABIS, required=True)
    parser.add_argument('--zipalign', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if args.apk and not args.zipalign:
        parser.error('--apk requires the Android SDK --zipalign executable')
    path = args.apk or args.library
    try:
        if args.apk:
            details = verify_apk(path, args.abi, args.zipalign)
        else:
            if path.stat().st_size > MAX_LIBRARY_BYTES:
                raise ValueError('Library exceeds verifier size limit')
            details = {'libraries': [verify_elf(path.read_bytes(), args.abi, path.name)],
                       'zipalign': None}
        with path.open('rb') as source_file:
            hasher = hashlib.sha256()
            for block in iter(lambda: source_file.read(1024 * 1024), b''):
                hasher.update(block)
            digest = hasher.hexdigest()
        receipt = {'schema_version': 1, 'status': 'passed', 'file': path.name,
                   'sha256': digest,
                   'runtime_tested': False, **details}
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(receipt, indent=2) + '\n')
        print(f'{path.name}: ELF page alignment passed; runtime validation still required')
    except (OSError, ValueError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        print(f'Android page alignment: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
