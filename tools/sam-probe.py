"""Two SAM sessions on one router — the case that failed in CI.

Session 1 stands in for the relay, session 2 for bot-a. Before the per-session
I2CP client change, session 2 got no reply at all (EOF after the bridge's 60 s
command deadline) because go-i2cp only processed the first session's
SessionStatus.
"""
import socket
import sys
import time

HOST, PORT = "127.0.0.1", 7656


def cmd(sock, line, timeout):
    print(f"\n>>> {line[:110]}")
    sock.sendall((line + "\n").encode())
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
    print(f"<<< [{time.time()-t0:.1f}s] {out[:110]}")
    return out


def make_session(name):
    s = socket.create_connection((HOST, PORT), timeout=10)
    if not cmd(s, "HELLO VERSION MIN=3.0 MAX=3.3", 15):
        return False
    dest = cmd(s, "DEST GENERATE SIGNATURE_TYPE=7", 60)
    if not dest:
        return False
    priv = next((t[5:] for t in dest.split() if t.startswith("PRIV=")), None)
    if not priv:
        print("no PRIV=")
        return False

    s2 = socket.create_connection((HOST, PORT), timeout=10)
    cmd(s2, "HELLO VERSION MIN=3.0 MAX=3.3", 15)
    reply = cmd(s2, f"SESSION CREATE STYLE=STREAM ID={name} DESTINATION={priv}", 180)
    ok = bool(reply) and "RESULT=OK" in reply
    print(f"[probe] session {name}: {'OK' if ok else 'FAILED'}")
    return ok


results = [make_session("probe-relay"), make_session("probe-bot-a")]
print(f"\n[probe] sessions OK: {sum(results)}/2")
sys.exit(0 if all(results) else 1)
