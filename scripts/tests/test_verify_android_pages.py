#!/usr/bin/env python3
"""Exercise Android packaging rejection paths without an SDK or native execution."""
import importlib.util
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('pages', Path(__file__).parents[1] / 'verify-android-pages.py')
pages = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pages)


def elf(abi='arm64-v8a', align=16384, machine=None, offset=0, address=0):
    elf_class, expected_machine, _ = pages.ABIS[abi]
    ident = b'\x7fELF' + bytes((elf_class, 1, 1)) + bytes(9)
    header = struct.Struct('<HHIQQQIHHHHHH' if elf_class == 2 else '<HHIIIIIHHHHHH')
    program = struct.Struct('<IIQQQQQQ' if elf_class == 2 else '<IIIIIIII')
    ehsize = 16 + header.size
    size = ehsize + program.size
    values = (3, machine or expected_machine, 1, 0, ehsize, 0, 0, ehsize, program.size, 1, 0, 0, 0)
    segment = ((1, 5, offset, address, 0, size - offset, size - offset, align) if elf_class == 2
               else (1, offset, address, 0, size - offset, size - offset, 5, align))
    return ident + header.pack(*values) + program.pack(*segment)


class AndroidPagePolicy(unittest.TestCase):
    def test_64_bit_libraries_accept_16k_and_reject_4k(self):
        for abi in ('arm64-v8a', 'x86_64'):
            with self.subTest(abi=abi):
                self.assertEqual(pages.verify_elf(elf(abi), abi, 'fixture')['required_page_bytes'], 16384)
                with self.assertRaisesRegex(ValueError, 'LOAD alignment'):
                    pages.verify_elf(elf(abi, 4096), abi, 'old-router')

    def test_legacy_32_bit_policy_remains_4k(self):
        for abi in ('armeabi-v7a', 'x86'):
            self.assertEqual(pages.verify_elf(elf(abi, 4096), abi, 'fixture')['required_page_bytes'], 4096)

    def test_directory_name_cannot_disguise_machine_or_class(self):
        for data in (elf(machine=62), elf('armeabi-v7a')):
            with self.assertRaisesRegex(ValueError, 'does not match'):
                pages.verify_elf(data, 'arm64-v8a', 'pretend-arm64')

    def test_truncated_header_and_program_table_rejected(self):
        for length in (0, 16, 63, 64, 119):
            with self.subTest(length=length), self.assertRaises(ValueError):
                pages.verify_elf(elf()[:length], 'arm64-v8a', 'truncated')

    def test_invalid_alignment_or_segment_address_rejected(self):
        for data in (elf(align=24576), elf(address=1), elf(offset=1)):
            with self.assertRaisesRegex(ValueError, 'alignment'):
                pages.verify_elf(data, 'arm64-v8a', 'invalid-load')

    def test_empty_or_out_of_bounds_loads_rejected(self):
        data = bytearray(elf())
        struct.pack_into('<I', data, 64, 0)  # PT_NULL, not PT_LOAD.
        with self.assertRaisesRegex(ValueError, 'no LOAD'):
            pages.verify_elf(data, 'arm64-v8a', 'no-load')
        struct.pack_into('<I', data, 64, 1)
        struct.pack_into('<Q', data, 96, 9999)  # p_filesz exceeds the fixture.
        with self.assertRaisesRegex(ValueError, 'outside file'):
            pages.verify_elf(data, 'arm64-v8a', 'bad-size')


class ApkPolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'fixture.apk'
        self.base = {'lib/arm64-v8a/libi2pd.so': elf(), 'lib/arm64-v8a/libgipny_lib.so': elf()}
        self.zipalign = patch.object(pages.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0))
        self.command = self.zipalign.start()
        self.addCleanup(self.zipalign.stop)

    def verify(self, members=None):
        with zipfile.ZipFile(self.path, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            for name, data in (self.base if members is None else members).items():
                archive.writestr(name, data)
        return pages.verify_apk(self.path, 'arm64-v8a', Path('/sdk/zipalign'))

    def test_checks_both_elfs_and_calls_official_zipalign_even_when_compressed(self):
        report = self.verify()
        self.assertEqual(len(report['libraries']), 2)
        self.assertEqual(report['zipalign']['status'], 'passed')
        self.assertEqual(self.command.call_args.args[0], ['/sdk/zipalign', '-c', '-P', '16', '4', str(self.path)])

    def test_any_bad_extra_library_rejects_complete_apk(self):
        with self.assertRaisesRegex(ValueError, 'LOAD alignment'):
            self.verify({**self.base, 'lib/arm64-v8a/libthird.so': elf(align=4096)})
        self.command.assert_not_called()

    def test_missing_native_library_rejected(self):
        with self.assertRaisesRegex(ValueError, 'both the native router'):
            self.verify({'lib/arm64-v8a/libi2pd.so': elf()})

    def test_unsafe_paths_and_unexpected_abi_rejected_without_extraction(self):
        for name in ('../escape.so', '/absolute.so', 'lib/../escape.so',
                     'lib/arm64-v8a/nested/fake.so', 'lib/x86_64/fake.so', 'assets/hidden.so'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.verify({**self.base, name: elf()})

    def test_duplicate_entry_rejected(self):
        with zipfile.ZipFile(self.path, 'w') as archive:
            archive.writestr('lib/arm64-v8a/libi2pd.so', elf())
            with self.assertWarns(UserWarning):
                archive.writestr('lib/arm64-v8a/libi2pd.so', elf())
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            pages.verify_apk(self.path, 'arm64-v8a', Path('/sdk/zipalign'))

    def test_zipalign_failure_and_timeout_reject_apk(self):
        self.command.return_value = subprocess.CompletedProcess([], 1)
        with self.assertRaisesRegex(ValueError, 'zipalign check failed'):
            self.verify()
        self.command.side_effect = subprocess.TimeoutExpired('zipalign', 120)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.verify()

    def test_parser_limits_reject_oversized_native_payload(self):
        with patch.object(pages, 'MAX_LIBRARY_BYTES', 100):
            with self.assertRaisesRegex(ValueError, 'native library limits'):
                self.verify()
        with patch.object(pages, 'MAX_ENTRIES', 1):
            with self.assertRaisesRegex(ValueError, 'entry limit'):
                self.verify()

    def test_entry_limit_checked_before_zipfile_allocates_directory(self):
        self.verify()
        data = bytearray(self.path.read_bytes())
        end = data.rfind(b'PK\x05\x06')
        struct.pack_into('<HH', data, end + 8, 20001, 20001)
        self.path.write_bytes(data)
        with patch.object(pages.zipfile, 'ZipFile') as constructor:
            with self.assertRaisesRegex(ValueError, 'entry limit'):
                pages.verify_apk(self.path, 'arm64-v8a', Path('/sdk/zipalign'))
            constructor.assert_not_called()

    def test_false_directory_count_or_appended_data_rejected(self):
        self.verify()
        data = bytearray(self.path.read_bytes())
        end = data.rfind(b'PK\x05\x06')
        struct.pack_into('<HH', data, end + 8, 1, 1)
        self.path.write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'entry count mismatch'):
            pages.verify_apk(self.path, 'arm64-v8a', Path('/sdk/zipalign'))
        self.path.write_bytes(data + b'junk')
        with self.assertRaisesRegex(ValueError, 'inconsistent ZIP'):
            pages.verify_apk(self.path, 'arm64-v8a', Path('/sdk/zipalign'))


if __name__ == '__main__':
    unittest.main()
