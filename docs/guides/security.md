# Security and correctness invariants

> **When to read this:** Read before touching URL handling (`src/links.rs`, `src/images.rs`, `src/image_fetch.rs`, `src/read_url.rs`), tool registration or dispatch (`src/tool_registry.rs`, `src/chat.rs`, `src/code_inspect.rs`), prompt/memory filtering (`src/memory.rs`), cookies (`src/cookies.rs`, `src/cookie_refresh.rs`, `tools/cookie_refresh_service.py`), or the self-improvement sandbox (`src/self_improve.rs`, `tools/self_improve_orchestrator.py`, `tools/omp_worker.py`).

## Read-only and forbidden paths

- `docs/communication-guide.md` is **read-only**. It is compiled into `maksut` via `include_str!("../docs/communication-guide.md")` in `src/chat.rs`. Never edit, reformat, or regenerate it; change bot behavior in code, `SYSTEM_PROMPT`, tool descriptions, `README.md`, or `docs/`.
- In the self-improvement pipeline, `tools/self_improve_orchestrator.py` enforces `FORBIDDEN`:
  `docs/communication-guide.md`, `AGENTS.md`, `.env`, `.secrets/`, `self-improve/`, `self_improve/`, `tools/self_improve_orchestrator.py`, `tools/self_improve_deploy.py`, `tools/omp_worker.py`, and `target/`. Any diff touching those paths or containing `LLM_API_KEY` is rejected before commit or deploy.

## Untrusted URLs and SSRF gates

Every URL from a chat message, redirect header, metadata payload, or search result is untrusted.

- **Media URL allowlist (`src/links.rs`).** `allowed_url` accepts only `http`/`https` without credentials, explicit ports, or `%`-encoded path segments (which a server could decode into a redirect traversal), and matches exact host + post-path shapes for YouTube, Pinterest, X/Twitter, Instagram, and TikTok. `yt-dlp` runs with `--ies default,-generic` (`/etc/yt-dlp.conf`) so the generic extractor can never be reached via an embedded redirect.
- **Short-link resolution (`src/links.rs`).** `resolve_short_link` handles `pin.it`, `vm.tiktok.com`, and `vt.tiktok.com` manually (at most 5 redirects, `https` only, no credentials or port). Only `short_link_contact_allowed` hosts are dialed; regional Pinterest domains (`pinterest.<cc>`) are accepted in a `Location` header when `canonical_short_target` can extract the numeric `/pin/<id>/` directly without dialing them.
- **Photo CDN allowlist and DNS pinning (`src/images.rs`, `src/image_fetch.rs`).** `image_cdn_url` requires `https://` with no authority `@` or `:` (checked on the raw string before `Url::parse` normalizes `:443` away) and restricts hosts to `pinimg.com`, `cdninstagram.com`/`fbcdn.net`, `pbs.twimg.com/media/`, and signed `*.tiktokcdn*.com` (`x-signature` query required). `pinned_client` resolves the host on port 443, filters addresses through `permitted_ipv4`, disables proxies and redirects (`no_proxy()`, `redirect(Policy::none())`, `https_only(true)`), and pins the resolved socket addresses with `resolve_to_addrs`. Note that `permitted_ipv4` intentionally permits `198.18.0.0/15` because the host environment uses that range for transparent-proxy fake-IP routing; `read_url` below does **not** permit `198.18.0.0/15`.
- **TikTok cookies isolation (`src/images.rs`).** When fetching a TikTok photo post (`/@user/photo/<id>`), cookies from `COOKIES_FILE` are sent **only** to `https://www.tiktok.com` for the page HTML, never to CDN image hosts.
- **Public page reader (`src/read_url.rs`, [ADR 019](../adr/019-read-url.md)).** `read_url` is `ToolVisibility::All`, so its target must match an exact normalized URL present in the current user prompt (`prompt_urls` / `allowed_target`). The model cannot invent a URL or exfiltrate chat memory via query parameters. `read_url` rejects credentials, non-standard ports, IP literals, single-label hosts, `localhost`, `.local`/`.internal`/`.lan`/`.home.arpa`, IPv6-only hosts, and all non-public IPv4 (including `198.18.0.0/15`). Every redirect hop (max 5) is re-validated and dialed through a DNS-pinned client with no cookies, no proxy, a 2 MiB body cap, a 20s total deadline, and an HTML complexity guard (`<= 20,000` elements, `<= 128` depth).

## Tool authorization and prompt boundaries

- **Single authorization point (`src/tool_registry.rs`, [ADR 015](../adr/015-tool-registry.md)).** `ToolRegistry::find(name, owner)` is the only gate between a model tool call and execution. Never check tool names against a separate string list. Adding a `ToolVisibility::All` tool exposes it to every user in every enabled group; tools with owner-only side effects or private data (`feature_confirmation`, `inspect_code`, `find_stickers`, `send_sticker`) must stay `ToolVisibility::Owner`.
- **Owner-only prompt isolation (`src/chat.rs`).** Instructions for `Owner` tools (`OWNER_STICKER_PROMPT`, `OWNER_FEATURE_PROMPT`) are appended only when `owner == true` (and when the queue context is present). Never mention owner-only tools in `SYSTEM_PROMPT`.
- **Post-research privilege reduction (`src/tool_registry.rs`, `src/chat.rs`, [ADR 016](../adr/016-research.md)).** Search results and `read_url` pages are untrusted data. After a successful `research` pass, both `payload_for_mode` and the tool dispatcher restrict available tools to `web_search`, `research`, and `read_url` unless the user's own prompt explicitly asked for a sticker or a feature.
- **Snapshot confinement (`src/code_inspect.rs`, [ADR 018](../adr/018-owner-code-inspection.md)).** `inspect_code` canonicalizes `base` (`/var/lib/maksut/code`), canonicalizes any requested relative path, verifies `candidate.starts_with(&base)`, skips symlinks during directory traversal, skips `.git`, `target`, `self-improve`, `self_improve`, `cookies`, `.secrets`, and `node_modules`, and allows only `rs`, `toml`, `md`, `py`, `yaml`, `yml`, `json`, and `sh` files up to 256 KiB.

## Prompt injection and chat memory hygiene

- **Two-tier lexical detection (`src/memory.rs`, [ADR 014](../adr/014-injection-ignore.md)).**
  - `detect_flags` marks messages with `FLAG_INJECTION` (1), `FLAG_SECRET` (2), or `FLAG_JUNK` (4). Flagged rows remain in `chat_messages` for audit but are excluded from `load_memory` and `compact_chat` (`WHERE flags = 0`).
  - `injection_attempt` checks a narrower list of high-confidence override markers (`INJECTION_MARKERS`). When a non-owner triggers `injection_attempt` in an enabled group, the bot records `user_id` in `ignored_users` for `IGNORE_SECS = 86_400` (24 hours, non-extending while active) and marks their messages with `FLAG_IGNORED` (8). Ignored users receive no replies and no `answerCallbackQuery`. `OWNER_ID` is never ignored.

## Secrets, diagnostics, and sandbox isolation

- **No raw stderr or secret echoes ([ADR 006](../adr/006-albums-errors-cache.md), [ADR 008](../adr/008-docker-cookie-refresh-service.md)).** Never send raw `yt-dlp` / `ffmpeg` stderr, API response bodies, diagnostic URLs with query parameters, `.env`, or `cookies.txt` to Telegram or SQLite. `download_failures` stores only `url_host`, `url_path` (without query), static `stage`, static `detail`, and `exit_code`.
- **Cookie refresh privilege separation.** The `maksut` container mounts `/run/secrets/maksut` read-only and has no SSH client or key. Only the `cookie-refresh` sidecar mounts `.secrets/cookie-refresh` (`ro`) and `/run/secrets/maksut` (`rw`).
- **Self-improve approval and container isolation ([ADR 017](../adr/017-omp-worker-contract.md)).**
  - Neither `feature_status` nor `feature_confirmation` can approve, reject, or mutate queue state. Only an owner inline-button callback (`handle_callback`) or an explicit owner command in a private chat (never forwarded, never in a group) writes to `plan-approvals/`, `approvals/`, or `rejections/`.
  - `tools/omp_worker.py` runs `omp` inside a `--read-only`, `--cap-drop ALL`, `no-new-privileges` container on `maksut-self-improve-internal`, reaching only `self_improve/proxy.py` (`llm.labile.cc:443`). `LLM_API_KEY` is mounted via a read-only `models.yml` in `profile_dir`, never passed in argv or job JSON.
  - Offline verification (`test_worktree`) runs `cargo test --offline` with `--network none`.
