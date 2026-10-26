"""One prompt with two pieces labelled as the cockpit labels them (what
they link, where, and how long), then the session replayed in a fresh
process: the user's message as the agent replays it, every chunk.
usage: acp_pieces.py NAME OUT.json -- PROGRAM ARGS..."""
import json, sys, time, queue
name, out = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], name, out] + sys.argv[3:]
exec(open(__file__.replace("acp_pieces.py", "acp_probe.py")).read().split("caps = {")[0])

def escaped(text):
    return "".join(chr(b) if (chr(b).isascii() and (chr(b).isalnum() or chr(b) in "-._~")) else "%%%02X" % b for b in text.encode())

words = "see #flaky and @a.rs: reply with the two secret words, nothing else."
first = "#flaky (a tag)\nThe first secret word is HERON."
second = "@a.rs\n- a.rs:\n  // The second secret word is MAPLE.\n"
subject = json.dumps({"kind": "code_file", "path": "a.rs"}, separators=(",", ":"))
pieces = [
    {"label": f"nah://tag/flaky?at=4-10-auto&len={len(first.encode())}", "text": first},
    {"label": f"nah://thing?subject={escaped(subject)}&at=15-20-pointer&len={len(second.encode())}", "text": second},
]
caps = {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False}
rec = {"words": words, "pieces": pieces}
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps}, timeout=900)
sid = c.call("session/new", {"cwd": cwd, "mcpServers": []}, timeout=900)["result"]["sessionId"]
c.notes = []
prompt = [{"type": "text", "text": words}] + [{"type": "resource", "resource": {"uri": p["label"], "mimeType": "text/plain", "text": p["text"]}} for p in pieces]
r = c.call("session/prompt", {"sessionId": sid, "prompt": prompt}, timeout=1200)
rec["said"] = "".join(n["params"]["update"].get("content", {}).get("text", "") for n in c.notes if n.get("method") == "session/update" and n["params"]["update"].get("sessionUpdate") == "agent_message_chunk")
c.close()
time.sleep(2)
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps}, timeout=900)
c.notes = []
c.call("session/load", {"sessionId": sid, "cwd": cwd, "mcpServers": []}, timeout=900)
time.sleep(3)
while True:
    try: c.notes.append(c.q.get(timeout=2))
    except queue.Empty: break
rec["replayed"] = [n["params"]["update"] for n in c.notes if n.get("method") == "session/update" and n["params"]["update"].get("sessionUpdate") == "user_message_chunk"]
json.dump(rec, open(out, "w"), indent=1)
print("said:", rec["said"][:100], "| chunks:", len(rec["replayed"]), flush=True)
c.close()
