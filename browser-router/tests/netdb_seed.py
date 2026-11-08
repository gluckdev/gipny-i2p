"""Seed a native i2pd datadir with fresh public RouterInfos from another netDb.

The live CI peer used to start from reseed alone. A reseed bundle can be a day
or more old: the 2026-11-08 runs got 61 RouterInfos, every one older than the
36 h window `verified_peer` accepts, sessions to those stale endpoints died
with EOF/reset, nothing fresher arrived in 240 s, and the handshake step failed
on data, not code. `collect-netdb` (the release snapshot's router, default
transports, up to 360 s) learns the live network on the same runner; copying
its fresh records in before the peer starts gives `prepare-netdb` verified
peers at once and the peer live neighbours to talk to.

Only records fresher than the window are copied. i2pd reseeds only below
`reseed.threshold` (25 routers); seeding it with stale files would suppress
that fallback and reproduce the failure with a worse diagnosis, so fewer than
`minimum` fresh records seed nothing and the peer reseeds as before.

Public netDb records only: no router.keys, no private keys, no destinations.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
import shutil
import struct
import time

MAX_AGE_MS = 36 * 60 * 60 * 1000  # the same window as ntcp2::verified_peer
MAX_FUTURE_MS = 120_000
MAX_RECORD_BYTES = 64 * 1024
RESEED_THRESHOLD = 25  # i2pd reseed.threshold default
RECORD = re.compile(r'^routerInfo-[A-Za-z0-9~=-]+\.dat$')
SHARD = re.compile(r'^r.$')  # i2pd lays netDb out as netDb/r<c>/routerInfo-*.dat


def router_info_published_ms(record: bytes) -> int | None:
    """The 8-byte published date after the RouterIdentity, or None if the
    bytes are not shaped like a RouterInfo. Identity = 256 B public key +
    128 B signing key + certificate (1 B type, 2 B length, payload)."""
    if len(record) < 387 + 8:
        return None
    (certificate_length,) = struct.unpack('>H', record[385:387])
    offset = 384 + 3 + certificate_length
    if len(record) < offset + 8:
        return None
    (published,) = struct.unpack('>Q', record[offset:offset + 8])
    return published


@dataclass
class SeedReport:
    fresh: int = 0
    stale: int = 0
    skipped: int = 0
    seeded: int = 0

    def line(self) -> str:
        return f'NETDB_SEED fresh={self.fresh} stale={self.stale} skipped={self.skipped} seeded={self.seeded}'


def fresh_records(source_netdb: Path, now_ms: int | None = None, max_age_ms: int = MAX_AGE_MS) -> tuple[list[Path], SeedReport]:
    """RouterInfo files under `source_netdb` that are fresh and well formed."""
    now = int(time.time() * 1000) if now_ms is None else now_ms
    report = SeedReport()
    fresh: list[Path] = []
    if not source_netdb.is_dir():
        return fresh, report
    for shard in sorted(source_netdb.iterdir()):
        if not shard.is_dir() or not SHARD.match(shard.name):
            report.skipped += 1
            continue
        for path in sorted(shard.iterdir()):
            if not path.is_file() or path.is_symlink() or not RECORD.match(path.name) or path.stat().st_size > MAX_RECORD_BYTES:
                report.skipped += 1
                continue
            published = router_info_published_ms(path.read_bytes())
            if published is None:
                report.skipped += 1
            elif now - published > max_age_ms or published - now > MAX_FUTURE_MS:
                report.stale += 1
            else:
                report.fresh += 1
                fresh.append(path)
    return fresh, report


def seed_router_infos(source_netdb: Path, target_datadir: Path, now_ms: int | None = None,
                      minimum: int = RESEED_THRESHOLD, max_age_ms: int = MAX_AGE_MS) -> SeedReport:
    """Copy fresh RouterInfos into `<target_datadir>/netDb`, keeping i2pd's
    shard layout. Below `minimum` fresh records nothing is copied."""
    fresh, report = fresh_records(source_netdb, now_ms, max_age_ms)
    if len(fresh) < minimum:
        return report
    netdb = target_datadir / 'netDb'
    for path in fresh:
        destination = netdb / path.parent.name
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination / path.name)
        report.seeded += 1
    return report
