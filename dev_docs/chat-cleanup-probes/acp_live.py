"""One live prompt to an ACP agent, every raw message recorded (updates,
the agent's requests and our answers).
usage: acp_live.py CWD OUT.json PROMPT -- PROGRAM ARGS..."""
import json, sys, time, queue
cwd_arg, out, prompt = sys.argv[1], sys.argv[2], sys.argv[3]
sys.argv = [sys.argv[0], "live", out] + sys.argv[4:]
exec(open(__file__.replace("acp_live.py", "acp_probe.py")).read().split("caps = {")[0])
c = Conn()
import os
caps_extra = json.loads(os.environ.get("CAPS", "{}"))
init = c.call("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False, **caps_extra}})
new = c.call("session/new", {"cwd": cwd_arg, "mcpServers": []})
sid = new["result"]["sessionId"]
c.notes = []
r = c.call("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": prompt}]}, timeout=900)
time.sleep(3)
while True:
    try:
        c.notes.append(c.q.get(timeout=2))
    except queue.Empty:
        break
json.dump({"session": sid, "result": r, "messages": c.notes}, open(out, "w"), indent=1)
print("messages:", len(c.notes), "stop:", r.get("result", {}).get("stopReason"), flush=True)
c.close()
