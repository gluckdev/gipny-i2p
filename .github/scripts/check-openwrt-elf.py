#!/usr/bin/env python3
"""Reject wrong architecture, dynamic linking and ARM hard-float release assets."""
import struct
import subprocess
import sys
from pathlib import Path

TARGETS = {
    "aarch64-unknown-linux-musl": (2, 183),
    "armv7-unknown-linux-musleabi": (1, 40),
    "x86_64-unknown-linux-musl": (2, 62),
    "i686-unknown-linux-musl": (1, 3),
}


def check(target, path):
    expected_class, expected_machine = TARGETS[target]
    data = Path(path).read_bytes()
    assert data[:4] == b"\x7fELF", "not ELF"
    assert data[4] == expected_class and data[5] == 1, "wrong word size/endianness"
    assert struct.unpack_from("<H", data, 18)[0] == expected_machine, "wrong CPU machine"
    elf64 = expected_class == 2
    phoff = struct.unpack_from("<Q" if elf64 else "<I", data, 32 if elf64 else 28)[0]
    phentsize, phnum = struct.unpack_from("<HH", data, 54 if elf64 else 42)
    for index in range(phnum):
        position = phoff + index * phentsize
        kind = struct.unpack_from("<I", data, position)[0]
        assert kind != 3, "PT_INTERP: binary requires a runtime loader"
        if kind == 2:
            offset = struct.unpack_from("<Q" if elf64 else "<I", data, position + (8 if elf64 else 4))[0]
            size = struct.unpack_from("<Q" if elf64 else "<I", data, position + (32 if elf64 else 16))[0]
            width = 16 if elf64 else 8
            for location in range(offset, offset + size, width):
                tag = struct.unpack_from("<q" if elf64 else "<i", data, location)[0]
                assert tag != 1, "DT_NEEDED: binary requires a shared library"
                if tag == 0:
                    break
    if expected_machine == 40:
        flags = struct.unpack_from("<I", data, 36)[0]
        assert not flags & 0x400, "ARM hard-float ELF ABI"
        attributes = subprocess.check_output(["readelf", "-A", path], text=True)
        assert "Tag_ABI_VFP_args: VFP registers" not in attributes, "ARM hard-float calling convention"
        print(attributes, end="")
    # Native log messages remain compiled for runtime filtering; these are Rust
    # production trace call sites that must disappear from an optimized release.
    for trace in [b"[relay-wire] recv ", b"[agent] executing command with "]:
        assert trace not in data, "developer trace survived in the release binary"
    print(f"{target}: static little-endian ELF, correct CPU/ABI, no release traces: PASS")


if __name__ == "__main__":
    check(sys.argv[1], sys.argv[2])
