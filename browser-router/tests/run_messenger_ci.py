"""Real browser messenger integration. Execution belongs to GitHub Actions.

Two Chrome data directories own independent OPFS profiles and WASM routers.
CDP calls the production worker RPC; no test bridge ships in the application.
Only public assertions/timings enter artifacts, never passwords, configuration,
private browser directories or CDP/network dumps.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
  const attach = target => {
    worker = target;
    target.addEventListener('message', event => {
      const message = event.data;
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


def wait_for(check, seconds, label):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(2)
    raise RuntimeError(f'deadline: {label}')


def ready(cdp):
    info = rpc(cdp, 'get_relay_info')
    if info['hosted']['state'] == 'failed':
        raise RuntimeError('I2P client startup failed: ' + info['hosted']['reason'][:300])
    return info['hosted']['state'] == 'ready'


def create_ui(cdp, profile, password):
    # Exercise the shipped auth form and Store/API adapter as well as RPC.
    values = json.dumps([profile, profile, password, password])
    evaluate(cdp, f'''(() => {{
      const fields = [...document.querySelectorAll('.auth input')];
      const values = {values};
      if (fields.length !== 4) throw new Error('Browser profile form is missing');
      fields.forEach((field, i) => {{ field.value = values[i]; field.dispatchEvent(new Event('input', {{bubbles:true}})); }});
      const button = document.querySelector('.auth-card .row .btn:not(.btn-ghost)');
      if (!button) throw new Error('Create profile button is missing');
      button.click();
    }})()''')
    wait_for(lambda: rpc(cdp, 'vault_status', profile=profile)['unlocked'], 30, 'production profile creation form')


def unlock_ui(cdp, profile, password, wrong_password='incorrect password'):
    cdp.call('Page.navigate', url='http://localhost:18770/browser.html')
    label = json.dumps('◉  ' + profile)
    wait_for(lambda: evaluate(cdp, f'[...document.querySelectorAll(".auth button")].some(button=>button.textContent === {label})'), 30, 'saved profile selector')
    evaluate(cdp, f'[...document.querySelectorAll(".auth button")].find(button=>button.textContent === {label}).click()')
    wait_for(lambda: evaluate(cdp, '!!document.querySelector(".auth input[type=password]")'), 10, 'profile password form')
    wrong_rejected = evaluate(cdp, '__coreCall("vault_unlock", {profile:' + json.dumps(profile) + ',pass:' + json.dumps(wrong_password) + '}).then(()=>false,()=>true)')
    if not wrong_rejected:
        raise RuntimeError('wrong password accepted')
    evaluate(cdp, f'''(() => {{
      document.querySelector('.auth input[type=password]').value = {json.dumps(password)};
      document.querySelector('.auth-card .row .btn:not(.btn-ghost)').click();
    }})()''')
    wait_for(lambda: rpc(cdp, 'vault_status', profile=profile)['unlocked'], 30, 'production unlock form')


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise SystemExit('Run browser integration only in GitHub Actions')
    root = Path(__file__).resolve().parents[1]
    output = Path(os.environ['RUNNER_TEMP']) / 'browser-messenger'
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
    receipt = dict(stage='starting', physical_phone=False, independent_browser_profiles=2, assertions=[])
    browsers, sockets, logs, clients = [], [], [], []
    forwarder = server = None
    started = time.monotonic()
    try:
        log = (output / 'forwarder.log').open('w'); logs.append(log)
        forwarder = subprocess.Popen([sys.executable, 'forwarder/server.py',
            '--host', '127.0.0.1:18767', '--origin', 'http://localhost:18770',
            '--capability-file', str(cap_file), '--candidates', str(candidates),
            '--max-connections', '32', '--session-seconds', '600', '--idle-seconds', '120',
            '--max-bytes', str(64 * 1024 * 1024)], cwd=root, stdout=log, stderr=subprocess.STDOUT)
        wait_port(18767, process=forwarder)
        server = ThreadingHTTPServer(('127.0.0.1', 18770), Assets)
        server.assets = (root.parent / 'ui/browser-dist').resolve()
        server.snapshot = snapshot
        server.config = dict(snapshot_url='http://localhost:18770/snapshot.tar.gz',
            snapshot_sha256=manifest['sha256'], forwarder_url='ws://127.0.0.1:18767/stream', capability=capability)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        browser = next((shutil.which(name) for name in ('google-chrome', 'chromium', 'chromium-browser') if shutil.which(name)), None)
        if browser is None:
            raise RuntimeError('Chrome is required')
        with tempfile.TemporaryDirectory(prefix='gipny-messenger-private-') as private:
            clients = []
            for name, port in [('alice', 18771), ('bob', 18772)]:
                log = (output / f'chrome-{name}.log').open('w'); logs.append(log)
                process = subprocess.Popen([browser, '--headless=new', '--no-sandbox', '--disable-dev-shm-usage',
                    '--disable-background-networking', '--no-first-run', '--remote-debugging-address=127.0.0.1',
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
                wait_for(lambda: evaluate(cdp, 'typeof __coreCall === "function" && document.querySelectorAll("input").length > 0'), 30, 'production UI and worker')
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
            for client in clients:
                wait_for(lambda: ready(client), 330, 'client destination tunnels')
            receipt['assertions'].append('two independent WASM client destinations ready over live I2P')
            print('MESSENGER_PROGRESS live destinations ready', flush=True)
            alice_card, bob_card = rpc(alice, 'my_card'), rpc(bob, 'my_card')
            alice_address, bob_address = rpc(alice, 'my_b32'), rpc(bob, 'my_b32')
            alice_contact = rpc(alice, 'add_contact', onion=bob_address, signPk=bob_card['sign_pk'], dhPk=bob_card['dh_pk'], name='Bob', relay=bob_address)
            bob_contact = rpc(bob, 'add_contact', onion=alice_address, signPk=alice_card['sign_pk'], dhPk=alice_card['dh_pk'], name='Alice', relay=alice_address)
            first = rpc(alice, 'send_message', contactId=alice_contact, body='browser live alpha', attachments=[], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['body'] == 'browser live alpha' for row in rpc(bob, 'list_messages', contactId=bob_contact)), 150, 'Alice to Bob decrypted message')
            wait_for(lambda: any(row['id'] == first and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'authenticated end-to-end receipt')
            reply = rpc(bob, 'send_message', contactId=bob_contact, body='browser live beta', attachments=[dict(name='hello.txt', data='aGVsbG8=')], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['body'] == 'browser live beta' for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'Bob to Alice ratchet reply')
            wait_for(lambda: any(row['id'] == reply and row['delivered'] for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'reply receipt')
            receipt['assertions'].append('bidirectional shared X3DH/Double Ratchet, end-to-end delivery receipts and inline attachment')
            print('MESSENGER_PROGRESS bidirectional messages delivered', flush=True)
            rpc(bob, 'update_contact', id=bob_contact, name='Alice', trust=2)
            outbound_blocked = evaluate(bob, '__coreCall("send_message",{contactId:' + str(bob_contact) + ',body:"blocked outbound",attachments:[],ttlSecs:null,replyTo:null}).then(()=>false,()=>true)')
            if not outbound_blocked:
                raise RuntimeError('blocked contact accepted an outgoing message')
            held = rpc(alice, 'send_message', contactId=alice_contact, body='held while blocked', attachments=[], ttlSecs=None, replyTo=None)
            wait_for(lambda: any(row['id'] == held and row['sent'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'blocked mailbox deposit')
            if any(row['body'] == 'held while blocked' for row in rpc(bob, 'list_messages', contactId=bob_contact)):
                raise RuntimeError('blocked incoming contact entered history')
            rpc(bob, 'update_contact', id=bob_contact, name='Alice', trust=1)
            wait_for(lambda: any(row['id'] == held and row['delivered'] for row in rpc(alice, 'list_messages', contactId=alice_contact)), 120, 'unblocked exact ciphertext retry')
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
            offline = rpc(bob, 'send_message', contactId=bob_contact, body='durable offline outbox', attachments=[], ttlSecs=None, replyTo=None)
            time.sleep(3)
            unlock_ui(alice, 'alice', password_new, wrong_password=password_a)
            if rpc(alice, 'my_b32') != alice_address or rpc(alice, 'my_card') != alice_card:
                raise RuntimeError('destination or identity changed after worker restart')
            receipt['assertions'].append('password rotation mirrored across OPFS slots; old password rejected; identity and destination restored after page/worker restart')
            wait_for(lambda: ready(alice), 330, 'restored client destination')
            print('MESSENGER_PROGRESS restored client destination ready', flush=True)
            wait_for(lambda: any(row['body'] == 'durable offline outbox' for row in rpc(alice, 'list_messages', contactId=alice_contact)), 180, 'offline outbox recovered recipient')
            wait_for(lambda: any(row['id'] == offline and row['delivered'] for row in rpc(bob, 'list_messages', contactId=bob_contact)), 120, 'offline message delivery receipt')
            rows = rpc(alice, 'list_messages', contactId=alice_contact)
            if len([row for row in rows if row['body'] == 'durable offline outbox']) != 1:
                raise RuntimeError('retry duplicated history')
            receipt['assertions'].append('worker termination, password rotation across both slots, old-password rejection, identity/destination/ratchet/history restoration, offline outbox retry without duplicate history')
            for client in clients:
                rpc(client, 'vault_lock')
            wait_for(lambda: evaluate(alice, 'navigator.serviceWorker.ready.then(()=>!!navigator.serviceWorker.controller)'), 20, 'offline shell installed')
            forbidden = evaluate(alice, '(async()=>{for(const key of await caches.keys()){for(const request of await (await caches.open(key)).keys()){if(/browser-config\\.json|snapshot\\.tar\\.gz/.test(new URL(request.url).pathname))return true}}return false})()')
            if forbidden:
                raise RuntimeError('network configuration or snapshot entered PWA cache')
            # Stop the actual web server: an emulated network label alone does
            # not prove that service-worker/worker assets work offline.
            server.shutdown(); server.server_close(); server = None
            alice.call('Page.navigate', url='http://localhost:18770/')
            wait_for(lambda: evaluate(alice, '!!document.querySelector(".auth-card .stack .btn-block")'), 30, 'offline root navigation and saved profiles')
            unlock_ui(alice, 'alice', password_new)
            wait_for(lambda: evaluate(alice, '!!document.querySelector(".boot-enter:not(.hidden)") || !!document.querySelector(".sidebar")'), 20, 'offline history entry')
            evaluate(alice, 'document.querySelector(".boot-enter:not(.hidden)")?.click()')
            wait_for(lambda: evaluate(alice, '!!document.querySelector(".contact-list .contact")'), 20, 'offline contact list')
            evaluate(alice, 'document.querySelector(".contact-list .contact").click()')
            wait_for(lambda: evaluate(alice, '[...document.querySelectorAll(".msg-body")].some(element=>element.textContent.includes("durable offline outbox"))'), 20, 'offline transcript rendered')
            rpc(alice, 'vault_lock')
            receipt['assertions'].append('offline root navigation, cached WASM worker and rendered encrypted history with the web server stopped; network config excluded from caches')
            receipt['stage'] = 'passed'
            print('MESSENGER_PASS live browser exchange and restart recovery', flush=True)
    except Exception as error:
        receipt['stage'] = 'failed'
        receipt['error'] = str(error)[:500]
        # Only bounded public transport failure strings, never RPC arguments,
        # plaintext, credentials or protocol/network dumps.
        receipt['transport_failures'] = []
        for client in clients:
            try:
                receipt['transport_failures'].append(evaluate(client, 'globalThis.__coreFailures || []'))
            except Exception:
                receipt['transport_failures'].append(['diagnostics unavailable'])
        raise
    finally:
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
