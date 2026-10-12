#!/usr/bin/env python3
"""A language server for the service's tests (stdio, JSON-RPC).

  fake_lsp.py --log FILE [--prefer PATH] [--progress-ms N] [--version V]
              [--answer-ms N] [--bad-positions] [--refuse-first N]
              [--no-kinds]

A definition request is answered by reading the name at the position and
finding where it's defined in the project (`fn NAME`, `struct NAME`,
`trait NAME`, `def NAME`, `class NAME`, `function NAME`), the first file
in path order winning, or the file named by --prefer when it defines it.
A name not defined so, but declared as a field (`NAME: Type` at the start
of a line), answers that field; else a local binding (`let NAME`) or a
generic (`<NAME`) answers that. A documentHighlight request answers the
name at the position, read (2), or write (3) when an assignment follows
it or a `&mut` borrow takes it (which our reader doesn't count as a write,
so a test can tell whose answer was used); --no-kinds leaves the kind out.
Names it can't find answer nothing. --answer-ms makes each answer wait;
--bad-positions answers with places past the end of their lines;
--refuse-first answers the first N definition requests with an error
(content modified). Every
request is logged as
`definition <path> <line>:<character>`; every changed file as
`changed <path> <type>`. Once initialized it registers to watch
`**/Cargo.{toml,lock}`, as rust-analyzer does.
"""

import argparse
import json
import os
import re
import sys
import time
import urllib.parse

ap = argparse.ArgumentParser()
ap.add_argument("--log", required=True)
ap.add_argument("--prefer", default="")
ap.add_argument("--progress-ms", type=int, default=0)
ap.add_argument("--version", default="9.9.9")
ap.add_argument("--answer-ms", type=int, default=0)
ap.add_argument("--bad-positions", action="store_true")
ap.add_argument("--refuse-first", type=int, default=0)
ap.add_argument("--no-kinds", action="store_true")
a = ap.parse_args()
log = open(a.log, "a", buffering=1)
out = sys.stdout.buffer
root = None
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


def path_of(uri):
    return urllib.parse.unquote(uri[len("file://"):])


def uri_of(path):
    return "file://" + urllib.parse.quote(path)


def name_at(text, line, character):
    lines = text.split("\n")
    if line >= len(lines):
        return None
    s = lines[line]
    # Positions are UTF-16; these tests' files are ASCII.
    if character > len(s):
        return None
    start = character
    while start > 0 and (s[start - 1].isalnum() or s[start - 1] == "_"):
        start -= 1
    end = character
    while end < len(s) and (s[end].isalnum() or s[end] == "_"):
        end += 1
    return s[start:end] or None


DEFINES = r"\b(?:fn|struct|trait|enum|def|class|function)\s+({})\b"
FIELD = r"^\s*(?:pub\s+)?({})\s*:"
LOCAL = r"(?:\blet\s+(?:mut\s+)?|<\s*)({})\b"
WRITES = re.compile(r"^\s*(?:=(?!=)|\+=|-=|\*=|/=)")


def files():
    found = []
    for d, dirs, names in os.walk(root):
        dirs[:] = sorted(x for x in dirs if not x.startswith("."))
        for n in sorted(names):
            if n.endswith((".rs", ".py", ".ts")):
                found.append(os.path.join(d, n))
    found.sort()
    if a.prefer:
        preferred = os.path.join(root, a.prefer)
        if preferred in found:
            found.remove(preferred)
            found.insert(0, preferred)
    return found


def definition(name):
    for template in (DEFINES, FIELD, LOCAL):
        pattern = re.compile(template.format(re.escape(name)))
        for path in files():
            text = open(path, encoding="utf-8").read()
            for i, line in enumerate(text.split("\n")):
                m = pattern.search(line)
                if m:
                    return {"uri": uri_of(path), "range": {"start": {"line": i, "character": m.start(1)}, "end": {"line": i, "character": m.end(1)}}}
    return None


def highlight(path, line, character):
    s = open(path, encoding="utf-8").read().split("\n")[line]
    start = character
    while start > 0 and (s[start - 1].isalnum() or s[start - 1] == "_"):
        start -= 1
    end = character
    while end < len(s) and (s[end].isalnum() or s[end] == "_"):
        end += 1
    borrowed = re.search(r"&mut\s+[\w.]*$", s[:start])
    kind = 3 if WRITES.match(s[end:]) or borrowed else 2
    answer = {"range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}}}
    if not a.no_kinds:
        answer["kind"] = kind
    return [answer]


while True:
    msg = read()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        root = path_of(msg["params"]["rootUri"])
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {"positionEncoding": "utf-16", "definitionProvider": True, "documentHighlightProvider": True}, "serverInfo": {"name": "fake-lsp", "version": a.version}}})
    elif method == "initialized":
        # As rust-analyzer does: it wants to hear when the manifest changes.
        send({"jsonrpc": "2.0", "id": "watch-1", "method": "client/registerCapability", "params": {"registrations": [{"id": "manifest", "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": [{"globPattern": "**/Cargo.{toml,lock}"}]}}]}})
        if a.progress_ms:
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "load", "value": {"kind": "begin", "title": "Loading"}}})
            time.sleep(a.progress_ms / 1000)
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "load", "value": {"kind": "end"}}})
    elif method == "textDocument/definition":
        p = msg["params"]
        path = path_of(p["textDocument"]["uri"])
        pos = p["position"]
        log.write(f"definition {os.path.relpath(path, root)} {pos['line']}:{pos['character']}\n")
        try:
            name = name_at(open(path, encoding="utf-8").read(), pos["line"], pos["character"])
        except OSError:
            name = None
        time.sleep(a.answer_ms / 1000)
        answered += 1
        if answered <= a.refuse_first:
            send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32801, "message": "content modified"}})
            continue
        found = definition(name) if name else None
        if found and a.bad_positions:
            found["range"]["start"]["character"] = 9999
            found["range"]["end"]["character"] = 9999
        send({"jsonrpc": "2.0", "id": msg["id"], "result": found})
    elif method == "textDocument/documentHighlight":
        p = msg["params"]
        path = path_of(p["textDocument"]["uri"])
        pos = p["position"]
        log.write(f"highlight {os.path.relpath(path, root)} {pos['line']}:{pos['character']}\n")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": highlight(path, pos["line"], pos["character"])})
    elif method == "workspace/didChangeWatchedFiles":
        for change in msg["params"]["changes"]:
            log.write(f"changed {os.path.relpath(path_of(change['uri']), root)} {change['type']}\n")
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
    elif method is not None and "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
