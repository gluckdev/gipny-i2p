#!/usr/bin/env python3
"""Data-only integrity regressions. Never compiles or runs an application."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('provenance', Path(__file__).with_name('build-provenance.py'))
provenance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provenance)


class ArtifactIntegrity(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / 'inputs'
        self.inputs.mkdir()
        self.source = {'commit': 'a' * 40, 'repository': 'https://github.com/example/public-source', 'submodules': []}
        self.package = self.inputs / 'package.tar.gz'
        self.package.write_bytes(b'synthetic package fixture; no executable')
        self.archive = self.inputs / 'source.tar.gz'
        self.archive.write_bytes(b'synthetic public source fixture')
        self.source['archive'] = self.asset(self.archive)
        provenance.write_json(self.inputs / 'SOURCE-MANIFEST.json', self.source)
        self.manifest = {'source': self.source, 'target': 'fixture-target', 'artifacts': [self.asset(self.package)]}
        provenance.write_json(self.inputs / 'build-manifest-fixture.json', self.manifest)
        self.mock_source = patch.object(provenance, 'source_identity', return_value=self.source)
        self.mock_source.start()
        self.addCleanup(self.mock_source.stop)

    @staticmethod
    def asset(path):
        return {'name': path.name, 'sha256': provenance.digest(path), 'size': path.stat().st_size}

    def collect(self):
        provenance.collect(self.inputs, self.root / 'output')

    def test_valid_source_and_binary_hashes_are_all_in_the_final_manifest(self):
        self.collect()
        output = self.root / 'output'
        self.assertEqual((output / self.package.name).read_bytes(), self.package.read_bytes())
        sums = (output / 'SHA256SUMS.txt').read_text()
        for name in ['package.tar.gz', 'source.tar.gz', 'SOURCE-MANIFEST.json', 'BUILD-MANIFEST.json']:
            self.assertIn(f'{provenance.digest(output / name)}  {name}\n', sums)
        self.assertEqual(json.loads((output / 'BUILD-MANIFEST.json').read_text())['builds'][0]['target'], 'fixture-target')

    def test_tampered_or_missing_artifact_never_reaches_release_collection(self):
        self.package.write_bytes(b'tampered fixture')
        with self.assertRaisesRegex(ValueError, 'checksum/size'):
            self.collect()
        self.package.unlink()
        with self.assertRaisesRegex(ValueError, 'missing build artifacts'):
            self.collect()

    def test_wrong_source_commit_duplicate_manifest_and_untracked_file_are_rejected(self):
        self.manifest['source'] = dict(self.source, commit='b' * 40)
        provenance.write_json(self.inputs / 'build-manifest-fixture.json', self.manifest)
        with self.assertRaisesRegex(ValueError, 'source SHA'):
            self.collect()
        self.manifest['source'] = self.source
        provenance.write_json(self.inputs / 'build-manifest-fixture.json', self.manifest)
        provenance.write_json(self.inputs / 'build-manifest-duplicate.json', self.manifest)
        with self.assertRaisesRegex(ValueError, 'duplicate build provenance'):
            self.collect()
        (self.inputs / 'build-manifest-duplicate.json').unlink()
        (self.inputs / 'opaque.exe').write_bytes(b'untracked fixture')
        with self.assertRaisesRegex(ValueError, 'no unique source provenance'):
            self.collect()

    def test_stub_check_only_and_developer_release_profiles_are_rejected(self):
        for variable, value in [('I2P_EMBED_STUB', '1'), ('I2P_EMBED_SKIP_NATIVE', '1'),
                                ('CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS', 'true')]:
            with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', variable: value}, clear=True):
                with self.assertRaises(ValueError):
                    provenance.guard()
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}, clear=True):
            provenance.guard()


class ReviewedLockfiles(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.names = ('Cargo.lock', 'core/relay/Cargo.lock', 'ui/package-lock.json')
        self.committed = {name: f'synthetic reviewed input: {name}\n'.encode() for name in self.names}
        for name, content in self.committed.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        for target in (patch.object(provenance, 'ROOT', self.root),
                       patch.object(provenance, 'git', side_effect=self.git_blob)):
            target.start()
            self.addCleanup(target.stop)

    def git_blob(self, *args):
        self.assertEqual(args[0], 'show')
        self.assertTrue(args[1].startswith('HEAD:'))
        return self.committed[args[1][5:]]

    def test_all_reviewed_lockfiles_have_the_exact_committed_digest(self):
        self.assertEqual(provenance.reviewed_lockfiles(), {
            name: hashlib.sha256(content).hexdigest() for name, content in self.committed.items()
        })

    def test_crlf_checkout_is_diagnosed_and_rejected_instead_of_normalized(self):
        (self.root / self.names[0]).write_bytes(self.committed[self.names[0]].replace(b'\n', b'\r\n'))
        with self.assertRaisesRegex(ValueError, 'CRLF checkout'):
            provenance.reviewed_lockfiles()

    def test_content_drift_is_rejected_even_when_it_is_valid_text(self):
        (self.root / self.names[1]).write_bytes(b'different synthetic dependency\n')
        with self.assertRaisesRegex(ValueError, 'dependency content differs'):
            provenance.reviewed_lockfiles()

    def test_missing_lockfile_is_not_omitted_from_the_manifest(self):
        (self.root / self.names[2]).unlink()
        with self.assertRaisesRegex(ValueError, 'lockfile is missing'):
            provenance.reviewed_lockfiles()


if __name__ == '__main__':
    unittest.main()
