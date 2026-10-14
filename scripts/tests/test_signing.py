import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('manifest', ROOT / 'scripts/release-manifest.py')
manifest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manifest)

MAC = dict(APPLE_CERTIFICATE='private-p12-fixture', APPLE_CERTIFICATE_PASSWORD='private-password-fixture',
           APPLE_SIGNING_IDENTITY='Developer ID Application: Test Publisher (ABCDEFGHIJ)',
           APPLE_ID='fixture@example.invalid', APPLE_PASSWORD='private-notary-fixture', APPLE_TEAM_ID='ABCDEFGHIJ')
WIN = dict.fromkeys(['AZURE_CLIENT_ID', 'AZURE_TENANT_ID', 'AZURE_SUBSCRIPTION_ID', 'AZURE_SIGNING_ENDPOINT',
                     'AZURE_SIGNING_ACCOUNT', 'AZURE_CERTIFICATE_PROFILE', 'WINDOWS_PUBLISHER_SUBJECT'], 'fixture')
WIN['WINDOWS_SIGNING_PROVIDER'] = 'azure-artifact-signing'


class BuildConfigTests(unittest.TestCase):
    def config(self, platform, mode, values=None):
        env = {'PATH': os.environ['PATH'], **(values or {})}
        return subprocess.run(['bash', str(ROOT / 'scripts/ci-build-config.sh'), '0.2.10', platform, mode],
                              env=env, text=True, capture_output=True)

    def test_ci_ignores_even_present_credentials(self):
        for platform in ['macos', 'windows']:
            p = self.config(platform, 'ci', {**MAC, **WIN, 'TAURI_SIGNING_PRIVATE_KEY': 'private-updater-fixture'})
            self.assertEqual(p.returncode, 0, p.stderr)
            config = json.loads(p.stdout)
            self.assertFalse(config['bundle']['createUpdaterArtifacts'])
            self.assertNotIn('windows', config['bundle'])
            if platform == 'macos':
                self.assertEqual(config['bundle']['macOS']['signingIdentity'], '-')
                self.assertFalse(config['bundle']['macOS']['hardenedRuntime'])

    def test_every_missing_release_field_blocks_output(self):
        for platform, values in [('macos', MAC), ('windows', WIN)]:
            for key in values:
                with self.subTest(platform=platform, missing=key):
                    p = self.config(platform, 'release', {k: v for k, v in values.items() if k != key})
                    self.assertNotEqual(p.returncode, 0)
                    self.assertIn(key, p.stderr)
                    self.assertEqual(p.stdout, '')
                    self.assertNotIn('private-', p.stderr)

    def test_mismatched_apple_team_and_non_developer_id_rejected(self):
        for identity in ['-', 'Apple Development: Test (ABCDEFGHIJ)', 'Developer ID Application: Test (ZYXWVUTSRQ)']:
            p = self.config('macos', 'release', {**MAC, 'APPLE_SIGNING_IDENTITY': identity})
            self.assertNotEqual(p.returncode, 0)

    def test_release_configuration(self):
        p = self.config('macos', 'release', MAC)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertTrue(json.loads(p.stdout)['bundle']['macOS']['hardenedRuntime'])
        p = self.config('windows', 'release', WIN)
        self.assertEqual(p.returncode, 0, p.stderr)
        command = json.loads(p.stdout)['bundle']['windows']['signCommand']
        self.assertEqual(command['cmd'], 'pwsh')
        self.assertEqual(command['args'][-1], '%1')
        self.assertTrue((ROOT / 'app/src-tauri' / command['args'][3]).is_file())

    def test_unknown_modes_and_platforms_rejected(self):
        for platform, mode in [('macos', 'Release'), ('linux', 'release'), ('windows', '')]:
            self.assertNotEqual(self.config(platform, mode).returncode, 0)

    def test_unimplemented_provider_cannot_silently_use_azure(self):
        p = self.config('windows', 'release', {**WIN, 'WINDOWS_SIGNING_PROVIDER': 'another-provider'})
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('WINDOWS_SIGNING_PROVIDER', p.stderr)
        self.assertEqual(p.stdout, '')


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dist = Path(self.tmp.name)
        self.env = patch.dict(os.environ, GITHUB_SHA='a' * 40)
        self.env.start()
        self.addCleanup(self.env.stop)
        for stem, status in manifest.EXPECTED.items():
            name = stem + '.zip' if stem.startswith('Gatit-macos-') else 'Gatit-windows-setup.exe'
            (self.dist / name).write_bytes(b'verified artifact fixture')
            data = dict(status=status, publisher='approved fixture', commit=os.environ['GITHUB_SHA'],
                        artifacts={name: manifest.digest(self.dist / name)})
            (self.dist / (stem + '.verification.json')).write_text(json.dumps(data))

    def change_receipt(self, stem, update):
        p = self.dist / (stem + '.verification.json')
        d = json.loads(p.read_text())
        update(d)
        p.write_text(json.dumps(d))

    def test_complete_manual_release_passes(self):
        manifest.verify(self.dist)

    def test_complete_updater_release_passes(self):
        for stem in manifest.EXPECTED:
            package = stem + '.app.tar.gz' if stem.startswith('Gatit-macos-') else 'Gatit-windows-setup.exe'
            names = [package, package + '.sig']
            for name in names:
                (self.dist / name).write_text('fixture signed updater bytes')
            self.change_receipt(stem, lambda d: d['artifacts'].update(
                {name: manifest.digest(self.dist / name) for name in names}))
        manifest.verify(self.dist)

    def test_missing_download_fails(self):
        (self.dist / 'Gatit-windows-setup.exe').unlink()
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_modified_download_fails(self):
        (self.dist / 'Gatit-windows-setup.exe').write_bytes(b'modified after signing')
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_one_notarized_mac_does_not_mask_other_adhoc_mac(self):
        self.change_receipt('Gatit-macos-intel', lambda d: d.update(status='ad-hoc'))
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_wrong_commit_fails(self):
        self.change_receipt('Gatit-windows', lambda d: d.update(commit='b' * 40))
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_missing_platform_or_receipt_fails(self):
        (self.dist / 'Gatit-macos-intel.verification.json').unlink()
        with self.assertRaises(OSError): manifest.verify(self.dist)

    def test_extra_unverified_download_fails(self):
        (self.dist / 'unverified.exe').write_bytes(b'unsigned')
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_partial_updates_fail(self):
        sig = self.dist / 'Gatit-windows-setup.exe.sig'
        sig.write_text('fixture')
        self.change_receipt('Gatit-windows', lambda d: d['artifacts'].update({sig.name: manifest.digest(sig)}))
        with self.assertRaises(ValueError): manifest.verify(self.dist)

    def test_receipt_cannot_reference_outside_dist(self):
        self.change_receipt('Gatit-windows', lambda d: d['artifacts'].update({'../extra.exe': 'a' * 64}))
        with self.assertRaises(ValueError): manifest.verify(self.dist)


if __name__ == '__main__':
    unittest.main()
