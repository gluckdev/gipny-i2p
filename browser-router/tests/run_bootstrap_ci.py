"""Cold HTTP/WASM bootstrap in real Chrome. Run only in GitHub Actions.

The constrained profile is desktop Chrome emulation, not a phone measurement.
No capability, private keys, CDP traffic or Chrome profile enter artifacts.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from urllib.request import urlopen

from websockets.sync.client import connect
from run_ntcp2_ci import wait_port


class Assets(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass  # Never log configuration/capability or request headers.

    def do_GET(self):
        if self.headers.get_all('Host') != ['127.0.0.1:18770']:
            self.send_error(403)
            return
        paths = {
            '/': self.server.root / 'tests/bootstrap-page/index.html',
            '/probe.js': self.server.root / 'tests/bootstrap-page/probe.js',
            '/pkg/gipny_browser_router.js': self.server.root / 'pkg/gipny_browser_router.js',
            '/pkg/gipny_browser_router_bg.wasm': self.server.root / 'pkg/gipny_browser_router_bg.wasm',
            '/snapshot.tar.gz': self.server.snapshot,
        }
        if self.path == '/config.json':
            if self.headers.get('Sec-Fetch-Site') != 'same-origin':
                self.send_error(403)
                return
            data = json.dumps(self.server.config).encode()
            content_type = 'application/json'
        elif self.path in paths:
            data = paths[self.path].read_bytes()
            content_type = ('application/wasm' if self.path.endswith('.wasm') else
                            'text/javascript' if self.path.endswith('.js') else
                            'application/gzip' if self.path.endswith('.gz') else 'text/html; charset=utf-8')
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Content-Security-Policy', "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self' ws://127.0.0.1:18767; frame-ancestors 'none'; base-uri 'none'")
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass


class Cdp:
    def __init__(self, socket):
        self.socket = socket
        self.sequence = 0

    def call(self, method, **params):
        self.sequence += 1
        request_id = self.sequence
        self.socket.send(json.dumps(dict(id=request_id, method=method, params=params)))
        while True:
            result = json.loads(self.socket.recv(timeout=30))
            if result.get('id') == request_id:
                if 'error' in result:
                    raise RuntimeError(f'CDP method failed: {method}')
                return result.get('result', {})


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def profile(root, output, snapshot, candidates, name, cpu_rate):
    capability = secrets.token_urlsafe(32)
    print(f'::add-mask::{capability}', flush=True)
    cap_file = output / f'.capability-{name}'
    cap_file.write_text(capability)
    cap_file.chmod(0o600)
    forwarder = chrome = None
    server = None
    result = None
    with tempfile.TemporaryDirectory(prefix='browser-bootstrap-') as private:
        try:
            with (output / f'forwarder-{name}.log').open('w') as log:
                forwarder = subprocess.Popen([sys.executable, 'forwarder/server.py',
                    '--host', '127.0.0.1:18767', '--origin', 'http://127.0.0.1:18770',
                    '--capability-file', str(cap_file), '--candidates', str(candidates),
                    '--max-connections', '24', '--session-seconds', '600', '--idle-seconds', '120',
                    '--max-bytes', str(32 * 1024 * 1024)], cwd=root, stdout=log, stderr=subprocess.STDOUT)
                wait_port(18767, process=forwarder)
                server = ThreadingHTTPServer(('127.0.0.1', 18770), Assets)
                server.root, server.snapshot = root, snapshot
                manifest = json.loads(Path(str(snapshot) + '.json').read_text())
                server.config = dict(sha256=manifest['sha256'], endpoint='ws://127.0.0.1:18767/stream', capability=capability)
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                browser = next((shutil.which(p) for p in ('google-chrome', 'chromium', 'chromium-browser') if shutil.which(p)), None)
                if browser is None:
                    raise RuntimeError('Chrome is required')
                with (output / f'chrome-{name}.log').open('w') as browser_log:
                    chrome = subprocess.Popen([browser, '--headless=new', '--no-sandbox',
                        '--disable-dev-shm-usage', '--disable-background-networking', '--no-first-run',
                        '--enable-precise-memory-info', '--remote-debugging-address=127.0.0.1',
                        '--remote-debugging-port=18771', f'--user-data-dir={private}', 'about:blank'],
                        stdout=browser_log, stderr=subprocess.STDOUT)
                    wait_port(18771, process=chrome)
                    with urlopen('http://127.0.0.1:18771/json', timeout=5) as response:
                        target = next(t for t in json.load(response) if t['type'] == 'page')
                    with connect(target['webSocketDebuggerUrl'], open_timeout=5, close_timeout=2) as socket:
                        cdp = Cdp(socket)
                        cdp.call('Page.enable')
                        cdp.call('Runtime.enable')
                        cdp.call('Performance.enable')
                        cdp.call('Network.enable')
                        cdp.call('Network.setCacheDisabled', cacheDisabled=True)
                        cdp.call('Emulation.setCPUThrottlingRate', rate=cpu_rate)
                        if cpu_rate > 1:
                            cdp.call('Emulation.setDeviceMetricsOverride', width=390, height=844, deviceScaleFactor=3, mobile=True)
                            cdp.call('Network.emulateNetworkConditions', offline=False, latency=80,
                                     downloadThroughput=200 * 1024, uploadThroughput=100 * 1024)
                        version = cdp.call('Browser.getVersion')
                        cdp.call('Page.navigate', url='http://127.0.0.1:18770/')
                        deadline = time.monotonic() + 380
                        js_heap_max = 0
                        last_stage = None
                        while time.monotonic() < deadline:
                            time.sleep(1)
                            state = cdp.call('Runtime.evaluate', expression='globalThis.bootstrapState || null', returnByValue=True).get('result', {}).get('value')
                            metrics = cdp.call('Performance.getMetrics').get('metrics', [])
                            js_heap_max = max([js_heap_max] + [int(m['value']) for m in metrics if m['name'] == 'JSHeapUsedSize'])
                            if not state:
                                continue
                            if state['stage'] != last_stage:
                                last_stage = state['stage']
                                print(f'BOOTSTRAP_PROGRESS profile={name} stage={last_stage}', flush=True)
                            if state['stage'] in ('failed', 'passed'):
                                result = dict(profile=name, cpu_rate=cpu_rate,
                                    emulated=cpu_rate > 1, physical_phone=False,
                                    network=dict(latency_ms=80, download_bytes_per_second=200*1024, upload_bytes_per_second=100*1024) if cpu_rate > 1 else None,
                                    chrome_version=version['product'], sampled_js_heap_max_bytes=js_heap_max, **state)
                                break
                        if result is None:
                            raise RuntimeError(f'bootstrap page deadline: {name}, stage={last_stage}')
                        # This is the only retained CDP result: the page's public
                        # receipt and aggregate memory, no protocol/network dump.
                        text = json.dumps(result, ensure_ascii=False, indent=2)
                        if capability in text:
                            raise RuntimeError('capability unexpectedly entered bootstrap receipt')
                        (output / f'receipt-{name}.json').write_text(text)
                        if result['stage'] != 'passed':
                            raise RuntimeError(f'bootstrap failed for {name}: {result.get("error")}')
                        receipt = result['result']
                        assert receipt['router']['inbound'] > 0 and receipt['router']['outbound'] > 0
                        assert receipt['router']['held_seconds'] >= 30 and receipt['router']['roundtrips'] >= 2
                        assert receipt['snapshot']['accepted'] == manifest['records']
                        assert receipt['navigation_to_ready_ms'] >= receipt['module_ready_ms']
                        print('BOOTSTRAP_PASS ' + json.dumps(result, ensure_ascii=False), flush=True)
                        with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as summary:
                            summary.write(f'### Cold browser bootstrap: {name}\n\n```json\n{text}\n```\n\n')
        finally:
            stop(chrome)
            if server is not None:
                server.shutdown()
                server.server_close()
            stop(forwarder)
            cap_file.unlink(missing_ok=True)


def main():
    root = Path(__file__).resolve().parents[1]
    output = Path(os.environ['RUNNER_TEMP']) / 'browser-bootstrap'
    output.mkdir(exist_ok=True)
    data = output / 'snapshot-source'
    with (output / 'snapshot-source.log').open('w') as log:
        subprocess.run([str(root / 'target/release/collect-netdb'), str(data)],
                       check=True, stdout=log, stderr=subprocess.STDOUT, timeout=600)
    candidates, snapshot = output / 'candidates.tsv', output / 'snapshot.tar.gz'
    subprocess.run([str(root / 'target/release/prepare-netdb'), str(data), str(candidates), '2048', str(snapshot)], check=True)
    for name, rate in [('desktop-cold', 1), ('constrained-cold', 4)]:
        profile(root, output, snapshot, candidates, name, rate)


if __name__ == '__main__':
    main()
