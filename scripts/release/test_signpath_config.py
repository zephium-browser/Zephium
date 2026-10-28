import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
KEYS = ('SIGNPATH_API_TOKEN', 'SIGNPATH_RELEASE_CERT_THUMBPRINT')
PIN = 'A' * 40


class SignPathConfigurationTests(unittest.TestCase):
    def test_windows_job_covers_build_and_signing_approval_budgets(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        windows = workflow.split('\n  windows:\n', 1)[1].split('\n  publish:\n', 1)[0]
        job = int(re.search(r'^    timeout-minutes: (\d+)', windows, re.M).group(1))
        build = re.search(r'- name: Build\n        timeout-minutes: (\d+)', windows)
        stages = ['Sign packaged browser executable', 'Sign generated uninstaller', 'Sign final installer']
        signing = [re.search(rf'- name: {name}\n        timeout-minutes: (\d+)', windows) for name in stages]
        self.assertIsNotNone(build, 'the build needs an explicit bounded share of the job')
        self.assertTrue(all(signing), 'all three signing stages need bounded approval budgets')
        budgets = [int(stage.group(1)) for stage in signing]
        action = (ROOT / '.github/actions/signpath-sign/action.yml').read_text()
        waits = sum(int(re.search(rf'{field}: (\d+)', action).group(1)) for field in (
            'wait-for-completion-timeout-in-seconds',
            'service-unavailable-timeout-in-seconds',
            'download-signed-artifact-timeout-in-seconds',
        ))
        for budget in budgets:
            # Hook readiness (600), bounded verification (300), upload/overhead (120).
            self.assertGreaterEqual(budget * 60, waits + 600 + 300 + 120)
        self.assertGreaterEqual(job, int(build.group(1)) + sum(budgets) + 30)
        self.assertLessEqual(job, 360)

    def test_final_updater_signature_binds_version_and_target_after_signpath(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        windows = workflow.split('\n  windows:\n', 1)[1].split('\n  publish:\n', 1)[0]
        signer = windows.split('      - name: Sign the final Windows bytes for the updater\n', 1)[1].split('\n      - name:', 1)[0]
        self.assertNotIn('        if:', signer)
        self.assertNotIn('working-directory:', signer)
        self.assertIn('cargo run --locked -p xtask -- sign-windows-updater', signer)
        self.assertIn('"target/release/bundle/nsis/Zephium_${VERSION}_x64-setup.exe" "${VERSION}"', signer)
        self.assertIn('TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}', signer)
        self.assertLess(windows.index('Sign final installer'), windows.index('-Operation Finish'))
        self.assertLess(windows.index('-Operation Finish'), windows.index('Sign the final Windows bytes for the updater'))
        self.assertLess(windows.index('-Operation Smoke'), windows.index('Sign the final Windows bytes for the updater'))
        for earlier, later in [('Sign packaged browser executable', 'Sign generated uninstaller'), ('Sign generated uninstaller', 'Sign final installer')]:
            self.assertLess(windows.index(earlier), windows.index(later))
        self.assertNotIn('needs.gate.outputs.signpath', windows)
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', windows.split('Sign the final Windows bytes for the updater')[0])
        self.assertLess(windows.index('Sign the final Windows bytes for the updater'), windows.index('Stage release assets'))
        self.assertEqual(windows.count('sign-windows-updater'), 1)
        self.assertNotIn('pnpm tauri signer sign', windows)

    def run_config(self, configured):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'output'
            env = {k: v for k, v in os.environ.items() if k not in KEYS}
            env.update(configured, GITHUB_OUTPUT=str(output))
            result = subprocess.run(['bash', str(ROOT / 'scripts/release/signpath_config.sh')],
                                    env=env, capture_output=True, text=True)
            return result.returncode, output.read_text() if output.exists() else '', result.stdout

    def test_absent_secrets_block_release(self):
        status, output, _ = self.run_config({})
        self.assertNotEqual(status, 0)
        self.assertEqual(output, '')

    def test_partial_configuration_never_silently_ships_unsigned(self):
        for configured in [{'SIGNPATH_API_TOKEN': 'private-fixture'}, {'SIGNPATH_RELEASE_CERT_THUMBPRINT': PIN}]:
            status, output, _ = self.run_config(configured)
            self.assertNotEqual(status, 0)
            self.assertEqual(output, '')

    def test_malformed_pins_are_rejected(self):
        for pin in ['fixture', 'A' * 39, 'A' * 41, 'Z' * 40, PIN + '\n']:
            status, output, _ = self.run_config({'SIGNPATH_API_TOKEN': 'private-fixture', 'SIGNPATH_RELEASE_CERT_THUMBPRINT': pin})
            self.assertNotEqual(status, 0)
            self.assertEqual(output, '')

    def test_complete_configuration_activates_without_disclosing_values(self):
        status, output, message = self.run_config({'SIGNPATH_API_TOKEN': 'private-fixture', 'SIGNPATH_RELEASE_CERT_THUMBPRINT': PIN})
        self.assertEqual(status, 0)
        self.assertEqual(output, 'enabled=true\n')
        self.assertNotIn('private-fixture', message)
        self.assertNotIn(PIN, message)


if __name__ == '__main__': unittest.main()
