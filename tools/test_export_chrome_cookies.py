import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

try:
    from tools.export_chrome_cookies import db_mtime_ns, decrypt
except ModuleNotFoundError:
    from export_chrome_cookies import db_mtime_ns, decrypt


def encrypt(plaintext, key):
    padding = 16 - len(plaintext) % 16
    padded = plaintext + bytes([padding]) * padding
    result = subprocess.run(
        ["openssl", "enc", "-e", "-aes-128-cbc", "-K", key.hex(),
         "-iv", (b" " * 16).hex(), "-nopad"],
        input=padded, capture_output=True, check=True,
    )
    return b"v10" + result.stdout


@unittest.skipUnless(shutil.which("openssl"), "openssl is required by the exporter")
class ChromeDecryptTests(unittest.TestCase):
    def setUp(self):
        self.key = bytes(range(16))
        self.host = ".example.com"

    def test_decrypt_strips_host_hash_and_pkcs7_padding(self):
        payload = hashlib.sha256(self.host.encode()).digest() + b"secret-value"
        self.assertEqual(
            decrypt(encrypt(payload, self.key), self.host, self.key, 24),
            "secret-value",
        )

    def test_version_24_requires_host_hash(self):
        self.assertIsNone(decrypt(encrypt(b"secret-value", self.key), self.host, self.key, 24))

    def test_wrong_key_is_rejected(self):
        payload = hashlib.sha256(self.host.encode()).digest() + b"secret-value"
        self.assertIsNone(
            decrypt(encrypt(payload, self.key), self.host, bytes([7]) * 16, 24)
        )

    def test_non_v10_or_v11_values_are_ignored(self):
        payload = hashlib.sha256(self.host.encode()).digest() + b"secret-value"
        self.assertIsNone(decrypt(b"v12" + encrypt(payload, self.key)[3:], self.host,
                                  self.key, 24))


class ChromeMtimeTests(unittest.TestCase):
    def test_db_mtime_uses_newest_database_or_sidecar(self):
        with tempfile.TemporaryDirectory() as root:
            database = Path(root) / "Cookies"
            for name, timestamp in (
                ("Cookies", 1_000_000_000),
                ("Cookies-wal", 2_000_000_000),
                ("Cookies-shm", 1_500_000_000),
            ):
                path = database.parent / name
                path.write_bytes(b"x")
                os.utime(path, ns=(timestamp, timestamp))
            self.assertEqual(db_mtime_ns(database), 2_000_000_000)

    def test_db_mtime_ignores_missing_sidecars(self):
        with tempfile.TemporaryDirectory() as root:
            database = Path(root) / "Cookies"
            database.write_bytes(b"x")
            os.utime(database, ns=(3_000_000_000, 3_000_000_000))
            self.assertEqual(db_mtime_ns(database), 3_000_000_000)


if __name__ == "__main__":
    unittest.main()
