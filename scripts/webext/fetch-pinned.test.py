"""Pin bootstrap regression checks with a local fake download, never the network."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest
import zipfile


class FetchPinned(unittest.TestCase):
    def test_missing_pin_requires_explicit_bootstrap_then_enforces_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / 'fetch-pinned.sh'
            shutil.copyfile(Path(__file__).with_name('fetch-pinned.sh'), script)
            identity = 'a' * 32
            pin = root / 'pinned-crx.toml'
            pin.write_text(f'[[extension]]\nid = "{identity}"\nversion = ""\nsha256 = ""\n')
            payload = io.BytesIO()
            with zipfile.ZipFile(payload, 'w') as archive:
                archive.writestr('manifest.json', json.dumps({'version': '1.2.3'}))
            crx = b'Cr24' + (3).to_bytes(4, 'little') + (0).to_bytes(4, 'little') + payload.getvalue()
            download = root / 'download.crx'
            download.write_bytes(crx)
            commands = root / 'bin'
            commands.mkdir()
            curl = commands / 'curl'
            curl.write_text('#!/bin/sh\nwhile [ "$1" != -o ]; do shift; done\ncp "$FAKE_DOWNLOAD" "$2"\n')
            curl.chmod(0o700)
            environment = dict(os.environ, PATH=str(commands) + ':' + os.environ['PATH'], XDG_CACHE_HOME=str(root / 'cache'), FAKE_DOWNLOAD=str(download))

            def fetch(*args):
                return subprocess.run(['sh', str(script), *args], env=environment, capture_output=True, text=True, check=False)

            result = fetch('--extract')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('unresolved pin', result.stderr)
            result = fetch('--pin-missing', '--extract')
            self.assertEqual(result.returncode, 0, result.stderr)
            entry = tomllib.loads(pin.read_text())['extension'][0]
            self.assertEqual(entry['version'], '1.2.3')
            self.assertEqual(entry['sha256'], hashlib.sha256(crx).hexdigest())
            self.assertEqual(fetch('--extract').returncode, 0)
            cached = root / 'cache' / 'zephium' / 'webext-pinned' / f'{identity}-1.2.3.crx'
            cached.write_bytes(b'corrupted')
            download.write_bytes(b'different download')
            result = fetch('--pin-missing', '--extract')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('sha256 mismatch', result.stderr)
            self.assertEqual(tomllib.loads(pin.read_text())['extension'][0], entry)


if __name__ == '__main__':
    unittest.main()
