"""Real browser messenger integration. Execution belongs to GitHub Actions.

Two Chrome data directories own independent OPFS profiles and WASM routers.
CDP calls the production worker RPC; no test bridge ships in the application.
Only public assertions/timings enter artifacts, never passwords, configuration,
private browser directories or CDP/network dumps. A failure also keeps file
part counters and each core's own scrubbed action log (local ids, sizes and
states; no text, names, keys or addresses).
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import json
import mimetypes
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import unquote, urlsplit
from urllib.request import urlopen

from websockets.sync.client import connect
from run_bootstrap_ci import Cdp, stop
from run_ntcp2_ci import wait_port


class Assets(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.headers.get_all('Host') != ['localhost:18770']:
            self.send_error(403)
            return
        name = unquote(urlsplit(self.path).path)
        if name == '/browser-config.json':
            if self.headers.get('Sec-Fetch-Site') != 'same-origin':
                self.send_error(403)
                return
            data = json.dumps(self.server.config).encode()
            mime = 'application/json'
        elif name == '/snapshot.tar.gz':
            data, mime = self.server.snapshot.read_bytes(), 'application/gzip'
        else:
            path = (self.server.assets / name.lstrip('/')).resolve()
            if not path.is_relative_to(self.server.assets) or not path.is_file():
                self.send_error(404)
                return
            data = path.read_bytes()
            mime = 'application/wasm' if path.suffix == '.wasm' else mimetypes.guess_type(path)[0] or 'application/octet-stream'
        self.send_response(200)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        self.send_header('X-Frame-Options', 'DENY')
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass


BRIDGE = r"""
(() => {
  const OriginalWorker = globalThis.Worker;
  let worker, url, options, next = -1;
  const pending = new Map();
  globalThis.__coreFailures = [];
  // Public stage counters only. Never retain frame contents or call IDs.
  const counts = () => ({invite: 0, accept: 0, probe: 0, echo: 0, audio: 0, ack: 0, end: 0});
  globalThis.__voiceCounts = {submitted: counts(), received: counts()};
  const countFrame = (direction, frame) => {
    const kind = JSON.parse(frame).kind;
    const counters = globalThis.__voiceCounts[direction];
    if (Object.hasOwn(counters, kind)) counters[kind]++;
  };

  const attach = target => {
    worker = target;
    target.addEventListener('message', event => {
      const message = event.data;
      const frame = message.event === 'core_event' && message.payload?.CallFrame?.frame;
      if (frame) countFrame('received', frame);
      const failure = message.event === 'core_event' && message.payload?.MessageFailed?.reason;
      if (typeof failure === 'string') {
        globalThis.__coreFailures.push(failure.slice(0, 300));
        globalThis.__coreFailures = globalThis.__coreFailures.slice(-6);
      }
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      if (message.error) waiter.reject(new Error(message.error));
      else waiter.resolve(message.result);
    });
  };
  globalThis.Worker = class extends OriginalWorker {
    constructor(location, settings) {
      super(location, settings);
      if (settings?.name === 'gipny-core') { url = location; options = settings; attach(this); }
    }
    postMessage(request, ...options) {
      if (this === worker && request?.command === 'send_call_frame') countFrame('submitted', request.args.frame);
      return super.postMessage(request, ...options);
    }
  };
  globalThis.__coreCall = (command, args = {}) => new Promise((resolve, reject) => {
    if (!worker) return reject(new Error('Production worker is not ready'));
    const id = next--;
    pending.set(id, { resolve, reject });
    worker.postMessage({ id, command, args });
  });
  globalThis.__restartCore = async () => {
    worker.terminate();
    for (const waiter of pending.values()) waiter.reject(new Error('Worker restarted'));
    pending.clear();
    attach(new OriginalWorker(url, options));
    const response = await fetch('/browser-config.json', { cache: 'no-store' });
    await __coreCall('init', { config: await response.json() });
  };
})();
"""


def evaluate(cdp, expression):
    result = cdp.call('Runtime.evaluate', expression=expression, awaitPromise=True, returnByValue=True, userGesture=True)
    if 'exceptionDetails' in result:
        # No expression/args or full exception object can enter evidence.
        detail = result['exceptionDetails'].get('exception', {}).get('description', 'browser operation failed')
        raise RuntimeError(detail.split('\n')[0][:300])
    return result.get('result', {}).get('value')


def rpc(cdp, command, **args):
    return evaluate(cdp, f'__coreCall({json.dumps(command)}, {json.dumps(args)})')


def recovery_metrics(client):
    # A returned peer has a new router. The continuing peer's counters include
    # earlier successful calls and files, so take baselines for both timelines.
    try:
        counters = rpc(client, 'browser_diagnostics')['counters']
        names = (
            'client_stream_syn_created', 'client_stream_syn_received',
            'client_stream_syn_ack_queued', 'client_stream_syn_ack_received',
            'client_stream_syn_rejected', 'client_lease_refresh_requested',
            'client_lease_query_success', 'client_newer_lease_received',
            'client_lease_encryption_key_changed', 'client_stale_lease_query_ignored',
            'client_stale_encryption_key_ignored', 'client_stale_garlic_lease_ignored',
        )
        return {name: int(counters.get(name, 0)) for name in names}
    except Exception:
        return None


def wait_for(check, seconds, label):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(2)
    raise RuntimeError(f'deadline: {label}')


STARTUP_FAILURES = []
# One failed start (its own 300 s deadline), the client's 30 s retry delay and a
# second start: what a person waits on a bad network before it gives up.
STARTUP_WAIT = 300 + 30 + 300 + 30
# After an unblock Alice may only re-encrypt once resendDelay(0) = 120 s has
# passed (ui/src/browser-core/model.ts). One delivery attempt is bounded by the
# 90 s connect deadline (browser-router/src/network.rs) plus two 60 s relay
# answers (ui/src/browser-core/deadline.ts), and retryDelay() adds at most 60 s
# before the next one. The receipt leg has no resend delay but the same bound,
# so it gets its own deadline: a late receipt fails as "receipt late", not as a
# retry that never happened (CI 2026-11-07: the letter arrived 158 s after the
# unblock, the receipt's fifth attempt was due 9 s after a single 300 s budget).
RESEND_WAIT = 120 + 2 * (90 + 60 + 60) + 60
RECEIPT_WAIT = 2 * (90 + 60 + 60) + 60


def ready(cdp):
    info = rpc(cdp, 'get_relay_info')
    if info['hosted']['state'] in ('failed', 'unconfigured'):
        # `failed`: the client restarts its network 30 s later by itself.
        # `unconfigured`: the page got no browser-config.json, nothing will
        # retry, and waiting out STARTUP_WAIT would hide a fixture fault.
        reason = 'I2P client startup failed: ' + info['hosted']['reason'][:300]
        if not STARTUP_FAILURES or STARTUP_FAILURES[-1] != reason:
            STARTUP_FAILURES.append(reason)
        return False
    return info['hosted']['state'] == 'ready'


def files_complete(cdp, message_id):
    progress = rpc(cdp, 'files_progress', messageId=message_id)
    return bool(progress) and all(item['status'] == 'complete' for item in progress)


def file_progress(cdp, message_id):
    return [[item['status'], item['done'], item['total']] for item in rpc(cdp, 'files_progress', messageId=message_id)]


def creation_step(cdp, step):
    return evaluate(cdp, f'''(() => {{
      const visible = [...document.querySelectorAll('.auth-create-step')]
        .filter(panel => !panel.hidden && panel.getBoundingClientRect().height > 0);
      return visible.length === 1 && visible[0].dataset.step === {json.dumps(str(step))};
    }})()''')


def setup_ui(cdp):
    # Fresh browser directories must exercise the real first-run language and
    # keyboard screen; its checkboxes are not a profile creation form.
    wait_for(lambda: evaluate(cdp, '!!document.querySelector(".language-setup > .btn")'), 30, 'first-run language setup')
    evaluate(cdp, '''(() => {
      const setup = document.querySelector('.language-setup');
      const english = setup.querySelector('.setup-layouts input[value=en]');
      if (!english?.checked || !english.disabled) throw new Error('Required English keyboard is missing');
      const button = setup.querySelector(':scope > .btn');
      if (button.disabled || button.getBoundingClientRect().height === 0) throw new Error('Language setup action is unavailable');
      button.click();
      if (document.querySelector('.language-setup') || localStorage.getItem('gipny.setup') !== 'done')
        throw new Error('Language setup did not complete');
    })()''')
    wait_for(lambda: creation_step(cdp, 1), 30, 'profile name step')


def create_ui(cdp, profile, password):
    # Exercise all shipped wizard steps and the Store/API adapter. `profile`
    # is already its own slug, matching the on-device name derived by the UI.
    wait_for(lambda: creation_step(cdp, 1), 30, 'profile name step')
    values = json.dumps({'create-name': profile, 'create-password': password, 'create-confirm': password})
    for step, fields in [(1, ['create-name']), (2, ['create-password', 'create-confirm']), (3, [])]:
        if not creation_step(cdp, step):
            raise RuntimeError(f'Profile creation step {step} is not visible')
        evaluate(cdp, f'''(() => {{
          const values = {values};
          for (const id of {json.dumps(fields)}) {{
            const field = document.getElementById(id);
            if (!field || field.getBoundingClientRect().height === 0) throw new Error('Profile field is not visible');
            field.value = values[id]; field.dispatchEvent(new Event('input', {{bubbles:true}}));
          }}
          const button = document.querySelector('.salon-create .auth-actions .btn:not(.btn-ghost)');
          if (!button || button.disabled || button.getBoundingClientRect().height === 0)
            throw new Error('Profile creation action is unavailable');
          button.click();
        }})()''')
        if step < 3:
            wait_for(lambda: creation_step(cdp, step + 1), 10, f'profile creation step {step + 1}')
            if rpc(cdp, 'vault_status', profile=profile)['unlocked']:
                raise RuntimeError('Profile opened before the final creation step')
    wait_for(lambda: rpc(cdp, 'vault_status', profile=profile)['unlocked'], 30, 'production profile creation form')


def remembered_profile_ready(cdp, profile):
    label = json.dumps(profile)
    return evaluate(cdp, f'''(() => {{
      const password = document.querySelector('#unlock-password');
      return password?.getBoundingClientRect().height > 0
        && document.querySelector('.login-cyber .auth-title')?.textContent === {label}
        && !document.querySelector('.main');
    }})()''')


def unlock_ui(cdp, profile, password, wrong_password='incorrect password'):
    cdp.call('Page.navigate', url='http://localhost:18770/browser.html')
    label = json.dumps(profile)
    wait_for(lambda: remembered_profile_ready(cdp, profile), 30, 'remembered profile remains locked after reload')
    evaluate(cdp, 'document.querySelector(".auth-other-profiles").click()')
    wait_for(lambda: evaluate(cdp, f'[...document.querySelectorAll(".salon-profile")].some(button=>button.textContent === {label})'), 30, 'saved profile selector')
    evaluate(cdp, f'[...document.querySelectorAll(".salon-profile")].find(button=>button.textContent === {label}).click()')
    wait_for(lambda: evaluate(cdp, '''(() => {
      const button = document.querySelector('.login-cyber .auth-actions .btn:not(.btn-ghost)');
      return !!document.querySelector('#unlock-password') && button && !button.disabled;
    })()'''), 10, 'profile password form')
    wrong_rejected = evaluate(cdp, '__coreCall("vault_unlock", {profile:' + json.dumps(profile) + ',pass:' + json.dumps(wrong_password) + '}).then(()=>false,()=>true)')
    if not wrong_rejected:
        raise RuntimeError('wrong password accepted')
    evaluate(cdp, f'''(() => {{
      document.querySelector('#unlock-password').value = {json.dumps(password)};
      document.querySelector('.login-cyber .auth-actions .btn:not(.btn-ghost)').click();
    }})()''')
    wait_for(lambda: rpc(cdp, 'vault_status', profile=profile)['unlocked'], 30, 'production unlock form')


def check_view_once(alice, bob, alice_contact, bob_contact):
    body = secrets.token_hex(5200)  # Over two 4 KiB parts even before authenticated encryption.
    # Enqueue returns a local ID before the worker delivers the parts. Both
    # enqueue and the appearance of the peer's notice share the exchange budget.
    deadline = time.monotonic() + 230
    args = json.dumps(dict(contactId=alice_contact, body=body, ttlSecs=300))
    evaluate(alice, f"""delete globalThis.__viewOnceSent; void (__coreCall('send_view_once', {args}).then(
      id => {{ globalThis.__viewOnceSent = {{id}}; }},
      () => {{ globalThis.__viewOnceSent = {{failed:true}}; }}))""")
    result = wait_for(lambda: evaluate(alice, 'globalThis.__viewOnceSent || null'),
                      deadline - time.monotonic(), 'view-once enqueue')
    if result.get('failed'):
        raise RuntimeError('sender could not queue the view-once message')
    item = result['id']

    def received_notice():
        pending = rpc(bob, 'list_view_once', contactId=bob_contact)
        return next((entry for entry in pending if entry.get('id') == item), None)

    notice = wait_for(received_notice, deadline - time.monotonic(), 'view-once recipient notice')
    if set(notice) != {'id', 'contact_id', 'sent_at', 'expires_at'} or notice['contact_id'] != bob_contact:
        raise RuntimeError('view-once receiver did not list a metadata-only notice')
    for client, contact in [(alice, alice_contact), (bob, bob_contact)]:
        if any(body in row.get('body', '') for row in rpc(client, 'list_messages', contactId=contact)):
            raise RuntimeError('view-once text entered ordinary message history')
    # Keep contents in the browser; return only the concurrent-open assertion.
    checked = evaluate(bob, f"""(async () => {{
      const outcomes = await Promise.allSettled([1,2].map(() => __coreCall('open_view_once', {{id:{json.dumps(item)}}})));
      return outcomes.filter(x => x.status === 'rejected').length === 1 &&
        outcomes.some(x => x.status === 'fulfilled' && x.value === {json.dumps(body)});
    }})()""")
    if not checked or any(entry['id'] == item for entry in rpc(bob, 'list_view_once', contactId=bob_contact)):
        raise RuntimeError('view-once concurrent opens did not consume the message exactly once')


def check_voice(alice, bob):
    def click_voice(cdp, selector):
        return evaluate(cdp, "(() => {const b=document.querySelector(" + json.dumps(selector) + ");if(!b || b.hidden)return false;b.click();return true})()")

    wait_for(lambda: evaluate(alice, """(() => {
      const row = [...document.querySelectorAll('.contact')].find(el => el.querySelector('.contact-name')?.textContent === 'Bob');
      if (row) row.click(); return !!row;
    })()"""), 30, 'caller opens accepted contact')
    wait_for(lambda: click_voice(alice, '.voice-call'), 30, 'start voice call')
    wait_for(lambda: click_voice(bob, '.voice-answer'), 30, 'answer voice call')
    wait_for(lambda: all(evaluate(cdp, '__voiceCounts.received.audio >= 2 && __voiceCounts.received.ack >= 2') for cdp in (alice, bob)), 60, 'bidirectional voice over I2P')
    if not click_voice(alice, '.voice-end'):
        raise RuntimeError('caller lost the active voice call')
    wait_for(lambda: evaluate(bob, "!document.querySelector('.voice-call-panel')"), 30, 'remote hangup')


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise SystemExit('Run browser integration only in GitHub Actions')
    root = Path(__file__).resolve().parents[1]
    transport = os.environ.get('GIPNY_BROWSER_TRANSPORT', 'wss')
    if transport not in ('wss', 'webrtc'):
        raise SystemExit('unsupported test transport')
    output = Path(os.environ['RUNNER_TEMP']) / ('browser-messenger-webrtc' if transport == 'webrtc' else 'browser-messenger')
    output.mkdir(exist_ok=True)
    snapshot = Path(os.environ['RUNNER_TEMP']) / 'browser-bootstrap/snapshot.tar.gz'
    candidates = snapshot.parent / 'candidates.tsv'
    manifest = json.loads(Path(str(snapshot) + '.json').read_text())
    capability = secrets.token_urlsafe(32)
    password_a, password_b = secrets.token_urlsafe(24), secrets.token_urlsafe(24)
    password_new = secrets.token_urlsafe(24)
    private_secrets = (capability, password_a, password_b, password_new)
    for secret in private_secrets:
        print(f'::add-mask::{secret}', flush=True)
    cap_file = output / '.capability'
    cap_file.write_text(capability)
    cap_file.chmod(0o600)
    receipt = dict(transport=transport, turn_relay_tested=False, stage='starting', physical_phone=False, independent_browser_profiles=2, assertions=[])
    browsers, sockets, logs, clients = [], [], [], []
    watched = {}
    forwarder = server = None
    started = time.monotonic()
    try:
        log = (output / 'forwarder.log').open('w'); logs.append(log)
        forwarder = subprocess.Popen([str(root / 'forwarder/target/release/gipny-forwarder'),
            '--host', '127.0.0.1:18767', '--origin', 'http://localhost:18770',
            '--capability-file', str(cap_file), '--candidates', str(candidates),
            '--allow-fixture-ice-private', '--max-connections', '32', '--session-seconds', '600', '--idle-seconds', '120',
            '--max-bytes', str(64 * 1024 * 1024)], cwd=root, stdout=log, stderr=subprocess.STDOUT)
        wait_port(18767, process=forwarder)
        server = ThreadingHTTPServer(('127.0.0.1', 18770), Assets)
        server.assets = (root.parent / 'ui/browser-dist').resolve()
        server.snapshot = snapshot
        server.config = dict(snapshot_url='http://localhost:18770/snapshot.tar.gz',
            snapshot_sha256=manifest['sha256'], forwarder_url='ws://127.0.0.1:18767/' + ('rtc' if transport == 'webrtc' else 'stream'), capability=capability,
            transport=transport, rtc_ice_policy='all', rtc_ice_servers=[], rtc_fixture_private=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        browser = next((shutil.which(name) for name in ('google-chrome', 'chromium', 'chromium-browser') if shutil.which(name)), None)
        if browser is None:
            raise RuntimeError('Chrome is required')
        with tempfile.TemporaryDirectory(prefix='gipny-messenger-private-') as private:
            clients = []
            for name, port in [('alice', 18771), ('bob', 18772)]:
                log = (output / f'chrome-{name}.log').open('w'); logs.append(log)
                process = subprocess.Popen([browser, '--headless=new', '--no-sandbox', '--disable-dev-shm-usage',
                    '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
                    '--autoplay-policy=no-user-gesture-required', '--disable-background-networking', '--no-first-run', '--disable-features=WebRtcHideLocalIpsWithMdns', '--remote-debugging-address=127.0.0.1',
                    f'--remote-debugging-port={port}', f'--user-data-dir={private}/{name}', 'about:blank'], stdout=log, stderr=subprocess.STDOUT)
                browsers.append(process)
                wait_port(port, process=process)
                with urlopen(f'http://127.0.0.1:{port}/json', timeout=5) as response:
                    target = next(item for item in json.load(response) if item['type'] == 'page')
                socket = connect(target['webSocketDebuggerUrl'], open_timeout=5, close_timeout=2)
                sockets.append(socket)
                cdp = Cdp(socket); clients.append(cdp)
                cdp.call('Page.enable'); cdp.call('Runtime.enable')
                cdp.call('Page.addScriptToEvaluateOnNewDocument', source=BRIDGE)
                cdp.call('Page.navigate', url='http://localhost:18770/browser.html')
                setup_ui(cdp)
            alice, bob = clients
            # Exercise the actual main-thread passkey helper with Chrome's
            # virtual PRF authenticator. This is not a physical-device claim.
            alice.call('WebAuthn.enable', enableUI=False)
            alice.call('WebAuthn.addVirtualAuthenticator', options=dict(protocol='ctap2', ctap2Version='ctap2_1', transport='internal', hasResidentKey=True, hasUserVerification=True, hasPrf=True, automaticPresenceSimulation=True, isUserVerified=True))
            rpc(alice, 'init', config=None)
            create_ui(alice, 'alice', password_a)
            evaluate(alice, 'new Promise((resolve,reject)=>window.dispatchEvent(new CustomEvent("gipny-enroll-passkey",{detail:{profile:"alice",done:error=>error?reject(error):resolve(true)}})))')
            rpc(alice, 'vault_lock')
            evaluate(alice, 'fetch("/browser-config.json",{cache:"no-store"}).then(r=>r.json()).then(config=>__coreCall("init",{config}))')
            evaluate(alice, 'new Promise((resolve,reject)=>window.dispatchEvent(new CustomEvent("gipny-unlock-passkey",{detail:{profile:"alice",done:error=>error?reject(error):resolve(true)}})))')
            receipt['assertions'].append('actual WebAuthn PRF helper enrollment and unlock with a virtual CTAP2 authenticator')
            create_ui(bob, 'bob', password_b)
            # This scenario verifies saved history after process loss. Both fixture
            # accounts opt in; ordinary profiles default to clearing local history.
            for client in (alice, bob):
                rpc(client, 'set_ui_data', key='chat_prefs', json=json.dumps({'clearOnLock': False}))
            for client in clients:
                wait_for(lambda: ready(client), STARTUP_WAIT, 'client destination tunnels')
            receipt['assertions'].append('two independent WASM client destinations ready over live I2P')
            # link_stats reads the same WASM adapter constants used for client
            # pools. This records configured policy, not measured path hops
            # or an independent observation of padding on the wire.
            receipt['application_tunnel_policy'] = []
            for client in clients:
                stats = rpc(client, 'link_stats')
                policy = {key: stats.get(key) for key in ('our_hops', 'their_hops', 'padded')}
                receipt['application_tunnel_policy'].append(policy)
                if policy != dict(our_hops=2, their_hops=2, padded=True):
                    raise RuntimeError('Ready client reported an unexpected application tunnel policy')
            receipt['assertions'].append('both ready clients report configured 2/2 application tunnel policy and padding')
            print('MESSENGER_PROGRESS live destinations ready', flush=True)
            alice_card, bob_card = rpc(alice, 'my_card'), rpc(bob, 'my_card')
            alice_address, bob_address = rpc(alice, 'my_b32'), rpc(bob, 'my_b32')
            alice_contact = rpc(alice, 'add_contact', i2pAddress=bob_address, signPk=bob_card['sign_pk'], dhPk=bob_card['dh_pk'], name='Bob', relay=bob_address)
            # A file-only introduction must retain its offer before Bob accepts,
            # without downloading the file or prematurely reporting delivery.
            introduction = evaluate(alice, '(async()=>{const data=new Uint8Array(262151).fill(73);return __coreCall("send_message",{contactId:' + str(alice_contact) + ',body:"",browserFiles:[new File([data],"introduction.bin")],ttlSecs:null,replyTo:null})})()')
            watched['introduction sender'] = (alice, introduction)
            wait_for(lambda: any(c['sign_pk'] == alice_card['sign_pk'] and c['request'] == 'incoming' for c in rpc(bob, 'list_contacts')), 150, 'incoming contact request with a file')
            bob_contact = next(c['id'] for c in rpc(bob, 'list_contacts') if c['sign_pk'] == alice_card['sign_pk'])
            wait_for(lambda: any(row['body'] == '' for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'file-only introduction retained')
            received_intro = next(row['id'] for row in rpc(bob, 'list_messages', contactId=bob_contact) if row['body'] == '')
            watched['introduction receiver'] = (bob, received_intro)
            # The UI-facing status of an offer held until acceptance is
            # 'waiting' (the internal transfer state is 'offered').
            if rpc(bob, 'list_attachments', messageId=received_intro) or not any(item['status'] == 'waiting' for item in rpc(bob, 'files_progress', messageId=received_intro)):
                raise RuntimeError('unaccepted request allocated a file or lost its offer')
            if any(row['id'] == introduction and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)):
                raise RuntimeError('unaccepted introduction was acknowledged')
            rpc(bob, 'accept_contact_request', id=bob_contact)
            wait_for(lambda: len(rpc(bob, 'list_attachments', messageId=received_intro)) == 1, 180, 'file-only request received after acceptance')
            wait_for(lambda: any(row['id'] == introduction and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'accepted introduction receipt')
            wait_for(lambda: files_complete(alice, introduction), 120, 'accepted introduction final file receipt')
            receipt['assertions'].append('file-only contact request retains bounded offer without file allocation or content ACK, then resumes after acceptance')
            first = rpc(alice, 'send_message', contactId=alice_contact, body='browser live alpha', attachments=[], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['body'] == 'browser live alpha' for row in rpc(bob, 'list_messages', contactId=bob_contact)), 150, 'Alice to Bob decrypted message')
            wait_for(lambda: any(row['id'] == first and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'authenticated end-to-end receipt')
            reply = rpc(bob, 'send_message', contactId=bob_contact, body='browser live beta', attachments=[dict(name='hello.txt', data='aGVsbG8=')], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['body'] == 'browser live beta' for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'Bob to Alice ratchet reply')
            wait_for(lambda: any(row['id'] == reply and row['delivered'] for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'reply receipt')
            received_reply = next(row for row in rpc(alice, 'list_messages', contactId=alice_contact) if row['body'] == 'browser live beta')
            wait_for(lambda: len(rpc(alice, 'list_attachments', messageId=received_reply['id'])) == 1, 150, 'verified attachment saved in OPFS')
            attachments = rpc(alice, 'list_attachments', messageId=received_reply['id'])
            if len(attachments) != 1 or attachments[0]['name'] != 'hello.txt':
                raise RuntimeError('attachment metadata did not survive delivery')
            if rpc(alice, 'load_attachment', attachmentId=attachments[0]['id']) != 'aGVsbG8=':
                raise RuntimeError('attachment bytes changed during delivery')
            receipt['assertions'].append('bidirectional shared X3DH/Double Ratchet, end-to-end delivery receipts and authenticated multipart attachment')
            print('MESSENGER_PROGRESS bidirectional messages delivered', flush=True)
            try:
                check_view_once(alice, bob, alice_contact, bob_contact)
                receipt['assertions'].append('live view-once I2P exchange, metadata-only notification and single successful concurrent open')
            except (RuntimeError, TimeoutError) as error:
                receipt['view_once_error'] = str(error)[:500]
            # A media failure still fails this run, while healthy profiles can
            # exercise the independent file, outbox and host-recovery checks.
            try:
                check_voice(alice, bob)
            except RuntimeError as error:
                receipt['voice_error'] = str(error)[:500]
            receipt['voice_frames'] = {name: evaluate(client, '__voiceCounts') for name, client in zip(('alice', 'bob'), clients)}
            if 'voice_error' in receipt:
                for client in clients:
                    evaluate(client, "document.querySelector('.voice-end')?.click()")
                wait_for(lambda: all(evaluate(client, "!document.querySelector('.voice-call-panel')") for client in clients), 10, 'voice cleanup')
            else:
                receipt['assertions'].append('bidirectional microphone frames and frame acknowledgements through production I2P routers')
            large_size = 5 * 1024 * 1024 + 19
            large_id = evaluate(bob, '(async()=>{const data=new Uint8Array(' + str(large_size) + ');for(let i=0;i<data.length;i++)data[i]=i%251;return __coreCall("send_message",{contactId:' + str(bob_contact) + ',body:"multipart fixture",browserFiles:[new File([data],"fixture.bin")],ttlSecs:null,replyTo:null})})()')
            wait_for(lambda: any(row['body'] == 'multipart fixture' for row in rpc(alice, 'list_messages', contactId=alice_contact)), 150, 'multipart offer delivered')
            incoming_large = next(row for row in rpc(alice, 'list_messages', contactId=alice_contact) if row['body'] == 'multipart fixture')['id']
            watched['large sender'], watched['large receiver'] = (bob, large_id), (alice, incoming_large)
            # Live throughput varies widely on CI: sample how far the parts got.
            samples = receipt['large_file_samples'] = []
            def large_saved():
                elapsed = time.monotonic() - sampled_from
                if elapsed >= 20 * len(samples):
                    samples.append([round(elapsed)] + [file_progress(*watched[side]) for side in ('large sender', 'large receiver')])
                    print('MESSENGER_PROGRESS large file', json.dumps(samples[-1]), flush=True)
                return len(rpc(alice, 'list_attachments', messageId=incoming_large)) == 1
            sampled_from = time.monotonic()
            wait_for(large_saved, 300, 'multipart file verified and saved')
            large_attachment = rpc(alice, 'list_attachments', messageId=incoming_large)[0]
            expected_digest = hashlib.sha256(bytes(i % 251 for i in range(large_size))).hexdigest()
            actual_digest = evaluate(alice, '(async()=>{const text=await __coreCall("load_attachment",{attachmentId:' + str(large_attachment['id']) + '});const bytes=Uint8Array.from(atob(text),c=>c.charCodeAt(0));return [...new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))].map(b=>b.toString(16).padStart(2,"0")).join("")})()')
            if actual_digest != expected_digest:
                raise RuntimeError('multipart attachment hash mismatch')
            wait_for(lambda: files_complete(bob, large_id), 120, 'final file receipt')
            receipt['assertions'].append('5 MiB attachment exceeds old per-message/profile limits; streaming OPFS storage and end-to-end SHA256/final receipt')
            stream_result = evaluate(alice, '(async()=>{const info=await __coreCall("attachment_download_info",{attachmentId:' + str(large_attachment['id']) + '});const reg=await navigator.serviceWorker.ready;const channel=new MessageChannel();const token=crypto.randomUUID();let pulls=0;const ready=new Promise((resolve,reject)=>{channel.port1.onmessage=async e=>{if(e.data.ready)resolve(true);else if(e.data.error)reject(new Error(e.data.error));else if(Number.isInteger(e.data.pull)){pulls++;const bytes=await __coreCall("attachment_part",{attachmentId:' + str(large_attachment['id']) + ',index:e.data.pull});channel.port1.postMessage({index:e.data.pull,bytes},[bytes.buffer])}}});reg.active.postMessage({kind:"gipny-download",token,...info},[channel.port2]);await ready;const url=new URL("__gipny_download/"+token,reg.scope);const response=await fetch(url);const bytes=await response.arrayBuffer();const digest=[...new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))].map(b=>b.toString(16).padStart(2,"0")).join("");const replay=await fetch(url);channel.port1.close();return {digest,pulls,replay:replay.status,cached:!!(await caches.match(url))}})()')
            if stream_result['digest'] != expected_digest or stream_result['pulls'] != 27 or stream_result['replay'] != 404 or stream_result['cached']:
                raise RuntimeError('streaming service-worker download failed bounds, integrity, one-use or no-cache checks')
            receipt['assertions'].append('streaming download uses bounded pulls, verifies bytes, rejects URL replay and never enters CacheStorage')

            rpc(bob, 'update_contact', id=bob_contact, name='Alice', trust=2)
            outbound_blocked = evaluate(bob, '__coreCall("send_message",{contactId:' + str(bob_contact) + ',body:"blocked outbound",attachments:[],ttlSecs:null,replyTo:null}).then(()=>false,()=>true)')
            if not outbound_blocked:
                raise RuntimeError('blocked contact accepted an outgoing message')
            held = rpc(alice, 'send_message', contactId=alice_contact, body='held while blocked', attachments=[], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['id'] == held and row['sent'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'blocked mailbox deposit')
            if any(row['body'] == 'held while blocked' for row in rpc(bob, 'list_messages', contactId=bob_contact)):
                raise RuntimeError('blocked incoming contact entered history')
            rpc(bob, 'update_contact', id=bob_contact, name='Alice', trust=1)
            # The blocked mailbox answered Deposited and dropped the letter, so
            # Alice encrypts it again only after resendDelay(0) = 120 s; Bob's
            # receipt then travels back on its own budget.
            wait_for(lambda: any(row['body'] == 'held while blocked' for row in rpc(bob, 'list_messages', contactId=bob_contact)), RESEND_WAIT, 'unblocked exact ciphertext retry')
            wait_for(lambda: any(row['id'] == held and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), RECEIPT_WAIT, 'receipt after unblock')
            receipt['assertions'].append('blocked contacts cannot send or enter history; unblocking resumes exact ciphertext delivery')
            rpc(alice, 'change_passphrase', **{'old': password_a, 'new': password_new})
            # Stop Alice's whole worker: this discards WASM memory and sockets.
            rpc(alice, 'vault_lock')
            mirrored = evaluate(alice, '''(async()=>{
              const root = await (await navigator.storage.getDirectory()).getDirectoryHandle('gipny-browser-v1');
              for await (const directory of root.values()) {
                if (directory.kind !== 'directory') continue;
                const label = JSON.parse(await (await (await directory.getFileHandle('profile.json')).getFile()).text());
                if (label.name !== 'alice') continue;
                const headers = [];
                for (const slot of ['a','b']) {
                  const bytes = await (await (await directory.getFileHandle('vault-'+slot+'.bin')).getFile()).arrayBuffer();
                  const length = new DataView(bytes).getUint32(16,true);
                  headers.push(JSON.parse(new TextDecoder().decode(new Uint8Array(bytes,20,length))));
                }
                return JSON.stringify(headers[0].password) === JSON.stringify(headers[1].password) && JSON.stringify(headers[0].salt) === JSON.stringify(headers[1].salt);
              }
              return false;
            })()''')
            if not mirrored:
                raise RuntimeError('password change left an old credential wrapper in a live OPFS slot')
            recovery = receipt['outbox_recovery_metrics'] = {
                'before_send': {'bob': recovery_metrics(bob)}
            }
            offline = rpc(bob, 'send_message', contactId=bob_contact, body='durable offline outbox', attachments=[], ttlSecs=None, replyTo=None)
            time.sleep(3)
            unlock_ui(alice, 'alice', password_new, wrong_password=password_a)
            if rpc(alice, 'my_b32') != alice_address or rpc(alice, 'my_card') != alice_card:
                raise RuntimeError('destination or identity changed after worker restart')
            receipt['assertions'].append('password rotation mirrored across OPFS slots; old password rejected; identity and destination restored after page/worker restart')
            wait_for(lambda: ready(alice), STARTUP_WAIT, 'restored client destination')
            print('MESSENGER_PROGRESS restored client destination ready', flush=True)
            end = time.monotonic() + 300
            recovery['destination_ready'] = {'alice': recovery_metrics(alice), 'bob': recovery_metrics(bob)}
            # Alice's new router publishes new tunnels; Bob's copy of her old
            # lease set fails first (dead streams, CANT_REACH_PEER) and his
            # retries back off up to 60 s between attempts.
            try:
                wait_for(lambda: any(row['body'] == 'durable offline outbox' for row in rpc(alice, 'list_messages', contactId=alice_contact)), end - time.monotonic(), 'offline outbox recovered recipient')
            finally:
                recovery['recipient_wait_end'] = {'alice': recovery_metrics(alice), 'bob': recovery_metrics(bob)}
            wait_for(lambda: any(row['id'] == offline and row['delivered'] for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'offline message delivery receipt')
            rows = rpc(alice, 'list_messages', contactId=alice_contact)
            if len([row for row in rows if row['body'] == 'durable offline outbox']) != 1:
                raise RuntimeError('retry duplicated history')
            reopened_digest = evaluate(alice, '(async()=>{const text=await __coreCall("load_attachment",{attachmentId:' + str(large_attachment['id']) + '});const bytes=Uint8Array.from(atob(text),c=>c.charCodeAt(0));return [...new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))].map(b=>b.toString(16).padStart(2,"0")).join("")})()')
            if reopened_digest != expected_digest:
                raise RuntimeError('external attachment did not survive password rotation/restart')
            receipt['assertions'].append('worker termination, password rotation across both slots, old-password rejection, identity/destination/ratchet/history restoration, offline outbox retry without duplicate history')
            for client in clients:
                rpc(client, 'vault_lock')
            wait_for(lambda: evaluate(alice, 'navigator.serviceWorker.ready.then(()=>!!navigator.serviceWorker.controller)'), 20, 'offline shell installed')
            # Names the offending cache: keys and paths here are public.
            cache_boundary = evaluate(alice, r'''(async()=>{
              let configuration=false, peers=false;
              const paths=requests=>requests.map(request=>new URL(request.url).pathname).join(',').slice(0,200);
              for(const key of await caches.keys()) {
                const cache=await caches.open(key), requests=await cache.keys();
                if(key==='gipny-public-network-v1') {
                  if(requests.length!==1 || !requests[0].url.endsWith('/browser-config.json')) return key+' holds '+paths(requests);
                  configuration=true;
                } else if(key==='gipny-public-netdb-v1') {
                  if(requests.length!==1 || !/\/__gipny-public-netdb\/v1\/[0-9a-f]{64}$/i.test(new URL(requests[0].url).pathname)) return key+' holds '+paths(requests);
                  if((await (await cache.match(requests[0])).arrayBuffer()).byteLength>4*1024*1024) return key+' oversize';
                  peers=true;
                } else if(key==='gipny-public-netdb-live-v1') {
                  // Optional (and may be empty) until the router first saves what it learned.
                  if(requests.length>1 || requests.some(request=>new URL(request.url).pathname!=='/__gipny-public-netdb/live/v1')) return key+' holds '+paths(requests);
                  if(requests.length && (await (await cache.match(requests[0])).arrayBuffer()).byteLength>4*1024*1024) return key+' oversize';
                } else if(!key.startsWith('gipny-shell-')) return 'unexpected cache '+key.slice(0,80);
                else if(requests.some(request=>/browser-config|snapshot|vault|attachments/.test(new URL(request.url).pathname))) return key+' holds '+paths(requests.filter(request=>/browser-config|snapshot|vault|attachments/.test(new URL(request.url).pathname)));
              }
              return !configuration ? 'no configuration cache' : !peers ? 'no netDb snapshot cache' : 'ok';
            })()''')
            if cache_boundary != 'ok':
                raise RuntimeError('public network caches missing or mixed with application/private storage: ' + str(cache_boundary)[:300])
            # Stop the actual web server: an emulated network label alone does
            # not prove that service-worker/worker assets work offline.
            server.shutdown(); server.server_close(); server = None
            alice.call('Page.navigate', url='http://localhost:18770/')
            wait_for(lambda: remembered_profile_ready(alice, 'alice'), 30, 'offline root navigation and locked remembered profile')
            unlock_ui(alice, 'alice', password_new)
            wait_for(lambda: evaluate(alice, '!!document.querySelector(".boot-enter:not(.hidden)") || !!document.querySelector(".sidebar")'), 20, 'offline history entry')
            evaluate(alice, 'document.querySelector(".boot-enter:not(.hidden)")?.click()')
            wait_for(lambda: evaluate(alice, '!!document.querySelector(".contact-list .contact")'), 20, 'offline contact list')
            evaluate(alice, 'document.querySelector(".contact-list .contact").click()')
            wait_for(lambda: evaluate(alice, '[...document.querySelectorAll(".msg-body")].some(element=>element.textContent.includes("durable offline outbox"))'), 20, 'offline transcript rendered')
            # The independent bridge remains available. Both fresh WASM routers
            # must reconnect and deliver while the application host stays down.
            unlock_ui(bob, 'bob', password_b)
            wait_for(lambda: ready(alice) and ready(bob), STARTUP_WAIT + 30, 'both routers reconnect without application hosting')
            no_host = rpc(bob, 'send_message', contactId=bob_contact, body='delivery without application hosting', attachments=[], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['body'] == 'delivery without application hosting' for row in rpc(alice, 'list_messages', contactId=alice_contact)), 180, 'message received with application host stopped')
            wait_for(lambda: any(row['id'] == no_host and row['delivered'] for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'end-to-end receipt without application hosting')
            for client in clients:
                rpc(client, 'vault_lock')
            receipt['assertions'].append('application host stopped: root navigation, cached WASM, decrypted history, two fresh routers, message and end-to-end receipt through independent transport using revalidated local bootstrap')
            deferred_errors = [receipt[key] for key in ('voice_error', 'view_once_error') if key in receipt]
            if deferred_errors:
                raise RuntimeError(' / '.join(deferred_errors))
            receipt['stage'] = 'passed'
            print('MESSENGER_PASS live browser exchange and restart recovery', flush=True)
    except Exception as error:
        receipt['stage'] = 'failed'
        receipt['error'] = str(error)[:500]
        # Only bounded public transport failure strings, never RPC arguments,
        # plaintext, credentials or protocol/network dumps.
        receipt['transport_failures'] = []
        receipt['network_metrics'] = []
        for name, client in zip(('alice', 'bob'), clients):
            try:
                receipt['transport_failures'].append(evaluate(client, 'globalThis.__coreFailures || []'))
                receipt['network_metrics'].append(rpc(client, 'browser_diagnostics'))
            except Exception:
                receipt['transport_failures'].append(['diagnostics unavailable'])
            try:
                (output / f'core-{name}.log').write_text(rpc(client, 'read_debug_log'))
            except Exception:
                pass
        receipt['file_progress'] = {}
        for label, (client, message_id) in watched.items():
            try:
                receipt['file_progress'][label] = file_progress(client, message_id)
            except Exception:
                receipt['file_progress'][label] = 'unavailable'
        raise
    finally:
        # Starts that failed and were retried by the client (public reasons).
        receipt['startup_failures'] = STARTUP_FAILURES[-6:]
        for socket in sockets:
            socket.close()
        for process in browsers:
            stop(process)
        if server is not None:
            server.shutdown(); server.server_close()
        stop(forwarder)
        for log in logs:
            log.close()
        cap_file.unlink(missing_ok=True)
        receipt['elapsed_seconds'] = round(time.monotonic() - started, 2)
        content = json.dumps(receipt, indent=2)
        if any(secret in content for secret in private_secrets):
            raise RuntimeError('secret unexpectedly entered public receipt')
        (output / 'receipt.json').write_text(content)
        with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as summary:
            summary.write(f'### Browser messenger integration\n\n```json\n{content}\n```\n')


if __name__ == '__main__':
    main()
