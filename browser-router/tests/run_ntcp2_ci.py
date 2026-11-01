"""CI orchestration; compilation and test execution belong to GitHub Actions."""
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import time


def wait_port(port, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.2)
    raise RuntimeError(f"port {port} did not become ready")


def main():
    root = Path(__file__).resolve().parents[1]
    output = Path(os.environ["RUNNER_TEMP"]) / "browser-ntcp2"
    output.mkdir(exist_ok=True)
    data = output / "native-peer"
    candidates = output / "candidates.tsv"
    capability = secrets.token_urlsafe(32)
    print(f"::add-mask::{capability}", flush=True)
    capability_file = output / "capability"
    capability_file.write_text(capability)
    capability_file.chmod(0o600)
    peer_log = (output / "native-peer-stdout.log").open("w")
    forwarder_log = (output / "forwarder.log").open("w")
    peer = subprocess.Popen([str(root / "target/release/native-peer"), str(data)], stdout=peer_log, stderr=subprocess.STDOUT)
    forwarder = None
    try:
        wait_port(19720, 30)
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            if peer.poll() is not None:
                raise RuntimeError("native peer exited")
            files = list((data / "netDb").rglob("routerInfo-*.dat"))
            if len(files) >= 25:
                break
            time.sleep(2)
        subprocess.run([str(root / "target/release/prepare-netdb"), str(data), str(candidates), "12"], check=True)
        forwarder = subprocess.Popen([sys.executable, "forwarder/server.py",
            "--host", "127.0.0.1:18767", "--origin", "http://127.0.0.1:18769",
            "--capability-file", str(capability_file), "--candidates", str(candidates),
            "--allow-fixture-loopback"], cwd=root, stdout=forwarder_log, stderr=subprocess.STDOUT)
        wait_port(18767)
        environment = dict(os.environ, GIPNY_NTCP2_CANDIDATES=str(candidates),
            GIPNY_FORWARDER_CAPABILITY=capability, WASM_BINDGEN_TEST_ADDRESS="127.0.0.1:18769",
            WASM_BINDGEN_TEST_TIMEOUT="300")
        # Stream to a retained log without printing the capability-bearing env.
        with (output / "wasm-ntcp2.log").open("w") as log:
            process = subprocess.Popen(["wasm-pack", "test", "--headless", "--chrome", "--locked",
                "--features", "ntcp2-live-tests", "--test", "ntcp2", "--", "--nocapture"],
                cwd=root, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            for line in process.stdout:
                print(line, end="", flush=True)
                log.write(line)
            if process.wait() != 0:
                raise RuntimeError("WASM NTCP2 tests failed")
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
            summary.write("### Browser NTCP2\n\n")
            for line in (output / "wasm-ntcp2.log").read_text().splitlines():
                if "NTCP2_NATIVE_PASS" in line or "NTCP2_PUBLIC_PASS" in line or "test result:" in line:
                    summary.write(f"{line}\n\n")
    finally:
        if forwarder is not None:
            forwarder.terminate()
            forwarder.wait(timeout=10)
        peer.terminate()
        try:
            peer.wait(timeout=15)
        except subprocess.TimeoutExpired:
            peer.kill()
            peer.wait()
        capability_file.unlink(missing_ok=True)
        peer_log.close()
        forwarder_log.close()


if __name__ == "__main__":
    main()
