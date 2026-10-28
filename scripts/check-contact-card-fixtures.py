#!/usr/bin/env python3
"""Independent, data-only v3 golden vectors; Python stdlib, no app build."""
import base64
import hashlib
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PATH = ROOT / 'tests/fixtures/contact-cards-v3.json'
DOMAIN = b'gipny/contact-card/v3\0'


def wrap(body):
    payload = body + hashlib.sha256(DOMAIN + body).digest()[:4]
    return 'gipny:v3:' + base64.urlsafe_b64encode(payload).decode().rstrip('=')


def b32(raw):
    return base64.b32encode(raw).decode().lower().rstrip('=') + '.b32.i2p'


def destination(certificate):
    # Public destination framing: 256 encryption +128 signing key bytes,
    # certificate type, big-endian length, and that exact certificate payload.
    raw = bytes(n % 256 for n in range(384)) + certificate
    return base64.b64encode(raw).decode().replace('+', '-').replace('/', '~')


def address_hash(address):
    address = address.strip()
    if address.lower().endswith('.b32.i2p'):
        return base64.b32decode(address[:-8].upper() + '====')
    return hashlib.sha256(base64.b64decode(address.replace('-', '+').replace('~', '/'))).digest()


def vector(identifier, onion, relay=None, name=None):
    sign, dh = bytes(range(32)), bytes(range(32, 64))
    body = bytes([int(relay is not None) | (int(bool(name)) << 1)]) + sign + dh + address_hash(onion)
    if relay is not None:
        body += address_hash(relay)
    if name:
        encoded = name.encode('utf-8')
        assert 0 < len(encoded) <= 96
        body += bytes([len(encoded)]) + encoded
    source = dict(onion=onion, sign_pk=sign.hex(), dh_pk=dh.hex())
    decoded = dict(onion=b32(address_hash(onion)), sign_pk=sign.hex(), dh_pk=dh.hex())
    if relay is not None:
        source['relay'], decoded['relay'] = relay, b32(address_hash(relay))
    if name:
        source['name'] = decoded['name'] = name
    return dict(id=identifier, input=source, encoded=wrap(body), decoded=decoded)


def fixtures():
    outbound, relay = b32(bytes(range(64, 96))), b32(bytes(range(96, 128)))
    null_destination = destination(b'\0\0\0')
    key_destination = destination(b'\x05\x00\x04\x00\x07\x00\x04')
    valid = [
        vector('two-hashes-no-name', outbound, relay),
        vector('outbound-only', outbound),
        vector('uppercase-addresses', outbound.upper(), relay.upper()),
        vector('unicode-name', outbound, relay, 'Мурка 🐈: café'),
        vector('max-name-96-utf8-bytes', outbound, relay, 'я' * 48),
        vector('utf8-bom-preserved', outbound, relay, '\ufeffМурка'),
        vector('full-null-certificate', null_destination, relay),
        vector('full-key-certificate', null_destination, key_destination),
    ]
    assert len(valid[0]['encoded']) == 187
    assert len(valid[1]['encoded']) == 144
    assert len(valid[4]['encoded']) == 316
    body = base64.urlsafe_b64decode(valid[0]['encoded'][9:] + '==')[:-4]
    malformed = [
        ('checksum-bitflip', valid[0]['encoded'][:-2] + 'AA'),
        ('padding', valid[0]['encoded'] + '=='),
        ('unknown-flags-authenticated-checksum', wrap(bytes([0x81]) + body[1:])),
        ('trailing-byte', wrap(body + b'\0')),
        ('missing-relay-byte', wrap(body[:-1])),
        ('empty-name', wrap(bytes([3]) + body[1:] + b'\0')),
        ('name-over-96', wrap(bytes([3]) + body[1:] + bytes([97]) + b'x' * 96)),
        ('name-length-mismatch', wrap(bytes([3]) + body[1:] + b'\x03ab')),
        ('invalid-utf8-name', wrap(bytes([3]) + body[1:] + b'\x02\xc3(')),
        ('overlong-utf8-name', wrap(bytes([3]) + body[1:] + b'\x03\xe0\x80\x80')),
        ('overlong-card', 'gipny:v3:' + 'A' * 308),
        ('internal-whitespace', valid[0]['encoded'][:20] + '\n' + valid[0]['encoded'][20:]),
    ]
    alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'
    card = valid[0]['encoded']
    malformed.append(('nonzero-base64-tail-bits', card[:-1] + alphabet[alphabet.index(card[-1]) | 1]))
    return dict(format='gipny:v3', valid=valid, invalid=[dict(id=name, encoded=encoded) for name, encoded in malformed])


expected = fixtures()
if sys.argv[1:] == ['--write']:
    PATH.parent.mkdir(parents=True, exist_ok=True)
    PATH.write_text(json.dumps(expected, ensure_ascii=False, indent=2) + '\n')
elif sys.argv[1:]:
    raise SystemExit('usage: check-contact-card-fixtures.py [--write]')
actual = json.loads(PATH.read_text())
assert actual == expected, 'golden fixture differs from Python hashlib/base64 reference'
for item in actual['valid']:
    assert item['encoded'].isascii()
    assert all(char.isalnum() or char in ':_-' for char in item['encoded'])
print(f"Contact-card data fixtures: {len(actual['valid'])} valid, {len(actual['invalid'])} invalid; 187/144/316 character bounds verified")
