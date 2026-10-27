import json
import os
import stat
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

try:
    from tools.cookie_refresh_service import (
        BadRequest, Config, ExportResult, Handler, RefreshResult, RefreshService,
        Server, Unavailable, _drain, covers, install_jar, parse_jar, parse_targets, summary_mtime,
    )
except ModuleNotFoundError:
    from cookie_refresh_service import (
        BadRequest, Config, ExportResult, Handler, RefreshResult, RefreshService,
        Server, Unavailable, _drain, covers, install_jar, parse_jar, parse_targets, summary_mtime,
    )


def jar(*rows):
    return ("# Netscape HTTP Cookie File\n" + "".join(rows)).encode()


def row(host, name, value="synthetic", expiry="4102444800"):
    return f"{host}\tTRUE\t/\tTRUE\t{expiry}\t{name}\t{value}\n"


def make_config(root, targets=("pc", "notebook"), old=None):
    key = Path(root) / "key"
    known = Path(root) / "known_hosts"
    exporter = Path(root) / "exporter.py"
    key.write_text("synthetic", encoding="utf-8")
    known.write_text("pc ssh-ed25519 synthetic\n", encoding="utf-8")
    exporter.write_text("print('synthetic')\n", encoding="utf-8")
    cookie_path = Path(root) / "cookies.txt"
    cookie_path.write_bytes(old or jar(row(".youtube.com", "SID", "old")))
    return Config(targets=targets, key=key, known_hosts=known,
                  jar=cookie_path, exporter=exporter, ssh_timeout=1, port=8080)


class FakeRunner:
    def __init__(self, results):
        self.results = results
        self.calls = []

    def __call__(self, target, _config, _exporter):
        self.calls.append(target)
        result = self.results.get(target)
        if isinstance(result, Exception):
            raise result
        if result is None:
            return ExportResult(1, b"", b"")
        data, mtime = result
        return ExportResult(0, data, f"exported 1 cookies; db_mtime_ns={mtime}\n".encode())


class TargetTests(unittest.TestCase):
    def test_parse_targets_strips_and_keeps_order(self):
        self.assertEqual(parse_targets("labile@pc, labile@notebook"),
                         ("labile@pc", "labile@notebook"))

    def test_parse_targets_rejects_empty_or_option_like_entries(self):
        for raw in ("", ",pc", "pc,", "-oProxyCommand=x", "pc user"):
            with self.subTest(raw=raw):
                with self.assertRaises(Exception):
                    parse_targets(raw)


class JarTests(unittest.TestCase):
    def test_parse_jar_accepts_httponly_and_session_cookies(self):
        data = jar(row(".youtube.com", "SID"), row("#HttpOnly_.x.com", "auth_token"),
                   row(".youtube.com", "session", expiry="0"))
        cookies = parse_jar(data)
        self.assertEqual(cookies[0][:2], (".youtube.com", "SID"))
        self.assertEqual(cookies[1][:2], (".x.com", "auth_token"))
        self.assertEqual(cookies[2][2], 0)

    def test_parse_jar_rejects_malformed_rows(self):
        for data in (
            b"not a jar\n",
            b"# Netscape HTTP Cookie File\nno-tabs\n",
            b"# Netscape HTTP Cookie File\n.host\tTRUE\t/\tTRUE\t-1\tname\tvalue\n",
        ):
            with self.subTest(data=data):
                with self.assertRaises(ValueError):
                    parse_jar(data)

    def test_summary_mtime_is_machine_readable_and_optional(self):
        self.assertEqual(summary_mtime(b"x db_mtime_ns=123\n"), 123)
        self.assertEqual(summary_mtime(b"no summary"), 0)


class CoverageTests(unittest.TestCase):
    def test_each_source_requires_its_own_cookie(self):
        cases = {
            "youtube": jar(row(".youtube.com", "LOGIN_INFO")),
            "tiktok": jar(row(".tiktok.com", "sessionid")),
            "instagram": jar(row(".instagram.com", "sessionid")),
            "twitter": jar(row(".x.com", "auth_token")),
            "pinterest": jar(row(".pinterest.com", "csrftoken")),
            "generic": jar(row(".example.com", "anything")),
        }
        unrelated = parse_jar(jar(row(".example.com", "unrelated")))
        for source, data in cases.items():
            with self.subTest(source=source):
                self.assertTrue(covers(parse_jar(data), source))
                if source != "generic":
                    self.assertFalse(covers(unrelated, source))

    def test_cookie_name_must_belong_to_the_source_host(self):
        data = parse_jar(jar(row(".example.com", "sessionid"),
                             row(".youtube.com", "SID")))
        self.assertFalse(covers(data, "tiktok"))
        self.assertTrue(covers(data, "youtube"))

    def test_expired_cookie_does_not_cover_a_source(self):
        data = parse_jar(jar(row(".youtube.com", "SID", expiry="1")))
        self.assertFalse(covers(data, "youtube"))
        session = parse_jar(jar(row(".youtube.com", "SID", expiry="0")))
        self.assertTrue(covers(session, "youtube"))


class ServiceTests(unittest.TestCase):
    def test_refresh_selects_newest_covering_target(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            pc = jar(row(".youtube.com", "SID", "pc"))
            notebook = jar(row(".youtube.com", "SID", "notebook"))
            runner = FakeRunner({"pc": (pc, 100), "notebook": (notebook, 200)})
            service = RefreshService(config, runner=runner)
            result = service.refresh("youtube")
            self.assertEqual((result.host, result.cookies, result.cached),
                             ("notebook", 1, False))
            self.assertEqual(config.jar.read_bytes(), notebook)
            self.assertEqual(runner.calls, ["pc", "notebook"])

    def test_newest_non_covering_target_is_skipped(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            pc = jar(row(".youtube.com", "SID", "pc"))
            pinterest = jar(row(".pinterest.com", "csrftoken", "new"))
            runner = FakeRunner({"pc": (pc, 100), "notebook": (pinterest, 999)})
            service = RefreshService(config, runner=runner)
            result = service.refresh("youtube")
            self.assertEqual(result.host, "pc")
            self.assertEqual(config.jar.read_bytes(), pc)

    def test_no_covering_target_leaves_old_jar_untouched(self):
        with tempfile.TemporaryDirectory() as root:
            old = jar(row(".youtube.com", "SID", "old"))
            config = make_config(root, old=old)
            runner = FakeRunner({"pc": (jar(row(".tiktok.com", "sessionid")), 2),
                                 "notebook": (b"", 3)})
            service = RefreshService(config, runner=runner)
            with self.assertRaises(Unavailable):
                service.refresh("youtube")
            self.assertEqual(config.jar.read_bytes(), old)

    def test_failed_runner_leaves_old_jar_untouched(self):
        with tempfile.TemporaryDirectory() as root:
            old = jar(row(".youtube.com", "SID", "old"))
            config = make_config(root, old=old)
            runner = FakeRunner({"pc": OSError("ssh unavailable"),
                                 "notebook": (jar(row(".youtube.com", "SID", "new")), 1)})
            service = RefreshService(config, runner=runner)
            result = service.refresh("youtube")
            self.assertEqual(result.host, "notebook")
            self.assertEqual(config.jar.read_bytes(), jar(row(".youtube.com", "SID", "new")))

    def test_install_is_atomic_and_mode_600(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            data = jar(row(".youtube.com", "SID", "new"))
            install_jar(config, data)
            self.assertEqual(config.jar.read_bytes(), data)
            self.assertEqual(stat.S_IMODE(config.jar.stat().st_mode), 0o600)

    def test_cache_returns_second_request_without_runner(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            data = jar(row(".youtube.com", "SID", "new"))
            runner = FakeRunner({"pc": (data, 1), "notebook": (data, 2)})
            service = RefreshService(config, runner=runner)
            first = service.refresh("youtube")
            second = service.refresh("youtube")
            self.assertFalse(first.cached)
            self.assertTrue(second.cached)
            self.assertEqual(runner.calls, ["pc", "notebook"])

    def test_cache_does_not_cover_a_different_source(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            youtube = jar(row(".youtube.com", "SID", "new"))
            pinterest = jar(row(".pinterest.com", "csrftoken", "new"))
            runner = FakeRunner({"pc": (youtube, 1), "notebook": (pinterest, 2)})
            service = RefreshService(config, runner=runner)
            service.refresh("youtube")
            result = service.refresh("pinterest")
            self.assertFalse(result.cached)
            self.assertEqual(len(runner.calls), 4)

    def test_oversized_export_is_rejected_without_replacing_jar(self):
        with tempfile.TemporaryDirectory() as root:
            old = jar(row(".youtube.com", "SID", "old"))
            config = make_config(root, old=old)
            runner = FakeRunner({"pc": (b"x" * (4 * 1024 * 1024 + 1), 1)})
            service = RefreshService(config, runner=runner)
            with self.assertRaises(Unavailable):
                service.refresh("youtube")
            self.assertEqual(config.jar.read_bytes(), old)

    def test_unsupported_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            service = RefreshService(config, runner=FakeRunner({}))
            with self.assertRaises(BadRequest):
                service.refresh("unknown")

    @unittest.skipIf(os.geteuid() == 0, "root bypasses directory permissions")
    def test_install_failure_keeps_old_jar(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_config(root)
            old = config.jar.read_bytes()
            data = jar(row(".youtube.com", "SID", "new"))
            os.chmod(config.jar.parent, 0o500)
            try:
                with self.assertRaises(OSError):
                    install_jar(config, data)
                self.assertEqual(config.jar.read_bytes(), old)
            finally:
                os.chmod(config.jar.parent, 0o700)


class DrainTests(unittest.TestCase):
    class Stream:
        def __init__(self, data):
            self.data = data

        def read(self, size):
            chunk = self.data[:size]
            self.data = self.data[size:]
            return chunk

    def test_drain_returns_one_byte_over_limit(self):
        limit = 1024
        result = _drain(self.Stream(b"x" * (limit + 5)), limit)
        self.assertEqual(len(result), limit + 1)


class StubRefreshService:
    def refresh(self, source):
        if source == "bad":
            raise BadRequest("bad")
        if source == "unavailable":
            raise Unavailable("unavailable")
        return RefreshResult("pc", 2, False)


class HttpHandlerTests(unittest.TestCase):
    def setUp(self):
        self.server = Server(("127.0.0.1", 0), Handler)
        self.server.service = StubRefreshService()
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def request(self, path, data=None):
        request = urllib.request.Request(
            self.base + path, data=data,
            headers={"Content-Type": "application/json"} if data is not None else {})
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            try:
                return error.code, error.read()
            finally:
                error.close()

    def test_health_and_success_response_have_no_cookie_values(self):
        self.assertEqual(self.request("/healthz"), (200, b'{"ok":true}'))
        status, body = self.request("/refresh", b'{"source":"youtube"}')
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), {"ok": True, "host": "pc", "cookies": 2, "cached": False})

    def test_malformed_unknown_and_oversized_requests_are_rejected(self):
        for data in (b"{", b'{"source":"bad"}', b"x" * 4097):
            with self.subTest(data=data[:20]):
                self.assertEqual(self.request("/refresh", data)[0], 400)

    def test_unavailable_and_unknown_paths_are_distinct(self):
        self.assertEqual(self.request("/refresh", b'{"source":"unavailable"}')[0], 503)
        self.assertEqual(self.request("/missing")[0], 404)


if __name__ == "__main__":
    unittest.main()
