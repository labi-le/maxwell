#!/usr/bin/env python3
"""Refresh Maksut's Chrome cookie jar from SSH-reachable browser hosts."""

import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SOURCES = frozenset(("youtube", "tiktok", "instagram", "twitter", "pinterest", "generic"))
MAX_REQUEST_BYTES = 4096
MAX_JAR_BYTES = 4 * 1024 * 1024
MAX_STDERR_BYTES = 64 * 1024
CACHE_SECONDS = 5.0
LOCK_SECONDS = 25.0
TARGET_RE = re.compile(r"[A-Za-z0-9_.@:-]+\Z")
MTIME_RE = re.compile(rb"db_mtime_ns=(\d+)")


class ConfigError(Exception):
    pass


class BadRequest(Exception):
    pass


class Unavailable(Exception):
    pass


@dataclass(frozen=True)
class Config:
    targets: tuple
    key: Path
    known_hosts: Path
    jar: Path
    exporter: Path
    ssh_timeout: float = 10.0
    port: int = 8080

    @classmethod
    def from_env(cls):
        targets = parse_targets(os.environ.get(
            "COOKIE_SSH_TARGETS", "labile@pc,labile@notebook"))
        try:
            timeout = float(os.environ.get("COOKIE_SSH_TIMEOUT", "10"))
            port = int(os.environ.get("COOKIE_REFRESH_PORT", "8080"))
        except ValueError as error:
            raise ConfigError("invalid numeric setting") from error
        if timeout <= 0 or port < 1 or port > 65535:
            raise ConfigError("invalid numeric setting")
        return cls(
            targets=targets,
            key=Path(os.environ.get(
                "COOKIE_SSH_KEY", "/run/secrets/cookie_refresh/id_ed25519")),
            known_hosts=Path(os.environ.get(
                "COOKIE_SSH_KNOWN_HOSTS", "/run/secrets/cookie_refresh/known_hosts")),
            jar=Path(os.environ.get("COOKIE_JAR", "/run/secrets/maksut/cookies.txt")),
            exporter=Path("/app/export_chrome_cookies.py"),
            ssh_timeout=timeout,
            port=port,
        )

    def validate(self):
        for path, label in (
            (self.key, "SSH key"),
            (self.known_hosts, "known-hosts"),
            (self.exporter, "exporter"),
        ):
            if not path.is_file() or not os.access(path, os.R_OK):
                raise ConfigError(f"{label} is unavailable")
        if not self.jar.parent.is_dir():
            raise ConfigError("cookie directory is unavailable")
        if self.jar.exists() and not self.jar.is_file():
            raise ConfigError("cookie path is not a file")


@dataclass(frozen=True)
class ExportResult:
    returncode: int
    stdout: bytes
    stderr: bytes


@dataclass(frozen=True)
class RefreshResult:
    host: str
    cookies: int
    cached: bool


@dataclass(frozen=True)
class CacheEntry:
    at: float
    identity: tuple
    cookies: tuple
    host: str


def parse_targets(raw):
    targets = []
    for part in raw.split(","):
        target = part.strip()
        if not target or target.startswith("-") or not TARGET_RE.fullmatch(target):
            raise ConfigError("invalid SSH target")
        targets.append(target)
    if not targets:
        raise ConfigError("no SSH targets")
    return tuple(targets)


def parse_jar(data):
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("cookie jar is not UTF-8") from error
    lines = text.splitlines()
    if not lines or lines[0].strip() != "# Netscape HTTP Cookie File":
        raise ValueError("not a Netscape cookie jar")
    cookies = []
    for line in lines[1:]:
        row = line.rstrip("\r")
        if not row or (row.startswith("#") and not row.startswith("#HttpOnly_")):
            continue
        fields = row.split("\t")
        if len(fields) != 7:
            raise ValueError("malformed cookie row")
        host, subdomains, path, secure, expiry, name, _value = fields
        host = host.removeprefix("#HttpOnly_")
        if (not host or not path.startswith("/") or not name
                or subdomains not in ("TRUE", "FALSE")
                or secure not in ("TRUE", "FALSE")):
            raise ValueError("malformed cookie row")
        try:
            expires = int(expiry)
        except ValueError as error:
            raise ValueError("malformed cookie expiry") from error
        if expires < 0:
            raise ValueError("malformed cookie expiry")
        cookies.append((host.lower(), name, expires))
    if not cookies:
        raise ValueError("cookie jar is empty")
    return tuple(cookies)


def _host_matches(host, domain):
    host = host.lower().lstrip(".")
    return host == domain or host.endswith("." + domain)


def covers(cookies, source):
    now = int(time.time())
    cookies = tuple(cookie for cookie in cookies
                    if cookie[2] == 0 or cookie[2] > now)
    if source == "generic":
        return bool(cookies)
    if source == "youtube":
        names = {"SID", "__Secure-1PSID", "LOGIN_INFO"}
        return any(_host_matches(host, "youtube.com") and name in names
                   for host, name, _ in cookies)
    if source == "tiktok":
        names = {"sessionid", "sessionid_ss"}
        return any(_host_matches(host, "tiktok.com") and name in names
                   for host, name, _ in cookies)
    if source == "instagram":
        names = {"sessionid", "ds_user_id"}
        return any(_host_matches(host, "instagram.com") and name in names
                   for host, name, _ in cookies)
    if source == "twitter":
        names = {"auth_token"}
        return any((_host_matches(host, "x.com") or _host_matches(host, "twitter.com"))
                   and name in names for host, name, _ in cookies)
    if source == "pinterest":
        return any(name == "csrftoken"
                   and (_host_matches(host, "pinterest.com")
                        or _host_matches(host, "pinterest.ca")
                        or _host_matches(host, "pinterest.co.uk")
                        or _host_matches(host, "pin.it"))
                   for host, name, _ in cookies)
    return False


def summary_mtime(stderr):
    match = MTIME_RE.search(stderr)
    return int(match.group(1)) if match else 0


def _drain(stream, limit):
    data = bytearray()
    while True:
        chunk = stream.read(4096)
        if not chunk:
            break
        if len(data) < limit + 1:
            data.extend(chunk[:limit + 1 - len(data)])
    return bytes(data)


def run_exporter(target, config, exporter_source):
    """Run the exporter on one SSH target without ever logging its output."""
    command = [
        "ssh", "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
        "-o", f"UserKnownHostsFile={config.known_hosts}",
        "-o", "IdentitiesOnly=yes", "-o", "ConnectTimeout=5",
        "-i", str(config.key), "--", target, "python3", "-",
    ]
    process = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = []
    errors = []
    readers = [
        threading.Thread(target=lambda: output.append(
            _drain(process.stdout, MAX_JAR_BYTES))),
        threading.Thread(target=lambda: errors.append(
            _drain(process.stderr, MAX_STDERR_BYTES))),
    ]
    for reader in readers:
        reader.daemon = True
        reader.start()
    try:
        process.stdin.write(exporter_source)
    except (BrokenPipeError, OSError):
        pass
    finally:
        try:
            process.stdin.close()
        except OSError:
            pass
    try:
        returncode = process.wait(timeout=config.ssh_timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        returncode = process.wait()
    for reader in readers:
        reader.join(timeout=2)
    return ExportResult(returncode, output[0] if output else b"",
                        errors[0] if errors else b"")


def install_jar(config, data):
    """Replace the jar atomically; a failure leaves the old file untouched."""
    parent = config.jar.parent
    fd, temporary = tempfile.mkstemp(prefix=".cookies-", dir=parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, config.jar)
        try:
            directory = os.open(parent, os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        except OSError:
            pass
    except Exception:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def _identity(path):
    try:
        stat = path.stat()
    except OSError:
        return None
    return (stat.st_ino, stat.st_mtime_ns, stat.st_size)


class RefreshService:
    def __init__(self, config, runner=None):
        self.config = config
        self.runner = runner or run_exporter
        try:
            self.exporter_source = config.exporter.read_bytes()
        except OSError:
            self.exporter_source = b""
        self.lock = threading.Lock()
        self.cache_lock = threading.Lock()
        self.last_success = None

    def _cached(self, source):
        with self.cache_lock:
            entry = self.last_success
        if entry is None or time.monotonic() - entry.at > CACHE_SECONDS:
            return None
        if _identity(self.config.jar) != entry.identity:
            return None
        if not covers(entry.cookies, source):
            return None
        return RefreshResult(entry.host, len(entry.cookies), True)

    def _remember(self, host, cookies):
        identity = _identity(self.config.jar)
        if identity is None:
            return
        with self.cache_lock:
            self.last_success = CacheEntry(
                time.monotonic(), identity, cookies, host)

    def refresh(self, source):
        if source not in SOURCES:
            raise BadRequest("unsupported source")
        if not self.lock.acquire(timeout=LOCK_SECONDS):
            raise Unavailable("refresh busy")
        try:
            cached = self._cached(source)
            if cached is not None:
                return cached
            candidates = []
            for index, target in enumerate(self.config.targets):
                try:
                    result = self.runner(target, self.config, self.exporter_source)
                except Exception:
                    continue
                if (result.returncode != 0 or not result.stdout
                        or len(result.stdout) > MAX_JAR_BYTES):
                    continue
                try:
                    cookies = parse_jar(result.stdout)
                except ValueError:
                    continue
                if not covers(cookies, source):
                    continue
                candidates.append((summary_mtime(result.stderr), index, target,
                                   result.stdout, cookies))
            if not candidates:
                raise Unavailable("no cookie source")
            mtime, _index, target, data, cookies = max(
                candidates, key=lambda candidate: (candidate[0], -candidate[1]))
            install_jar(self.config, data)
            self._remember(target, cookies)
            print(f"refreshed source={source} target={target} "
                  f"cookies={len(cookies)} mtime={mtime}", file=sys.stderr, flush=True)
            return RefreshResult(target, len(cookies), False)
        finally:
            self.lock.release()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    timeout = 10

    def _send(self, status, payload):
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionError):
            pass

    def do_GET(self):
        if self.path == "/healthz":
            self._send(200, {"ok": True})
        else:
            self._send(404, {"ok": False, "error": "not_found"})

    def do_POST(self):
        if self.path != "/refresh":
            self._send(404, {"ok": False, "error": "not_found"})
            return
        try:
            length = int(self.headers.get("Content-Length", ""))
        except ValueError:
            self._send(400, {"ok": False, "error": "bad_request"})
            return
        if length < 0 or length > MAX_REQUEST_BYTES:
            self._send(400, {"ok": False, "error": "bad_request"})
            return
        try:
            body = json.loads(self.rfile.read(length))
            source = body["source"]
            if not isinstance(source, str):
                raise ValueError
        except (ValueError, KeyError, TypeError):
            self._send(400, {"ok": False, "error": "bad_request"})
            return
        try:
            result = self.server.service.refresh(source)
        except BadRequest:
            self._send(400, {"ok": False, "error": "bad_request"})
            return
        except Unavailable:
            self._send(503, {"ok": False, "error": "unavailable"})
            return
        except Exception:
            self._send(503, {"ok": False, "error": "unavailable"})
            return
        self._send(200, {"ok": True, "host": result.host,
                         "cookies": result.cookies, "cached": result.cached})

    def log_message(self, _format, *_args):
        return


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    try:
        config = Config.from_env()
        config.validate()
        service = RefreshService(config)
    except (ConfigError, OSError, ValueError):
        print("cookie-refresh configuration is invalid", file=sys.stderr)
        return 1
    server = Server(("0.0.0.0", config.port), Handler)
    server.service = service
    print(f"cookie-refresh listening on :{config.port}", file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
