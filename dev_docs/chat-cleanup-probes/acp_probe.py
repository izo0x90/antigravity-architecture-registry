"""Probes an ACP agent: does it take an embedded resource in a prompt, read
it, and replay it as its own piece on session/load?
usage: acp_probe.py NAME OUT.json -- PROGRAM ARGS..."""
import json, os, subprocess, sys, threading, time, queue

name, out_path = sys.argv[1], sys.argv[2]
cmd = sys.argv[sys.argv.index("--") + 1:]
cwd = f"/tmp/acp-probe-{name}"
os.makedirs(cwd, exist_ok=True)
log = {"name": name, "steps": []}

def start():
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(f"{cwd}.stderr", "ab"), cwd=cwd)
    q = queue.Queue()
    def read():
        for line in p.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                q.put(json.loads(line))
            except Exception:
                q.put({"unparsed": line.decode(errors="replace")[:500]})
    threading.Thread(target=read, daemon=True).start()
    return p, q

class Conn:
    def __init__(self):
        self.p, self.q = start()
        self.n = 0
        self.notes = []
    def send(self, msg):
        self.p.stdin.write((json.dumps(msg) + "\n").encode()); self.p.stdin.flush()
    def call(self, method, params, timeout=600):
        self.n += 1; i = self.n
        self.send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.q.get(timeout=1)
            except queue.Empty:
                continue
            if m.get("id") == i and ("result" in m or "error" in m):
                return m
            if "method" in m and "id" in m:  # a request from the agent
                meth = m["method"]
                self.notes.append({"agent_asked": meth, "params": m.get("params")})
                if meth == "session/request_permission":
                    opts = m["params"].get("options", [])
                    allow = next((o["optionId"] for o in opts if o.get("kind", "").startswith("allow")), opts[0]["optionId"] if opts else None)
                    self.send({"jsonrpc": "2.0", "id": m["id"], "result": {"outcome": {"outcome": "selected", "optionId": allow}}})
                elif meth == "fs/read_text_file":
                    path = m["params"].get("path", "")
                    try:
                        text = open(path).read()
                        self.send({"jsonrpc": "2.0", "id": m["id"], "result": {"content": text}})
                    except Exception as e:
                        self.send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": str(e)}})
                else:
                    self.send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "not supported by the probe"}})
            elif "method" in m:
                self.notes.append(m)
        return {"timeout": method}
    def close(self):
        try:
            self.p.stdin.close(); self.p.wait(timeout=10)
        except Exception:
            self.p.kill()

def step(label, value):
    log["steps"].append({label: value})
    json.dump(log, open(out_path, "w"), indent=1)

caps = {"fs": {"readTextFile": True, "writeTextFile": False}, "terminal": False}
c = Conn()
init = c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps})
step("initialize", init)
new = c.call("session/new", {"cwd": cwd, "mcpServers": []})
step("session/new", new)
sid = new.get("result", {}).get("sessionId")
if not sid:
    sys.exit(1)
c.notes = []
prompt = [
    {"type": "text", "text": "Look at #probe-note. What is the secret word in it? Answer with the word only."},
    {"type": "resource", "resource": {"uri": "nah://tag/probe-note", "mimeType": "text/plain", "text": "#probe-note (a tag)\nThe secret word is PELICAN-42."}},
]
r = c.call("session/prompt", {"sessionId": sid, "prompt": prompt})
said = "".join(n["params"]["update"].get("content", {}).get("text", "") for n in c.notes if n.get("method") == "session/update" and n["params"]["update"].get("sessionUpdate") == "agent_message_chunk")
step("session/prompt", {"result": r, "agent_said": said, "other_updates": [n["params"]["update"].get("sessionUpdate") for n in c.notes if n.get("method") == "session/update"], "agent_requests": [n for n in c.notes if "agent_asked" in n]})
c.close()

# Replayed in a fresh process.
time.sleep(2)
c = Conn()
step("initialize again", c.call("initialize", {"protocolVersion": 1, "clientCapabilities": caps}).get("result", {}).get("agentCapabilities"))
load = c.call("session/load", {"sessionId": sid, "cwd": cwd, "mcpServers": []})
user = [n["params"]["update"] for n in c.notes if n.get("method") == "session/update" and n["params"]["update"].get("sessionUpdate") == "user_message_chunk"]
step("session/load", {"result": load, "user_message_chunks": user, "updates": [n["params"]["update"].get("sessionUpdate") for n in c.notes if n.get("method") == "session/update"]})
c.close()
print("done")
