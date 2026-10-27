"""Extract tweet photos and classify videos from pinned yt-dlp status data."""

import json
import sys
from collections.abc import ValuesView


def collect_media(status):
    """Return unique original photo URLs and whether video media is present."""
    result = []
    seen = set()
    has_video = False

    def collect(media):
        nonlocal has_video
        if isinstance(media, dict):
            media = media.values()
        if not isinstance(media, (list, tuple, ValuesView)):
            return
        for entry in media:
            if not isinstance(entry, dict):
                continue
            media_type = entry.get("type")
            if media_type in ("video", "animated_gif"):
                has_video = True
                continue
            if media_type != "photo":
                continue
            url = entry.get("media_url_https")
            if isinstance(url, str) and url not in seen:
                seen.add(url)
                result.append(url)

    # Unified cards may be present directly or JSON-encoded in card binding_values.
    def collect_card(card):
        if isinstance(card, str):
            try:
                card = json.loads(card)
            except (ValueError, TypeError):
                return
        if isinstance(card, dict):
            collect(card.get("media_entities"))

    if not isinstance(status, dict):
        return result, has_video
    quoted = status.get("quoted_status")
    for item in (status, status.get("legacy"), quoted,
                 quoted.get("legacy") if isinstance(quoted, dict) else None):
        if not isinstance(item, dict):
            continue
        entities = item.get("extended_entities")
        if isinstance(entities, dict):
            collect(entities.get("media"))
        collect_card(item.get("unified_card"))
        card = item.get("card")
        if isinstance(card, dict):
            bindings = card.get("binding_values")
            if isinstance(bindings, dict):
                unified = bindings.get("unified_card")
                if isinstance(unified, dict):
                    collect_card(unified.get("string_value", unified))
                else:
                    collect_card(unified)
    return result, has_video


def photo_urls(status):
    """Return unique original photo URLs, never profile or video thumbnails."""
    return collect_media(status)[0]



def main():
    if len(sys.argv) not in (2, 3) or not sys.argv[1].isascii() or not sys.argv[1].isdigit():
        return 2
    try:
        from yt_dlp import YoutubeDL

        options = {"quiet": True, "no_warnings": True, "noplaylist": True}
        if len(sys.argv) == 3:
            options["cookiefile"] = sys.argv[2]
        with YoutubeDL(options) as ydl:
            status = ydl.get_info_extractor("Twitter")._extract_status(sys.argv[1])
        description = None
        if isinstance(status, dict):
            legacy = status.get("legacy")
            text = legacy.get("full_text") if isinstance(legacy, dict) else None
            if not isinstance(text, str) or not text:
                text = status.get("full_text")
            if isinstance(text, str) and text:
                description = text
        photos, has_video = collect_media(status)
        print(json.dumps({"photos": photos, "description": description, "has_video": has_video},
                         separators=(",", ":")))
    except Exception as error:
        # Never echo cookies, URLs, private yt-dlp responses, or credentials.
        detail = str(error).casefold()
        if any(token in detail for token in (
            "401", "unauthorized", "login required", "sign in",
            "could not authenticate", "cookies are no longer valid",
        )):
            return 3
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
