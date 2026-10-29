#!/usr/bin/env python3
"""Small OMP execution worker for the self-improvement control plane.

The orchestrator owns queue state, approvals, git worktrees, tests, and deploy.
This process owns one thing: run an OMP agent against an existing workspace in
a disposable Docker container. It speaks one JSON object on stdin and one JSON
object on stdout, so the execution backend can later be replaced without
touching the control plane.

Request:
    {
      "job_id": "plan-...",
      "role": "planner|reviewer|implementer",
      "workspace": "/abs/path",
      "profile_dir": "/abs/path",
      "prompt": "...",
      "tools": "read,grep,glob",
      "model": "...",
      "timeout": 900,
      "max_cost": "0.50",
      "thinking": "high",
      "container_name": "maksut-improve-...",
      "image": "rust:1.98.1-bookworm",
      "network": "maksut-self-improve-internal",
      "omp": "/nix/store/.../bin/omp",
      "nix_store": "/nix/store",
      "proxy_url": "http://maksut-self-improve-proxy:3128"
    }

Response:
    {"ok": true, "output": "...", "error": "", "timed_out": false}

`--dry-run` validates the request and prints the Docker command without running
it. The worker never reads the LLM key; it only mounts the profile directory
that the orchestrator prepared.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

MAX_OUTPUT = 16000
DEFAULT_IMAGE = "rust:1.98.1-bookworm"
DEFAULT_NETWORK = "maksut-self-improve-internal"
DEFAULT_NIX_STORE = "/nix/store"


def _fail(error: str, output: str = "", timed_out: bool = False) -> dict:
    return {"ok": False, "output": output[-MAX_OUTPUT:], "error": error, "timed_out": timed_out}


def _absolute_dir(value: object, field: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{field} must be a non-empty string")
    path = Path(value)
    if not path.is_absolute() or not path.is_dir():
        raise ValueError(f"{field} must be an existing absolute directory")
    return str(path)


def _string(value: object, field: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value):
        raise ValueError(f"{field} must be a non-empty string")
    return value


def _container_name(job: dict) -> str:
    value = job.get("container_name")
    if value is not None:
        name = _string(value, "container_name")
    else:
        role = re.sub(r"[^A-Za-z0-9_.-]+", "-", _string(job.get("role"), "role"))
        job_id = re.sub(r"[^A-Za-z0-9_.-]+", "-", _string(job.get("job_id"), "job_id"))
        name = f"maksut-improve-{job_id}-{role}"[:63]
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,62}", name):
        raise ValueError("container_name is invalid")
    return name


def build_command(job: dict) -> list[str]:
    """Validate a job and return the fixed Docker argv."""
    workspace = _absolute_dir(job.get("workspace"), "workspace")
    profile = _absolute_dir(job.get("profile_dir"), "profile_dir")
    prompt = _string(job.get("prompt"), "prompt")
    tools = _string(job.get("tools"), "tools")
    model = _string(job.get("model"), "model")
    max_cost = _string(job.get("max_cost"), "max_cost")
    thinking = _string(job.get("thinking"), "thinking")
    omp = _string(job.get("omp"), "omp")
    image = _string(job.get("image", DEFAULT_IMAGE), "image")
    network = _string(job.get("network", DEFAULT_NETWORK), "network")
    nix_store = _string(job.get("nix_store", DEFAULT_NIX_STORE), "nix_store")
    proxy_url = job.get("proxy_url")
    if proxy_url is not None:
        proxy_url = _string(proxy_url, "proxy_url")
    timeout = job.get("timeout")
    if not isinstance(timeout, int) or isinstance(timeout, bool) or not 1 <= timeout <= 86400:
        raise ValueError("timeout must be an integer between 1 and 86400")
    name = _container_name(job)

    command = (
        'mkdir -p "$HOME/.omp/agent" && '
        'cp /profile/config.yml /profile/models.yml "$HOME/.omp/agent/" || exit 1; '
        '"$OMP" models refresh >/dev/null 2>&1 || true; '
        'exec "$OMP" -p --tools="$TOOLS" --no-lsp --no-session '
        '--no-title --approval-mode=write --thinking="$THINKING" --max-cost="$MAX_COST" --max-time="$MAX_TIME" '
        '--model="$MODEL" "$IMPROVE_PROMPT"'
    )
    args = [
        "docker", "run", "--rm", "--name", name, "--network", network,
        "--user", "1000:1000", "--cap-drop", "ALL",
        "--security-opt", "no-new-privileges",
        "--cpus", "2", "--memory", "2g", "--pids-limit", "256",
        "--read-only", "--tmpfs", "/tmp:rw,size=512m",
        "-v", f"{nix_store}:/nix/store:ro",
        "-v", f"{workspace}:/workspace:rw",
        "-v", f"{profile}:/profile:ro",
        "-w", "/workspace",
        "-e", "HOME=/tmp/home",
        "-e", f"OMP={omp}",
        "-e", f"MODEL={model}",
        "-e", f"TOOLS={tools}",
        "-e", f"MAX_COST={max_cost}",
        "-e", f"MAX_TIME={timeout}",
        "-e", f"THINKING={thinking}",
        "-e", f"IMPROVE_PROMPT={prompt}",
    ]
    if proxy_url:
        for key in ("HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"):
            args.extend(["-e", f"{key}={proxy_url}"])
    args.extend(["--entrypoint", "/bin/sh", image, "-c", command])
    return args


def run_job(job: dict) -> dict:
    try:
        command = build_command(job)
    except ValueError as error:
        return _fail(str(error))
    name = command[command.index("--name") + 1]
    try:
        proc = subprocess.run(
            command, capture_output=True, text=True, timeout=job["timeout"],
        )
    except subprocess.TimeoutExpired:
        try:
            subprocess.run(
                ["docker", "rm", "-f", name],
                capture_output=True, text=True, timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
        return _fail(f"{job.get('role', 'worker')} timeout", timed_out=True)
    except OSError as error:
        return _fail(f"worker failed to start: {error.__class__.__name__}")
    stdout = proc.stdout or ""
    stderr = proc.stderr or ""
    if proc.returncode != 0:
        return _fail(f"omp exited with code {proc.returncode}", (stdout + stderr)[-MAX_OUTPUT:])
    output = (stdout if stdout.strip() else stderr)[-MAX_OUTPUT:]
    return {"ok": True, "output": output, "error": "", "timed_out": False}


def main(argv: list[str]) -> int:
    dry_run = "--dry-run" in argv[1:]
    try:
        raw = sys.stdin.read()
        job = json.loads(raw)
        if not isinstance(job, dict):
            raise ValueError("request must be a JSON object")
        if dry_run:
            print(json.dumps({"ok": True, "command": build_command(job)}, ensure_ascii=False))
            return 0
        result = run_job(job)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        result = _fail(str(error))
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
