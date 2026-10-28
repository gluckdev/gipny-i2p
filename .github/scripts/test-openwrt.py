#!/usr/bin/env python3
"""Offline regressions for the installer parser and real ELF rejection gates."""
import contextlib
import io
import runpy
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CHECK = runpy.run_path(str(REPO / '.github/scripts/check-openwrt-elf.py'))['check']


class InstallerVerification(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        source = (REPO / 'scripts/install-agent-openwrt.sh').read_text().split('# Installer entry point')[0]
        self.library = self.root / 'functions.sh'
        self.library.write_text(source + '\n"$@"\n')
        self.shell = ['sh']
        self.environment = None
        if busybox := shutil.which('busybox'):
            import os
            (self.root / 'awk').symlink_to(busybox)
            self.environment = dict(os.environ, PATH=f'{self.root}:{os.environ["PATH"]}')
            self.shell = [busybox, 'sh']

    def call(self, function, *args):
        return subprocess.run([*self.shell, str(self.library), function, *map(str, args)],
                              env=self.environment, capture_output=True, text=True)

    def test_manifest_requires_one_valid_digest_and_rejects_any_malformed_row(self):
        asset = 'gipny-agent_0.4.19_armv7-unknown-linux-musleabi.tar.gz'
        digest = 'ABCDEF01' * 8
        valid = f'{digest}  {asset}\n'
        path = self.root / 'SHA256SUMS.txt'
        for manifest in [valid, valid.replace('  ', ' *')]:
            path.write_text(manifest)
            result = self.call('manifest_digest', path, asset)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), digest.lower())
        for manifest in ['', valid + valid, valid + 'malformed\n', valid.replace(digest, '0' * 63),
                         valid.replace(digest, 'g' * 64), valid.replace('0.4.19', '0.4.18'),
                         valid + ('0' * 64) + '  ../escape\n']:
            path.write_text(manifest)
            result = self.call('manifest_digest', path, asset)
            self.assertNotEqual(result.returncode, 0, manifest)
            self.assertEqual(result.stdout, '')

    def test_master_card_validation_never_echoes_the_card(self):
        address = 'a' * 52 + '.b32.i2p'
        card = f'gipny:v2:{address}:{"1" * 64}:{"2" * 64}:{address}:PrivateName'
        self.assertEqual(self.call('validate_master', card).returncode, 0)
        for bad in ['not-a-card', card.replace('1' * 64, '1' * 63), card.replace(address, 'broken')]:
            result = self.call('validate_master', bad)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout + result.stderr, '')


class ElfVerification(unittest.TestCase):
    def test_real_elf_detects_loader_needed_machine_and_trace_regressions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'probe.c'
            source.write_text('int main(void) { return 0; }\n')
            binary = root / 'probe'
            subprocess.run(['cc', '-static', str(source), '-o', str(binary)], check=True, capture_output=True)
            with contextlib.redirect_stdout(io.StringIO()):
                CHECK('x86_64-unknown-linux-musl', str(binary))
            with self.assertRaisesRegex(AssertionError, 'CPU machine'):
                CHECK('aarch64-unknown-linux-musl', str(binary))
            subprocess.run(['cc', str(source), '-o', str(binary)], check=True, capture_output=True)
            with self.assertRaisesRegex(AssertionError, 'PT_INTERP'):
                CHECK('x86_64-unknown-linux-musl', str(binary))
            # Remove only PT_INTERP from this real dynamically linked ELF:
            # DT_NEEDED must independently keep it from becoming a release.
            import struct
            data = bytearray(binary.read_bytes())
            phoff = struct.unpack_from('<Q', data, 32)[0]
            phentsize, count = struct.unpack_from('<HH', data, 54)
            for index in range(count):
                offset = phoff + index * phentsize
                if struct.unpack_from('<I', data, offset)[0] == 3:
                    struct.pack_into('<I', data, offset, 0)
            binary.write_bytes(data)
            with self.assertRaisesRegex(AssertionError, 'DT_NEEDED'):
                CHECK('x86_64-unknown-linux-musl', str(binary))
            source.write_text('#include <stdio.h>\nint main(void) { puts("[relay-wire] recv trace"); }\n')
            subprocess.run(['cc', '-static', str(source), '-o', str(binary)], check=True, capture_output=True)
            with self.assertRaisesRegex(AssertionError, 'trace survived'):
                CHECK('x86_64-unknown-linux-musl', str(binary))


if __name__ == '__main__':
    unittest.main()
