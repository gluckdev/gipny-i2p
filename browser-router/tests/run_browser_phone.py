"""Browser messenger (headless Chrome, local forwarder) <-> a phone over adb.

A local tool, not a CI job: it needs a phone on USB with USB debugging and a
debug or prerelease build of the app open on its main screen (those expose
the webview to chrome://inspect; see docs/release-diagnostics.md), the
forwarder built (`cargo build --release --manifest-path forwarder/Cargo.toml`),
the browser messenger built (`wasm-pack build --target web --release --locked`
then `npm run build:browser --prefix ../ui`), and a snapshot made by
`prepare-netdb` from a netDb directory:

    GIPNY_SNAPSHOT=/path/snapshot.tar.gz python3 tests/run_browser_phone.py

The browser creates a profile, builds tunnels through the local forwarder,
sends the phone a contact request with a message, the phone accepts and
answers; both receipts are awaited. Reuses the CI harness pieces: the
Assets server, BRIDGE (worker RPC), Cdp, create_ui. Nothing private is
printed: addresses are cut to a prefix, no passwords, no keys.
First run 2026-11-07 against an Honor KOZ-AL00: browser tunnels 64 s,
request and message on the phone 32 s later, phone to browser 2 s.
"""
import json, os, secrets, shutil, subprocess, sys, tempfile, threading, time
from pathlib import Path
from http.server import ThreadingHTTPServer
from urllib.request import urlopen
from websockets.sync.client import connect

ROOT = Path(__file__).resolve().parents[2]
SNAPSHOT = Path(os.environ['GIPNY_SNAPSHOT'])
OUT = Path(os.environ.get('GIPNY_OUT', tempfile.gettempdir())) / 'browser-phone'
sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_messenger_ci as H  # noqa: E402
from run_bootstrap_ci import Cdp, stop  # noqa: E402
from run_ntcp2_ci import wait_port  # noqa: E402

# Used only when the phone sits on its profile list: the test profile to open.
PHONE_PROFILE, PHONE_PASS = os.environ.get('GIPNY_PHONE_PROFILE', ''), os.environ.get('GIPNY_PHONE_PASS', '')

def log(*a):
    print(time.strftime('%H:%M:%S'), *a, flush=True)

def adb(*args, **kw):
    return subprocess.run(['adb', *args], capture_output=True, text=True, **kw).stdout.strip()

class Phone:
    """The Tauri app on the Honor over DevTools; `invoke` = window.__TAURI_INTERNALS__.invoke."""
    def __init__(self):
        self.connect()
    def connect(self):
        pid = adb('shell', 'pidof', 'app.gipny.i2p')
        if not pid: raise RuntimeError('app not running on the phone')
        adb('forward', 'tcp:9222', f'localabstract:webview_devtools_remote_{pid}')
        with urlopen('http://127.0.0.1:9222/json', timeout=5) as r:
            target = next(t for t in json.load(r) if t['type'] == 'page')
        self.socket = connect(target['webSocketDebuggerUrl'], open_timeout=5, close_timeout=2, max_size=None)
        self.cdp = Cdp(self.socket)
    def js(self, expression):
        # The WebView drops an idle DevTools socket after a couple of minutes.
        for attempt in (1, 2):
            try:
                return H.evaluate(self.cdp, expression)
            except Exception as error:
                if attempt == 2 or 'Closed' not in type(error).__name__: raise
                log('phone: DevTools socket closed, reconnecting'); self.connect()
    def invoke(self, cmd, **args):
        return self.js(f'window.__TAURI_INTERNALS__.invoke({json.dumps(cmd)}, {json.dumps(args)})')
    def screen(self):
        return self.js('document.querySelector("#app > * > *")?.className')
    def ensure_main(self):
        if self.js('!!document.querySelector(".main")'): return
        if not PHONE_PROFILE: raise RuntimeError('open a profile on the phone, or set GIPNY_PHONE_PROFILE/GIPNY_PHONE_PASS')
        log('phone: unlocking', PHONE_PROFILE)
        self.js(f'''(async () => {{ const sleep = ms => new Promise(r => setTimeout(r, ms));
          const pb = [...document.querySelectorAll('.auth-card button')].find(b => b.textContent.includes({json.dumps(PHONE_PROFILE)}));
          if (pb) {{ pb.click(); await sleep(1200); }}
          const p = document.querySelector('.auth-card input[type=password]'); if (!p) return 'no password input';
          p.value = {json.dumps(PHONE_PASS)}; p.dispatchEvent(new Event('input', {{bubbles: true}}));
          p.dispatchEvent(new KeyboardEvent('keydown', {{key: 'Enter', bubbles: true}}));
          return 'submitted'; }})()''')
        H.wait_for(lambda: self.js('!!document.querySelector(".main")'), 400, 'phone main screen')
    def close(self):
        try: self.socket.close()
        except Exception: pass
        adb('forward', '--remove', 'tcp:9222')

def main():
    transport = 'wss'
    out = OUT; out.mkdir(parents=True, exist_ok=True)
    snapshot = SNAPSHOT; candidates = snapshot.parent / 'candidates.tsv'
    manifest = json.loads(Path(str(snapshot) + '.json').read_text())
    capability = secrets.token_urlsafe(32); password = secrets.token_urlsafe(24)
    cap_file = out / '.capability'; cap_file.write_text(capability); cap_file.chmod(0o600)
    forwarder = server = chrome = None; phone = None; socket = None
    try:
        flog = (out / 'forwarder.log').open('w')
        forwarder = subprocess.Popen([str(ROOT / 'browser-router/forwarder/target/release/gipny-forwarder'),
            '--host', '127.0.0.1:18767', '--origin', 'http://localhost:18770', '--capability-file', str(cap_file),
            '--candidates', str(candidates), '--allow-fixture-ice-private', '--max-connections', '32',
            '--session-seconds', '600', '--idle-seconds', '120', '--max-bytes', str(64 * 1024 * 1024)],
            cwd=ROOT / 'browser-router', stdout=flog, stderr=subprocess.STDOUT)
        wait_port(18767, process=forwarder)
        server = ThreadingHTTPServer(('127.0.0.1', 18770), H.Assets)
        server.assets = (ROOT / 'ui/browser-dist').resolve(); server.snapshot = snapshot
        server.config = dict(snapshot_url='http://localhost:18770/snapshot.tar.gz', snapshot_sha256=manifest['sha256'],
            forwarder_url='ws://127.0.0.1:18767/stream', capability=capability, transport=transport,
            rtc_ice_policy='all', rtc_ice_servers=[], rtc_fixture_private=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        log('forwarder + assets up')
        private = tempfile.mkdtemp(prefix='gipny-browser-phone-')
        clog = (out / 'chrome.log').open('w')
        chrome = subprocess.Popen(['/usr/bin/google-chrome', '--headless=new', '--no-sandbox', '--disable-dev-shm-usage',
            '--disable-background-networking', '--no-first-run', '--remote-debugging-address=127.0.0.1',
            '--remote-debugging-port=18771', f'--user-data-dir={private}/browser', 'about:blank'], stdout=clog, stderr=subprocess.STDOUT)
        wait_port(18771, process=chrome)
        with urlopen('http://127.0.0.1:18771/json', timeout=5) as r:
            target = next(t for t in json.load(r) if t['type'] == 'page')
        socket = connect(target['webSocketDebuggerUrl'], open_timeout=5, close_timeout=2, max_size=None)
        cdp = Cdp(socket)
        cdp.call('Page.enable'); cdp.call('Runtime.enable'); cdp.call('Page.addScriptToEvaluateOnNewDocument', source=H.BRIDGE)
        cdp.call('Page.navigate', url='http://localhost:18770/browser.html')
        H.wait_for(lambda: H.evaluate(cdp, 'typeof __coreCall === "function" && document.querySelectorAll("input").length > 0'), 60, 'browser UI and worker')
        log('browser UI up; creating profile')
        H.create_ui(cdp, 'browser', password)
        t0 = time.monotonic()
        H.wait_for(lambda: H.ready(cdp), H.STARTUP_WAIT, 'browser destination tunnels')
        log(f'browser destination ready in {round(time.monotonic() - t0)} s; startup failures: {H.STARTUP_FAILURES}')
        b_card, b_addr = H.rpc(cdp, 'my_card'), H.rpc(cdp, 'my_b32')
        log('browser card ok; address', b_addr[:12] + '…')

        phone = Phone(); phone.ensure_main()
        p_card, p_addr = phone.invoke('my_card'), phone.invoke('my_b32')
        p_relay = phone.invoke('get_relay_address') or p_addr
        log('phone card ok; address', p_addr[:12] + '…', 'relay', (p_relay or '')[:12] + '…')

        # browser -> phone: contact request + first message
        b_contact = H.rpc(cdp, 'add_contact', onion=p_addr, signPk=p_card['sign_pk'], dhPk=p_card['dh_pk'], name='Honor', relay=p_relay)
        first = H.rpc(cdp, 'send_message', contactId=b_contact, body='hello from the browser', attachments=[], ttlSecs=None, replyTo=None)
        t1 = time.monotonic()
        H.wait_for(lambda: any(c['sign_pk'] == b_card['sign_pk'] for c in phone.invoke('list_contacts')), 300, 'phone sees the browser contact request')
        p_contact = next(c for c in phone.invoke('list_contacts') if c['sign_pk'] == b_card['sign_pk'])
        log(f'phone got the request after {round(time.monotonic() - t1)} s (request={p_contact.get("request")}); accepting')
        if p_contact.get('request') == 'incoming': phone.invoke('accept_contact_request', id=p_contact['id'])
        H.wait_for(lambda: any(m['body'] == 'hello from the browser' for m in phone.invoke('list_messages', contactId=p_contact['id'], limit=50, beforeId=None)), 300, 'phone decrypted the browser message')
        log(f'phone has the browser message after {round(time.monotonic() - t1)} s')
        H.wait_for(lambda: any(m['id'] == first and m['delivered'] for m in H.rpc(cdp, 'list_messages', contactId=b_contact)), 300, 'browser receipt')
        log('browser got the delivery receipt')

        # phone -> browser
        t2 = time.monotonic()
        reply = phone.invoke('send_message', contactId=p_contact['id'], body='hello from the Honor', attachments=[], ttlSecs=None, replyTo=None)
        H.wait_for(lambda: any(m['body'] == 'hello from the Honor' for m in H.rpc(cdp, 'list_messages', contactId=b_contact)), 300, 'browser decrypted the phone message')
        log(f'browser has the phone message after {round(time.monotonic() - t2)} s')
        H.wait_for(lambda: any(m['id'] == reply and m['delivered'] for m in phone.invoke('list_messages', contactId=p_contact['id'], limit=50, beforeId=None)), 300, 'phone receipt')
        log('phone got the delivery receipt')
        log('RESULT: browser <-> Honor messaging OK')
    finally:
        try:
            if phone: phone.close()
        except Exception: pass
        try:
            if socket: socket.close()
        except Exception: pass
        stop(chrome); stop(forwarder)
        if server: server.shutdown()

if __name__ == '__main__':
    main()
