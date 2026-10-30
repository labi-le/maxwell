# Free chat, memory, stickers, and tools

> **When to read this:** Read before touching `src/chat.rs`, `src/chat_client.rs`, `src/chat_media.rs`, `src/chat_status.rs`, `src/chat_style.rs`, `src/chat_tools.rs`, `src/search.rs`, `src/research.rs`, `src/tool_registry.rs`, `src/read_url.rs`, `src/memory.rs`, `src/stickers.rs`, or `tools/build_sticker_index.py`.

## Module map

Free chat is split by concern; each module has a sibling test file:

| Module | Test | Owns |
|---|---|---|
| `src/chat.rs` | `src/chat_test.rs` | turn orchestration (`reply_with_chat`, `chat_reply_with_proposal`), gating, typing keepalive, tool loop, delivery |
| `src/chat_client.rs` | `src/chat_test.rs` | `ChatConfig`, prompt assembly, completion transport, cooldowns, summarization |
| `src/chat_media.rs` | `src/chat_test.rs` | vision attachments |
| `src/chat_status.rs` | `src/chat_status_test.rs` | deterministic `feature_status` answers |
| `src/chat_style.rs` | `src/chat_test.rs` | addressing, chunking, reply styling, citation sanitizing |
| `src/chat_tools.rs` | `src/chat_test.rs` | function-tool execution and per-reply budgets |
| `src/search.rs` | `src/chat_test.rs` | search providers and HTML result parsing |
| `src/research.rs` | `src/research_test.rs` | multi-query research and dossier rendering |
| `src/tool_registry.rs` | `src/tool_registry_test.rs` | tool schemas, visibility, authorization |
| `src/read_url.rs` | `src/read_url_test.rs` | public page reader |
| `src/memory.rs` | `src/memory_test.rs` | chat history storage, flags, ignores, compaction |

## Activation and addressing (`src/chat.rs`, `src/chat_client.rs`, `src/handler.rs`, [ADR 012](../adr/012-free-chat.md))

- Free chat is enabled when `LLM_API_KEY` is non-empty (`LLM_BASE_URL` defaults to `https://llm.labile.cc/v1`, `LLM_MODEL` to `deepseek-v4.1-flash`). Without `LLM_API_KEY`, the bot operates as a link downloader and classic sticker responder only.
- **Private chat:** responds to `OWNER_ID` on any non-command message that has no supported media link and did not fire a standalone sticker trigger.
- **Enabled group (`/maksut on`):** responds when addressed via `@<bot_username>`, a reply to the bot's message, or the word `максут` / `maksut` (stripped from the prompt; `максвелл` is not a chat trigger). After the bot replies in a group, it keeps a 15-minute active-conversation window (`FOLLOWUP_WINDOW = 900s`, capped at `MAX_ACTIVE_CHATS = 1024`) for that chat so any participant in the ongoing thread can continue the exchange without repeating `максут` or replying to the bot's message (including replies to other thread messages, with the replied-to snippet passed via `reply_context`); during a follow-up turn (`FOLLOWUP_PROMPT`) the model may return `PASS` (`is_pass_reply`) to stay silent if the remark is unrelated. On an LLM error in a group the bot stays silent; in a private chat it sends a short error message.
- **Delivery:** both private and group replies (first text chunk, button prompt, and stickers) attach `reply_parameters` (`ReplyParameters::new(message.id)`) with `disable_notification = true` and split at `TELEGRAM_MESSAGE_CHARS = 3900`.
- **Rate limits and budgets:** per-user cooldown `CHAT_COOLDOWN = 3s`, `CHAT_CONCURRENCY = 4`, `CHAT_TIMEOUT = 40s`, `CHAT_MAX_TOKENS = 300` (`STATUS_MAX_TOKENS = 1200`, `RESEARCH_MAX_TOKENS = 1500`), up to `TOOL_ROUNDS = 5` tool iterations per reply, with `TYPING_KEEPALIVE = 4s`.

## Voice and styling (`src/chat_style.rs`, `src/chat_client.rs`)

- `docs/communication-guide.md` is compiled into the binary (`COMMUNICATION_GUIDE = include_str!("../docs/communication-guide.md")`) and prepended to every chat's system prompt, followed by `GUIDE_PREFIX` and `SYSTEM_PROMPT`, which take precedence. Never edit `docs/communication-guide.md`.
- Normal replies pass through `style_reply`: forced lowercase (`lowercase_reply`, preserving case inside URLs), lines joined into one, em/en dashes (`—`, `–`) replaced with `-`, trailing `.`/`…` stripped, and length capped at `MAX_REPLY_CHARS = 280`. It leaves `@username` mentions intact; only research sources pass through `sanitize_research_text`, which renders `@` as a space.
- Research replies pass through `style_research_reply`: forced lowercase outside URLs, paragraphs preserved, body capped at `RESEARCH_BODY_CHARS = 4800`, total with the bot-appended `источники:` block capped at `RESEARCH_MAX_CHARS = 6000`.

## Vision attachments (`src/chat_media.rs`)

- Supported inputs: photos (largest size), static stickers, animated sticker/video/GIF thumbnails, and image documents, taken from the current message or the message being replied to (video audio is not transcribed).
- Capped at `CHAT_IMAGE_MAX_BYTES = 8 MiB` and sent as a `data:` base64 URL.
- Requests carrying an image set `reasoning_effort: "none"`, because otherwise the model spends the entire `max_tokens` budget on internal reasoning and returns an empty `content`.

## Chat memory and compaction (`src/memory.rs`, [ADR 012](../adr/012-free-chat.md), [ADR 014](../adr/014-injection-ignore.md))

- Every message in an active chat is recorded in `chat_messages` with `author_id`, `author_name`, `role`, `flags`, and `compacted_at` (serialized per chat via `Settings::chat_lock`). Raw rows are never deleted.
- `load_memory` feeds the prompt with the chat's `chat_summaries.summary` (`SUMMARY_MAX_CHARS = 1500`) plus up to `RECENT_MESSAGES = 40` uncompacted messages (`RECENT_MAX_CHARS = 3000`) having `flags = 0`.
- Every `COMPACT_INTERVAL_SECS = 600` (10 minutes), `memory_compactor` summarizes old uncompacted messages (`KEEP_FRESH = 40`, `KEEP_NEWEST = 5` with `COMPACT_AFTER_SECS = 12h`, up to `COMPACT_BATCH_MESSAGES = 200` / `COMPACT_BATCH_CHARS = 12_000` per pass, `COMPACT_PASSES = 3`) into `chat_summaries`, marking compacted rows with `compacted_at` and pruning expired rows from `ignored_users`.

## Function tool registry (`src/tool_registry.rs`, `src/chat_tools.rs`, [ADR 015](../adr/015-tool-registry.md))

`ToolRegistry::builtin()` is the single source of truth for the schemas, visibility and research-mode filtering of all 9 function tools; `run_tool_call` (`src/chat_tools.rs`) is the only executor and enforces the per-reply budgets. Web search providers live in `src/search.rs`, multi-query research in `src/research.rs`, the deterministic `feature_status` renderer in `src/chat_status.rs`, and the page reader in `src/read_url.rs`.

| Tool | Visibility | Kind | Purpose |
|---|---|---|---|
| `web_search` | `All` | `WebSearch` | Single fresh fact (up to 5 results, 2500 chars). Default provider: keyless `firecrawl` (`api.firecrawl.dev/v1/search`); override via `SEARCH_PROVIDER=tavily` (`TAVILY_API_KEY`) or `ddg` |
| `research` | `All` | `Research` | Multi-query survey/comparison (2 to 4 queries in parallel, up to 8 deduplicated ranked sources, 5000-char dossier, 15s timeout, 20s per-user cooldown, 1 per reply, max 6 total search calls per reply; [ADR 016](../adr/016-research.md)) |
| `read_url` | `All` | `ReadUrl` | Read one public `http(s)` URL present in the current user prompt (max 3 calls/reply, DNS-pinned public IPv4, max 5 redirects, 2 MiB / 24,000 chars, 10-min cache; [ADR 019](../adr/019-read-url.md)) |
| `propose_feature` | `All` | `ProposeFeature` | Queue a feature request in `self-improve/inbox` (stage `plan`); supports `refines=<id>` for owner plan revisions |
| `feature_status` | `All` | `FeatureStatus` | Read-only queue inspection (`summary`, `details`, `links`). Owner sees the whole queue; non-owners see only their own proposals. Replaces the model's final text with deterministic output when appropriate |
| `feature_confirmation` | `Owner` | `FeatureConfirmation` | Read-only list of tasks waiting at the `plan` or `deploy` gate with inline approval buttons |
| `inspect_code` | `Owner` | `InspectCode` | Read-only search or file read over `/var/lib/maksut/code` (`src/`, `tools/`, `docs/`, `Cargo.toml`, `README.md`, `AGENTS.md`; [ADR 018](../adr/018-owner-code-inspection.md)) |
| `find_stickers` | `Owner` | `FindStickers` | Semantic search over the owner's 15 newest sticker packs (`sticker_descriptions.json`) |
| `send_sticker` | `Owner` | `SendSticker` | Send at most one sticker per reply by `sticker_id`, semantic `query`, or classic alias |

## Stickers (`src/stickers.rs`, `tools/build_sticker_index.py`, [ADR 010](../adr/010-sticker-replies.md), [ADR 011](../adr/011-semantic-sadness.md), [ADR 013](../adr/013-newest-sticker-packs.md))

- **Classic owner triggers (`sticker_rule`).** Standalone word/stem matches in owner messages when no supported link and no pending self-improve decision (`has_pending_decision`) is present:
  - `"да"` (in messages `<= 3` words) -> «Пизда»
  - `"нет"` (in messages `<= 3` words) -> «Пидора ответ»
  - `"ну"` (single-word message) -> «Баранки гну»
  - `"300"` / `"триста"` (including `"300$"`, `"300 баксов"`, word-bounded so `1300` / `3000` do not match) -> «Отсоси у тракториста»
  - `"молодец"` / `"молодца"` / `"молодцом"` / `"молодцы"` / `"молодчина"` -> «Молодцы, сосут концы»
  - Sadness stems (`груст*`, `депрес*`, `депрешн*`, `печал*`, `тоска`/`тосклив*`, `хандра`, `расстро*`, `подавлен*`, `поник*`, `тлен`, `паршив*`, `одинок*`) and phrases (`кошки скребут`, `нет сил`, `настроение на нуле`, `валится из рук`, `хочется плакать`) -> «Грустно везде»
  Stickers are matched by `file_unique_id` and resolved to bot-specific `file_id`s at startup via `getStickerSet`.
- **15-pack semantic catalog.** `sticker_sets.txt` lists the 15 newest non-emoji sticker packs on the owner's account, and `sticker_descriptions.json` maps `<pack_short_name>:<1-based_index>` to Russian descriptions. At startup `load_sticker_catalog` fetches live `file_id`s with `getStickerSet` and joins them by index.
- **Rebuilding the sticker index (on the host running Telegram MCP):**
  ```sh
  STICKER_NEWEST=15 uv run --with mcp --with httpx python tools/build_sticker_index.py
  ```
  The script writes `sticker_sets.txt` and `sticker_descriptions.json` atomically after each sheet.
