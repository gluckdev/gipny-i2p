#!/usr/bin/env python3
"""Run bounded sequential I2P measurements with one verified native build."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time


POINT_TIMEOUT = 20 * 60
FORBIDDEN_ENV = ("GIPNY_FAST", "GIPNY_FAST_HOPS", "GIPNY_SERVER_HOPS")


def inputs(streams, file_bytes):
    """No shell expressions, empty entries, silent coercion, or unbounded runs."""
    if not re.fullmatch(r"[1-8](?:,[1-8]){0,11}", streams):
        raise ValueError("streams must be 1 to 12 comma-separated integers from 1 to 8")
    if not re.fullmatch(r"[0-9]{1,9}", file_bytes):
        raise ValueError("file_bytes must be a decimal integer")
    size = int(file_bytes)
    if not 1024 * 1024 <= size <= 128 * 1024 * 1024:
        raise ValueError("file_bytes must be between 1 MiB and 128 MiB")
    return [int(value) for value in streams.split(",")], size


def save(path, value):
    pending = path.with_suffix(path.suffix + ".tmp")
    pending.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    pending.replace(path)


def verified_build(directory):
    build = json.loads((directory / "build.json").read_text())
    if not isinstance(build, dict) or build.get("schema_version") != 1:
        raise ValueError("unsupported build manifest")
    expected = os.environ.get("GITHUB_SHA")
    if expected and build["application_sha"] != expected:
        raise ValueError("artifact application SHA differs from this workflow SHA")
    for key in ("application_sha", "router_sha"):
        if not re.fullmatch(r"[0-9a-f]{40}", build[key]):
            raise ValueError(f"invalid {key}")
    if build.get("native_router") is not True:
        raise ValueError("measurement requires a native router build")
    binary = directory / "e2e-harness"
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("missing regular binary: e2e-harness")
    if hashlib.sha256(binary.read_bytes()).hexdigest() != build["sha256"]["e2e-harness"]:
        raise ValueError("artifact hash mismatch: e2e-harness")
    binary.chmod(0o755)
    return build


def runtime_environment():
    environment = os.environ.copy()
    if any(key in environment for key in FORBIDDEN_ENV):
        raise ValueError("shortened-path environment variables must be absent")
    for key in list(environment):
        if key.startswith("E2E_") or key in (
                "GIPNY_FILE_LANES", "I2P_EMBED_STUB", "I2P_EMBED_SKIP_NATIVE"):
            environment.pop(key)
    # Build switches are removed only after verified_build has checked the
    # native artifacts. The protected harness rejects their presence, even '0'.
    environment["GIPNY_I2P_LOGLEVEL"] = "warn"
    return environment


def report_outcome(result, returncode, lanes, size):
    if (not isinstance(result, dict) or result.get("schema_version") != 1
            or result.get("configured_file_lanes") != lanes
            or result.get("status") not in ("passed", "failed")):
        raise ValueError("harness did not produce a matching measurement report")
    measured = result.get("measurement_started") is True
    if measured and (result.get("topology") != "shared_router_personal_terminals"
                     or result.get("participant_hops") != [3, 3, 3, 3]
                     or result.get("padded") is not True):
        raise ValueError("measurement lacks the protected three-hop padded policy")
    if result["status"] == "passed":
        file = result.get("file")
        if (not measured or returncode != 0 or not isinstance(file, dict)
                or file.get("verified_unique_bytes") != size):
            raise ValueError("successful report contradicts exit status or verified file size")
        return None
    if returncode == 0:
        raise ValueError("failed report contradicts a successful harness exit")
    # Only a measured file/probe failure is a valid point to continue past.
    # Missing policy/setup evidence must never become a fabricated slow result.
    return "measurement" if measured else "setup"


def interrupted_phase(result, lanes, size, application_sha):
    """A checkpoint proves phase/policy, never a final delivery outcome."""
    if (not isinstance(result, dict) or result.get("schema_version") != 1
            or result.get("configured_file_lanes") != lanes
            or result.get("source_sha") != application_sha
            or result.get("status") not in ("running", "passed", "failed")
            or not isinstance(result.get("config"), dict)
            or result["config"].get("file_bytes") != size):
        raise ValueError("interrupted harness has no matching checkpoint")
    if result.get("measurement_started") is not True:
        return "setup"
    if (result.get("topology") != "shared_router_personal_terminals"
            or result.get("participant_hops") != [3, 3, 3, 3]
            or result.get("padded") is not True
            or result.get("phase") not in ("measurement", "complete")
            or not isinstance(result.get("setup_elapsed_ms"), int)
            or not isinstance(result.get("warmup_rtt_ms"), int)):
        raise ValueError("interrupted measurement lacks a protected warmup checkpoint")
    return "measurement"


def stop(process):
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=15)
    except ProcessLookupError:
        process.wait(timeout=15)


def run_point(index, lanes, size, binaries, reports, state_root, build):
    report_dir = reports / f"{index:02d}-lanes-{lanes}"
    report_dir.mkdir()
    started = time.monotonic()
    deadline = started + POINT_TIMEOUT
    point = {
        "schema_version": 1, "status": "failed", "index": index,
        "file_lanes": lanes, "file_bytes": size, "errors": [],
        "failure_kind": "setup",
        "report_state": "missing",
        "application_sha": build["application_sha"], "router_sha": build["router_sha"],
        "point_timeout_seconds": POINT_TIMEOUT,
        "topology": "two clients with personal terminals on one embedded router",
        "client_router_policy": {"transit_profile": "balanced", "bandwidth": "O",
                                 "share_percent": 50, "transit_tunnels": 256},
        "bandwidth_note": "Router bandwidth classification is not a local streaming hard cap.",
    }
    log = None
    harness_process = None
    state = Path(tempfile.mkdtemp(prefix=f"{index:02d}-lanes-{lanes}-", dir=state_root))
    try:
        environment = runtime_environment()
        harness_env = dict(environment, E2E_PERFORMANCE="1", E2E_BIG_FILE=str(size),
                           GIPNY_FILE_LANES=str(lanes), E2E_WORK_DIR=str(state / "clients"),
                           E2E_TIMEOUT_SECS=str(max(1, int(deadline - time.monotonic()) - 5)),
                           E2E_PERF_REPORT=str(report_dir / "performance.json"),
                           E2E_PERF_PROBE_INTERVAL_MS="2000", E2E_PERF_PROBE_TIMEOUT_SECS="60",
                           E2E_PERF_MAX_IN_FLIGHT="4")
        log = (report_dir / "harness.log").open("wb")
        process = subprocess.Popen(["/usr/bin/time", "-v", "-o", str(report_dir / "resources.txt"),
                                    str(binaries / "e2e-harness")], env=harness_env,
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        harness_process = process
        while process.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError("measurement exceeded its 20-minute total budget")
            time.sleep(1)
        point["harness_exit_code"] = process.returncode
        result = json.loads((report_dir / "performance.json").read_text())
        if isinstance(result, dict) and result.get("status") == "running":
            point["report_state"] = "incomplete"
            raise RuntimeError("harness exited without a final report")
        point["failure_kind"] = report_outcome(result, process.returncode, lanes, size)
        point["report_state"] = "complete"
        if point["failure_kind"] is None:
            point["status"] = "passed"
        else:
            point["errors"].extend(result.get("errors") or ["harness reported failure"])
    except TimeoutError as error:
        point["report_state"] = "incomplete"
        point["errors"].append(str(error))
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        point["errors"].append(str(error))
    finally:
        if harness_process is not None:
            try:
                stop(harness_process)
            except (OSError, subprocess.SubprocessError) as error:
                point["status"] = "failed"
                point["failure_kind"] = "cleanup"
                point["errors"].append(f"process cleanup: {error}")
        if log is not None:
            log.close()
        # Read only after the writer was stopped. A valid checkpoint can prove
        # that protected measurement began, but its initial counters are not a
        # final result. Keep the point red and never present them as zero rates.
        if point["report_state"] == "incomplete" and point["failure_kind"] != "cleanup":
            if harness_process is not None:
                point["harness_exit_code"] = harness_process.returncode
            try:
                result = json.loads((report_dir / "performance.json").read_text())
                point["failure_kind"] = interrupted_phase(
                    result, lanes, size, build["application_sha"])
            except (OSError, ValueError, KeyError) as error:
                point["errors"].append(f"incomplete checkpoint: {error}")
        # Upload only test diagnostics, never the router identities or databases.
        try:
            for number, path in enumerate(sorted(state.rglob("i2pd.log"))):
                shutil.copyfile(path, report_dir / f"router-{number}.log")
            shutil.rmtree(state)
        except OSError as error:
            point["status"] = "failed"
            point["failure_kind"] = "cleanup"
            point["errors"].append(f"state cleanup: {error}")
        point["total_elapsed_ms"] = round((time.monotonic() - started) * 1000)
        save(report_dir / "point.json", point)
    return point


def summary(reports, points):
    lines = ["# I2P throughput exploration", "",
             "Three-hop padded client traffic; existing router capacity/transit policies.",
             "Both clients share one router. This is not an anonymity proof or a voice/screen-sharing readiness test.",
             "Startup/warmup are excluded from file goodput. Failed or missing metrics are not zero.", "",
             "| Point | Lanes requested | Peak send available / receive | Status | Verified bytes | Goodput B/s | Probe p95 ms |",
             "| --- | --- | --- | --- | --- | --- | --- |"]
    for point in points:
        path = reports / f"{point['index']:02d}-lanes-{point['file_lanes']}" / "performance.json"
        try:
            result = json.loads(path.read_text())
        except (OSError, ValueError):
            result = {}
        if (not isinstance(result, dict) or result.get("status") not in ("passed", "failed")
                or point.get("report_state") not in (None, "complete")):
            result = {}
        file = result.get("file") or {}
        probes = (result.get("probes") or {}).get("rtt_ms") or {}
        transport = result.get("transport") or {}
        send = (transport.get("sender") or {}).get("peak_send_lanes")
        receive = (transport.get("receiver") or {}).get("peak_receive_lanes")
        peaks = f"{send if send is not None else '—'} / {receive if receive is not None else '—'}"
        values = [point["index"], point["file_lanes"], peaks, point["status"],
                  file.get("verified_unique_bytes"), file.get("goodput_bytes_per_sec"), probes.get("p95")]
        lines.append("| " + " | ".join("—" if value is None else str(value) for value in values) + " |")
    lines.extend(["", "Peak send available counts open lanes at a successful enqueue; it does not prove all were busy.",
                  "resources.txt records GNU time CPU/RSS for the harness process, including both personal terminals."])
    text = "\n".join(lines) + "\n"
    (reports / "summary.md").write_text(text)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as output:
            output.write(text)


def run_sweep(streams, size, binaries, reports, state_root, build):
    points, failed = [], False
    for index, lanes in enumerate(streams, 1):
        print(f"point {index}/{len(streams)}: {lanes} file lanes, {size} bytes", flush=True)
        point = run_point(index, lanes, size, binaries, reports, state_root, build)
        points.append(point)
        summary(reports, points)
        if point["status"] != "passed":
            failed = True
            print(f"point failed: {point['errors']}", file=sys.stderr)
            if point.get("failure_kind") != "measurement":
                return 1
    return int(failed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--binaries", type=Path, default=Path("throughput-bin"))
    parser.add_argument("--reports", type=Path, default=Path("throughput-reports"))
    parser.add_argument("--state-root", type=Path, default=Path(tempfile.gettempdir()) / "gipny-throughput")
    args = parser.parse_args()
    streams, size = inputs(os.environ.get("INPUT_STREAMS", "1,2,3,4,5,6,7,8"),
                           os.environ.get("INPUT_FILE_BYTES", "33554432"))
    if args.validate_only:
        print(json.dumps({"streams": streams, "file_bytes": size}))
        return 0
    binaries, reports = args.binaries.resolve(), args.reports.resolve()
    reports.mkdir(parents=True, exist_ok=False)
    args.state_root.mkdir(parents=True, exist_ok=True)
    try:
        build = verified_build(binaries)
    except (OSError, ValueError, KeyError) as error:
        save(reports / "failure.json", {"status": "failed", "phase": "artifact-verification",
                                        "errors": [str(error)]})
        raise
    save(reports / "build.json", build)
    save(reports / "plan.json", {"streams": streams, "file_bytes": size,
                                 "point_timeout_seconds": POINT_TIMEOUT})
    return run_sweep(streams, size, binaries, reports, args.state_root, build)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError) as error:
        sys.exit(f"throughput: {error}")
