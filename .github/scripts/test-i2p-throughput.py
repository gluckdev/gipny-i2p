#!/usr/bin/env python3
"""Offline regressions for the protected throughput runner; no routers started."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "throughput", Path(__file__).with_name("i2p-throughput.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
MIB = 1024 * 1024
APP_SHA = "a" * 40


def report(status="passed", measured=True):
    return {"schema_version": 1, "status": status,
            "configured_file_lanes": 4, "measurement_started": measured,
            "participant_hops": [3, 3, 3, 3] if measured else None,
            "topology": "shared_router_personal_terminals",
            "padded": True, "file": {"verified_unique_bytes": MIB}}


def checkpoint(measured=True):
    return dict(report("running", measured), source_sha=APP_SHA,
                config={"file_bytes": MIB}, phase="measurement" if measured else "setup",
                setup_elapsed_ms=50 if measured else None,
                warmup_rtt_ms=10 if measured else None)


class InputTests(unittest.TestCase):
    def test_bounds_and_repeat_baselines(self):
        self.assertEqual(runner.inputs("4,5,6,7,8,4", str(MIB)),
                         ([4, 5, 6, 7, 8, 4], MIB))
        self.assertEqual(runner.inputs(",".join(["8"] * 12), str(128 * MIB)),
                         ([8] * 12, 128 * MIB))

    def test_rejects_ambiguous_or_unbounded_inputs(self):
        for streams in ("", "0", "9", "01", "1,", ",1", "1,,2", "1, 2", "1\n",
                        "$(id)", "1;id", ",".join(["1"] * 13)):
            with self.subTest(streams=streams), self.assertRaises(ValueError):
                runner.inputs(streams, str(MIB))
        for size in ("", "-1", "+1048576", "1e6", "1048576\n", str(MIB - 1),
                     str(128 * MIB + 1), "9" * 10):
            with self.subTest(size=size), self.assertRaises(ValueError):
                runner.inputs("4", size)

    def test_runtime_removes_build_and_scenario_switches(self):
        environment = {"PATH": "/usr/bin", "I2P_EMBED_STUB": "0",
                       "I2P_EMBED_SKIP_NATIVE": "0", "GIPNY_FILE_LANES": "8",
                       "E2E_OFFLINE_RETURN": "1", "E2E_PERFORMANCE": "0"}
        with patch.dict(os.environ, environment, clear=True):
            actual = runner.runtime_environment()
        self.assertEqual(actual, {"PATH": "/usr/bin", "GIPNY_I2P_LOGLEVEL": "warn"})

    def test_runtime_rejects_shortening_switch_even_when_zero(self):
        for name in runner.FORBIDDEN_ENV:
            with self.subTest(name=name), patch.dict(os.environ, {name: "0"}, clear=True):
                with self.assertRaisesRegex(ValueError, "must be absent"):
                    runner.runtime_environment()


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        data = b"test harness fixture"
        (self.directory / "e2e-harness").write_bytes(data)
        hashes = {"e2e-harness": hashlib.sha256(data).hexdigest()}
        self.build = {"schema_version": 1, "application_sha": APP_SHA,
                      "router_sha": "b" * 40, "native_router": True, "sha256": hashes}
        self.write_manifest()

    def write_manifest(self):
        (self.directory / "build.json").write_text(json.dumps(self.build))

    def test_verified_artifact_and_executable_permissions(self):
        with patch.dict(os.environ, {"GITHUB_SHA": APP_SHA}, clear=True):
            self.assertEqual(runner.verified_build(self.directory), self.build)
        self.assertTrue((self.directory / "e2e-harness").stat().st_mode & 0o111)

    def test_tampered_binary_is_rejected(self):
        (self.directory / "e2e-harness").write_bytes(b"different executable")
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "artifact hash mismatch"):
                runner.verified_build(self.directory)

    def test_source_sha_and_native_mode_are_required(self):
        with patch.dict(os.environ, {"GITHUB_SHA": "c" * 40}, clear=True):
            with self.assertRaisesRegex(ValueError, "application SHA"):
                runner.verified_build(self.directory)
        self.build["native_router"] = False
        self.write_manifest()
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "native router"):
                runner.verified_build(self.directory)

    def test_symlink_cannot_substitute_an_artifact(self):
        binary = self.directory / "e2e-harness"
        binary.rename(self.directory / "replacement")
        binary.symlink_to("replacement")
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "regular binary"):
                runner.verified_build(self.directory)


class PolicyAndResultTests(unittest.TestCase):
    def test_interruption_checkpoint_proves_phase_but_never_completion(self):
        self.assertEqual(runner.interrupted_phase(checkpoint(), 4, MIB, APP_SHA), "measurement")
        self.assertEqual(runner.interrupted_phase(checkpoint(False), 4, MIB, APP_SHA), "setup")
        for changes in ({"source_sha": "c" * 40}, {"configured_file_lanes": 6},
                        {"config": {"file_bytes": MIB * 2}}, {"padded": False},
                        {"participant_hops": [3, 3, 2, 3]}, {"warmup_rtt_ms": None},
                        {"phase": "setup"}, {"status": "invalid"}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                runner.interrupted_phase(dict(checkpoint(), **changes), 4, MIB, APP_SHA)

    def test_incomplete_snapshot_never_displays_initial_zero_or_apparent_success(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            point = {"index": 1, "file_lanes": 4, "status": "failed", "report_state": "incomplete"}
            result_dir = directory / "01-lanes-4"
            result_dir.mkdir()
            for state in ("incomplete", "missing"):
                point["report_state"] = state
                for value in (dict(checkpoint(False), file={"verified_unique_bytes": 0}), report()):
                    runner.save(result_dir / "performance.json", value)
                    runner.summary(directory, [point])
                    self.assertIn("| 1 | 4 | — / — | failed | — | — | — |",
                                  (directory / "summary.md").read_text())

    def test_measured_failure_is_distinct_from_setup_failure(self):
        self.assertIsNone(runner.report_outcome(report(), 0, 4, MIB))
        self.assertEqual(runner.report_outcome(report("failed"), 1, 4, MIB), "measurement")
        self.assertEqual(runner.report_outcome(report("failed", False), 1, 4, MIB), "setup")

    def test_invalid_reports_never_count_as_measurements(self):
        cases = [(None, 1), ({}, 1), (report(), 1), (report("failed"), 0),
                 (dict(report(), file=None), 0), (dict(report(), file={}), 0),
                 (dict(report(), configured_file_lanes=8), 0),
                 (dict(report(), measurement_started=False), 0),
                 (dict(report("failed"), participant_hops=[3, 3, 2, 3]), 1),
                 (dict(report("failed"), padded=False), 1),
                 (dict(report(), topology="different-fixture"), 0)]
        for value, code in cases:
            with self.subTest(value=value, code=code), self.assertRaises(ValueError):
                runner.report_outcome(value, code, 4, MIB)

    def test_summary_handles_null_or_absent_setup_metrics(self):
        variants = ({"file": None, "probes": None, "transport": None}, {}, None,
                    {"probes": {"rtt_ms": None}, "transport": {"sender": None, "receiver": None}})
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            point = {"index": 1, "file_lanes": 4, "status": "failed"}
            result_dir = directory / "01-lanes-4"
            result_dir.mkdir()
            for value in variants:
                with self.subTest(value=value):
                    runner.save(result_dir / "performance.json", value)
                    runner.summary(directory, [point])
                    text = (directory / "summary.md").read_text()
                    self.assertIn("| 1 | 4 | — / — | failed | — | — | — |", text)
                    self.assertNotIn("| 0 |", text)


class SweepTests(unittest.TestCase):
    def run_mocked_sweep(self, outcomes):
        def point(index, lanes, *_):
            kind = outcomes[index - 1]
            return {"index": index, "file_lanes": lanes, "errors": [],
                    "status": "passed" if kind is None else "failed", "failure_kind": kind}
        with patch.object(runner, "run_point", side_effect=point) as execute, \
                patch.object(runner, "summary") as summaries, \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            code = runner.run_sweep([1, 2, 3], MIB, None, None, None, {})
            return code, execute.call_count, summaries.call_count

    def test_measurement_failure_preserves_later_points_and_final_failure(self):
        self.assertEqual(self.run_mocked_sweep([None, "measurement", None]), (1, 3, 3))

    def test_invalid_setup_stops_sweep(self):
        self.assertEqual(self.run_mocked_sweep(["setup"]), (1, 1, 1))

    def test_cleanup_failure_stops_sweep(self):
        self.assertEqual(self.run_mocked_sweep([None, "cleanup"]), (1, 2, 2))

    def test_all_measured_points_pass(self):
        self.assertEqual(self.run_mocked_sweep([None, None, None]), (0, 3, 3))


class OuterTimeoutTests(unittest.TestCase):
    def run_timeout(self, value):
        class Process:
            returncode = None

            def poll(self):
                return self.returncode

        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            reports, state = directory / "reports", directory / "state"
            reports.mkdir()
            state.mkdir()
            processes = []

            def spawn(*args, **kwargs):
                process = Process()
                processes.append(process)
                if value is not None:
                    runner.save(reports / "01-lanes-4" / "performance.json", value)
                return process

            def stop(process):
                process.returncode = -15

            original_classifier = runner.interrupted_phase

            def classify(*args):
                self.assertTrue(all(p.returncode is not None for p in processes))
                return original_classifier(*args)

            with patch.object(runner.subprocess, "Popen", side_effect=spawn), \
                    patch.object(runner, "stop", side_effect=stop), \
                    patch.object(runner, "interrupted_phase", side_effect=classify), \
                    patch.object(runner.shutil, "copyfile"), \
                    patch.object(runner.time, "monotonic", side_effect=[0, 0, 1201, 1202]):
                point = runner.run_point(1, 4, MIB, directory / "bin", reports, state,
                                         {"application_sha": APP_SHA, "router_sha": "b" * 40})
            self.assertEqual(point["status"], "failed")
            self.assertEqual(point["report_state"], "incomplete")
            self.assertEqual(point["harness_exit_code"], -15)
            self.assertTrue(all(p.returncode == -15 for p in processes))
            return point

    def test_outer_timeout_after_protected_checkpoint_is_a_failed_measurement(self):
        point = self.run_timeout(checkpoint())
        self.assertEqual(point["failure_kind"], "measurement")
        self.assertIn("20-minute total budget", point["errors"][0])
        self.assertEqual(SweepTests().run_mocked_sweep([point["failure_kind"], None, None]), (1, 3, 3))

    def test_outer_timeout_before_checkpoint_or_with_wrong_policy_stops_sweep(self):
        for value in (None, checkpoint(False), dict(checkpoint(), padded=False)):
            with self.subTest(value=value):
                point = self.run_timeout(value)
                self.assertEqual(point["failure_kind"], "setup")
                self.assertEqual(SweepTests().run_mocked_sweep([point["failure_kind"]]), (1, 1, 1))


if __name__ == "__main__":
    unittest.main()
