# Module map and repository layout

> **When to read this:** Read for orientation, or when you need to know which file owns a concern.

## Rust binary (`src/`, crate `maksut`)

Every module is declared in `src/main.rs` and shares imports through `src/prelude.rs`. Inline unit tests sit in sibling `src/*_test.rs` files wired at the bottom of `src/main.rs` (plus `src/test_support.rs` and `src/feature_support.rs`).

- `src/main.rs`: entrypoint, process-wide limits (`MAX_FILES`, `MAX_TIKTOK_PHOTOS`, `USER_DOWNLOAD_LIMIT`, `TELEGRAM_FILE_LIMIT`, `DEFAULT_MEDIA_LIMIT`, `DOWNLOAD_TIMEOUT`, `MAX_VIDEO_DURATION`, `TELEGRAM_REQUEST_TIMEOUT`, `COOKIE_REFRESH_TIMEOUT`, `YOUTUBE_PROBE_TIMEOUT`, `MAX_METADATA_BYTES`, `MP4_MUX_HEADROOM`, `MAX_TWITTER_JSON_BYTES`, `MAX_VIDEO_DIMENSION`, `MAX_VIDEO_PIXELS`), background task spawns (`load_sticker_catalog`, `memory_compactor`, `improvement_notifier`), and the `teloxide` dispatcher (`distribution_function(|_| None::<i64>)` so a slow download in one chat never serializes other updates)
- `src/settings.rs`: `Settings::from_env()`, startup validation (`OWNER_ID`, `DATABASE_URL`, `MAX_MEDIA_BYTES`, `DOWNLOAD_DIR`, `COOKIES_FILE`, `COOKIE_REFRESH_URL`, `LLM_*`, `SEARCH_PROVIDER`, `TAVILY_API_KEY`, `SELF_IMPROVE_DIR`), per-chat locks (`chat_locks`), download semaphore (`Semaphore::new(2)`), and per-user download rate-limit semaphores
- `src/handler.rs`: top-level Telegram update routing (`handle_message`, `handle_callback_query`), `/maksut status|on|off` group control, owner standalone sticker triggers (`reply_with_sticker`), link extraction and download slot acquisition, and delegation to free chat or self-improve handlers
- `src/links.rs`: URL allowlist (`allowed_url`) for YouTube, Pinterest, X/Twitter, Instagram and TikTok; short-link resolution (`pin.it`, `vm.tiktok.com`, `vt.tiktok.com` via `resolve_short_link` with pinned DNS and bounded redirects)
- `src/download.rs`: end-to-end download orchestration for a single URL: short-link resolution, 24-hour `media_cache` lookup, parallel/sequential dispatch across video and photo pipelines, on-demand cookie refresh retry, album grouping, delivery, source-message deletion, and failure recording
- `src/download_videos.rs`: `yt-dlp` video/audio invocation, YouTube pre-probe selection, `after_move` description capture, `ffprobe` audio-stream verification, and `ffmpeg` transcoding integration
- `src/process.rs`: `DownloadProcess` lifecycle (`setsid` process-group isolation on Unix so `SIGKILL` terminates both `yt-dlp` and any child `ffmpeg`, bounded `stdout`/`stderr` capture tasks, `Drop` reaping, and `stderr_detail` classification)
- `src/youtube.rs`: single-video YouTube metadata probe (`yt-dlp -J`) and format selector that picks the highest-resolution H.264 + AAC stream pair fitting `MAX_MEDIA_BYTES` with `MP4_MUX_HEADROOM`
- `src/video_convert.rs`: `ffprobe` stream inspection (moving video vs cover art, audio track presence, dimension/pixel limits) and `ffmpeg` transcoding to MP4 (H.264/yuv420p + AAC, capped at 1920px per side with even-dimension padding)
- `src/images.rs`: photo metadata extraction for Pinterest, Instagram, X/Twitter (`tools/twitter_images.py`), and TikTok photo posts (`/@user/photo/<id>` from `webapp.video-detail`), CDN URL allowlisting (`image_cdn_url`), and `permitted_ipv4`
- `src/image_fetch.rs`: DNS-pinned HTTPS client cache (`PINNED_CLIENTS`) and streaming image downloader (`save_image_response`) with MIME verification, size caps, and `.part` atomic persistence
- `src/media.rs`: media file classification by extension (`MediaKind`: `Photo`, `Video`, `Audio`, `Document`) and directory scanning
- `src/delivery.rs`: Telegram sending: adjacent compatible media grouping into albums of 2 to 10 items (`sendMediaGroup`), caption deduplication across album items, silent delivery (`disable_notification = true`), single-item document fallback on explicit Telegram rejection, and `file_id` collection for `media_cache`
- `src/caption.rs`: HTML caption builder (`<title>\n\n<description>`, `<code>От first_name</code>`, clickable `Ссылка`) truncated cleanly within Telegram's 1024 UTF-16 code-unit limit
- `src/source_message.rs`: deleting the sender's original link message after full delivery, or warning when the bot lacks `can_delete_messages`
- `src/media_cache.rs`: 24-hour SQLite `media_cache` (keyed by SHA-256 of bot token + chat id + normalized URL with query, capped at 200 newest entries globally)
- `src/cookies.rs`: Netscape cookie jar validation (`check_cookies`), per-source cookie domain matching, and temporary cookie copy preparation for `yt-dlp`
- `src/cookie_refresh.rs`: synchronous HTTP client for the `cookie-refresh` sidecar (`POST /refresh`), 5-second deduplication window, and auth-failure classification that triggers a single retry
- `src/diagnostics.rs`: structured failure recording into SQLite (`download_failures` and `download_failure_events`, capped at 200 parent rows and 12 events per failure) and selection of the most actionable user-facing cause (`source_user_diagnostic`)
- `src/user_errors.rs`: mapping internal stages and `yt-dlp` / HTTP / Telegram errors into safe Russian user-facing messages without leaking raw stderr, URLs, or tokens
- `src/database.rs`: SQLite connection pool setup (`open_database`) and schema creation (`group_settings`, `download_failures`, `download_failure_events`, `chat_turns`, `chat_messages`, `chat_summaries`, `memory_meta`, `ignored_users`, `media_cache`)
- `src/chat.rs`: free-chat LLM client (`https://llm.labile.cc/v1`), system prompt assembly (`docs/communication-guide.md` via `include_str!` + `SYSTEM_PROMPT` + owner-only prompt blocks), vision attachment encoding, tool call loop (`TOOL_ROUNDS = 5`), `web_search` and multi-query `research` (`SearchProvider`: `Firecrawl`, `Tavily`, `DuckDuckGo`), reply styling (`style_reply`, `style_research_reply`), and per-user cooldowns
- `src/tool_registry.rs`: single source of truth (`ToolRegistry`, `ToolSpec`, `ToolKind`, `ToolVisibility`) for function-tool schemas, visibility (`All` vs `Owner`), research-mode filtering, and authorization (`ToolRegistry::find`)
- `src/read_url.rs`: public web page reader (`read_url` tool): exact prompt-URL matching, SSRF/DNS-pinning guard, manual redirect validation, HTML complexity guard, `dom_query` + `dom_smoothie` readability extraction, charset decoding (`chardetng`/`encoding_rs`), and 10-minute in-memory cache
- `src/code_inspect.rs`: owner-only `inspect_code` implementation over `/var/lib/maksut/code` (`MAX_FILE_BYTES = 256 KiB`, symlink skipping, extension allowlist, `SKIP_DIRS`)
- `src/memory.rs`: chat history storage (`chat_messages`), lexical flag detection (`FLAG_INJECTION`, `FLAG_SECRET`, `FLAG_JUNK`, `FLAG_IGNORED`), 24-hour prompt-injection ignore window (`ignored_users`), context loader (`load_memory`), and 10-minute background summarization (`memory_compactor` -> `chat_summaries`)
- `src/stickers.rs`: classic owner word-trigger sticker table (`sticker_rule` matched by `file_unique_id`) and semantic 15-pack catalog (`StickerCatalog`, `find_stickers`, `send_sticker`) loaded from `sticker_sets.txt` and `sticker_descriptions.json`
- `src/self_improve.rs`: chat-side self-improvement control plane: `/improve` commands, natural-language owner actions (`одобри`, `отклони`, `отмени`, `подними`, `опусти`, `приоритет`), short/prefix/suffix task ID resolution, `propose_feature` / `feature_status` / `feature_confirmation` tool handlers, inline approval callbacks, and `improvement_notifier`
- `src/cleanup.rs`: startup cleanup of stale temporary download directories in `DOWNLOAD_DIR`

## Python helpers and host-side services (`tools/`, `cookie_refresh/`, `self_improve/`)

- `tools/twitter_images.py` (`test_twitter_images.py`): runs inside the `maksut` container under `/opt/yt-dlp/bin/python`; uses `yt-dlp`'s `TwitterIE` to extract photo URLs, description, and `has_video` from X/Twitter posts
- `tools/export_chrome_cookies.py` (`test_export_chrome_cookies.py`): runs on `pc` or `notebook` over SSH; decrypts Chrome's SQLite cookie store (`v10`/`v11`) via the host keyring and outputs a filtered Netscape cookie jar on stdout
- `tools/cookie_refresh_service.py` (`test_cookie_refresh_service.py`): HTTP sidecar (`cookie_refresh/Dockerfile`, `:8080`) that SSHes to `pc` and `notebook`, picks the freshest Chrome profile containing cookies for the requested source, and atomically writes `cookies.txt` (`0600`)
- `tools/self_improve_orchestrator.py` (`test_self_improve_orchestrator.py`): host-side systemd user service (`packaging/maksut-self-improve.service`); polls `self-improve/inbox`, snapshots the repo, runs planner + reviewer + implementer through `omp_worker.py`, opens/closes GitHub issues via `gh`, runs offline tests in `self_improve/test.Dockerfile`, and applies/rolls back approved patches with `docker compose`
- `tools/omp_worker.py` (`test_omp_worker.py`): single entrypoint that runs an `omp` agent inside a disposable read-only Docker container connected only to `maksut-self-improve-internal` (`self_improve/proxy.Dockerfile` + `self_improve/proxy.py`)
- `tools/build_sticker_index.py`: host-side utility using Telegram MCP and a vision model to refresh `sticker_sets.txt` and `sticker_descriptions.json`

## Other top-level paths

- `compose.yaml`: production Docker Compose definition (`maksut` + `cookie-refresh`)
- `Dockerfile`: multi-stage build (`rust:1.98.1-bookworm` builder -> `node:22-bookworm-slim` runtime with `ffmpeg`, `python3`, `yt-dlp[default,curl-cffi]==2026.7.4`)
- `shell.nix`: local development shell (`cargo`, `rustc`, `rustfmt`, `clippy`, `sqlite`, `python3`, `ffmpeg`, `yt-dlp`)
- `docs/communication-guide.md`: **READ-ONLY** operating specification compiled into the binary via `include_str!`
- `docs/guides/`: task-triggered engineering guides
- `docs/adr/`: numbered architecture decision records (`001` to `019`)
