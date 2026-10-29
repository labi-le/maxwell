# Self-improvement pipeline and OMP worker

> **When to read this:** Read before touching `src/self_improve.rs`, `src/code_inspect.rs`, `tools/self_improve_orchestrator.py`, `tools/omp_worker.py`, `self_improve/*`, or `packaging/maksut-self-improve.service`.

## Architecture split: control plane vs execution worker ([ADR 017](../adr/017-omp-worker-contract.md))

The self-improvement system is split across three isolated processes:

1. **Bot (`src/self_improve.rs`, inside the `maksut` container):** writes proposals to `self-improve/inbox/*.json`, reads status from `self-improve/outbox/*.json`, sends notifications (`improvement_notifier` polling every 5s and tracking `notified/`), and writes owner decisions to `plan-approvals/`, `approvals/`, and `rejections/`. It never executes shell commands or git operations.
2. **Host orchestrator (`tools/self_improve_orchestrator.py`, systemd user unit `packaging/maksut-self-improve.service`):** owns the queue lifecycle, git worktrees (`self-improve/work/<id>`), base tree snapshots (`base_commit`), GitHub issues (`gh`), offline test execution (`test_worktree` in `self_improve/test.Dockerfile`), and `docker compose` deploy + health check + rollback.
3. **OMP execution worker (`tools/omp_worker.py`):** single entrypoint for running an `omp` agent (`planner`, `reviewer`, or `implementer`) against a disposable worktree inside a locked-down container (`rust:1.98.1-bookworm`, `--read-only`, `--cap-drop ALL`, `no-new-privileges`, `--network maksut-self-improve-internal`). It reads one JSON request on stdin and writes one JSON response (`{"ok": bool, "output": str, "error": str, "timed_out": bool}`) on stdout.

## Two-stage gate: `plan` -> `deploy`

1. **Proposal (`inbox/`):**
   - Created via `/improve <goal>` (owner in private chat) or the `propose_feature` function tool (stage `"plan"`).
   - `propose_feature` with `refines=<id>` queues a revision of an existing `planned` task, passing `previous_spec`, `previous_review`, and `feedback` (`goal`) to the planner without marking the previous plan rejected.
2. **Planning (`plan_task`):**
   - `base_commit` snapshots the working tree (tracked and untracked files) via a temporary `GIT_INDEX_FILE` without touching the real git index.
   - `planner` (`tools="read,grep,glob"`, `PLAN_MODEL`, `PLAN_THINKING="high"`, `PLAN_MAX_COST="0.30"`, `PLAN_TIMEOUT=900`) writes a Markdown specification.
   - `reviewer` (`tools="read,grep,glob"`, `PLAN_REVIEW_THINKING="medium"`) audits the spec against the codebase for gaps, edge cases, security issues, and missing tests.
   - The orchestrator opens a GitHub issue (`SELF_IMPROVE_GITHUB_REPO`, default `labi-le/maxwell`) containing the spec and review, and writes `status: "planned"` with `issue_url` to `outbox/<id>.json`.
   - The bot notifies the owner with the goal, the GitHub issue URL, and two inline buttons (`Делаем` / `В помойку`).
3. **Plan approval (`plan-approvals/` or `rejections/`):**
   - Recorded only from an owner inline button press or an explicit owner command in private chat (`одобри #id`, `отклони #id`, or `/improve approve|reject <id>`). Group messages and forwarded messages never approve or reject.
   - Rejecting a plan closes the GitHub issue (`"План отклонён владельцем."`) and marks the task `rejected`.
4. **Implementation and offline testing (`implement_planned` / `implement`):**
   - Creates a fresh detached worktree at `base` and runs `implementer` (`tools="read,write,edit,grep,glob"`, `MODEL`, `MAX_COST="0.50"`, `IMPLEMENTER_TIMEOUT=900`).
   - Rejects empty diffs, any path matching `FORBIDDEN` (`docs/communication-guide.md`, `AGENTS.md`, `.env`, `.secrets/`, `self-improve/`, `self_improve/`, `tools/self_improve_orchestrator.py`, `tools/self_improve_deploy.py`, `tools/omp_worker.py`, `target/`), or any output/diff containing `LLM_API_KEY`.
   - Commits on branch `improve/<id>` and runs `cargo test --offline` inside `maksut-self-improve-test` (`--network none`, `/nix/store:ro` mounted for `ffmpeg`).
   - Writes `status: "proposed"` (or `"failed"`) and test logs (`self-improve/logs/<id>-tests.log`).
5. **Deploy approval and rollback (`approvals/` -> `deploy`):**
   - Before applying the patch, `deploy` checks `tree_of(current) == tree_of(base)` and `git apply --check`. If the live working tree drifted since `base`, deploy aborts cleanly.
   - Applies the patch, runs `docker compose build maksut` and `docker compose up -d maksut`, and polls container health (`running` with `RestartCount <= 1` for 3 consecutive checks).
   - If `build` or health checks fail, it reverses the patch (`git apply -R`), retags the previous image (`maksut-maksut`), restarts the container with `--no-build`, and marks the task `failed`.
   - On success, it marks the task `deployed`, closes the GitHub issue (`"Задеплоено."`), and removes the worktree and profile directory.

## Task IDs, priority, and cancellation (`src/self_improve.rs`)

- **Task ID matching:** IDs such as `plan-1793270374-1002045081420-214537` display as `#214537` (`short_task_id`). Owner commands accept the full ID, the short numeric tail (`214537`), or any unique numeric prefix/suffix (`37`). Ambiguous fragments return the candidate list instead of guessing or claiming the task is missing.
- **Batch cancel:** `отмени #id1 #id2` (or `/improve cancel <id>`) cancels pending tasks (`inbox` or waiting at a gate) in a single reply; already-running tasks are reported separately and are not killed mid-flight.
- **Priority:** `подними #id`, `опусти #id`, `приоритет #id <n>` (clamped to `-1000..=1000`, `/improve priority <id> [n]`). Both `inbox` and `outbox` sort by `(-priority, filename)`.
- **Kill switch:** `/improve off` writes `self-improve/off`, pausing the orchestrator loop until the file is removed.

## Running the orchestrator on the host

```sh
cp packaging/maksut-self-improve.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now maksut-self-improve.service
journalctl --user -u maksut-self-improve.service -f
```
