"""CI orchestration; compilation and test execution belong to GitHub Actions."""
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import time
import argparse


def wait_port(port, seconds=15, process=None):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"fixture exited with code {process.returncode} before opening port {port}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.2)
    raise RuntimeError(f"port {port} did not become ready")


def main(router_mode=False):
    root = Path(__file__).resolve().parents[1]
    output = Path(os.environ["RUNNER_TEMP"]) / ("browser-router-live" if router_mode else "browser-ntcp2")
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
        # InitI2P may perform synchronous verified reseed before listening.
        wait_port(19720, 180, peer)
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            if peer.poll() is not None:
                raise RuntimeError("native peer exited")
            files = list((data / "netDb").rglob("routerInfo-*.dat"))
            if len(files) >= (150 if router_mode else 25):
                break
            time.sleep(2)
        subprocess.run([str(root / "target/release/prepare-netdb"), str(data), str(candidates), "512" if router_mode else "12"], check=True)
        limits = ["--max-connections", "24", "--session-seconds", "600", "--idle-seconds", "120", "--max-bytes", str(32 * 1024 * 1024)] if router_mode else []
        forwarder = subprocess.Popen([sys.executable, "forwarder/server.py",
            "--host", "127.0.0.1:18767", "--origin", "http://127.0.0.1:18769",
            "--capability-file", str(capability_file), "--candidates", str(candidates),
            "--allow-fixture-loopback", *limits], cwd=root, stdout=forwarder_log, stderr=subprocess.STDOUT)
        wait_port(18767, process=forwarder)
        environment = dict(os.environ, GIPNY_NTCP2_CANDIDATES=str(candidates),
            GIPNY_FORWARDER_CAPABILITY=capability, WASM_BINDGEN_TEST_ADDRESS="127.0.0.1:18769",
            WASM_BINDGEN_TEST_TIMEOUT="420" if router_mode else "300")
        # Stream to a retained log without printing the capability-bearing env.
        log_path = output / ("wasm-router.log" if router_mode else "wasm-ntcp2.log")
        with log_path.open("w") as log:
            process = subprocess.Popen(["wasm-pack", "test", "--headless", "--chrome", "--locked",
                "--features", "router-live-tests" if router_mode else "ntcp2-live-tests", "--test", "router" if router_mode else "ntcp2", "--", "--nocapture"],
                cwd=root, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            for line in process.stdout:
                print(line, end="", flush=True)
                log.write(line)
            if process.wait() != 0:
                raise RuntimeError("WASM router tests failed" if router_mode else "WASM NTCP2 tests failed")
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
            summary.write("### Browser exploratory router\n\n" if router_mode else "### Browser NTCP2\n\n")
            for line in log_path.read_text().splitlines():
                if any(marker in line for marker in ["NTCP2_NATIVE_PASS", "NTCP2_PUBLIC_PASS", "EXPLORATORY_PASS", "test result:"]):
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
    parser = argparse.ArgumentParser()
    parser.add_argument("--router", action="store_true")
    main(parser.parse_args().router)
