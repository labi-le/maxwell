import importlib.util
import json
import os
import pathlib
import tempfile
import unittest


def load(tmp):
    os.environ["SELF_IMPROVE_DIR"] = str(pathlib.Path(tmp) / "queue")
    os.environ["SELF_IMPROVE_REPO"] = str(pathlib.Path(tmp) / "repo")
    os.environ["OWNER_ID"] = "42"
    for name in ("queue/inbox", "queue/outbox", "queue/approvals", "queue/plan-approvals", "queue/rejections", "queue/work"):
        (pathlib.Path(tmp) / name).mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location("orch", pathlib.Path("tools/self_improve_orchestrator.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ApprovalTests(unittest.TestCase):
    def test_plan_approval_starts_implementation_not_deploy(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            module.status(
                {"id": "plan-1", "chat_id": 10, "goal": "сделай кнопку"},
                "planned", "план готов", stage="plan", base="base",
                spec="spec", review="review",
            )
            (pathlib.Path(tmp) / "queue" / "plan-approvals" / "plan-1.json").write_text(
                json.dumps({"id": "plan-1", "requested_by": 42}))
            called = {}
            module.implement_planned = lambda value: called.update(
                task=value["id"], spec=value["spec"], base=value["base"])
            module.deploy = lambda *args: self.fail("deploy must not run before implementation")
            module.process_approvals()
            self.assertEqual(called, {"task": "plan-1", "spec": "spec", "base": "base"})

    def test_deploy_marker_does_not_approve_a_plan(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            module.status(
                {"id": "plan-2", "chat_id": 10, "goal": "x"},
                "planned", "plan", stage="plan", base="base", spec="s", review="r",
            )
            (pathlib.Path(tmp) / "queue" / "approvals" / "plan-2.json").write_text(
                json.dumps({"id": "plan-2", "requested_by": 42}))
            called = {}
            module.implement_planned = lambda value: called.update(value)
            module.process_approvals()
            self.assertEqual(called, {})

    def test_plan_rejection_is_terminal(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            module.status(
                {"id": "plan-3", "chat_id": 10, "goal": "x"},
                "planned", "plan", stage="plan", base="base", spec="s", review="r",
            )
            (pathlib.Path(tmp) / "queue" / "rejections" / "plan-3.json").write_text("reject")
            called = {}
            module.implement_planned = lambda value: called.update(value)
            module.process_approvals()
            self.assertEqual(called, {})
            value = json.loads((pathlib.Path(tmp) / "queue" / "outbox" / "plan-3.json").read_text())
            self.assertEqual(value["status"], "rejected")

    def test_plan_task_uses_read_only_planner_and_reviewer(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "plan-1"
            worktree.mkdir(parents=True)
            module.base_commit = lambda task_id: "base"
            module.make_worktree = lambda task_id, base: worktree
            module.read_llm_key = lambda: "secret"
            calls = []
            def fake_agent(task, wt, text, tools, model, timeout, cost, role, thinking="max"):
                calls.append((role, tools, model))
                return True, "spec" if role == "planner" else "review"
            module.run_agent = fake_agent
            module.run = lambda *args, **kwargs: type("P", (), {"returncode": 0, "stdout": ""})()
            module.plan_task({"id": "plan-1", "goal": "g", "chat_id": 10})
            value = json.loads((pathlib.Path(tmp) / "queue" / "outbox" / "plan-1.json").read_text())
            self.assertEqual(value["status"], "planned")
            self.assertEqual(value["spec"], "spec")
            self.assertEqual(value["review"], "review")
            self.assertEqual([call[0] for call in calls], ["planner", "reviewer"])
            self.assertTrue(all(call[1] == "read,grep,glob" for call in calls))

    def test_plan_task_does_not_publish_a_planner_key_leak(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "plan-leak"
            worktree.mkdir(parents=True)
            module.base_commit = lambda task_id: "base"
            module.make_worktree = lambda task_id, base: worktree
            module.read_llm_key = lambda: "secret"
            module.run_agent = lambda *args: (True, "spec contains secret")
            module.run = lambda *args, **kwargs: type("P", (), {"returncode": 0, "stdout": ""})()
            module.plan_task({"id": "plan-leak", "goal": "g", "chat_id": 10})
            raw = (pathlib.Path(tmp) / "queue" / "outbox" / "plan-leak.json").read_text()
            value = json.loads(raw)
            self.assertEqual(value["status"], "failed")
            self.assertNotIn("secret", raw)
            self.assertEqual(value["spec"], "")

    def test_create_issue_returns_the_url_and_keeps_spec_out_of_the_title(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            calls = []
            def fake_run(args, **kwargs):
                calls.append(args)
                return type("P", (), {"returncode": 0, "stdout": "https://github.com/labi-le/maxwell/issues/9\n"})()
            module.run = fake_run
            url = module.create_issue({"id": "plan-9", "goal": "добавь кнопку\nвторая строка"}, "spec", "review")
            self.assertEqual(url, "https://github.com/labi-le/maxwell/issues/9")
            self.assertIn("--body-file", calls[0])
            body = pathlib.Path(calls[0][calls[0].index("--body-file") + 1]).read_text()
            self.assertIn("spec", body)
            self.assertIn("review", body)
            self.assertNotIn("\n", calls[0][calls[0].index("--title") + 1][-20:])

    def test_close_issue_uses_deploy_comment(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            calls = []
            module.run = lambda args, **kwargs: calls.append(args) or type(
                "P", (), {"returncode": 0, "stdout": ""})()
            module.close_issue({"issue_url": "https://github.com/labi-le/maxwell/issues/7"}, "Задеплоено.")
            self.assertEqual(calls[0][0], module.GH)
            self.assertEqual(calls[0][1:4], ["issue", "close", "7"])
            self.assertIn("--comment", calls[0])
            self.assertEqual(calls[0][calls[0].index("--comment") + 1], "Задеплоено.")

    def test_close_issue_ignores_non_github_urls(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            calls = []
            module.run = lambda args, **kwargs: calls.append(args)
            module.close_issue({"issue_url": "https://example.com/issue/1"}, "Задеплоено.")
            self.assertEqual(calls, [])

    def test_plan_task_does_not_publish_a_planner_failure_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "plan-fail"
            worktree.mkdir(parents=True)
            module.base_commit = lambda task_id: "base"
            module.make_worktree = lambda task_id, base: worktree
            module.read_llm_key = lambda: "secret"
            module.run_agent = lambda *args: (False, "planner crashed with secret")
            module.run = lambda *args, **kwargs: type("P", (), {"returncode": 0, "stdout": ""})()
            module.plan_task({"id": "plan-fail", "goal": "g", "chat_id": 10})
            raw = (pathlib.Path(tmp) / "queue" / "outbox" / "plan-fail.json").read_text()
            value = json.loads(raw)
            self.assertEqual(value["status"], "failed")
            self.assertNotIn("secret", raw)
            self.assertEqual(value["summary"], "planner failed")

    def test_implement_planned_preserves_owner_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "plan-1"
            worktree.mkdir(parents=True)
            module.make_worktree = lambda task_id, base: worktree
            captured = {}
            module.implement = lambda task, base, wt: captured.update(task)
            module.implement_planned({
                "id": "plan-1", "goal": "g", "chat_id": -100,
                "source_chat_id": -100, "proposed_by": 42,
                "proposed_by_name": "owner", "issue_url": "https://github.com/x/y/issues/1",
                "spec": "s", "review": "r", "base": "base",
            })
            self.assertEqual(captured["source_chat_id"], -100)
            self.assertEqual(captured["proposed_by"], 42)
            self.assertEqual(captured["proposed_by_name"], "owner")
            self.assertEqual(captured["issue_url"], "https://github.com/x/y/issues/1")
            module.status(captured, "running", "started", base="base")
            stored = json.loads((pathlib.Path(tmp) / "queue" / "outbox" / "plan-1.json").read_text())
            self.assertEqual(stored["source_chat_id"], -100)
            self.assertEqual(stored["proposed_by"], 42)
            self.assertEqual(stored["proposed_by_name"], "owner")
            self.assertEqual(stored["issue_url"], "https://github.com/x/y/issues/1")

    def test_run_agent_sends_job_to_worker_without_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            repo = pathlib.Path(tmp) / "repo"
            (repo / "self_improve").mkdir(parents=True, exist_ok=True)
            (repo / "self_improve" / "config.yml").write_text("")
            worktree = pathlib.Path(tmp) / "queue" / "work" / "plan-1"
            worktree.mkdir(parents=True)
            module.ensure_proxy = lambda: None
            module.read_llm_key = lambda: "secret"
            module.docker = lambda *args, **kwargs: type(
                "P", (), {"returncode": 0, "stdout": "", "stderr": ""})()
            calls = []
            def fake_run(args, **kwargs):
                calls.append((args, kwargs))
                return type("P", (), {
                    "returncode": 0,
                    "stdout": json.dumps({"ok": True, "output": "done"}),
                    "stderr": "",
                })()
            module.run = fake_run
            ok, output = module.run_agent(
                {"id": "plan-1"}, worktree, "prompt", "read", "model",
                30, "0.1", "planner",
            )
            self.assertTrue(ok)
            self.assertEqual(output, "done")
            self.assertEqual(calls[0][0][-1], str(repo / "tools" / "omp_worker.py"))
            job = json.loads(calls[0][1]["input"])
            self.assertEqual(job["workspace"], str(worktree))
            self.assertEqual(job["role"], "planner")
            self.assertNotIn("secret", calls[0][1]["input"])
            self.assertIn("secret", (pathlib.Path(tmp) / "queue" / "profile" / "plan-1" / "models.yml").read_text())

    def test_approval_dispatches_deploy(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "task-1"
            worktree.mkdir(parents=True)
            module.status({"id": "task-1"}, "proposed", "ok", files=["README.md"],
                          base="base", branch="improve/task-1", worktree=str(worktree))
            (pathlib.Path(tmp) / "queue" / "approvals" / "task-1.json").write_text(
                json.dumps({"id": "task-1", "requested_by": 42}))
            called = {}
            module.deploy = lambda task, base, branch, worktree, paths: called.update(
                task=task["id"], base=base, branch=branch, paths=paths)
            module.process_approvals()
            self.assertEqual(called, {"task": "task-1", "base": "base", "branch": "improve/task-1", "paths": ["README.md"]})

    def test_rejection_does_not_deploy(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            worktree = pathlib.Path(tmp) / "queue" / "work" / "task-2"
            worktree.mkdir(parents=True)
            module.status({"id": "task-2"}, "proposed", "ok", files=["README.md"],
                          base="base", branch="improve/task-2", worktree=str(worktree))
            (pathlib.Path(tmp) / "queue" / "rejections" / "task-2.json").write_text("reject")
            module.deploy = lambda *args: self.fail("deploy must not run")
            module.process_approvals()
            value = json.loads((pathlib.Path(tmp) / "queue" / "outbox" / "task-2.json").read_text())
            self.assertEqual(value["status"], "rejected")


    def test_queue_sort_key_orders_priority_then_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            low = pathlib.Path(tmp) / "queue" / "outbox" / "low.json"
            high = pathlib.Path(tmp) / "queue" / "outbox" / "high.json"
            low.write_text(json.dumps({"priority": 0}))
            high.write_text(json.dumps({"priority": 5}))
            ordered = sorted([low, high], key=module.queue_sort_key)
            self.assertEqual([path.name for path in ordered], ["high.json", "low.json"])

    def test_status_preserves_priority(self):
        with tempfile.TemporaryDirectory() as tmp:
            module = load(tmp)
            module.status({"id": "plan-1", "priority": 7}, "planned", "ok")
            stored = json.loads((pathlib.Path(tmp) / "queue" / "outbox" / "plan-1.json").read_text())
            self.assertEqual(stored["priority"], 7)

if __name__ == "__main__":
    unittest.main()
