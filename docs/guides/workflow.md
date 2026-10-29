# Review loop: the evidence

> **When to read this:** Read before your FIRST edit to production code (`src/`, `tools/`, `compose.yaml`, `Dockerfile`), to a contract (an env var, a SQLite table or column, a function-tool schema or visibility, the `omp_worker.py` JSON wire format, a queue file shape), or to a document agents act on (`AGENTS.md`, `README.md`, `docs/guides/*`, `docs/adr/*`). `AGENTS.md` carries the rules; this file carries why each one exists and what it cost to learn in this repository.

## Workflow: mandatory, not advisory

Every change to production code, to a contract, or to a document agents act on MUST go through this loop:

> **implement → review (performance + architecture, as SEPARATE passes) → fix → repeat**

**Write the agreement down BEFORE the first implement step:** a numbered list of the agreed points, plus what is explicitly out of scope, so a round is not spent proposing redesigns nobody asked for. Without that list "100% of what was agreed" is unfalsifiable in both directions: a reviewer can assert a point was agreed, an implementer can assert a dropped point never was, and neither can be disproved.

The loop terminates on two conditions, both required: a full round returns **zero** open findings, and **every numbered point is either implemented or struck by explicit agreement**, the strike recorded in the list. Not "the happy path works". Not "tests pass".

**Proportionality.** The gate is risk, not diff size. A low-risk change (docs-only, config-only) takes ONE pass, and that pass is the ARCHITECTURE one: a document or a key that points at the wrong seam sends the next agent there, and neither has a hot path. Never leave which pass survives to the agent's guess. Documents agents act on are IN scope precisely because they fail this way: `README.md` once carried half the architectural rationale alongside operator runbooks while `AGENTS.md` held a single bullet, so agents read neither the right file nor the current contract. The termination conditions NEVER relax: a one-line change with an open finding is not done either.

**A finding may be REFUTED, not only fixed, and the loop must allow it.** A round is clean when every finding is either fixed or disproved with evidence, and a wrong finding implemented is a regression the process invited. Verify a finding against the code before implementing it; say so with evidence when it is wrong. Examples already settled in this tree:

- A proposal to wire semantic emotion models (`cointegrated/rubert-tiny2-cedr-emotion-detection` and `paraphrase-multilingual-MiniLM-L12-v2`) into the sadness sticker trigger was measured before shipping ([ADR 011](../adr/011-semantic-sadness.md)): the classifier scored `"на душе кошки скребут"` at `0.004` while scoring `"я голоден"` at `0.948`, and contrastive embeddings fired false-positive sadness stickers on `"смеюсь до слёз"` (`0.246`) and `"я плачу от счастья"` (`0.242`). Struck with evidence; lexical stems and idioms stayed.
- A proposal to replace `read_url`'s custom DNS pinning and redirect loop with `reqwest-ssrf-guard` was refuted against `Cargo.toml` ([ADR 019](../adr/019-read-url.md)): the crate requires `reqwest 0.13` and `rustc 1.87`, while this package's floor is `rust-version = "1.85"` on `reqwest 0.12`.
- A proposal to reuse `memory::detect_flags` for the 24-hour prompt-injection ignore window was refuted ([ADR 014](../adr/014-injection-ignore.md)): `detect_flags` includes weak conversational markers (`"act as"`, `"ты теперь"`, `"system prompt"`, `"prompt injection"`) that appear in ordinary technical chat and would mute participants discussing LLMs.

When a refutation is ITSELF disputed, neither side closes the finding unilaterally: it goes to whoever owns the numbered list (the agent that set the scope), who rules and records the ruling in that list.

**A green suite is not evidence, and MUST NEVER be reported as a review result.** A review earns its cost only by trying to break the change; mutation is the cheapest way and is expected: revert the fix, confirm the new test fails, restore. **Mutate in a `git archive` export under `/tmp`, never in the live checkout.** In this repository the live checkout is doubly shared: `compose.yaml` bind-mounts `./src`, `./tools`, `./docs`, `./Cargo.toml`, `./README.md` and `./AGENTS.md` into the running `maksut` container (`/var/lib/maksut/code`), and `tools/self_improve_orchestrator.py`'s `base_commit` snapshots the working tree (tracked and untracked files alike) via a temporary `GIT_INDEX_FILE` whenever a self-improve task starts or deploys (`deploy` aborts if `tree_of(current) != tree_of(base)`). A mutation in place both exposes a broken tree to `inspect_code` and aborts any concurrent self-improve deploy.

**A half-applied edit is caught by building before yielding, not by review.** Review passes are read-only and never execute the tree. Run `env CARGO_TARGET_DIR=/tmp/maxwell-target nix-shell --run "cargo test"` and `python3 -m unittest discover -s tools -p 'test_*.py'` before yielding; a tree that does not compile or pass its tests is not yieldable.

**Why architecture is its OWN pass.** Slice-scoped changes repeatedly miss cross-module seams:

- Before `src/tool_registry.rs` ([ADR 015](../adr/015-tool-registry.md)), tool schemas were assembled inline in `complete_with`, owner checks lived inside individual dispatch arms, `ALLOWED_TOOLS` only gated whether any tools were sent, and `SYSTEM_PROMPT` told non-owners about hidden sticker tools. Every local site looked plausible; only a whole-seam pass moved schema, visibility, `ToolRegistry::find(name, owner)` authorization, and owner-only prompt blocks under one source of truth.
- In `src/chat.rs`, letting the model summarize the `feature_status` tool result caused it to hallucinate queue counts from earlier chat turns instead of reporting the file-backed queue; the architectural fix was deterministic rendering from the snapshot when `feature_status` runs, plus stripping side-effect tools after `research` unless the user explicitly asked for a sticker or feature ([ADR 016](../adr/016-research.md)).
- In `src/handler.rs`, the standalone owner sticker trigger (`"да"` / `"нет"`) fired before the chat handler and swallowed the owner's reply to a pending plan or deploy gate until `has_pending_decision` was checked ahead of `sticker_rule`.
- Placing `#[cfg(test)] mod tests` or `#[cfg(test)]` helper methods inside production `src/*.rs` files, or piling unrelated responsibilities into `src/chat.rs` (2,800+ lines) and `src/self_improve.rs` (1,500+ lines). An architecture review pass must reject any diff that adds test code to a production module or grows a multi-concern file instead of extracting a focused module.

**Why performance and resource bounds are their OWN pass.** Every path in `maksut` runs under tight container and Telegram limits:

- Media downloads are bounded by `downloads: Semaphore::new(2)` and per-user `USER_DOWNLOAD_LIMIT = 3`, with `MAX_MEDIA_BYTES` (default 45 MiB, ceiling 50 MB) and `DOWNLOAD_TIMEOUT = 180s`.
- Single-video YouTube links probe metadata (`YOUTUBE_PROBE_TIMEOUT = 25s`) to pick H.264 + AAC streams whose combined size plus `MP4_MUX_HEADROOM` (64 KiB) fits `MAX_MEDIA_BYTES`, muxing without re-encoding instead of burning CPU in `ffmpeg`.
- `read_url` bounds HTML complexity (`READ_URL_MAX_HTML_ELEMENTS = 20_000`, `READ_URL_MAX_HTML_DEPTH = 128`) before handing markup to `dom_smoothie`, because parsing hostile DOM trees is a CPU bound, not just a byte cap (`READ_URL_MAX_BYTES = 2 MiB`).
- Avoidable allocations, unbounded reads (`take` / chunked size checks are required on every HTTP and subprocess pipe), and holding locks across `.await` are blocking findings.

**Comments and docs are first-class review targets.** The recurring defect is the true-when-written claim: a doc or comment that still names Firefox cookie timers after [ADR 008](../adr/008-docker-cookie-refresh-service.md), `ALLOWED_TOOLS` after [ADR 015](../adr/015-tool-registry.md), or a 5-tool registry after `research`, `feature_status`, `feature_confirmation`, `inspect_code` and `read_url` landed. A review that approves the code and ignores the comments or guides its change falsified has not finished.
