#!/usr/bin/env python3
"""A scripted language server for the client's tests (stdio, JSON-RPC).

  fake_server.py --log FILE [--encoding utf-8|utf-16] [--start-ms N]
                 [--progress-ms N] [--crash-after K] [--stderr-mb M]
                 [--status-ms N] [--refuse-first N] [--hang-after K]
                 [--no-definition] [--ask-config SECTION] [--never-ready]
                 [--highlight-kind 1|2|3] [--batch-ms N] [--deaf]

--status-ms: it says it reports its status, and is quiescent N ms after
`initialized`. --refuse-first: the first N definitions are answered with
an error (content modified). --hang-after: after K definitions it answers
no more. --ask-config: after `initialized` it asks for SECTION and logs
`config <answer>`. --never-ready: its progress never ends.
--highlight-kind: it answers documentHighlight with the asked place, of
that kind (2 read, 3 write); without it, it doesn't offer highlights.
--bad-highlights: those places are past the end of their lines.
--batch-ms: definitions are held until nothing more arrives for N ms, then
answered together, and `batch <count>` is logged (how many the client had
waiting on it at once). --deaf: after `initialized` it reads nothing more
(its input fills up, as a busy server's does).

A definition request is answered with the place it was asked about (same
file, same position), so a test can check positions survive the round
trip. Every message's method, and every changed file, goes to the log.
"""

import argparse
import json
import queue
import sys
import threading
import time

ap = argparse.ArgumentParser()
ap.add_argument("--log", required=True)
ap.add_argument("--encoding", default="utf-16")
ap.add_argument("--start-ms", type=int, default=0)
ap.add_argument("--progress-ms", type=int, default=0)
ap.add_argument("--crash-after", type=int, default=0)
ap.add_argument("--stderr-mb", type=int, default=0)
ap.add_argument("--status-ms", type=int, default=0)
ap.add_argument("--refuse-first", type=int, default=0)
ap.add_argument("--hang-after", type=int, default=0)
ap.add_argument("--no-definition", action="store_true")
ap.add_argument("--ask-config", default="")
ap.add_argument("--never-ready", action="store_true")
ap.add_argument("--highlight-kind", type=int, default=0)
ap.add_argument("--bad-highlights", action="store_true")
ap.add_argument("--batch-ms", type=int, default=0)
ap.add_argument("--deaf", action="store_true")
a = ap.parse_args()
log = open(a.log, "a", buffering=1)
out = sys.stdout.buffer
answered = 0


def send(msg):
    data = json.dumps(msg).encode()
    out.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
    out.flush()


def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode().strip()
        if not line:
            break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])))


if a.stderr_mb:
    chunk = "x" * 1023 + "\n"
    for _ in range(a.stderr_mb * 1024):
        sys.stderr.write(chunk)
    sys.stderr.flush()

# Messages as they arrive (a thread reads them, so waiting can time out).
incoming = queue.Queue()
def read_all():
    for m in iter(read, None):
        incoming.put(m)
        if a.deaf and m.get("method") == "initialized":
            threading.Event().wait()
    incoming.put(None)


threading.Thread(target=read_all, daemon=True).start()
held = []


def answer_held():
    log.write(f"batch {len(held)}\n")
    for m in held:
        pos = m["params"]["position"]
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"uri": m["params"]["textDocument"]["uri"], "range": {"start": pos, "end": pos}}})
    held.clear()


while True:
    try:
        msg = incoming.get(timeout=a.batch_ms / 1000 if held else None)
    except queue.Empty:
        answer_held()
        continue
    if msg is None:
        break
    method = msg.get("method")
    log.write(f"{method}\n")
    if method == "initialize":
        time.sleep(a.start_ms / 1000)
        offered = msg["params"]["capabilities"].get("general", {}).get("positionEncodings", [])
        encoding = a.encoding if a.encoding in offered or a.encoding == "utf-16" else "utf-16"
        caps = {"positionEncoding": encoding}
        if not a.no_definition:
            caps["definitionProvider"] = True
        if a.highlight_kind:
            caps["documentHighlightProvider"] = True
        if a.status_ms:
            caps["experimental"] = {"serverStatusNotification": True}
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": caps, "serverInfo": {"name": "fake-server", "version": "1.2.3"}}})
    elif method == "initialized":
        if a.ask_config:
            send({"jsonrpc": "2.0", "id": 9001, "method": "workspace/configuration", "params": {"items": [{"section": a.ask_config}]}})
        if a.never_ready:
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "load", "value": {"kind": "begin", "title": "Loading"}}})
        if a.status_ms:
            send({"jsonrpc": "2.0", "method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": False}})
            time.sleep(a.status_ms / 1000)
            send({"jsonrpc": "2.0", "method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": True}})
        if a.progress_ms:
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "load", "value": {"kind": "begin", "title": "Loading"}}})
            time.sleep(a.progress_ms / 1000)
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "load", "value": {"kind": "end"}}})
    elif method == "textDocument/definition":
        answered += 1
        if a.crash_after and answered > a.crash_after:
            sys.exit(3)
        if a.hang_after and answered > a.hang_after:
            continue
        if answered <= a.refuse_first:
            send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32801, "message": "content modified"}})
            continue
        if a.batch_ms:
            held.append(msg)
            continue
        p = msg["params"]
        pos = p["position"]
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"uri": p["textDocument"]["uri"], "range": {"start": pos, "end": pos}}})
    elif method == "textDocument/documentHighlight":
        pos = msg["params"]["position"]
        end = {"line": pos["line"], "character": pos["character"] + 1}
        if a.bad_highlights:
            pos, end = {"line": pos["line"], "character": 9999}, {"line": pos["line"], "character": 10000}
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [{"range": {"start": pos, "end": end}, "kind": a.highlight_kind}]})
    elif method == "workspace/didChangeWatchedFiles":
        for change in msg["params"]["changes"]:
            log.write(f"changed {change['uri']} {change['type']}\n")
    elif "id" in msg and "method" not in msg:
        log.write(f"config {json.dumps(msg.get('result'))}\n")
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
