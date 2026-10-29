import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import omp_worker


class OmpWorkerTests(unittest.TestCase):
    def job(self, root: Path, **overrides):
        workspace = root / "workspace"
        profile = root / "profile"
        workspace.mkdir(exist_ok=True)
        profile.mkdir(exist_ok=True)
        value = {
            "job_id": "plan-1",
            "role": "planner",
            "workspace": str(workspace),
            "profile_dir": str(profile),
            "prompt": "read the code",
            "tools": "read,grep,glob",
            "model": "deepseek-v4.1-flash",
            "timeout": 60,
            "max_cost": "0.30",
            "thinking": "high",
            "omp": "/nix/store/omp/bin/omp",
        }
        value.update(overrides)
        return value

    def test_build_command_is_isolated_and_has_no_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = self.job(Path(tmp))
            command = omp_worker.build_command(job)
        self.assertEqual(command[:3], ["docker", "run", "--rm"])
        self.assertIn("--name", command)
        self.assertIn("maksut-improve-plan-1-planner", command)
        self.assertIn("--read-only", command)
        self.assertIn(f"{job['workspace']}:/workspace:rw", command)
        self.assertIn(f"{job['profile_dir']}:/profile:ro", command)
        self.assertIn("IMPROVE_PROMPT=read the code", command)
        self.assertIn("MODEL=deepseek-v4.1-flash", command)
        self.assertNotIn("LLM_API_KEY", " ".join(command))

    def test_run_job_prefers_stdout_and_falls_back_to_stderr(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = self.job(Path(tmp))
            completed = subprocess.CompletedProcess(["docker"], 0, "spec\n", "warning\n")
            with mock.patch.object(omp_worker.subprocess, "run", return_value=completed):
                result = omp_worker.run_job(job)
            self.assertEqual(result["output"], "spec\n")
            completed = subprocess.CompletedProcess(["docker"], 0, "", "fallback\n")
            with mock.patch.object(omp_worker.subprocess, "run", return_value=completed):
                result = omp_worker.run_job(job)
            self.assertEqual(result["output"], "fallback\n")

    def test_invalid_workspace_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = self.job(Path(tmp), workspace="/does/not/exist")
            with self.assertRaises(ValueError):
                omp_worker.build_command(job)

    def test_run_job_returns_agent_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = self.job(Path(tmp))
            completed = subprocess.CompletedProcess(["docker"], 0, "report\n", "")
            with mock.patch.object(omp_worker.subprocess, "run", return_value=completed):
                result = omp_worker.run_job(job)
        self.assertTrue(result["ok"])
        self.assertEqual(result["output"], "report\n")
        self.assertFalse(result["timed_out"])

    def test_timeout_removes_container(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = self.job(Path(tmp), timeout=9)
            timeout = subprocess.TimeoutExpired(["docker"], 9)
            cleanup = subprocess.CompletedProcess(["docker"], 0, "", "")
            with mock.patch.object(
                omp_worker.subprocess, "run", side_effect=[timeout, cleanup],
            ) as run:
                result = omp_worker.run_job(job)
        self.assertFalse(result["ok"])
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["error"], "planner timeout")
        self.assertIn("docker", run.call_args_list[1].args[0])
        self.assertIn("rm", run.call_args_list[1].args[0])


if __name__ == "__main__":
    unittest.main()
