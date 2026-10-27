import io
import json
from contextlib import redirect_stdout
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

try:
    from tools.twitter_images import main, photo_urls
except ModuleNotFoundError:
    from twitter_images import main, photo_urls


class TwitterPhotoTests(unittest.TestCase):
    def test_photo_only_ignores_profile_and_video_thumbnails(self):
        status = {
            "user": {"profile_image_url_https": "https://pbs.twimg.com/profile.jpg"},
            "extended_entities": {"media": [
                {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/one?format=jpg&name=orig"},
                {"type": "video", "media_url_https": "https://pbs.twimg.com/media/cover.jpg"},
            ]},
        }
        self.assertEqual(photo_urls(status), ["https://pbs.twimg.com/media/one?format=jpg&name=orig"])

    def test_mixed_unified_card_deduplicates_only_photo_media(self):
        url = "https://pbs.twimg.com/media/one.jpg"
        status = {
            "extended_entities": {"media": [{"type": "photo", "media_url_https": url}]},
            "card": {"binding_values": {"unified_card": {"string_value": json.dumps({
                "media_entities": {
                    "one": {"type": "photo", "media_url_https": url},
                    "two": {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/two.png"},
                    "three": {"type": "animated_gif", "media_url_https": "https://pbs.twimg.com/media/gif.jpg"},
                }
            })}}},
        }
        self.assertEqual(photo_urls(status), [url, "https://pbs.twimg.com/media/two.png"])

    def test_quoted_status_legacy_photos_are_included_without_thumbnails_or_duplicates(self):
        photo = "https://pbs.twimg.com/media/quoted.jpg"
        status = {
            "extended_entities": {"media": [{"type": "photo", "media_url_https": "outer"}]},
            "quoted_status": {
                "user": {"profile_image_url_https": "profile.jpg"},
                "extended_entities": {"media": [
                    {"type": "photo", "media_url_https": photo},
                    {"type": "video", "media_url_https": "video-cover.jpg"},
                ]},
                "legacy": {
                    "extended_entities": {"media": [
                        {"type": "photo", "media_url_https": photo},
                        {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/legacy.png"},
                    ]},
                },
            },
        }
        self.assertEqual(photo_urls(status), [
            "outer", photo, "https://pbs.twimg.com/media/legacy.png",
        ])

    def test_malformed_card_does_not_erase_extended_entities(self):
        status = {"extended_entities": {"media": [{"type": "photo", "media_url_https": "photo"}]},
                  "unified_card": "not-json", "thumbnail": "video-cover"}
        self.assertEqual(photo_urls(status), ["photo"])


class TwitterOutputTests(unittest.TestCase):
    def output_for_status(self, status):
        ydl = MagicMock()
        ydl.__enter__.return_value.get_info_extractor.return_value._extract_status.return_value = status
        output = io.StringIO()
        with patch.dict("sys.modules", {"yt_dlp": SimpleNamespace(
                YoutubeDL=MagicMock(return_value=ydl))}), \
                patch("sys.argv", ["twitter_images.py", "12345"]), \
                redirect_stdout(output):
            self.assertEqual(main(), 0)
        return json.loads(output.getvalue())

    def test_main_emits_outer_legacy_text_and_unique_outer_and_quoted_photos(self):
        outer = "https://example.test/outer.jpg"
        quoted = "https://example.test/quoted.jpg"
        status = {
            "legacy": {
                "full_text": 'Outer "caption" & more\nSecond line',
                "extended_entities": {"media": [{"type": "photo", "media_url_https": outer}]},
            },
            "quoted_status": {
                "legacy": {
                    "full_text": "Quoted text must not replace outer caption",
                    "extended_entities": {"media": [
                        {"type": "photo", "media_url_https": outer},
                        {"type": "photo", "media_url_https": quoted},
                    ]},
                },
            },
        }
        self.assertEqual(self.output_for_status(status), {
            "photos": [outer, quoted],
            "description": 'Outer "caption" & more\nSecond line',
            "has_video": False,
        })

    def test_main_uses_top_level_full_text_when_legacy_text_is_absent(self):
        status = {
            "full_text": "Top-level tweet text",
            "legacy": {},
            "quoted_status": {"legacy": {"full_text": "Only quoted text"}},
        }
        self.assertEqual(self.output_for_status(status), {
            "photos": [],
            "description": "Top-level tweet text",
            "has_video": False,
        })

    def test_main_emits_null_description_when_outer_text_is_missing_or_empty(self):
        for status in ({}, {"legacy": {"full_text": ""}}):
            with self.subTest(status=status):
                self.assertEqual(self.output_for_status(status), {
                    "photos": [],
                    "description": None,
                    "has_video": False,
                })

    def test_main_classifies_direct_video_without_including_its_cover_photo(self):
        photo = "https://pbs.twimg.com/media/photo.jpg"
        status = {"extended_entities": {"media": [
            {"type": "photo", "media_url_https": photo},
            {"type": "video", "media_url_https": "https://pbs.twimg.com/media/video-cover.jpg"},
        ]}}
        self.assertEqual(self.output_for_status(status), {
            "photos": [photo],
            "description": None,
            "has_video": True,
        })

    def test_main_classifies_legacy_animated_gif_without_including_its_cover_photo(self):
        status = {"legacy": {"extended_entities": {"media": [
            {"type": "animated_gif", "media_url_https": "https://pbs.twimg.com/media/gif-cover.jpg"},
        ]}}}
        self.assertEqual(self.output_for_status(status), {
            "photos": [],
            "description": None,
            "has_video": True,
        })

    def test_main_classifies_quoted_legacy_video_and_preserves_outer_photos(self):
        photo = "https://pbs.twimg.com/media/outer.jpg"
        status = {
            "extended_entities": {"media": [
                {"type": "photo", "media_url_https": photo},
            ]},
            "quoted_status": {"legacy": {"extended_entities": {"media": [
                {"type": "video", "media_url_https": "https://pbs.twimg.com/media/quoted-cover.jpg"},
            ]}}},
        }
        self.assertEqual(self.output_for_status(status), {
            "photos": [photo],
            "description": None,
            "has_video": True,
        })

    def test_main_classifies_json_unified_card_video_without_including_its_cover_photo(self):
        status = {"card": {"binding_values": {"unified_card": {"string_value": json.dumps({
            "media_entities": {
                "video": {"type": "video", "media_url_https": "https://pbs.twimg.com/media/card-cover.jpg"},
            },
        })}}}}
        self.assertEqual(self.output_for_status(status), {
            "photos": [],
            "description": None,
            "has_video": True,
        })

    def test_main_classifies_top_level_unified_card_animated_gif_without_including_its_cover_photo(self):
        status = {"unified_card": {"media_entities": {
            "gif": {"type": "animated_gif", "media_url_https": "https://pbs.twimg.com/media/card-gif-cover.jpg"},
        }}}
        self.assertEqual(self.output_for_status(status), {
            "photos": [],
            "description": None,
            "has_video": True,
        })


class TwitterFailureTests(unittest.TestCase):
    def run_with_error(self, message):
        ydl = MagicMock()
        ydl.__enter__.return_value.get_info_extractor.return_value._extract_status.side_effect = RuntimeError(message)
        output = io.StringIO()
        with patch.dict("sys.modules", {"yt_dlp": SimpleNamespace(
                YoutubeDL=MagicMock(return_value=ydl))}), \
                patch("sys.argv", ["twitter_images.py", "12345"]), \
                redirect_stdout(output):
            code = main()
        return code, output.getvalue()

    def test_auth_failures_return_exit_code_3_without_output(self):
        for message in (
            "HTTP 401 Unauthorized",
            "login required",
            "Sign in to continue",
            "Could not authenticate you",
            "cookies are no longer valid",
        ):
            with self.subTest(message=message):
                code, output = self.run_with_error(message)
                self.assertEqual(code, 3)
                self.assertEqual(output, "")

    def test_other_failures_return_exit_code_1_without_output(self):
        for message in ("network down", "HTTP 500"):
            with self.subTest(message=message):
                code, output = self.run_with_error(message)
                self.assertEqual(code, 1)
                self.assertEqual(output, "")


if __name__ == "__main__":
    unittest.main()
