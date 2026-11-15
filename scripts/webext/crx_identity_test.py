import hashlib
import pathlib
import subprocess
import tempfile
import unittest
from crx_identity import fields, identity_key

class CrxIdentityTests(unittest.TestCase):
    def test_protobuf_bounds_and_supported_wire_values(self):
        self.assertEqual(list(fields(bytes([8, 7, 18, 2, 65, 66]))), [(1, 0, 7), (2, 2, b"AB")])
        for malformed in [b"\x00", b"\x80", b"\x80" * 10, b"\x12\xff\xff\x7f", b"\x0b", b"\x09\x00"]:
            with self.assertRaises(ValueError): list(fields(malformed))

    def test_publisher_signature_authenticates_archive_and_identity(self):
        def varint(value):
            output = bytearray()
            while value > 127:
                output.append((value & 127) | 128)
                value >>= 7
            output.append(value)
            return bytes(output)

        def field(number, value):
            return varint((number << 3) | 2) + varint(len(value)) + value

        with tempfile.TemporaryDirectory(prefix="crx-proof-test-") as temporary:
            root = pathlib.Path(temporary)
            def openssl(*args):
                return subprocess.run(["openssl", *args], check=True, capture_output=True, timeout=30)
            openssl("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(root / "private.pem"))
            openssl("pkey", "-in", str(root / "private.pem"), "-pubout", "-outform", "DER", "-out", str(root / "public.der"))
            key = (root / "public.der").read_bytes()
            digest = hashlib.sha256(key).digest()[:16]
            identity = "".join(chr(97 + value) for byte in digest for value in (byte >> 4, byte & 15))
            signed = field(1, digest)
            archive = b"authenticated test archive bytes"
            (root / "message").write_bytes(b"CRX3 SignedData\x00" + len(signed).to_bytes(4, "little") + signed + archive)
            openssl("dgst", "-sha256", "-sign", str(root / "private.pem"), "-out", str(root / "signature"), str(root / "message"))
            header = field(2, field(1, key) + field(2, (root / "signature").read_bytes())) + field(10000, signed)
            package = b"Cr24\x03\x00\x00\x00" + len(header).to_bytes(4, "little") + header + archive
            self.assertTrue(identity_key(package, identity))
            with self.assertRaises(subprocess.CalledProcessError):
                identity_key(package[:-1] + bytes([package[-1] ^ 1]), identity)
            with self.assertRaises(ValueError):
                identity_key(package, "a" * 32)

    def test_identity_checks_run_before_loading_a_package(self):
        for malformed in [b"", b"Cr24", b"Cr24\x02\x00\x00\x00" + bytes(4), b"Cr24\x03\x00\x00\x00" + (65537).to_bytes(4, "little"), b"Cr24\x03\x00\x00\x00" + bytes(4)]:
            with self.assertRaises(ValueError): identity_key(malformed, "a" * 32)

if __name__ == "__main__": unittest.main()
