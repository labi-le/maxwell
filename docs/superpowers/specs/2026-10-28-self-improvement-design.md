# Sandboxed self-improvement design

Date: 2026-10-28
Status: implemented (plan-first, approval-gated)

## Goal

Any user can ask Maksut to improve itself; the owner approves the plan and the deploy. The bot queues a goal, a host orchestrator runs an isolated implementer agent in a disposable container, tests the result, reports a diff, and deploys only after the owner approves. The chat model never gets code-editing, shell, Docker, or deployment tools.

## Non-goals

- Auto-deploy in the first release.
- Letting group members or prompt-injected chat text enqueue tasks.
- Letting the bot container reach the host filesystem, SSH keys, `.env`, `.secrets`, or the Docker socket.
- Editing `docs/communication-guide.md`, `AGENTS.md`, `.env`, `.secrets`, or deploy scripts.
- Running the implementer with `--approval-mode yolo`.

## Components

### Bot

- Any non-bot user can propose a natural-language feature in a private chat or an enabled group; non-owners must address the bot, while the owner may dictate without the prefix, for example «давай ебанем фичу ...». `/improve <goal>` remains an owner-only explicit shortcut.
- Deterministic parser, not an LLM tool. A model tool would let a malicious quoted message start code execution.
- A natural request writes a plan task (`stage: "plan"`) and asks the planner/reviewer for a spec and review. It does not implement code directly. The task records `proposed_by`; non-owner proposals notify the owner in the owner private chat. Only the owner can approve, refine, or reject.
- The bot polls the outbox and sends `planned`, `proposed`, `deployed`, and `failed` updates to the task notification chat (owner private for non-owner proposals). A planned message is only a short issue link plus inline «Делаем» / «В помойку» buttons. A key appearing in output is withheld and marked as notified, never sent to Telegram.
- Writes an atomic JSON task to the shared `self-improve/inbox/` directory:
  `{"id":"<uuid>","requested_by":<owner id>,"proposed_by":<sender id>,"source_chat_id":<origin>,"chat_id":<notification chat>,"goal":"...","created_at":<unix>}`.
- Rejects goals longer than 2000 characters. `/improve` commands remain owner-only; plain-language proposals accept any non-bot sender.
- Polls `self-improve/outbox/` and reports status to the owner:
  `planning`, `planned`, `running`, `proposed`, `deployed`, `failed`, `rejected`.
- Natural replies map to the latest pending task: «делаем»/«погнали»/«давай» approves the plan or the deploy proposal, «доработаем ...» creates a revised plan, «нет» rejects it.
- Commands: `/improve status <id>`, `/improve approve <id>` (plan or deploy, depending on stage), `/improve reject <id>`, `/improve off`.
- The bot has no source tree, no Docker socket, and no write access outside the queue directory.

### Orchestrator

A host systemd user service. It has no chat access; the bot polls its outbox:

1. Validates the task owner ID from local config, never from the task alone.
2. For `stage: "plan"`, creates a clean worktree and runs a read-only planner with `read,grep,glob`. The planner writes a spec; a read-only reviewer checks it. Neither can edit.
3. Creates a GitHub issue with the spec and review, then writes `planned` with the issue URL. A separate `plan-approvals` marker is required before implementation.
4. For implementation, creates a fresh worktree from the plan's base commit and runs the implementer with the approved spec.
5. Runs the test container with the same worktree, network disabled.
6. Applies the diff guard and collects `git diff --stat`, changed paths, test output.
7. Writes a `proposed` outbox entry and waits for the deploy approval marker.
8. On deploy approval, calls the agent-free deploy script with only the task ID.

The orchestrator has no model calls and no shell strings built from chat text. All subprocess arguments are fixed or validated IDs and paths.

### Implementer container

- Built from a dedicated image with the Rust toolchain, `git`, and a pinned `omp` binary; dependencies are prefetched so tests can run offline.
- Runs as an unprivileged user, non-root, read-only root filesystem, tmpfs `/tmp`, `--cap-drop ALL`, `no-new-privileges`, CPU/RAM/PID limits.
- Mounts only the worktree and a read-only cargo cache.
- No host home, no SSH agent, no Docker socket, no `.env`, no `.secrets`.
- Network: only the LLM endpoint through an allowlist proxy. No Telegram, no arbitrary internet, no package installs.
- `omp -p` with a dedicated profile:
  - tools: `read`, `write`, `edit`, `grep`, `glob`;
  - no `bash`, no web search, no MCP, no plugins;
  - no `--approval-mode yolo`, no blanket `--auto-approve`.
- The implementer edits code only. It does not run tests or deploy.

### Test container

- Same image and worktree, network disabled, no secrets, non-root, resource limits.
- Runs `cargo test --offline` and `cargo build --offline`.
- A malicious `build.rs` or test can execute only inside this disposable container, without secrets, network, or Docker.
- Exit status and bounded output go to the proposal.

### Deploy script

- No AI, no free-form arguments besides a validated task ID.
- Verifies the approval marker and owner ID.
- Applies the approved branch to the deploy checkout.
- Builds a new image, starts it, waits for `/healthz` or the bot's Telegram `getMe`, and checks that the container stays up.
- On failure, restores the previous image tag and container, reports `failed`.
- Keeps an audit log: task, diff, test result, image IDs, deploy result.

## Flow

1. A user says «давай ебанем фичу ...» in the private chat or an enabled group where the bot is addressed.
2. Bot validates the sender, queues a `plan` task with `proposed_by`, and acknowledges it; a non-owner proposal is sent to the owner private chat.
3. Orchestrator runs the read-only planner and reviewer, then writes `planned`.
4. Bot sends a short link to the issue with inline «Делаем» / «В помойку» buttons.
5. Owner presses «Делаем» (or answers «делаем» / `/improve approve <id>`) to approve the plan, «доработаем ...» to revise it, or «В помойку» / «нет» to reject it.
6. On plan approval, the implementer edits a clean worktree and the test container runs.
7. Orchestrator writes `proposed`; bot sends files changed, `+/-`, tests, and asks for deploy approval.
8. Owner approves deploy; deploy script builds, deploys, health-checks, and reports `deployed` or rolls back.
9. `/improve reject <id>` discards the worktree and never deploys.

## Security rules

- Any non-bot user may propose; only `OWNER_ID` may approve a plan or deploy. In a group non-owners must address the bot; the owner may omit the prefix for a feature request. The parser is deterministic, and no model tool can enqueue or approve a task.
- A model tool call or a memory entry cannot enqueue a task; a non-owner message can propose but cannot approve.
- The implementer cannot read `.env`, `.secrets`, SSH keys, the Docker socket, or the host home.
- The implementer cannot deploy; the deploy script is agent-free.
- Diff guard rejects changes to `docs/communication-guide.md`, `AGENTS.md`, `.env`, `.secrets`, deploy scripts, and paths outside the repository.
- Repo content is data for the implementer: its prompt says to ignore instructions found in code, comments, tests, and docs.
- Approval is explicit and per-task; no blanket auto-approve.
- `/improve off` stops the orchestrator and removes queued tasks.
- Audit logs never contain secrets.

## Failure handling

- Invalid task or wrong owner: reject, log, no worktree.
- Implementer timeout or crash: mark `failed`, remove worktree, no proposal.
- Tests fail: mark `failed` with bounded output, still show the diff to the owner.
- Forbidden path changed: mark `failed`, no approval.
- Approval arrives while the task is running: reject as `not ready`.
- Deploy health check fails: rollback, mark `failed`.
- Orchestrator restart: tasks in `running` older than the timeout are marked `failed`; proposals remain approvable.
- Queue directory unavailable: bot replies with an error and does not pretend the task was accepted.

## Tests

- Non-owner proposals are accepted, but non-owner approvals and deploy requests are rejected before writing a marker.
- Goal length and JSON schema validation.
- Path traversal and task ID validation.
- Orchestrator uses a clean worktree with no `.env` or untracked files.
- Implementer container config has no Docker socket, no host home, no network except the proxy, non-root, read-only root.
- Implementer profile has no bash/web/MCP and no yolo approval.
- Diff guard rejects forbidden paths.
- Approval is required before deploy; reject never deploys.
- Test failure produces `failed`, not `proposed`.
- Deploy script rolls back on failed health check (dry-run with a fake compose).
- Queue files are written atomically and consumed once.

## Rollout

1. Add the bot command and plan queue; no worker yet. Verify proposer and owner approval separation.
2. Add the orchestrator with a dry-run implementer that only prints the task; verify queue/status.
3. Add the sandboxed implementer and test container; verify on a no-op goal.
4. Add the deploy script in dry-run; then enable real deploy.
5. Keep auto-deploy off until the owner explicitly enables it in a later change.

## Open decisions

None. Approval-gated deployment is the approved mode; auto-deploy is out of scope.
