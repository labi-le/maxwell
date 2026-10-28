# 013: 15 newest sticker packs and semantic selection

## Context

The bot previously exposed six stickers from one pack through fixed aliases.
The owner wants the 15 most recently added packs on their Telegram account available,
with the model deciding whether a sticker fits and which one to send. Loading all
packs made the initial vision index slow, so the scope is the 15 most recently
installed packs.

Telegram's Bot API cannot enumerate a user's installed packs, but the host-side
Telegram MCP server can. Bot file ids are bot-specific and cannot be copied from
the MCP account, so the bot still loads file ids itself with `getStickerSet`.

## Decision

- `tools/build_sticker_index.py` enumerates the 15 newest non-emoji packs through Telegram MCP,
  asks the MCP server for labelled sticker sheets, and sends each sheet to the
  vision model. It writes one short Russian description per sticker.
- The index key is `pack short name:1-based pack index`. The Bot API returns
  stickers in the same order; this was checked against the known main pack.
- `sticker_sets.txt` lists pack names for the bot. At startup the bot loads the configured
  15 packs with `getStickerSet` and joins descriptions by the same key.
- The owner gets `find_stickers` (search descriptions, pack titles, and emoji)
  and `send_sticker` (`sticker_id`, semantic `query`, or a classic alias).
  Non-owners get neither tool. At most one sticker per reply; a sticker may be
  the whole reply, and the model sends it on its own initiative when it fits.
- MCP and the vision model are indexing-time dependencies only. The bot container
  needs neither network access to MCP nor the Telegram account session.

## Consequences

- The generated index must be rebuilt when the owner installs or changes packs.
  The script is resumable and writes atomically after every sheet.
- Bot API order is the compatibility contract for the index. A reordered pack
  would shift descriptions; re-running the builder restores the mapping.
- A pack the bot cannot access is skipped and counted in the startup log; the
  remaining configured packs still work.
- Sticker descriptions are untrusted data, not instructions. The `find_stickers`
  result carries that warning to the model.
