"""Tests for the requirement status checker."""

import io
import json
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

import req_status as rs

SPEC = """
[[req]]
id = "A.1"
slice = "S0"
title = "First"
acceptance = "first works"

[[req]]
id = "A.2"
slice = "S0"
title = "Second"
acceptance = "second works"

[[req]]
id = "A.3"
slice = "S0"
title = "Manual"
manual = true
acceptance = "reviewed"

[[req]]
id = "B.1"
slice = "S1"
title = "Later"
acceptance = "later works"
"""

TAG = "req"  # kept out of literal tag lines so the real scanner ignores this file

RUST = f"""
// {TAG}: A.1
#[test]
fn first_works() {{}}

// {TAG}: A.1, A.2
#[tokio::test]
async fn both_work() {{}}

// not a tag
fn helper() {{}}
"""

PY = f"""
# {TAG}: B.1
def test_later():
    pass
"""


class Fixture:
    def __init__(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        (root / "spec").mkdir()
        (root / "spec" / "requirements.toml").write_text(SPEC)
        (root / "spec" / "current_slice").write_text("S0\n")
        (root / "crates" / "x" / "src").mkdir(parents=True)
        (root / "crates" / "x" / "src" / "lib.rs").write_text(RUST)
        (root / "scripts").mkdir()
        (root / "scripts" / "test_x.py").write_text(PY)
        self.root = root
        self.patch = mock.patch.multiple(
            rs,
            ROOT=root,
            SPEC=root / "spec" / "requirements.toml",
            SLICE_FILE=root / "spec" / "current_slice",
            CACHE=root / ".nah" / "req_status.json",
        )

    def cache(self, results):
        (self.root / ".nah").mkdir(exist_ok=True)
        (self.root / ".nah" / "req_status.json").write_text(json.dumps({"at": time.time(), "results": results}))

    def __enter__(self):
        self.patch.start()
        return self

    def __exit__(self, *exc):
        self.patch.stop()
        self.dir.cleanup()


def run_main(args, stdin=""):
    out = io.StringIO()
    with redirect_stdout(out), mock.patch("sys.stdin", io.StringIO(stdin)):
        code = rs.main(args)
    return code, out.getvalue()


class ReqStatusTests(unittest.TestCase):
    # req: P0.1
    def test_real_spec_parses_with_unique_ids_and_slices(self):
        reqs = rs.load_spec()
        ids = [r.id for r in reqs]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertTrue(all(r.slice.strip() for r in reqs))
        for needed in ("P0.1", "P2.4", "P6.10", "P9.13", "R.3", "N.1"):
            self.assertIn(needed, ids)

    # req: P0.2
    def test_tags_map_to_the_function_below(self):
        with Fixture() as f:
            tagged = rs.scan_tags(f.root)
        self.assertEqual(tagged["A.1"], ["first_works", "both_work"])
        self.assertEqual(tagged["A.2"], ["both_work"])
        self.assertEqual(tagged["B.1"], ["test_later"])

    # req: P0.3
    def test_status_modes_and_result_parsing(self):
        cargo = "test a::first_works ... ok\ntest b::both_work ... FAILED\n"
        py = "test_later (test_x.T.test_later) ... ok\n"
        results = rs.parse_results(cargo + py)
        self.assertEqual(results, {"first_works": "ok", "both_work": "FAILED", "test_later": "ok"})
        with Fixture() as f:
            f.cache({"first_works": "ok", "both_work": "ok"})
            code, out = run_main(["--gate", "--cached"])
            self.assertEqual(code, 2)
            self.assertIn("A.3", out)
            self.assertNotIn("A.1 ", out)
            code, out = run_main(["--brief", "--cached"])
            self.assertIn("S0 2/3", out)
            self.assertIn("unproven: A.3", out)
            f.cache({"first_works": "ok", "both_work": "FAILED"})
            code, out = run_main(["--full", "--cached"])
            self.assertIn("[FAIL] A.1", out)
            self.assertIn("[open] A.3", out)
            f.cache({})
            code, out = run_main(["--full", "--cached"])
            self.assertIn("[unrun] A.1", out)

    # req: P0.4
    def test_slice_marker_drives_the_gate_and_must_exist(self):
        with Fixture() as f:
            f.cache({"test_later": "ok"})
            (f.root / "spec" / "current_slice").write_text("S1\n")
            code, out = run_main(["--gate", "--cached"])
            self.assertEqual(code, 0, out)
            (f.root / "spec" / "current_slice").unlink()
            with self.assertRaises(SystemExit):
                run_main(["--brief", "--cached"])

    # req: P0.6
    def test_stop_hook_blocks_until_proven_and_respects_active_flag(self):
        with Fixture() as f:
            f.cache({"first_works": "ok", "both_work": "ok"})
            code, out = run_main(["--hook-stop", "--cached"], stdin=json.dumps({"stop_hook_active": False}))
            self.assertEqual(code, 0)
            decision = json.loads(out)
            self.assertEqual(decision["decision"], "block")
            self.assertIn("A.3", decision["reason"])
            code, out = run_main(["--hook-stop", "--cached"], stdin=json.dumps({"stop_hook_active": True}))
            self.assertEqual(out, "")
            text = (f.root / "spec" / "requirements.toml").read_text().replace('manual = true', 'manual = true\ndone = "2026-10-03: ok"')
            (f.root / "spec" / "requirements.toml").write_text(text)
            code, out = run_main(["--hook-stop", "--cached"], stdin="{}")
            self.assertEqual(out, "")


if __name__ == "__main__":
    unittest.main()
