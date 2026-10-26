"""One prompt that checks several things at once (for a harness whose quota
is scarce): an attached piece, a sub-agent, every raw message recorded; then
the session's commands, and its replay in a fresh process.
usage: acp_combined.py CWD OUT.json -- PROGRAM ARGS..."""
import json, sys, time, queue
cwd_arg, out = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], "combined", out] + sys.argv[3:]
exec(open(__file__.replace("acp_combined.py", "acp_probe.py")).read().split("caps = {")[0])
caps = {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False, "subagents": {}}
rec = {}
def save():
    json.dump(rec, open(out, "w"), indent=1)
c = Conn()
rec["initialize"] = c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps}, timeout=900)
save()
new = c.call("session/new", {"cwd": cwd_arg, "mcpServers": []}, timeout=900)
rec["session/new"] = new
sid = new["result"]["sessionId"]
time.sleep(5)
rec["before_prompt"] = c.notes; c.notes = []
save()
prompt = [
    {"type": "text", "text": "Look at #probe-note. Start exactly one sub-agent and ask it to count the .py files in the current folder (not recursively) and report the number. Then reply in one line: the secret word from #probe-note, and the sub-agent's number."},
    {"type": "resource", "resource": {"uri": "nah://tag/probe-note", "mimeType": "text/plain", "text": "#probe-note (a tag)\nThe secret word is PELICAN-42."}},
]
rec["prompt"] = c.call("session/prompt", {"sessionId": sid, "prompt": prompt}, timeout=1200)
time.sleep(3)
while True:
    try: c.notes.append(c.q.get(timeout=2))
    except queue.Empty: break
rec["session"] = sid; rec["messages"] = c.notes
save()
c.close()
time.sleep(2)
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps}, timeout=900)
rec["load"] = c.call("session/load", {"sessionId": sid, "cwd": cwd_arg, "mcpServers": []}, timeout=900)
time.sleep(3)
while True:
    try: c.notes.append(c.q.get(timeout=2))
    except queue.Empty: break
rec["replay"] = c.notes
save()
c.close()
print("done", sid, len(rec["messages"]), "live messages,", len(rec["replay"]), "replayed", flush=True)
