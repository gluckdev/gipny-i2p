import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('pe', Path(__file__).parents[1] / 'verify-windows-pe.py')
pe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pe)


def fixture(machine=0xAA64):
    data = bytearray(1024)
    data[:2] = b'MZ'
    struct.pack_into('<I', data, 60, 128)
    data[128:132] = b'PE\0\0'
    struct.pack_into('<HHIIIHH', data, 132, machine, 1, 0, 0, 0, 240, 2)
    struct.pack_into('<H', data, 152, 0x20B)
    struct.pack_into('<I', data, 212, 512)
    struct.pack_into('<I', data, 260, 16)
    struct.pack_into('<II', data, 408, 512, 512)
    return data


class PeTests(unittest.TestCase):
    def inspect(self, data, arch='arm64'):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'app.exe'
            path.write_bytes(data)
            return pe.inspect_pe(path, arch)

    def test_real_machine_mapping_and_digest(self):
        for arch, machine in pe.MACHINES.items():
            result = self.inspect(fixture(machine), arch)
            self.assertEqual(result['machine'], f'0x{machine:04x}')
            self.assertEqual(len(result['sha256']), 64)
            self.assertEqual(result['size'], 1024)

    def test_renamed_x64_or_nsis_stub_is_not_arm64(self):
        for machine in [0x8664, 0x14C, 0xA641, 0]:
            with self.subTest(machine=machine), self.assertRaises(ValueError):
                self.inspect(fixture(machine))

    def test_truncated_headers_and_sections(self):
        for size in [0, 63, 130, 151, 200, 430, 700]:
            with self.subTest(size=size), self.assertRaises(ValueError):
                self.inspect(fixture()[:size])

    def test_header_bounds_and_optional_format(self):
        for offset, fmt, value in [(60,'I',0xFFFFFFFF), (134,'H',0), (134,'H',97),
                                    (148,'H',0), (148,'H',4097), (150,'H',0),
                                    (152,'H',0x10B), (212,'I',1), (212,'I',2000),
                                    (260,'I',17), (408,'I',1000), (412,'I',1)]:
            data = fixture()
            struct.pack_into('<' + fmt, data, offset, value)
            with self.subTest(offset=offset,value=value), self.assertRaises(ValueError):
                self.inspect(data)

    def test_dll_uses_same_machine_contract(self):
        data=fixture()
        struct.pack_into('<H', data, 150, 0x2002)
        self.assertTrue(self.inspect(data)['dll'])

    def test_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            actual=Path(root)/'real.exe'
            actual.write_bytes(fixture())
            link=Path(root)/'fake.exe'
            try: link.symlink_to(actual)
            except OSError: self.skipTest('symlink privilege unavailable')
            with self.assertRaises(ValueError): pe.inspect_pe(link,'arm64')


if __name__ == '__main__':
    unittest.main()
