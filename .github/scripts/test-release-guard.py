#!/usr/bin/env python3
"""Data-only regressions; execute in GitHub CI, without GitHub API mutations."""
import copy
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_guard', Path(__file__).with_name('release-guard.py'))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)
SHA = 'a' * 40
TAG = 'v0.4.19'


class ReleaseGuardTests(unittest.TestCase):
    def test_manual_preview_and_main_only_publish(self):
        self.assertFalse(guard.publish_requested('workflow_dispatch', 'refs/heads/feature', False))
        self.assertFalse(guard.publish_requested('workflow_dispatch', 'refs/heads/main', False))
        self.assertTrue(guard.publish_requested('workflow_dispatch', 'refs/heads/main', True))
        self.assertTrue(guard.publish_requested('push', 'refs/heads/main', False))
        for event in ('push', 'workflow_dispatch'):
            with self.assertRaises(guard.GuardError):
                guard.publish_requested(event, 'refs/heads/feature', True)

    def test_tag_absence_is_not_a_network_or_auth_failure(self):
        self.assertTrue(guard.tag_status(0))
        self.assertFalse(guard.tag_status(2))
        for code in (1, 128, -9):
            with self.assertRaises(guard.GuardError):
                guard.tag_status(code)

    def test_changed_main_or_wrong_ref_cannot_publish_old_assets(self):
        guard.validate_main('refs/heads/main', SHA, SHA)
        for ref, built, current in [('refs/heads/feature', SHA, SHA),
                                    ('refs/heads/main', SHA, 'b' * 40),
                                    ('refs/heads/main', SHA, 'main'),
                                    ('refs/heads/main', '', SHA)]:
            with self.assertRaises(guard.GuardError):
                guard.validate_main(ref, built, current)

    def test_a_tag_created_after_prepare_must_point_to_this_exact_source(self):
        guard.validate_remote_tag(None, SHA)
        guard.validate_remote_tag(None, SHA, prepared=True)
        guard.validate_remote_tag(SHA, SHA, prepared=True)
        for tag, prepared in [(SHA, False), ('b' * 40, False), ('b' * 40, True)]:
            with self.assertRaises(guard.GuardError):
                guard.validate_remote_tag(tag, SHA, prepared)

    def test_tag_absence_receipt_is_bound_to_the_run_attempt_and_source(self):
        context = {'schema': 1, 'tag': TAG, 'repository': 'example/repo', 'sha': SHA,
                   'ref': 'refs/heads/main', 'run': '123', 'attempt': '1'}
        receipt = {**context, 'tag_was_absent': True, 'draft_id': None}
        guard.validate_receipt(receipt, context)
        for changed in ({'sha': 'b' * 40}, {'run': '124'}, {'attempt': '2'},
                        {'tag_was_absent': False}, {'tag': 'v0.4.18'}):
            with self.assertRaises(guard.GuardError):
                guard.validate_receipt({**receipt, **changed}, context)

    def test_existing_release_must_be_an_unpublished_draft_of_this_exact_sha(self):
        draft = {'id': 12, 'tag_name': TAG, 'target_commitish': SHA, 'draft': True, 'published_at': None}
        guard.validate_draft(None, TAG, SHA)
        guard.validate_draft(draft, TAG, SHA, required=True)
        with self.assertRaises(guard.GuardError):
            guard.validate_draft(None, TAG, SHA, required=True)
        for changed in ({'draft': False}, {'published_at': '2026-10-29T00:00:00Z'},
                        {'target_commitish': 'b' * 40}, {'target_commitish': 'main'},
                        {'tag_name': 'v0.4.18'}, {'id': True}):
            with self.assertRaises(guard.GuardError):
                guard.validate_draft({**draft, **changed}, TAG, SHA)

    def test_exact_remote_assets_reject_missing_stale_partial_and_corrupt_uploads(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            (directory / 'app.bin').write_bytes(b'synthetic build output')
            (directory / 'SOURCE-MANIFEST.json').write_bytes(b'{"commit":"synthetic"}')
            expected = guard.local_assets(directory)
        remote = [{'name': name, 'state': 'uploaded', 'size': item['size'],
                   'digest': f'sha256:{item["sha256"]}'} for name, item in expected.items()]
        guard.validate_assets(expected, remote)
        without_digests = [{key: value for key, value in asset.items() if key != 'digest'} for asset in remote]
        invalid = [without_digests, [{**asset, 'digest': None} for asset in remote],
                   remote[:-1], [*remote, remote[0]],
                   [*remote, {'name': 'old-source.tar.gz', 'size': 1, 'state': 'uploaded'}]]
        # A valid digest of corrupt bytes must fail even when the uploaded
        # filename, state and size still match the reviewed local artifact.
        corrupted = copy.deepcopy(remote)
        app = next(asset for asset in corrupted if asset['name'] == 'app.bin')
        corrupt_bytes = b'synthetic build outpuX'
        self.assertEqual(len(corrupt_bytes), app['size'])
        app['digest'] = 'sha256:' + hashlib.sha256(corrupt_bytes).hexdigest()
        invalid.append(corrupted)
        for field, value in [('size', 0), ('size', True), ('state', 'starter'),
                             ('digest', 'sha256:' + '0' * 64), ('digest', ''),
                             ('digest', 'sha512:' + '0' * 128)]:
            mutated = copy.deepcopy(remote)
            mutated[0][field] = value
            invalid.append(mutated)
        for assets in invalid:
            with self.assertRaises(guard.GuardError):
                guard.validate_assets(expected, assets)

    def test_api_failure_never_looks_like_an_absent_release(self):
        with patch.object(guard.subprocess, 'run') as run:
            run.return_value.returncode = 1
            with self.assertRaises(guard.GuardError):
                guard.gh_json('repos/example/repo/releases')
            run.return_value.returncode = 0
            run.return_value.stdout = 'not JSON'
            with self.assertRaises(guard.GuardError):
                guard.gh_json('repos/example/repo/releases')

    def test_publication_payload_preserves_notes_and_latest_policy_as_data(self):
        notes = 'First line\n\nРусский текст $HOME $(command) `literal` {field}\n'
        stable = guard.publication_payload(TAG, notes)
        self.assertEqual(stable['body'], notes)
        self.assertIs(stable['draft'], False)
        self.assertIs(stable['prerelease'], False)
        self.assertEqual(stable['make_latest'], 'true')
        preview = guard.publication_payload('v0.4.19-rc1', notes)
        self.assertEqual(preview['body'], notes)
        self.assertIs(preview['prerelease'], True)
        self.assertEqual(preview['make_latest'], 'false')


if __name__ == '__main__':
    unittest.main()
