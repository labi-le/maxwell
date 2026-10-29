# 2026-10-29 — Owner code introspection

## Context

The chat model could discuss self-improvement but had no way to read the actual
code it was proposing to change. The only prior path was `propose_feature`,
which queues a planner that reads the repository later. That made the immediate
conversation shallow and produced proposals that missed existing interfaces.

Giving the model a generic filesystem or shell tool would violate the existing
no-bridge boundary and expose `.env`, cookies, keys, and the self-improve queue.

## Decision

- Add an owner-only `inspect_code` function tool.
- The bot mounts a read-only snapshot:
  - `src/`
  - `tools/`
  - `docs/`
  - `Cargo.toml`
  - `README.md`
- Excluded: `.env`, `.secrets`, `self-improve`, `cookies`, `target`, `.git`.
- The tool can search a query across allowed text files or read one relative
  file. It has no shell, no write path, and no network.
- Every explicit path is canonicalized and must stay under `/var/lib/maksut/code`; recursive search skips symlinks so a mounted snapshot cannot escape through a link.
- Only known text extensions are read. Output is capped and labelled as data,
  not instructions.
- The owner prompt tells the model to inspect code before proposing changes and
  to cite file paths.

## Consequences

- The model gets real file/line context for code review and proposals.
- The snapshot is deliberately smaller than the repository; adding another
  directory requires an explicit compose and code change.
- Non-owners never see the tool or its prompt text.
- The tool is read-only; actual edits still go through the self-improve plan
  and deploy gates.
