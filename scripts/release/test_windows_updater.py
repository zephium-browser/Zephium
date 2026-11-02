"""Keep the Windows signature producer with the updater that requires it."""
import json
import os
import re
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class WindowsUpdaterWorkflowTests(unittest.TestCase):
    def test_installer_downgrade_policy_uses_a_fixed_bundler(self):
        config = json.loads((ROOT / 'desktop/tauri.windows.conf.json').read_text())
        self.assertIs(config['bundle']['windows']['allowDowngrades'], False)
        package = json.loads((ROOT / 'desktop/package.json').read_text())
        version = package['devDependencies']['@tauri-apps/cli']
        self.assertRegex(version, r'^2\.\d+\.\d+$', 'pin a stable reviewed CLI release')
        self.assertGreaterEqual(tuple(map(int, version.split('.'))), (2, 12, 4),
                                'older bundled NSIS templates permit unattended downgrades')
        lock = (ROOT / 'pnpm-lock.yaml').read_text()
        desktop = lock.split('  desktop:\n', 1)[1].split('\n  frame:', 1)[0]
        self.assertIn(f'specifier: {version}\n        version: {version}', desktop)

    def producer(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        windows = workflow.split('\n  windows:\n', 1)[1].split('\n  publish:\n', 1)[0]
        match = re.search(
            r'      - name: Sign the final Windows bytes for the updater\n(.*?)(?=\n      - )',
            windows, re.S)
        self.assertIsNotNone(match, 'the updater consumer must ship with its release signature producer')
        step = match.group(1)
        self.assertNotIn('        if:', step)
        self.assertIn('TAURI_SIGNING_PRIVATE_KEY:', step)
        self.assertIn('TAURI_SIGNING_PRIVATE_KEY_PASSWORD:', step)
        self.assertLess(windows.index('      - name: Build\n'), match.start())
        self.assertLess(match.end(), windows.index('      - name: Stage release assets\n'))
        self.assertIn('"sign-windows-updater"', (ROOT / 'xtask/src/main.rs').read_text())
        return ' '.join(textwrap.dedent(step.split('        run: >-\n', 1)[1]).splitlines())

    def test_producer_is_unconditional_and_exists_on_the_consumer_branch(self):
        self.producer()

    def test_producer_signs_the_final_installer_and_fails_closed(self):
        script = self.producer()
        for result in [0, 7]:
            with self.subTest(result=result), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                artifact = root / 'target/release/bundle/nsis/Zephium_1.2.4_x64-setup.exe'
                artifact.parent.mkdir(parents=True)
                artifact.write_bytes(b'final publisher-signed installer bytes')
                cargo = root / 'cargo'
                cargo.write_text('''#!/bin/sh
test "$1 $2 $3 $4 $5 $6" = "run --locked -p xtask -- sign-windows-updater" || exit 90
test "$7" = "target/release/bundle/nsis/Zephium_1.2.4_x64-setup.exe" || exit 91
test "$8" = "1.2.4" || exit 92
test "$(cat "$7")" = "final publisher-signed installer bytes" || exit 93
test "$TAURI_SIGNING_PRIVATE_KEY" = fixture-key || exit 94
test "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" = fixture-password || exit 95
exit "$SIGN_RESULT"
''')
                cargo.chmod(0o755)
                run = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=root,
                    env=dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', VERSION='1.2.4',
                             TAURI_SIGNING_PRIVATE_KEY='fixture-key',
                             TAURI_SIGNING_PRIVATE_KEY_PASSWORD='fixture-password', SIGN_RESULT=str(result)),
                    capture_output=True, text=True)
                self.assertEqual(run.returncode, result, run.stderr)


if __name__ == '__main__':
    unittest.main()
