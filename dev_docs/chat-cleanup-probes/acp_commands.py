"""Lists an ACP agent's slash commands (available_commands_update) for a new session.
usage: acp_commands.py NAME -- PROGRAM ARGS..."""
import json, sys, time, queue
sys.argv = [sys.argv[0], sys.argv[1], "/dev/null"] + sys.argv[2:]
exec(open(__file__.replace("acp_commands.py", "acp_probe.py")).read().split("caps = {")[0])
c = Conn()
c.call("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False}})
new = c.call("session/new", {"cwd": cwd, "mcpServers": []})
end = time.time() + 60
while time.time() < end:
    try:
        m = c.q.get(timeout=1)
        if m.get("method") == "session/update":
            c.notes.append(m)
    except queue.Empty:
        pass
names = []
for n in c.notes:
    u = n.get("params", {}).get("update", {})
    if u.get("sessionUpdate") == "available_commands_update":
        names = [(x.get("name"), (x.get("description") or "")[:70]) for x in u.get("availableCommands", [])]
print(len(c.notes), "updates", [n.get("params",{}).get("update",{}).get("sessionUpdate") for n in c.notes])
for name, d in names:
    print(f"/{name}  {d}")
c.close()
