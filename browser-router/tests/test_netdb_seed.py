"""Unit tests for netdb_seed: python3 -m unittest discover -s browser-router/tests -p 'test_*.py'"""
import struct
import tempfile
import unittest
from pathlib import Path

from netdb_seed import MAX_AGE_MS, RESEED_THRESHOLD, fresh_records, router_info_published_ms, seed_router_infos

NOW = 1_800_000_000_000


def record(published_ms: int, certificate: bytes = b'\x00\x00\x00\x00') -> bytes:
    """Bytes shaped like a RouterInfo: identity (384 B + KEY certificate), then the date."""
    return bytes(384) + b'\x05' + struct.pack('>H', len(certificate)) + certificate + struct.pack('>Q', published_ms) + bytes(40)


def write(netdb: Path, shard: str, name: str, data: bytes) -> Path:
    path = netdb / shard / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


class PublishedDate(unittest.TestCase):
    def test_reads_the_date_after_the_certificate(self):
        self.assertEqual(router_info_published_ms(record(NOW)), NOW)
        self.assertEqual(router_info_published_ms(record(NOW - 5, certificate=bytes(12))), NOW - 5)

    def test_rejects_short_or_truncated_bytes(self):
        self.assertIsNone(router_info_published_ms(b'short'))
        self.assertIsNone(router_info_published_ms(record(NOW)[:390]))


class Freshness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.netdb = Path(self.tmp.name) / 'source' / 'netDb'

    def tearDown(self):
        self.tmp.cleanup()

    def test_sorts_fresh_stale_and_skipped(self):
        write(self.netdb, 'rA', 'routerInfo-aaaa.dat', record(NOW - 60_000))
        write(self.netdb, 'rB', 'routerInfo-bbbb.dat', record(NOW - MAX_AGE_MS - 1))
        write(self.netdb, 'rC', 'routerInfo-cccc.dat', record(NOW + 300_000))  # clock far ahead
        write(self.netdb, 'rD', 'routerInfo-dddd.dat', b'not a router info')
        write(self.netdb, 'rE', 'leaseSet-eeee.dat', record(NOW))  # not a RouterInfo name
        write(self.netdb, 'deep', 'routerInfo-ffff.dat', record(NOW))  # not an i2pd shard
        (self.netdb / 'router.keys').write_bytes(b'private')  # a file, not a shard
        fresh, report = fresh_records(self.netdb, NOW)
        self.assertEqual([path.name for path in fresh], ['routerInfo-aaaa.dat'])
        self.assertEqual((report.fresh, report.stale, report.skipped), (1, 2, 4))

    def test_missing_source_is_empty(self):
        fresh, report = fresh_records(Path(self.tmp.name) / 'absent', NOW)
        self.assertEqual(fresh, [])
        self.assertEqual(report.fresh, 0)


class Seeding(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.netdb = Path(self.tmp.name) / 'source' / 'netDb'
        self.target = Path(self.tmp.name) / 'peer'

    def tearDown(self):
        self.tmp.cleanup()

    def populate(self, fresh: int, stale: int = 0):
        for index in range(fresh):
            write(self.netdb, f'r{chr(65 + index % 26)}', f'routerInfo-fresh{index}.dat', record(NOW - index * 1000))
        for index in range(stale):
            write(self.netdb, 'rZ', f'routerInfo-stale{index}.dat', record(NOW - MAX_AGE_MS - 1000 * (index + 1)))

    def test_below_the_reseed_threshold_nothing_is_copied(self):
        self.populate(fresh=RESEED_THRESHOLD - 1, stale=40)
        report = seed_router_infos(self.netdb, self.target, NOW)
        self.assertEqual(report.seeded, 0)
        self.assertFalse((self.target / 'netDb').exists())

    def test_fresh_records_are_copied_in_the_shard_layout_and_stale_ones_left_behind(self):
        self.populate(fresh=RESEED_THRESHOLD, stale=3)
        report = seed_router_infos(self.netdb, self.target, NOW)
        self.assertEqual((report.fresh, report.stale, report.seeded), (RESEED_THRESHOLD, 3, RESEED_THRESHOLD))
        copied = sorted(path.relative_to(self.target / 'netDb').as_posix() for path in (self.target / 'netDb').rglob('routerInfo-*.dat'))
        self.assertEqual(len(copied), RESEED_THRESHOLD)
        self.assertTrue(all(path.startswith('r') and '/routerInfo-fresh' in path for path in copied))
        self.assertEqual(report.line(), f'NETDB_SEED fresh={RESEED_THRESHOLD} stale=3 skipped=0 seeded={RESEED_THRESHOLD}')

    def test_copies_are_byte_identical(self):
        self.populate(fresh=RESEED_THRESHOLD)
        seed_router_infos(self.netdb, self.target, NOW)
        source = (self.netdb / 'rA' / 'routerInfo-fresh0.dat').read_bytes()
        self.assertEqual((self.target / 'netDb' / 'rA' / 'routerInfo-fresh0.dat').read_bytes(), source)


if __name__ == '__main__':
    unittest.main()
