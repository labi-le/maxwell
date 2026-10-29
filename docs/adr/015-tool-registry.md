# 2026-10-29 — Реестр function tools с видимостью

## Context

Tool schemas were assembled inline in `complete_with`, and the owner check was
duplicated in every sticker dispatch arm. `ALLOWED_TOOLS` only gated whether
any tools were sent; it was not the actual authorization boundary. Adding a
public tool required editing a schema list, the prompt, and the dispatcher, so
the lists could drift.

## Decision

- `src/tool_registry.rs` is the single source of truth for a tool's name,
  description, JSON schema, visibility, and `ToolKind`.
- `ToolVisibility::All` means the tool is safe for every chat participant.
  `ToolVisibility::Owner` means owner-only. `ToolVisibility::allows` is the only
  predicate; `visible`, `find`, `names`, and `payload` all use it.
- `ToolRegistry::find(name, owner)` is the single authorization point. An
  unknown name and a hidden tool both return `None`, and the dispatcher replies
  `"Инструмент недоступен."`. Dispatch matches on the exhaustive `ToolKind`, so
  a new kind cannot compile without an explicit handler.
- `complete_with` sends `tools` and `tool_choice` only when the registry has at
  least one visible tool. A provider rejection still falls back to a request
  without tools; tool calls returned anyway pass through `find` like any other.
- Non-owners receive `All` tools only. The non-owner system line is generated
  from `ToolRegistry::names(false)`. Sticker instructions were moved out of the
  shared system prompt into an owner-only block, because the base prompt
  otherwise tells non-owners to call hidden sticker tools.
- The role remains binary (`owner: bool`, derived from `OWNER_ID`). A third role
  would extend `ToolVisibility` and `allows`, not add a second name list.

## Current registry

| name | visibility | kind |
|---|---|---|
| `web_search` | `All` | `WebSearch` |
| `read_url` | `All` | `ReadUrl` |
| `propose_feature` | `All` | `ProposeFeature` |
| `find_stickers` | `Owner` | `FindStickers` |
| `send_sticker` | `Owner` | `SendSticker` |

## Consequences

- A new `All` tool appears in the provider request and in the non-owner system
  line without editing a list of names. A new behavior still needs a `ToolKind`
  variant and a dispatch branch; reusing an existing kind needs only a registry
  entry.
- The security boundary is unchanged: there is no generic MCP bridge, shell,
  filesystem access, or arbitrary local/SSH access. `read_url` is a narrow
  public-web reader with SSRF checks; a registry entry alone does not grant
  filesystem or internal-scheme access.
- `ToolRegistry` is cloned with `ChatConfig`; four small records are cheap.
- Tests use `ToolRegistry::with` to inject a probe tool and assert that an
  `All` probe appears in the actual JSON sent to the provider for both the
  owner and a non-owner.
- `docs/communication-guide.md` stays untouched; it is a read-only operating
  specification. Tool usage instructions belong in each tool description.
