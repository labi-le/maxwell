# 2026-10-29 — Multi-query research tool

## Context

`web_search` was the only search tool. It returns one provider response,
formats it with 280-character chat styling, and has no source identifiers.
A model can call it several times, but collision-prone query rewriting, the
300-token completion limit, and the single-snippet formatting make a real
survey impossible.

The bot must answer research and comparison requests from several sources,
with citations and a reliable source list, without widening the security
boundary: no arbitrary URL fetching, redirects, or SSRF.

## Decision

- Add a public `research` tool next to `web_search`. The model decides when to
  use it; there is no keyword matcher. It passes a question and 2-4 distinct
  queries. `web_search` stays the tool for one fresh fact.
- Split provider transport into `SearchClient::search_hits(query, limit)`.
  Firecrawl, Tavily, and DuckDuckGo return `SearchHit` records; all providers
  share the same response size limit and error classification. `web_search`
  keeps its old output contract: five results, 2500 characters, and the same
  user-facing error strings.
- Run research in a `JoinSet` over an owned `Arc<SearchClient>` so tasks are
  `'static`. Each query asks for four results with the existing 10-second HTTP
  timeout. The whole pass has a 15-second timeout; late tasks are aborted and
  recorded as failures instead of silently disappearing.
- Normalize every hit: only `http`/`https`, no credentials, a host is
  required, fragment and tracking parameters are removed, the host is
  lower-cased, default ports are dropped, and URLs longer than 2048 characters
  are rejected. Deduplication uses a canonical URL with sorted query
  parameters; the displayed URL keeps the original order. `domain` drops a
  leading `www.` so the model does not mistake one site for two.
- Clean titles and snippets: strip HTML, decode numeric and common named
  entities, remove control/bidi characters, neutralize `@` mentions and
  non-web schemes, replace `[`/`]` with `(`/`)`, and cut on word boundaries.
  This prevents mention pings and citation spoofing from untrusted text.
- Rank by the number of distinct queries that returned the URL, then by the
  best provider position, then by URL. Keep at most eight sources and assign
  ids after ranking. The dossier is capped at 5000 characters: snippets are
  shortened first, then sources are dropped from the end.
- After a successful `research`, the final reply uses
  `style_research_reply`: lowercase except URLs, preserved paragraphs, body
  capped at 4800 characters, valid `[n]` markers kept, invalid ones removed,
  and a bot-generated `источники:` block added. If the model cited nothing,
  the first up to three sources are listed. The total is capped at 6000
  characters; the source URL is never cut.
- One research pass per reply, at most six search calls per reply, a 20-second
  per-user cooldown, and `RESEARCH_MAX_TOKENS = 1500` for every completion
  after a successful pass. The 6000-character limit is a hard ceiling, not a
  guarantee: 1500 Russian tokens usually produce less text.
- After research, the tool payload is reduced to `web_search`, `research`, and the read-only `read_url`.
  `propose_feature` returns only when the user explicitly asked for a feature;
  `find_stickers` and `send_sticker` return only when the user explicitly asked
  for a sticker. Queue tools are never offered in research mode. Dispatch
  repeats the same per-kind check, so an injected tool call cannot bypass it.
- The prompt documents tool choice and makes the research style explicitly
  override the "one or two short sentences" rule. A mode-specific system
  message is appended after a successful pass.
- Typing status is refreshed every four seconds while a reply is being
  produced, so a 15-second research pass plus generation still looks alive.

## Limits

| Constant | Value |
|---|---:|
| `RESEARCH_MIN_QUERIES` | 2 |
| `RESEARCH_MAX_QUERIES` | 4 |
| `RESEARCH_RESULTS_PER_QUERY` | 4 |
| `RESEARCH_MAX_SOURCES` | 8 |
| `RESEARCH_TITLE_CHARS` | 160 |
| `RESEARCH_SNIPPET_CHARS` | 300 |
| `RESEARCH_DOSSIER_CHARS` | 5000 |
| `RESEARCH_TIMEOUT` | 15 s |
| `RESEARCH_MAX_TOKENS` | 1500 |
| `RESEARCH_BODY_CHARS` | 4800 |
| `RESEARCH_SOURCES_CHARS` | 1200 |
| `RESEARCH_MAX_CHARS` | 6000 |
| `SEARCH_CALLS_PER_REPLY` | 6 |
| `RESEARCH_MAX_PER_REPLY` | 1 |
| `RESEARCH_COOLDOWN` | 20 s |

## Consequences

- Research never fetches the pages it cites. A source is a provider search
  result: title, URL, and snippet. The fixed search host and disabled redirects
  remain the network boundary.
- Partial failures produce a partial dossier with a per-query error list; all
  failures return `поиск недоступен` and no sources. Zero results return
  `ничего не найдено`.
- The model cannot fabricate the final source list: the bot builds it from the
  dossier, removes invalid markers, and adds fallback sources when needed.
- Empty research text is `empty` before stickers or buttons are attached.
- Automatic stickers are disabled in research mode; an explicit request still
  works.
- `docs/communication-guide.md` is unchanged. Tool behavior lives in tool
  descriptions, `SYSTEM_PROMPT`, and this ADR.
