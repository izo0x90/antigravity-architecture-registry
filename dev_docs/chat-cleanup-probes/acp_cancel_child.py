"""Starts a turn whose sub-agent waits, then cancels only the sub-agent
(session/cancel on its own session) and records what follows: whether the
sub-agent stops and whether the main agent's turn goes on.
usage: acp_cancel_child.py CWD OUT.json -- PROGRAM ARGS..."""
import json, sys, time, queue, threading
cwd_arg, out = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], "cancel", out] + sys.argv[3:]
exec(open(__file__.replace("acp_cancel_child.py", "acp_probe.py")).read().split("caps = {")[0])
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False, "subagents": {}}})
sid = c.call("session/new", {"cwd": cwd_arg, "mcpServers": []})["result"]["sessionId"]
c.notes = []
prompt = ("Start exactly one sub-agent (general-purpose) whose only job is to run the shell command `sleep 90` "
          "and then reply 'slept'. Wait for it. When it has finished or stopped, whatever its outcome, reply "
          "'parent done: ' followed by what the sub-agent reported.")
done = {}
def run():
    done["r"] = c.call("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": prompt}]}, timeout=600)
threading.Thread(target=run, daemon=True).start()
child, cancelled_at = None, None
end = time.time() + 600
while time.time() < end and "r" not in done:
    time.sleep(0.5)
    if child is None:
        for n in list(c.notes):
            u = n.get("params", {}).get("update", {}) if isinstance(n, dict) else {}
            if u.get("sessionUpdate") in ("subagent_update", "subagent_spawned"):
                child = u.get("sessionId") or u.get("subagentSessionId")
    if child and cancelled_at is None:
        time.sleep(8)  # let it get into its sleep
        c.n += 1
        c.send({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": child}})
        cancelled_at = len(c.notes)
        c.notes.append({"probe": "sent session/cancel to the child", "child": child})
time.sleep(3)
json.dump({"session": sid, "child": child, "result": done.get("r"), "messages": c.notes}, open(out, "w"), indent=1)
print("child:", child, "messages:", len(c.notes), "stop:", (done.get("r") or {}).get("result", {}).get("stopReason"), flush=True)
c.close()
