#!/usr/bin/env python3
"""Requirement status: which requirements in spec/requirements.toml are proven.

A requirement is VERIFIED when tests tagged with its id (`// req: ID` or
`# req: ID` directly above the test function) exist and all pass; OPEN when
no tagged test exists; FAILING when one fails; UNRUN when not run yet. `manual` requirements are VERIFIED
by a `done` field.

Usage (from meta-harness/):
  req_status.py --brief            one line for prompts (uses the cached run)
  req_status.py --full             table by slice
  req_status.py --gate [SLICE]     exit 2 listing unproven reqs of SLICE (default: spec/current_slice)
  req_status.py --hook-stop        Stop hook: block with the gate list unless stop_hook_active
  add --cached to any mode to skip running cargo and reuse the last results.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "spec" / "requirements.toml"
SLICE_FILE = ROOT / "spec" / "current_slice"
CACHE = ROOT / ".nah" / "req_status.json"
SCAN_DIRS = ["crates", "tests", "src", "scripts"]

TAG_RE = re.compile(r"^\s*(?://|#)\s*req:\s*(.+?)\s*$")
RUST_FN_RE = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)")
PY_FN_RE = re.compile(r"^\s*(?:async\s+)?def\s+(test_[A-Za-z0-9_]*)")
ATTR_RE = re.compile(r"^\s*#\[")
RESULT_RE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)")
# unittest -v: "test_name (module.Class.test_name) ... ok"
PY_RESULT_RE = re.compile(r"^(test_\w+) \(.*\) \.\.\. (ok|FAIL|ERROR)")


@dataclass
class Req:
    id: str
    slice: str
    title: str
    acceptance: str
    manual: bool = False
    done: str | None = None
    tests: list[str] = field(default_factory=list)


def load_spec(path: Path | None = None) -> list[Req]:
    data = tomllib.loads((path or SPEC).read_text())
    reqs = []
    seen = set()
    for raw in data.get("req", []):
        missing = [k for k in ("id", "slice", "title", "acceptance") if k not in raw]
        if missing:
            raise SystemExit(f"requirements.toml: entry {raw.get('id', '?')} lacks {missing}")
        if raw["id"] in seen:
            raise SystemExit(f"requirements.toml: duplicate id {raw['id']}")
        seen.add(raw["id"])
        reqs.append(Req(raw["id"], raw["slice"], raw["title"], raw["acceptance"], raw.get("manual", False), raw.get("done")))
    return reqs


def scan_tags(root: Path | None = None) -> dict[str, list[str]]:
    """Maps requirement id -> test function names tagged with it."""
    root = root or ROOT
    tagged: dict[str, list[str]] = {}
    for top in SCAN_DIRS:
        base = root / top
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if path.suffix not in (".rs", ".py") or "target" in path.parts or "fixtures" in path.parts:
                continue
            lines = path.read_text(errors="replace").splitlines()
            for i, line in enumerate(lines):
                m = TAG_RE.match(line)
                if not m:
                    continue
                ids = [x.strip() for x in m.group(1).split(",") if x.strip()]
                # The tag sits above the function, possibly with attributes in between.
                for nxt in lines[i + 1 : i + 8]:
                    fn = (RUST_FN_RE if path.suffix == ".rs" else PY_FN_RE).match(nxt)
                    if fn:
                        for rid in ids:
                            tagged.setdefault(rid, []).append(fn.group(1))
                        break
                    if not (ATTR_RE.match(nxt) or TAG_RE.match(nxt) or nxt.strip().startswith("@") or not nxt.strip()):
                        break
    return tagged


def run_tests() -> dict[str, str]:
    """Runs the workspace tests; returns test fn name -> ok/FAILED."""
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", str(Path.home() / ".cache" / "meta-harness-sandbox-target"))
    env.setdefault("CARGO_PROFILE_DEV_DEBUG", "0")
    env.setdefault("CARGO_INCREMENTAL", "0")
    cargo = subprocess.run(
        ["cargo", "test", "--workspace", "--no-fail-fast", "--color", "never"],
        cwd=ROOT, env=env, capture_output=True, text=True,
    )
    python = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py", "-v"],
        cwd=ROOT, capture_output=True, text=True,
    )
    return parse_results(cargo.stdout + cargo.stderr + "\n" + python.stdout + python.stderr)


def parse_results(output: str) -> dict[str, str]:
    results: dict[str, str] = {}
    for line in output.splitlines():
        m = RESULT_RE.match(line.strip()) or PY_RESULT_RE.match(line.strip())
        if m:
            name = m.group(1).split("::")[-1]
            outcome = "ok" if m.group(2) == "ok" else ("ignored" if m.group(2) == "ignored" else "FAILED")
            # A name that fails anywhere counts as failing.
            if results.get(name) != "FAILED":
                results[name] = outcome
    return results


def load_results(cached: bool) -> tuple[dict[str, str], float]:
    if cached:
        if not CACHE.exists():
            return {}, 0.0  # no run yet: everything test-backed reads as unproven
        data = json.loads(CACHE.read_text())
        return data["results"], data["at"]
    results = run_tests()
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    CACHE.write_text(json.dumps({"at": time.time(), "results": results}))
    return results, time.time()


def status(req: Req, tagged: dict[str, list[str]], results: dict[str, str]) -> str:
    if req.manual:
        return "VERIFIED" if req.done else "OPEN"
    tests = tagged.get(req.id, [])
    req.tests = tests
    if not tests:
        return "OPEN"
    outcomes = [results.get(t) for t in tests]
    if all(o == "ok" for o in outcomes):
        return "VERIFIED"
    if "FAILED" in outcomes:
        return "FAILING"
    return "UNRUN"


def current_slice() -> str:
    if not SLICE_FILE.exists():
        raise SystemExit(f"missing {SLICE_FILE}: write the slice being worked on (e.g. S1)")
    return SLICE_FILE.read_text().strip()


def evaluate(cached: bool) -> tuple[list[tuple[Req, str]], float]:
    reqs = load_spec()
    tagged = scan_tags()
    unknown = sorted(set(tagged) - {r.id for r in reqs})
    if unknown:
        print(f"warning: tests tagged with unknown ids: {', '.join(unknown)}", file=sys.stderr)
    results, at = load_results(cached)
    return [(r, status(r, tagged, results)) for r in reqs], at


def slice_order(evaluated: list[tuple[Req, str]]) -> list[str]:
    order: list[str] = []
    for r, _ in evaluated:
        if r.slice not in order:
            order.append(r.slice)
    return order


def brief(evaluated, at: float, slice_id: str) -> str:
    parts = []
    for s in slice_order(evaluated):
        items = [st for r, st in evaluated if r.slice == s]
        parts.append(f"{s} {items.count('VERIFIED')}/{len(items)}")
    open_now = [r.id for r, st in evaluated if r.slice == slice_id and st != "VERIFIED"]
    age = f"{int((time.time() - at) / 60)}m old" if at else "never run"
    head = f"[req status, tests {age}] " + " | ".join(parts)
    tail = f"current slice {slice_id} unproven: {', '.join(open_now) if open_now else 'none'}"
    return f"{head}. {tail}. Fully implement each (no placeholders) and tag its test with `// req: ID`."


def full(evaluated, at: float, slice_id: str) -> str:
    when = time.strftime('%H:%M', time.localtime(at)) if at else "never"
    out = [f"Requirement status (tests run: {when}); current slice: {slice_id}"]
    for s in slice_order(evaluated):
        out.append(f"\n== {s} ==")
        for r, st in evaluated:
            if r.slice == s:
                mark = {"VERIFIED": "ok ", "FAILING": "FAIL", "OPEN": "open", "UNRUN": "unrun"}[st]
                extra = f"  tests: {', '.join(r.tests)}" if r.tests else ""
                out.append(f"  [{mark}] {r.id:6} {r.title}{extra}")
    return "\n".join(out)


def gate_list(evaluated, slice_id: str) -> list[str]:
    return [f"{r.id} ({st.lower()}): {r.title} — {r.acceptance}" for r, st in evaluated if r.slice == slice_id and st != "VERIFIED"]


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = p.add_mutually_exclusive_group(required=True)
    mode.add_argument("--brief", action="store_true")
    mode.add_argument("--full", action="store_true")
    mode.add_argument("--gate", nargs="?", const="", metavar="SLICE")
    mode.add_argument("--hook-stop", action="store_true")
    p.add_argument("--cached", action="store_true", help="reuse the last test run")
    args = p.parse_args(argv)

    if args.hook_stop:
        hook_input = json.loads(sys.stdin.read() or "{}")
        if hook_input.get("stop_hook_active"):
            return 0
        evaluated, _ = evaluate(cached=args.cached)
        missing = gate_list(evaluated, current_slice())
        if missing:
            reason = (
                f"Slice {current_slice()} isn't proven yet. Keep going: implement fully (no placeholders), "
                "tag a real test with `// req: ID`, then re-run. Unproven:\n- " + "\n- ".join(missing)
                + "\nIf you're stopping to ask the user something or report a blocker, say so explicitly."
            )
            print(json.dumps({"decision": "block", "reason": reason}))
        return 0

    slice_id = args.gate or current_slice()
    evaluated, at = evaluate(cached=args.cached)
    if args.brief:
        print(brief(evaluated, at, slice_id))
    elif args.full:
        print(full(evaluated, at, slice_id))
    else:
        missing = gate_list(evaluated, slice_id)
        if missing:
            print(f"Slice {slice_id} unproven:\n- " + "\n- ".join(missing))
            return 2
        print(f"Slice {slice_id}: all requirements verified.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
