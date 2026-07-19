"""Connect to real eepsites through our router, bypassing NAMING LOOKUP.

The router's naming handler answers b32 lookups with KEY_NOT_FOUND instantly
(no netdb lookup attempted), so take the base64 destinations straight from the
official hosts.txt and hand them to STREAM CONNECT. This tests the part that
matters: tunnels, LeaseSet lookup for the peer, and streaming.
"""
import os
import socket
import sys
import time

HOST, PORT = "127.0.0.1", 7656
HOSTS = os.environ.get("I2P_HOSTS_TXT", "hosts.txt")
TARGETS = ["i2p-projekt.i2p", "stats.i2p", "zzz.i2p"]

book = {}
for raw in open(HOSTS):
    raw = raw.strip()
    if raw and not raw.startswith("#") and "=" in raw:
        name, _, dest = raw.partition("=")
        book[name.strip()] = dest.strip()
print(f"[eep] address book: {len(book)} entries")


def line(sock, text, timeout, label=None):
    print(f">>> {(label or text)[:100]}")
    sock.sendall((text + "\n").encode())
    sock.settimeout(timeout)
    buf = b""
    t0 = time.time()
    try:
        while not buf.endswith(b"\n"):
            chunk = sock.recv(4096)
            if not chunk:
                print(f"<<< [EOF after {time.time()-t0:.1f}s]")
                return None
            buf += chunk
    except socket.timeout:
        print(f"<<< [TIMEOUT after {time.time()-t0:.1f}s]")
        return None
    out = buf.decode(errors="replace").rstrip()
    print(f"<<< [{time.time()-t0:.1f}s] {out[:100]}")
    return out


c = socket.create_connection((HOST, PORT), timeout=10)
line(c, "HELLO VERSION MIN=3.0 MAX=3.3", 15)
dest = line(c, "DEST GENERATE SIGNATURE_TYPE=7", 60)
priv = next((t[5:] for t in dest.split() if t.startswith("PRIV=")), None) if dest else None
if not priv:
    sys.exit("no destination generated")

s = socket.create_connection((HOST, PORT), timeout=10)
line(s, "HELLO VERSION MIN=3.0 MAX=3.3", 15)
created = line(s, f"SESSION CREATE STYLE=STREAM ID=eeptest DESTINATION={priv}", 180,
               label="SESSION CREATE STYLE=STREAM ID=eeptest DESTINATION=<priv>")
if not created or "RESULT=OK" not in created:
    sys.exit("session create failed")

for name in TARGETS:
    target = book.get(name)
    if not target:
        print(f"\n[eep] {name}: not in hosts.txt")
        continue
    print(f"\n[eep] ==== {name} ({len(target)} char destination) ====")
    t = socket.create_connection((HOST, PORT), timeout=10)
    line(t, "HELLO VERSION MIN=3.0 MAX=3.3", 15)
    status = line(t, f"STREAM CONNECT ID=eeptest DESTINATION={target}", 240,
                  label=f"STREAM CONNECT ID=eeptest DESTINATION=<{name}>")
    if not status or "RESULT=OK" not in status:
        print(f"[eep] {name}: connect FAILED")
        t.close()
        continue

    print(f"[eep] {name}: stream up, sending HTTP GET")
    t.sendall(f"GET / HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n".encode())
    t.settimeout(120)
    body = b""
    t0 = time.time()
    try:
        while len(body) < 2048:
            chunk = t.recv(2048)
            if not chunk:
                break
            body += chunk
    except socket.timeout:
        print(f"[eep] {name}: read timed out after {time.time()-t0:.1f}s")
    print(f"[eep] {name}: received {len(body)} bytes in {time.time()-t0:.1f}s")
    if body:
        print("-" * 50)
        print(body[:400].decode(errors="replace"))
        print("-" * 50)
        sys.exit(0)
    t.close()

sys.exit(1)
