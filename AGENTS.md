# Project rules

## Read-only files

- `docs/communication-guide.md` is read-only. It is the authoritative operating specification compiled into the bot with `include_str!`; never edit, reformat, or regenerate it. Change bot behavior in code or in `README.md` / ADRs instead.
