#!/usr/bin/env python3
"""Validate public browser deployment data. No builds, network or secret output."""
import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit


def validate(value):
    allowed = {'snapshot_url', 'snapshot_sha256', 'forwarder_url', 'capability',
               'transport', 'rtc_ice_policy', 'rtc_ice_servers'}
    if not isinstance(value, dict) or set(value) - allowed:
        raise ValueError('unexpected browser configuration fields')
    for key in ('snapshot_url', 'snapshot_sha256', 'forwarder_url', 'capability'):
        if not isinstance(value.get(key), str) or len(value[key]) > 4096:
            raise ValueError('missing or invalid browser configuration field')
    transport = value.get('transport', 'wss')
    if transport not in ('wss', 'webrtc'):
        raise ValueError('unsupported browser transport')
    for key, scheme in (('snapshot_url', 'https'), ('forwarder_url', 'wss')):
        parsed = urlsplit(value[key])
        if parsed.scheme != scheme or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query:
            raise ValueError('public deployment requires HTTPS/WSS without credentials, query or fragment')
        if parsed.hostname in ('localhost', '127.0.0.1', '::1'):
            raise ValueError('loopback fixtures cannot be deployed')
    path = urlsplit(value['forwarder_url']).path
    if path != ('/rtc' if transport == 'webrtc' else '/stream'):
        raise ValueError('forwarder path does not match transport')
    if not re.fullmatch('[a-fA-F0-9]{64}', value['snapshot_sha256']) or not re.fullmatch('[A-Za-z0-9_-]{43}', value['capability']):
        raise ValueError('invalid snapshot digest or capability format')
    if transport == 'webrtc':
        policy = value.get('rtc_ice_policy', 'relay')
        servers = value.get('rtc_ice_servers', [])
        if policy not in ('relay', 'all') or not isinstance(servers, list) or len(servers) > 4:
            raise ValueError('invalid ICE configuration')
        turn = False
        for entry in servers:
            if not isinstance(entry, dict) or set(entry) - {'urls', 'username', 'credential'}:
                raise ValueError('invalid ICE server')
            urls = entry.get('urls')
            urls = [urls] if isinstance(urls, str) else urls
            if not isinstance(urls, list) or not 1 <= len(urls) <= 4:
                raise ValueError('invalid ICE URL list')
            for url in urls:
                if not isinstance(url, str) or len(url) > 2048 or not re.fullmatch(r'(turns?|stun):[^\s/@]+(?:\?transport=(udp|tcp))?', url):
                    raise ValueError('invalid ICE URL')
            for key in ('username', 'credential'):
                if key in entry and (not isinstance(entry[key], str) or not 1 <= len(entry[key]) <= 1024):
                    raise ValueError('invalid TURN credential')
            turn |= any(url.startswith(('turn:', 'turns:')) for url in urls) and bool(entry.get('username')) and bool(entry.get('credential'))
        if policy == 'relay' and not turn:
            raise ValueError('relay-only WebRTC requires TURN credentials')
    return value


if __name__ == '__main__':
    raw = os.environ.get('GIPNY_BROWSER_CONFIG_JSON', '')
    if not raw:
        print('No public network configuration supplied; the WASM client opens with offline storage only.')
        raise SystemExit(0)
    if len(raw.encode()) > 65536:
        raise SystemExit('Browser configuration exceeds 64 KiB')
    try:
        value = validate(json.loads(raw))
    except (ValueError, TypeError, KeyError):
        raise SystemExit('Browser configuration rejected; review the documented public schema (values withheld).')
    Path(sys.argv[1]).write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
    print('Validated public browser network configuration written.')
