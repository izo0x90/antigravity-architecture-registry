"""Replays an existing ACP session (session/list, then session/load) and
records every raw update. No prompt is sent: no model tokens.
usage: acp_replay.py CWD OUT.json -- PROGRAM ARGS..."""
import json, sys, time, queue
cwd_arg, out = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], "replay", out] + sys.argv[3:]
exec(open(__file__.replace("acp_replay.py", "acp_probe.py")).read().split("caps = {")[0])
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False}})
listed = c.call("session/list", {})
print("list:", json.dumps(listed)[:1500], flush=True)
sessions = listed.get("result", {}).get("sessions", [])
print("sessions:", [(s.get("sessionId"), s.get("title"), s.get("updatedAt")) for s in sessions][:5], flush=True)
target = sys.argv[-1] if False else None
pick = next((s["sessionId"] for s in sessions if s["sessionId"].startswith("171da13e")), None) or (sorted(sessions, key=lambda s: s.get("updatedAt") or "", reverse=True)[0]["sessionId"] if sessions else None)
cwd_arg = next((s.get("cwd") for s in sessions if s["sessionId"] == pick), cwd_arg)
print("pick", pick, cwd_arg, flush=True)
c.notes = []
load = c.call("session/load", {"sessionId": pick, "cwd": cwd_arg, "mcpServers": []}, timeout=900)
time.sleep(5)
while True:
    try:
        m = c.q.get(timeout=2)
        c.notes.append(m)
    except queue.Empty:
        break
json.dump({"session": pick, "load": load, "updates": [n for n in c.notes if n.get("method") == "session/update"]}, open(out, "w"), indent=1)
print("updates:", len(c.notes), flush=True)
c.close()
