"""Export an authenticated Chrome cookie jar from the host that owns the profile.

Run on notebook or pc, not on the bot host: Chrome's AES key is in that host's
login keyring. Writes a Netscape jar to stdout; writes only a summary to stderr.
"""

import glob
import hashlib
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

PLATFORM_DOMAINS = (
    "youtube.com", "youtube-nocookie.com", "youtu.be", "google.com",
    "googlevideo.com", "ytimg.com", "pinterest.com", "pinterest.ca",
    "pinterest.co.uk", "pin.it", "pinimg.com", "x.com", "twitter.com",
    "t.co", "twimg.com", "instagram.com", "cdninstagram.com",
    "tiktok.com", "tiktokcdn.com", "tiktokv.com",
)
HOST_LABEL = re.compile(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\Z")
DEFAULT_DB = Path.home() / ".config/google-chrome/Default/Cookies"


def safe_field(value):
    return isinstance(value, str) and all(32 <= ord(char) < 127 or ord(char) >= 160 for char in value)


def db_mtime_ns(database):
    """Return the newest mtime of the database and its SQLite sidecars."""
    newest = 0
    for candidate in (
        database,
        database.parent / (database.name + "-wal"),
        database.parent / (database.name + "-shm"),
    ):
        try:
            newest = max(newest, candidate.stat().st_mtime_ns)
        except OSError:
            continue
    return newest


def secret_tools():
    found = [shutil.which("secret-tool")]
    found.extend(sorted(glob.glob("/nix/store/*-libsecret-*/bin/secret-tool")))
    for candidate in found:
        if candidate and os.access(candidate, os.X_OK):
            yield candidate


def get_secret():
    for tool in secret_tools():
        for attrs in (
            ["application", "chrome", "xdg:schema", "chrome_libsecret_os_crypt_password_v2"],
            ["application", "chrome"],
        ):
            result = subprocess.run([tool, "lookup", *attrs], stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL)
            if result.returncode == 0 and result.stdout:
                return result.stdout.rstrip(b"\n")
    raise RuntimeError("Chrome Safe Storage key unavailable")


BLOCK = 16
CBC_IV = b" " * BLOCK


def pkcs7_unpad(plaintext):
    """Strip PKCS#7 padding, or return None when it is malformed."""
    if not plaintext:
        return None
    padding = plaintext[-1]
    if not 1 <= padding <= BLOCK:
        return None
    if plaintext[-padding:] != bytes([padding]) * padding:
        return None
    return plaintext[:-padding]


def ciphertext_of(encrypted):
    if not (encrypted.startswith(b"v10") or encrypted.startswith(b"v11")):
        return None
    ciphertext = encrypted[3:]
    if not ciphertext or len(ciphertext) % BLOCK:
        return None
    return ciphertext


class AesCbc:
    """Decrypt many block-aligned ciphertexts with one openssl call.

    CBC decryption resolves per block, so every ciphertext is prefixed with the
    cookie IV and the whole stream is decrypted with a zero IV and -nopad: the
    trailing len(ciphertext) bytes of each chunk then hold its plaintext,
    because each chunk's predecessor is again its own IV.
    """

    def __init__(self, key):
        self.key = key

    def decrypt_many(self, ciphertexts):
        """Return the padded plaintext per ciphertext, or None per failure."""
        if not ciphertexts:
            return []
        payload = b"".join(CBC_IV + ciphertext for ciphertext in ciphertexts)
        result = subprocess.run(
            ["openssl", "enc", "-d", "-aes-128-cbc", "-K", self.key.hex(),
             "-iv", bytes(BLOCK).hex(), "-nopad"],
            input=payload, capture_output=True,
        )
        if result.returncode != 0 or len(result.stdout) != len(payload):
            return [None] * len(ciphertexts)
        plaintexts = []
        offset = 0
        for ciphertext in ciphertexts:
            start = offset + BLOCK
            plaintexts.append(result.stdout[start:start + len(ciphertext)])
            offset = start + len(ciphertext)
        return plaintexts


def unwrap(plaintext, host, version):
    """Turn block plaintext into a cookie value, honouring the host hash."""
    if plaintext is None:
        return None
    plaintext = pkcs7_unpad(plaintext)
    if plaintext is None:
        return None
    host_hash = hashlib.sha256(host.encode()).digest()
    if plaintext.startswith(host_hash):
        plaintext = plaintext[32:]
    elif version >= 24:
        return None
    try:
        return plaintext.decode("utf-8")
    except UnicodeDecodeError:
        return None


def decrypt(encrypted, host, key, version):
    ciphertext = ciphertext_of(encrypted)
    if ciphertext is None:
        return None
    return unwrap(AesCbc(key).decrypt_many([ciphertext])[0], host, version)


def main():
    database = Path(sys.argv[1]).expanduser() if len(sys.argv) > 1 else Path(
        os.environ.get("CHROME_COOKIE_DB") or DEFAULT_DB).expanduser()
    if not database.is_file():
        raise SystemExit("Chrome cookie database unavailable")
    secret = get_secret()
    key = hashlib.pbkdf2_hmac("sha1", secret, b"saltysalt", 1, 16)
    peanuts = hashlib.pbkdf2_hmac("sha1", b"peanuts", b"saltysalt", 1, 16)
    connection = sqlite3.connect(database.resolve().as_uri() + "?mode=ro",
                                 uri=True, timeout=10)
    version_row = connection.execute("select value from meta where key='version'").fetchone()
    version = int(version_row[0]) if version_row else 0
    now = int(time.time())
    rows = connection.execute(
        "select host_key,name,encrypted_value,value,path,expires_utc,"
        "is_secure,is_httponly,is_persistent from cookies"
    ).fetchall()
    output = ["# Netscape HTTP Cookie File\n"]
    counts = {}
    skipped = 0
    candidates = []
    pending = []
    for host, name, encrypted, plain, path, expires, secure, http_only, persistent in rows:
        domain = host.lower().lstrip(".")
        if (not domain or host.startswith("..") or not safe_field(host) or not safe_field(name)
                or not path.startswith("/") or not name
                or not all(HOST_LABEL.fullmatch(label) for label in domain.split("."))
                or not any(domain == suffix or domain.endswith("." + suffix)
                           for suffix in PLATFORM_DOMAINS)):
            continue
        candidates.append((domain, host, name, plain, path, expires,
                           secure, http_only, persistent))
        ciphertext = None if plain else ciphertext_of(encrypted)
        if ciphertext is not None:
            pending.append((len(candidates) - 1, ciphertext))
    values = {}
    if pending:
        retry = []
        for (index, ciphertext), plaintext in zip(
                pending, AesCbc(key).decrypt_many([c for _, c in pending])):
            value = unwrap(plaintext, candidates[index][1], version)
            if value is None:
                retry.append((index, ciphertext))
            else:
                values[index] = value
        if retry:
            for (index, _), plaintext in zip(
                    retry, AesCbc(peanuts).decrypt_many([c for _, c in retry])):
                value = unwrap(plaintext, candidates[index][1], version)
                if value is not None:
                    values[index] = value
    for index, (domain, host, name, plain, path, expires,
                secure, http_only, persistent) in enumerate(candidates):
        value = plain if plain else values.get(index)
        if value is None or not safe_field(value):
            skipped += 1
            continue
        expiry = 0 if not persistent or not expires else expires // 1_000_000 - 11644473600
        if expiry and expiry <= now:
            continue
        output.append(
            f"{'#HttpOnly_' if http_only else ''}{host}\t"
            f"{'TRUE' if host.startswith('.') else 'FALSE'}\t{path}\t"
            f"{'TRUE' if secure else 'FALSE'}\t{expiry}\t{name}\t{value}\n"
        )
        counts[domain] = counts.get(domain, 0) + 1
    if len(output) == 1:
        raise SystemExit("No valid platform cookies")
    sys.stdout.write("".join(output))
    print(f"exported {sum(counts.values())} cookies, skipped {skipped}; "
          f"version {version}; db_mtime_ns={db_mtime_ns(database)}", file=sys.stderr)
    for domain in sorted(counts):
        print(f"  {domain}: {counts[domain]}", file=sys.stderr)


if __name__ == "__main__":
    main()
