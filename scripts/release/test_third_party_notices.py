"""The generated API surface must retain Chromium's distribution notice."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import third_party_notices


class ChromiumNoticesTest(unittest.TestCase):
    def test_schema_license_ships_without_a_cargo_or_npm_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "notices.txt"
            with patch.object(third_party_notices, "rust_packages", return_value={}), \
                 patch.object(third_party_notices, "npm_packages", return_value=[]), \
                 patch("sys.argv", ["third_party_notices.py", str(output)]):
                self.assertEqual(third_party_notices.main(), 0)
            text = output.read_text(encoding="utf-8")
            snapshot = third_party_notices.ROOT / "vendor/chromium-extension-api"
            self.assertIn("Chromium extension API schemas", text)
            self.assertIn((snapshot / "LICENSE").read_text(encoding="utf-8").strip(), text)
            self.assertIn((snapshot / "UPSTREAM.md").read_text(encoding="utf-8").strip(), text)


if __name__ == "__main__":
    unittest.main()
