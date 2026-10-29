import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

try:
    from tools.export_chrome_cookies import AesCbc, db_mtime_ns, decrypt, pkcs7_unpad, unwrap
except ModuleNotFoundError:
    from export_chrome_cookies import AesCbc, db_mtime_ns, decrypt, pkcs7_unpad, unwrap


def encrypt(plaintext, key):
    padding = 16 - len(plaintext) % 16
    padded = plaintext + bytes([padding]) * padding
    result = subprocess.run(
        ["openssl", "enc", "-e", "-aes-128-cbc", "-K", key.hex(),
         "-iv", (b" " * 16).hex(), "-nopad"],
        input=padded, capture_output=True, check=True,
    )
    return b"v10" + result.stdout


class PaddingTests(unittest.TestCase):
    def test_pkcs7_padding_is_stripped(self):
        self.assertEqual(pkcs7_unpad(b"data" + bytes([12]) * 12), b"data")
        self.assertEqual(pkcs7_unpad(bytes([16]) * 16), b"")

    def test_malformed_padding_is_rejected(self):
        for data in (b"", b"data" + bytes([5]) * 4, b"data" + bytes([17]),
                     b"x" * 15 + b"\x02", b"data" + bytes([9, 3])):
            with self.subTest(data=data):
                self.assertIsNone(pkcs7_unpad(data))


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


@unittest.skipUnless(shutil.which("openssl"), "openssl is required by the exporter")
class BatchDecryptTests(unittest.TestCase):
    def setUp(self):
        self.key = bytes(range(16))

    def test_many_cookies_need_one_openssl_call(self):
        hosts = [f".h{index}.example.com" for index in range(6)]
        payloads = [hashlib.sha256(host.encode()).digest() + b"value-%d" % index
                    for index, host in enumerate(hosts)]
        ciphertexts = [encrypt(payload, self.key)[3:] for payload in payloads]
        calls = []
        real_run = subprocess.run

        def counting_run(command, *args, **kwargs):
            calls.append(command)
            return real_run(command, *args, **kwargs)

        with mock.patch.object(subprocess, "run", counting_run):
            plaintexts = AesCbc(self.key).decrypt_many(ciphertexts)
        self.assertEqual(len(calls), 1)
        self.assertEqual([pkcs7_unpad(plaintext) for plaintext in plaintexts], payloads)

    def test_batch_matches_single_value_decryption(self):
        hosts = [".example.com", ".youtube.com", ".x.com"]
        values = ["a", "b" * 20, "c" * 32]
        payloads = [hashlib.sha256(host.encode()).digest() + value.encode()
                    for host, value in zip(hosts, values)]
        encrypted = [encrypt(payload, self.key) for payload in payloads]
        batched = AesCbc(self.key).decrypt_many([blob[3:] for blob in encrypted])
        for host, blob, plaintext in zip(hosts, encrypted, batched):
            self.assertEqual(unwrap(plaintext, host, 24), decrypt(blob, host, self.key, 24))
        self.assertEqual([decrypt(blob, host, self.key, 24)
                          for host, blob in zip(hosts, encrypted)], values)

    def test_failed_batch_yields_no_values(self):
        self.assertEqual(AesCbc(self.key).decrypt_many([]), [])
        failure = subprocess.CompletedProcess([], 1, b"")
        with mock.patch.object(subprocess, "run", return_value=failure):
            self.assertEqual(
                AesCbc(self.key).decrypt_many([b"\x00" * 16, b"\x01" * 32]), [None, None])


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
