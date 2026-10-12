"""The cockpit's frames in a real browser: opens the cockpit over Chrome's DevTools
protocol, shows every view (moved there through the server, as the agent's
`navigate` does, and checked: the server built that view), pans a big tree
in each, and reads from Chrome's own trace how long each frame's work took.
Fails when one took longer than the limit, or a view didn't show or had
no cards.

    uv run --with websockets python scripts/frame_check.py --url <cockpit URL>

It types nothing: it only drags across the graph. Still, run it against a
server with no agent harness (its HOME without one).

--cdp is Chrome's DevTools address (http://127.0.0.1:9222 by default);
--shots, a folder for a screenshot of each view. With no Chrome there it
says so and exits with SKIPPED.
"""
import argparse
import asyncio
import base64
import json
import pathlib
import sys
import tomllib
import urllib.error
import urllib.request

import websockets

# Each view: how it's shown, and its name as the server says it.
VIEWS = ["contains", "calls", "data_flow"]
# Levels shown: a big tree to pan.
DEPTH = 4


class Page:
    def __init__(self, ws):
        self.ws, self.next, self.events = ws, 0, []

    async def call(self, method, **params):
        self.next += 1
        mine = self.next
        await self.ws.send(json.dumps({"id": mine, "method": method, "params": params}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == mine:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})
            self.events.append(msg)

    async def until(self, method, timeout=60):
        async def wait():
            while True:
                for i, e in enumerate(self.events):
                    if e.get("method") == method:
                        return self.events.pop(i)
                self.events.append(json.loads(await self.ws.recv()))
        return await asyncio.wait_for(wait(), timeout)

    async def fetch(self, path, body=None):
        """What the page's own server answers (through the page, so it goes
        where the page's requests go)."""
        init = "{}" if body is None else json.dumps({"method": "POST", "headers": {"Content-Type": "application/json"}, "body": json.dumps(body)})
        script = f"fetch({json.dumps(path)}, {init}).then(r => r.text())"
        result = await self.call("Runtime.evaluate", expression=script, awaitPromise=True, returnByValue=True)
        return result.get("result", {}).get("value", "")

    async def drag(self, x, y, dx, dy, steps):
        await self.call("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
        await self.call("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="left", clickCount=1)
        for i in range(1, steps + 1):
            await self.call("Input.dispatchMouseEvent", type="mouseMoved", x=x + dx * i / steps, y=y + dy * i / steps, button="left", buttons=1)
            await asyncio.sleep(1 / 60)
        await self.call("Input.dispatchMouseEvent", type="mouseReleased", x=x + dx, y=y + dy, button="left", clickCount=1)


async def frames_while(page, work):
    """How long each animation frame's work took (ms) while `work` ran."""
    await page.call("Tracing.start", traceConfig={"includedCategories": ["devtools.timeline"]}, transferMode="ReportEvents")
    await work()
    await page.call("Tracing.end")
    durations = []
    done = False
    while not done:
        msg = page.events.pop(0) if page.events else json.loads(await asyncio.wait_for(page.ws.recv(), 60))
        if msg.get("method") == "Tracing.tracingComplete":
            done = True
        elif msg.get("method") == "Tracing.dataCollected":
            durations += [e.get("dur", 0) / 1000 for e in msg["params"]["value"] if e.get("name") == "FireAnimationFrame" and e.get("ph") == "X"]
    return durations


# The exit code when there's no Chrome to check with.
SKIPPED = 77

# The limit: limits.toml's [limits.perf] frame_ms.
LIMITS = pathlib.Path(__file__).resolve().parent.parent / "crates/arch-service/config/limits.toml"


def frame_limit_ms():
    with open(LIMITS, "rb") as f:
        return float(tomllib.load(f)["limits"]["perf"]["frame_ms"])


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--cdp", default="http://127.0.0.1:9222")
    ap.add_argument("--limit-ms", type=float, default=frame_limit_ms())
    ap.add_argument("--shots")
    a = ap.parse_args()
    request = urllib.request.Request(f"{a.cdp}/json/new?{a.url}", method="PUT")
    try:
        target = json.load(urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request))
    except urllib.error.URLError as e:
        print(f"skipped: no Chrome on the DevTools protocol at {a.cdp} ({e.reason})")
        sys.exit(SKIPPED)
    worst, shown, cards = {}, {}, {}
    try:
        async with websockets.connect(target["webSocketDebuggerUrl"], max_size=1 << 28) as ws:
            page = Page(ws)
            await page.call("Page.enable")
            await page.call("Page.bringToFront")
            await asyncio.sleep(8)  # the WASM loads, the first tree arrives
            # The window as it is (overriding its size breaks the canvas's
            # pointer positions): the graph is the right-hand view.
            metrics = await page.call("Page.getLayoutMetrics")
            width, height = metrics["cssLayoutViewport"]["clientWidth"], metrics["cssLayoutViewport"]["clientHeight"]
            gx, gy = width * 0.72, height * 0.6
            for view in VIEWS:
                # The view, from its starting points, a few levels deep.
                await page.fetch("/api/arch/navigate", {"view": view, "depth": DEPTH})
                await asyncio.sleep(6)
                # The server built it (the last tree it answered is this view).
                perf = json.loads(await page.fetch("/api/perf") or "{}")
                trees = [t for t in perf.get("recent", []) if t.get("what") == "tree"]
                shown[view] = bool(trees) and trees[-1].get("asked", "").startswith(f"{view} ")
                # What it shows: panning an empty view measures nothing.
                tree = json.loads(await page.fetch(f"/api/graph/tree?view={view}&depth={DEPTH}") or "{}")
                cards[view] = len(tree.get("cards", []))

                async def pan():
                    for (dx, dy) in [(-0.3, -0.3), (0.3, 0.3), (-0.2, 0.3), (0.2, -0.3)]:
                        await page.drag(gx, gy, dx * width, dy * height, 45)

                durations = await frames_while(page, pan)
                worst[view] = (max(durations, default=0.0), len(durations))
                if a.shots:
                    shot = await page.call("Page.captureScreenshot", format="png")
                    pathlib.Path(a.shots).mkdir(parents=True, exist_ok=True)
                    pathlib.Path(a.shots, f"{view.replace(' ', '_')}.png").write_bytes(base64.b64decode(shot["data"]))
    finally:
        urllib.request.build_opener(urllib.request.ProxyHandler({})).open(f"{a.cdp}/json/close/{target['id']}")
    failed = False
    for view, (longest, count) in worst.items():
        ok = shown.get(view, False) and cards.get(view, 0) > 0 and count > 0 and longest <= a.limit_ms
        failed |= not ok
        why = "NOT SHOWN" if not shown.get(view) else "NO CARDS" if not cards.get(view) else f"OVER {a.limit_ms} ms" if count and longest > a.limit_ms else "NO FRAMES SEEN" if not count else "ok"
        print(f"{view}: {cards.get(view, 0)} cards, longest frame {longest:.1f} ms of {count} frames {why}")
    sys.exit(1 if failed else 0)


asyncio.run(main())
