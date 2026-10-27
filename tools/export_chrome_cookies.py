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


def decrypt(encrypted, host, key, version):
    if not (encrypted.startswith(b"v10") or encrypted.startswith(b"v11")):
        return None
    ciphertext = encrypted[3:]
    if len(ciphertext) % 16:
        return None
    result = subprocess.run(
        ["openssl", "enc", "-d", "-aes-128-cbc", "-K", key.hex(),
         "-iv", (b" " * 16).hex(), "-nopad"],
        input=ciphertext, capture_output=True,
    )
    if result.returncode != 0 or not result.stdout:
        return None
    plaintext = result.stdout
    padding = plaintext[-1]
    if not (1 <= padding <= 16 and plaintext.endswith(bytes([padding]) * padding)):
        return None
    plaintext = plaintext[:-padding]
    host_hash = hashlib.sha256(host.encode()).digest()
    if plaintext.startswith(host_hash):
        plaintext = plaintext[32:]
    elif version >= 24:
        return None
    try:
        return plaintext.decode("utf-8")
    except UnicodeDecodeError:
        return None


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
    for host, name, encrypted, plain, path, expires, secure, http_only, persistent in rows:
        domain = host.lower().lstrip(".")
        if (not domain or host.startswith("..") or not safe_field(host) or not safe_field(name)
                or not path.startswith("/") or not name
                or not all(HOST_LABEL.fullmatch(label) for label in domain.split("."))
                or not any(domain == suffix or domain.endswith("." + suffix)
                           for suffix in PLATFORM_DOMAINS)):
            continue
        value = plain if plain else None
        if value is None:
            value = decrypt(encrypted, host, key, version)
        if value is None:
            value = decrypt(encrypted, host, peanuts, version)
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
