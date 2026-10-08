// Live demos. Every graph demo runs on one small engine and one small
// make-believe codebase, so they look and behave alike. The other demos are
// built from three shapes: a flow (steps in order), a machine (states and
// what moves between them) and lanes (who does what, step by step).
// Keys shown on buttons come from data/actions.json (window.DATA).

(function () {
  "use strict";

  // ── Small helpers ───────────────────────────────────────────────────
  const NS = "http://www.w3.org/2000/svg";
  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : v);
    }
    for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return el;
  }
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const look = () => window.DATA.look;
  const action = (id) => window.DATA.actionsById[id];
  const keyOf = (id) => {
    const a = action(id);
    if (!a) return "";
    return a.keys[0] || a.leader || "";
  };

  // A row of buttons, each named for a real action and showing its key.
  // Pressing the key while the demo has focus does the same.
  function controls(demo, buttons) {
    const bar = h("div", { class: "demo-controls" });
    const byKey = [];
    for (const b of buttons) {
      if (b.sep) {
        bar.append(h("span", { class: "demo-sep" }));
        continue;
      }
      const a = b.a ? action(b.a) : null;
      const label = b.label || (a ? a.label : "");
      const key = b.key !== undefined ? b.key : b.a ? keyOf(b.a) : "";
      const btn = h("button", { class: "demo-btn" + (b.primary ? " primary" : ""), type: "button", title: a ? `${a.label} (${a.id})` : label, onclick: () => { b.run(); demo.focus({ preventScroll: true }); } }, label, key ? h("kbd", {}, key) : null);
      b.el = btn;
      bar.append(btn);
      if (a) for (const k of a.keys) byKey.push([k, b]);
      if (b.alsoKeys) for (const k of b.alsoKeys) byKey.push([k, b]);
    }
    // Key sequences ("z o", "g m"): remember the last presses briefly.
    let typed = [];
    let timer = null;
    demo.addEventListener("keydown", (e) => {
      if (e.target.closest("input, textarea, select")) return;
      const k = chordLabel(e);
      if (!k) return;
      typed.push(k);
      clearTimeout(timer);
      timer = setTimeout(() => (typed = []), 900);
      const seq = typed.join(" ");
      const hit = byKey.find(([key]) => key === seq || seq.endsWith(" " + key) && !byKey.some(([other]) => other.startsWith(key + " ")));
      if (hit) {
        e.preventDefault();
        typed = [];
        hit[1].run();
        flash(hit[1].el);
      } else if (byKey.some(([key]) => key.startsWith(seq + " "))) {
        e.preventDefault();
      } else {
        typed = [];
      }
    });
    return bar;
  }
  function chordLabel(e) {
    if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return null;
    let k = e.key === " " ? "Space" : e.key;
    if (e.ctrlKey) return "ctrl+" + (k.length === 1 ? k.toUpperCase() : k);
    if (e.altKey) return "alt+" + k;
    if (e.shiftKey && k === "Tab") return "shift+Tab";
    return k;
  }
  function flash(el) {
    if (!el) return;
    el.classList.remove("flash");
    void el.offsetWidth;
    el.classList.add("flash");
  }
  function caption(text) {
    return h("div", { class: "demo-caption" }, text);
  }
  function setCaption(el, html) {
    el.innerHTML = html;
  }

  // ── Flow: steps in order ────────────────────────────────────────────
  // steps: [{ title, say, flag? }]. Click a step (or press Next) to read it.
  function Flow(host, steps, opts = {}) {
    const box = h("div", { class: "flow" + (opts.vertical ? " vertical" : "") });
    const detail = h("div", { class: "flow-detail" });
    let at = 0;
    const els = steps.map((s, i) => {
      const el = h("button", { class: "flow-step", type: "button", onclick: () => show(i) }, h("span", { class: "flow-n" }, i + 1), h("span", { class: "flow-title" }, s.title), s.flag ? h("span", { class: "flow-flag" }, s.flag) : null);
      return el;
    });
    els.forEach((el, i) => {
      box.append(el);
      if (i < els.length - 1) box.append(h("span", { class: "flow-arrow" }, opts.vertical ? "↓" : "→"));
    });
    function show(i) {
      at = i;
      els.forEach((el, k) => el.classList.toggle("on", k === i));
      detail.innerHTML = steps[i].say;
    }
    host.append(box, detail, h("div", { class: "demo-controls" }, h("button", { class: "demo-btn", type: "button", onclick: () => show((at + steps.length - 1) % steps.length) }, "Previous step"), h("button", { class: "demo-btn primary", type: "button", onclick: () => show((at + 1) % steps.length) }, "Next step")));
    show(0);
  }

  // ── Machine: states and what moves between them ─────────────────────
  // states: [{ id, word, color, x, y }]; moves: [{ from, to, by: {a|t|say}, say }].
  function Machine(host, spec) {
    const W = spec.width || 640, H = spec.height || 220;
    const wrap = h("div", { class: "machine" });
    const svgEl = document.createElementNS(NS, "svg");
    svgEl.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svgEl.setAttribute("class", "machine-svg");
    const log = h("div", { class: "demo-caption" });
    const buttons = h("div", { class: "demo-controls" });
    wrap.append(svgEl, buttons, log);
    host.append(wrap);
    let current = spec.start;
    const byId = Object.fromEntries(spec.states.map((s) => [s.id, s]));
    function draw() {
      const parts = [`<defs><marker id="mah" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,1 L9,5 L0,9 z" class="mk mk-machine"/></marker></defs>`];
      for (const m of spec.moves) {
        const a = byId[m.from], b = byId[m.to];
        const live = m.from === current;
        const bend = m.bend || 0;
        const mx = (a.x + b.x) / 2 + bend * (b.y - a.y) / 4, my = (a.y + b.y) / 2 - bend * 30;
        const ang = Math.atan2(b.y - my, b.x - mx);
        const ex = b.x - Math.cos(ang) * 58, ey = b.y - Math.sin(ang) * 20;
        const sang = Math.atan2(my - a.y, mx - a.x);
        const sx = a.x + Math.cos(sang) * 58, sy = a.y + Math.sin(sang) * 20;
        parts.push(`<path class="machine-move ${live ? "live" : ""}" d="M${sx},${sy} Q${mx},${my} ${ex},${ey}" marker-end="url(#mah)"/>`);
      }
      for (const s of spec.states) {
        parts.push(`<g class="machine-state ${s.id === current ? "on" : ""}" style="--c:var(--t-${s.color || "muted"})"><rect x="${s.x - 58}" y="${s.y - 18}" width="116" height="36" rx="18"/><text x="${s.x}" y="${s.y + 4}" text-anchor="middle">${esc(s.word)}</text></g>`);
      }
      svgEl.innerHTML = parts.join("");
      buttons.innerHTML = "";
      const here = spec.moves.filter((m) => m.from === current);
      for (const m of here) {
        const a = m.by && m.by.a ? action(m.by.a) : null;
        const key = a ? keyOf(m.by.a) : "";
        const who = m.by && m.by.t ? "agent: " + m.by.t : m.by && m.by.say ? m.by.say : a ? a.label : "";
        buttons.append(h("button", { class: "demo-btn" + (m.by && m.by.t ? " agent" : ""), type: "button", onclick: () => { current = m.to; log.innerHTML = m.say || ""; draw(); } }, who, key ? h("kbd", {}, key) : null));
      }
      if (!here.length) buttons.append(h("button", { class: "demo-btn", type: "button", onclick: () => { current = spec.start; log.innerHTML = ""; draw(); } }, "Start again"));
    }
    draw();
    log.innerHTML = spec.intro || "";
  }

  // ── Lanes: who does what, step by step ──────────────────────────────
  const LANES = [
    { id: "you", name: "You" },
    { id: "cockpit", name: "Cockpit" },
    { id: "server", name: "Server" },
    { id: "agent", name: "Agent" },
  ];
  function Lanes(host, wf, opts = {}) {
    const lanes = LANES.filter((l) => wf.steps.some((s) => s.lane === l.id) || l.id === "you" || l.id === "cockpit");
    const n = wf.steps.length;
    const colW = 156, laneH = 58, left = 82;
    const W = left + n * colW + 10, H = lanes.length * laneH + 8;
    const wrap = h("div", { class: "lanes" });
    const scroller = h("div", { class: "lanes-scroll" });
    const svgEl = document.createElementNS(NS, "svg");
    svgEl.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svgEl.setAttribute("width", W);
    svgEl.setAttribute("height", H);
    svgEl.setAttribute("class", "lanes-svg");
    scroller.append(svgEl);
    const detail = h("div", { class: "lanes-detail" });
    let at = 0;
    const laneY = (id) => lanes.findIndex((l) => l.id === id) * laneH + 4;
    function draw() {
      const parts = [`<defs><marker id="lah" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto"><path d="M0,1 L9,5 L0,9 z" class="mk mk-machine"/></marker></defs>`];
      lanes.forEach((l, i) => {
        parts.push(`<rect class="lane-band ${i % 2 ? "odd" : ""}" x="0" y="${i * laneH + 4}" width="${W}" height="${laneH}"/><text class="lane-name" x="10" y="${i * laneH + 4 + laneH / 2 + 4}">${l.name}</text>`);
      });
      wf.steps.forEach((s, i) => {
        const x = left + i * colW, y = laneY(s.lane);
        if (i > 0) {
          const p = wf.steps[i - 1];
          const px = left + (i - 1) * colW + colW - 14, py = laneY(p.lane) + laneH / 2;
          parts.push(`<path class="lane-arrow ${i <= at ? "done" : ""}" d="M${px},${py} C${px + 10},${py} ${x - 6},${y + laneH / 2} ${x + 4},${y + laneH / 2}" marker-end="url(#lah)"/>`);
        }
        const words = s.say.split(" ");
        const lines = [""];
        for (const w of words) { if ((lines[lines.length - 1] + " " + w).length > 21 && lines.length < 3) lines.push(w); else lines[lines.length - 1] = (lines[lines.length - 1] + " " + w).trim(); }
        if (lines.length === 3 && words.join(" ").length > lines.join(" ").length) lines[2] = lines[2].slice(0, 18) + "…";
        parts.push(`<g class="lane-step ${i === at ? "on" : i < at ? "done" : ""}" data-step="${i}"><rect x="${x + 4}" y="${y + 5}" width="${colW - 18}" height="${laneH - 10}" rx="7"/><text class="lane-n" x="${x + 12}" y="${y + 19}">${i + 1}</text>${lines.map((t, k) => `<text x="${x + 26}" y="${y + 19 + k * 12}">${esc(t)}</text>`).join("")}</g>`);
      });
      svgEl.innerHTML = parts.join("");
      const s = wf.steps[at];
      detail.innerHTML = "";
      detail.append(
        h("div", { class: "lanes-say" }, h("span", { class: "lanes-who" }, LANES.find((l) => l.id === s.lane).name), " ", s.say),
        s.do ? h("div", { class: "lanes-do" }, s.do.map((d) => window.EXPLAIN.trigger(d))) : null,
        s.item ? h("div", { class: "lanes-item" }, "About this: ", window.EXPLAIN.itemLink(s.item)) : null,
        opts.noteKey ? window.EXPLAIN.noteButton(`${wf.id}#${at + 1}`, `Step ${at + 1} of “${wf.title}”`) : null,
      );
      const stepEl = svgEl.querySelector(`[data-step="${at}"]`);
      if (stepEl) {
        const x = left + at * colW;
        if (x < scroller.scrollLeft || x + colW > scroller.scrollLeft + scroller.clientWidth) scroller.scrollTo({ left: x - 40, behavior: "smooth" });
      }
    }
    svgEl.addEventListener("click", (e) => {
      const st = e.target.closest("[data-step]");
      if (st) { at = +st.getAttribute("data-step"); draw(); }
    });
    wrap.append(scroller, detail, h("div", { class: "demo-controls" }, h("button", { class: "demo-btn", type: "button", onclick: () => { at = Math.max(0, at - 1); draw(); } }, "Previous step", h("kbd", {}, "←")), h("button", { class: "demo-btn primary", type: "button", onclick: () => { at = Math.min(n - 1, at + 1); draw(); } }, "Next step", h("kbd", {}, "→"))));
    wrap.tabIndex = 0;
    wrap.addEventListener("keydown", (e) => {
      if (e.key === "ArrowRight") { at = Math.min(n - 1, at + 1); draw(); e.preventDefault(); }
      if (e.key === "ArrowLeft") { at = Math.max(0, at - 1); draw(); e.preventDefault(); }
    });
    host.append(wrap);
    draw();
  }

  // ── A make-believe cockpit frame (for the frame demos) ──────────────
  const VIEW_NAMES = ["chat", "code", "graph", "tree", "spec", "queue"];
  const CODE = [
    "pub fn serve(config: Config) {",
    "    let view = get_view(query);",
    "}",
    "pub fn view(spec: Spec) -> View {",
    "    let graph = load_index();",
    "    graph.query(spec)",
    "}",
    "pub fn ingest(path: &Path) -> Graph {",
    "    let raw = parse_file(path);",
    "    let graph = link_calls(raw);",
    "    save_index(&graph);",
    "    publish(Event::Changed);",
    "}",
  ];
  const CODE_DEFS = { 0: "serve", 3: "view", 7: "ingest" };

  // ── The demos ───────────────────────────────────────────────────────
  const D = {};

  // A few made-up functions for the frame demos (the graph demos use the
  // real code, from the server).
  const CALLS = [["serve", "get_view"], ["serve", "ingest"], ["get_view", "view"], ["view", "load_index"], ["ingest", "parse_file"], ["ingest", "link_calls"], ["ingest", "save_index"], ["ingest", "publish"]];
  const SHAPES = { serve: "Config → ()", get_view: "Query → View", view: "Spec → View", load_index: "() → Graph", ingest: "Path → Graph", parse_file: "Path → RawFile", link_calls: "RawFile → Graph", save_index: "Graph → ()", publish: "Event → ()" };

  D.slots = (host, demo) => {
    let cols = 3, focused = 1;
    const slots = ["code", "graph", "spec"];
    const frame = h("div", { class: "frame" });
    const cap = caption("");
    function draw() {
      frame.innerHTML = "";
      frame.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
      for (let i = 0; i < cols; i++) {
        frame.append(h("div", { class: "frame-slot" + (i === focused ? " on" : ""), onclick: () => { focused = i; draw(); } }, h("div", { class: "frame-tab" }, `slot ${i + 1}`, h("b", {}, slots[i].toUpperCase())), h("div", { class: "frame-body" }, fakeBody(slots[i]))));
      }
      cap.innerHTML = `${cols} column${cols > 1 ? "s" : ""}. Slot ${focused + 1} has focus: keys go to its ${slots[focused]} view.`;
    }
    function put(view) {
      slots[focused] = view;
      draw();
    }
    host.append(controls(demo, [
      { a: "layout.1", run: () => { cols = 1; focused = 0; draw(); } },
      { a: "layout.2", run: () => { cols = 2; focused = Math.min(focused, 1); draw(); } },
      { a: "layout.3", run: () => { cols = 3; draw(); } },
      { sep: true },
      { a: "slot.next", run: () => { focused = (focused + 1) % cols; draw(); } },
      { sep: true },
      ...["graph", "code", "spec", "queue", "tree", "chat"].map((v) => ({ a: "buffer." + v, label: v, run: () => put(v) })),
    ]), frame, cap);
    draw();
  };
  function fakeBody(view) {
    const lines = {
      code: CODE.slice(3, 7),
      graph: ["┌ view ┐──▶┌ load_index ┐", "", "┌ get_view ┐──▶ view"],
      spec: ["view", "  Spec", "  Calls: load_index", "  Checks"],
      queue: ["view      ● running", "store     ○ queued", "ingest    ✓ done"],
      tree: ["▾ arch-service", "  ▾ service", "    view", "    ingest"],
      chat: ["you: where is view built?", "agent: in service.rs…"],
    }[view];
    return lines.map((l) => h("div", { class: "frame-line" }, l || " "));
  }

  D.cursor = (host, demo) => {
    const panes = [
      { name: "tree", rows: ["arch-service", "service", "view", "ingest", "store"] },
      { name: "spec", rows: ["Summary", "Signature", "Calls", "Data flow", "Checks"] },
      { name: "graph", rows: ["get_view", "view", "load_index", "ingest", "publish"] },
    ];
    let focused = 0;
    const cur = [2, 0, 1];
    const grid = h("div", { class: "panes" });
    const cap = caption("");
    function draw(said) {
      grid.innerHTML = "";
      panes.forEach((p, i) => {
        grid.append(h("div", { class: "pane" + (i === focused ? " on" : ""), onclick: () => { focused = i; draw(); } }, h("div", { class: "pane-title" }, p.name), p.rows.map((r, k) => h("div", { class: "pane-row" + (k === cur[i] ? " cur" : "") }, r))));
      });
      cap.innerHTML = said || `The cursor in <b>${panes[focused].name}</b> is on <b>${panes[focused].rows[cur[focused]]}</b>. Same keys in every pane.`;
    }
    const move = (d) => () => { cur[focused] = Math.max(0, Math.min(panes[focused].rows.length - 1, cur[focused] + d)); draw(); };
    host.append(controls(demo, [
      { a: "nav.down", run: move(1) },
      { a: "nav.up", run: move(-1) },
      { a: "nav.first", run: () => { cur[focused] = 0; draw(); } },
      { a: "nav.last", run: () => { cur[focused] = panes[focused].rows.length - 1; draw(); } },
      { sep: true },
      { a: "slot.next", run: () => { focused = (focused + 1) % panes.length; draw(); } },
    ]), grid, cap);
    draw();
    demo.tabIndex = 0;
  };

  D.goto = (host, demo) => {
    const state = { line: 3, slot: "code", graphSel: null, spec: null, jumps: [] };
    const frame = h("div", { class: "frame", style: "grid-template-columns: 1.2fr 1fr" });
    const picker = h("div", { class: "picker", hidden: true });
    const strip = h("div", { class: "jumpstrip" });
    const cap = caption("");
    function fnAtLine() {
      let at = null;
      for (const [l, f] of Object.entries(CODE_DEFS)) if (+l <= state.line) at = f;
      return at;
    }
    function draw(said) {
      frame.innerHTML = "";
      const code = h("div", { class: "frame-slot" + (state.slot === "code" ? " on" : "") }, h("div", { class: "frame-tab" }, "slot 1", h("b", {}, "CODE")), h("div", { class: "frame-body code" }, CODE.map((l, i) => h("div", { class: "frame-line" + (i === state.line ? " cur" : "") }, l))));
      const otherView = state.slot === "spec" ? "SPEC" : "GRAPH";
      const other = h("div", { class: "frame-slot" + (state.slot !== "code" ? " on" : "") }, h("div", { class: "frame-tab" }, "slot 2", h("b", {}, otherView)), h("div", { class: "frame-body", id: "" }));
      frame.append(code, other);
      const body = other.querySelector(".frame-body");
      if (state.slot === "spec" && state.spec) body.append(...["▸ " + state.spec, "  " + SHAPES[state.spec], "  Calls: " + CALLS.filter((c) => c[0] === state.spec).map((c) => c[1]).join(", ")].map((t) => h("div", { class: "frame-line" }, t)));
      else {
        const g = h("div", { class: "goto-graph" });
        body.append(g);
        const near = state.graphSel ? [state.graphSel, ...CALLS.filter((c) => c[0] === state.graphSel || c[1] === state.graphSel).flatMap((c) => [c[0], c[1]])] : [];
        [...new Set(near)].forEach((id) => g.append(h("span", { class: "chip" + (id === state.graphSel ? " sel" : "") }, id)));
        if (!near.length) g.append(h("span", { class: "muted" }, "(nothing selected)"));
      }
      strip.innerHTML = "";
      strip.append(h("span", { class: "muted" }, "Jump list: "), ...(state.jumps.length ? state.jumps.map((j) => h("span", { class: "chip" }, j)) : [h("span", { class: "muted" }, "empty")]));
      cap.innerHTML = said || `The code cursor is in <b>${fnAtLine() || "nothing"}</b>.`;
    }
    const jump = (label) => state.jumps.push(label);
    const withFn = (go) => () => {
      const f = fnAtLine();
      if (!f) return draw("Nothing under the cursor.");
      go(f);
    };
    host.append(controls(demo, [
      { a: "nav.down", run: () => { state.line = Math.min(CODE.length - 1, state.line + 1); state.slot = "code"; draw(); } },
      { a: "nav.up", run: () => { state.line = Math.max(0, state.line - 1); state.slot = "code"; draw(); } },
      { sep: true },
      { a: "goto.graph", run: withFn((f) => { jump(`code:${state.line + 1}`); state.graphSel = f; state.slot = "graph"; draw(`Shown in the graph, selected, with what it connects to. The place you left went on the jump list.`); }) },
      { a: "goto.spec", run: withFn((f) => { jump(`code:${state.line + 1}`); state.spec = f; state.slot = "spec"; draw(`Its spec opens in the other slot.`); }) },
      { a: "goto.callees", run: withFn((f) => {
        const callees = CALLS.filter((c) => c[0] === f).map((c) => c[1]);
        if (!callees.length) return draw(`${f} calls nothing.`);
        if (callees.length === 1) { jump(`code:${state.line + 1}`); state.graphSel = callees[0]; state.slot = "graph"; return draw(`One callee: straight there.`); }
        picker.hidden = false;
        picker.innerHTML = "";
        picker.append(h("div", { class: "picker-title" }, `${action("goto.pick") ? action("goto.pick").label.replace("…", "callee") : "Which callee?"}`), ...callees.map((c) => h("button", { class: "demo-btn", type: "button", onclick: () => { picker.hidden = true; jump(`code:${state.line + 1}`); state.graphSel = c; state.slot = "graph"; draw(`Picked ${c}.`); } }, c)));
        draw(`${f} calls ${callees.length} things: pick one.`);
      }) },
      { sep: true },
      { a: "nav.back", run: () => { const j = state.jumps.pop(); if (!j) return draw("The jump list is empty."); state.slot = "code"; state.line = +j.split(":")[1] - 1; draw(`Back to ${j}.`); } },
    ]), frame, picker, strip, cap);
    draw();
  };

  D.jumps = (host, demo) => {
    const places = ["code: view.rs:12", "graph: view", "spec: view · Calls", "graph: load_index", "tree: store", "queue: view job"];
    let list = [places[0]];
    let at = 0;
    const marks = {};
    let next = 1;
    const strip = h("div", { class: "jumpline" });
    const cap = caption("");
    function draw(said) {
      strip.innerHTML = "";
      list.forEach((p, i) => strip.append(h("div", { class: "jl-place" + (i === at ? " here" : "") }, h("div", { class: "jl-view" }, p.split(":")[0]), h("div", {}, p.split(": ")[1]), Object.entries(marks).filter(([, v]) => v === p).map(([k]) => h("span", { class: "jl-mark" }, "'" + k)))));
      cap.innerHTML = said || `You're at <b>${list[at]}</b>.`;
    }
    host.append(controls(demo, [
      { label: "Make a jump (a go-to)", key: "", run: () => { list = list.slice(0, at + 1); list.push(places[next++ % places.length]); at = list.length - 1; draw("A jump: the place you left stays on the list."); } },
      { a: "nav.back", run: () => { at = Math.max(0, at - 1); draw(); } },
      { a: "nav.forward", run: () => { at = Math.min(list.length - 1, at + 1); draw(); } },
      { sep: true },
      { a: "nav.mark_set", label: "Set mark a", key: (keyOf("nav.mark_set") + " a").trim(), run: () => { marks.a = list[at]; draw(`Mark <b>a</b> is ${list[at]}.`); } },
      { a: "nav.mark_jump", label: "Jump to mark a", key: (keyOf("nav.mark_jump") + " a").trim(), run: () => { if (!marks.a) return draw("No mark a yet."); list = list.slice(0, at + 1); list.push(marks.a); at = list.length - 1; draw("Back at mark a. That was a jump too."); } },
    ]), strip, cap);
    draw();
  };

  D.finding = (host, demo) => {
    const items = ["get_view", "view", "view_tool", "load_index", "ingest", "parse_file", "link_calls", "save_index", "publish"];
    let cur = 1, hints = false, typed = "", query = "", matches = [], mi = 0;
    const labels = "asdfghjkl".split("");
    const list = h("div", { class: "hintlist" });
    const input = h("input", { class: "demo-input", placeholder: "type to search, Enter to keep", hidden: true });
    const cap = caption("");
    function draw(said) {
      list.innerHTML = "";
      items.forEach((it, i) => {
        const hit = matches.includes(i);
        list.append(h("div", { class: "hint-item" + (i === cur ? " cur" : "") + (hit ? " hit" : "") }, hints ? h("span", { class: "hint-label" }, labels[i]) : null, it));
      });
      cap.innerHTML = said || "";
    }
    input.addEventListener("input", () => {
      query = input.value;
      matches = query ? items.map((x, i) => (x.includes(query) ? i : -1)).filter((i) => i >= 0) : [];
      mi = 0;
      if (matches.length) cur = matches[0];
      draw(`${matches.length} match${matches.length === 1 ? "" : "es"}.`);
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === "Escape") { input.hidden = true; demo.focus(); draw(matches.length ? `Kept the match. Step with the next and previous keys.` : ""); }
      e.stopPropagation();
    });
    demo.addEventListener("keydown", (e) => {
      if (!hints || e.target === input) return;
      const i = labels.indexOf(e.key);
      if (i >= 0 && i < items.length) { cur = i; hints = false; draw(`Typed <b>${e.key}</b>: the cursor is on ${items[i]}.`); e.preventDefault(); }
      else if (e.key === "Escape") { hints = false; draw(); }
    });
    host.append(controls(demo, [
      { a: "nav.hint", run: () => { hints = true; draw("Each visible thing has a label. Type one (click here first)."); } },
      { a: "nav.search", run: () => { input.hidden = false; input.value = ""; input.focus(); } },
      { a: "nav.search_next", run: () => { if (!matches.length) return; mi = (mi + 1) % matches.length; cur = matches[mi]; draw(`Match ${mi + 1} of ${matches.length}.`); } },
      { a: "nav.search_prev", run: () => { if (!matches.length) return; mi = (mi + matches.length - 1) % matches.length; cur = matches[mi]; draw(`Match ${mi + 1} of ${matches.length}.`); } },
    ]), input, list, cap);
    list.addEventListener("click", (e) => {
      const lab = e.target.closest(".hint-item");
      if (lab && hints) { const i = [...list.children].indexOf(lab); cur = i; hints = false; draw(`Picked ${items[i]}.`); }
    });
    draw();
  };

  D.menus = (host, demo) => {
    const data = window.DATA.actions;
    const targets = {
      "a graph card": ["nav.enter", "nav.fold_open", "graph.flip", "graph.depth_more", "goto.code", "goto.spec", "goto.callers", "goto.callees"],
      "a spec item": ["spec.comment", "spec.request_change", "spec.next_section", "goto.graph", "goto.code", "spec.pin"],
      "a code line": ["goto.graph", "goto.spec", "goto.callers", "code.next_def", "nav.search", "nav.hint"],
    };
    let target = "a graph card";
    let mode = "bar";
    const bar = h("div", { class: "keybar" });
    const pop = h("div", { class: "keypop" });
    const tabs = h("div", { class: "demo-controls" });
    function draw() {
      tabs.innerHTML = "";
      for (const t of Object.keys(targets)) tabs.append(h("button", { class: "demo-btn" + (t === target ? " primary" : ""), type: "button", onclick: () => { target = t; mode = "bar"; draw(); } }, "Cursor on " + t));
      bar.innerHTML = "";
      for (const id of targets[target]) {
        const a = action(id);
        if (!a) continue;
        bar.append(h("span", { class: "kb-item" }, h("kbd", {}, keyOf(id) || `SPC ${window.DATA.look.view_menu} …`), a.label));
      }
      pop.innerHTML = "";
      if (mode === "g") {
        pop.append(h("div", { class: "keypop-title" }, "After g, you can press:"));
        const after = data.filter((a) => a.keys.some((k) => k.startsWith("g ")));
        pop.append(h("div", { class: "keypop-grid" }, after.map((a) => h("div", { class: "kp" }, h("kbd", {}, a.keys.find((k) => k.startsWith("g ")).slice(2)), a.label))));
      } else if (mode === "leader") {
        pop.append(h("div", { class: "keypop-title" }, "Space, then:"));
        const first = new Map();
        for (const a of data) if (a.leader) { const k = a.leader.split(" ")[1]; if (!first.has(k)) first.set(k, []); first.get(k).push(a); }
        pop.append(h("div", { class: "keypop-grid" }, [...first].sort().map(([k, list]) => h("div", { class: "kp" }, h("kbd", {}, k), list.length === 1 && list[0].leader.split(" ").length === 2 ? list[0].label : `${list.length} actions…`))));
      } else if (mode === "palette") {
        const q = h("input", { class: "demo-input", placeholder: "type to filter every action" });
        const res = h("div", { class: "keypop-grid" });
        const fill = () => { res.innerHTML = ""; data.filter((a) => (a.label + " " + a.id).toLowerCase().includes(q.value.toLowerCase())).slice(0, 12).forEach((a) => res.append(h("div", { class: "kp" }, h("kbd", {}, a.keys[0] || a.leader || "·"), a.label))); };
        q.addEventListener("input", fill);
        q.addEventListener("keydown", (e) => e.stopPropagation());
        pop.append(h("div", { class: "keypop-title" }, `The palette: all ${data.length} actions by name.`), q, res);
        fill();
      }
    }
    host.append(tabs, controls(demo, [
      { label: "Pause on g", key: "g", run: () => { mode = mode === "g" ? "bar" : "g"; draw(); } },
      { label: "Leader menu", key: "Space", run: () => { mode = mode === "leader" ? "bar" : "leader"; draw(); } },
      { a: "palette.open", run: () => { mode = mode === "palette" ? "bar" : "palette"; draw(); } },
    ]), pop, h("div", { class: "keybar-label" }, "The key bar, at the bottom of the cockpit:"), bar);
    draw();
  };

  D.linkfollow = (host, demo) => {
    let on = true, line = 3, timer = null, selected = "view";
    const ms = look().links.follow_debounce_ms;
    const frame = h("div", { class: "frame", style: "grid-template-columns: 1.2fr 1fr" });
    const cap = caption("");
    function fnAt() { let at = null; for (const [l, f] of Object.entries(CODE_DEFS)) if (+l <= line) at = f; return at; }
    function draw() {
      frame.innerHTML = "";
      const near = [...new Set([selected, ...CALLS.filter((c) => c[0] === selected || c[1] === selected).flat()])];
      frame.append(
        h("div", { class: "frame-slot on" }, h("div", { class: "frame-tab" }, "slot 1", h("b", {}, "CODE")), h("div", { class: "frame-body code" }, CODE.map((l, i) => h("div", { class: "frame-line" + (i === line ? " cur" : "") }, l)))),
        h("div", { class: "frame-slot" }, h("div", { class: "frame-tab" }, "slot 2", h("b", {}, "GRAPH")), h("div", { class: "frame-body goto-graph" }, near.map((id) => h("span", { class: "chip" + (id === selected ? " sel" : "") }, id)))));
      cap.innerHTML = on ? `Linked. The graph selects what the code cursor rests on, after ${ms} ms.` : "Not linked. Moving the code cursor leaves the graph alone.";
    }
    const move = (d) => () => {
      line = Math.max(0, Math.min(CODE.length - 1, line + d));
      draw();
      clearTimeout(timer);
      if (on) timer = setTimeout(() => { const f = fnAt(); if (f) { selected = f; draw(); } }, ms);
    };
    host.append(controls(demo, [
      { a: "nav.down", run: move(1) },
      { a: "nav.up", run: move(-1) },
      { sep: true },
      { a: "link.follow", label: "Link on/off", run: () => { on = !on; draw(); } },
    ]), frame, cap);
    draw();
  };

  const word = (s) => (look().status[s] || { word: s }).word;
  const colorOf = (s) => (look().status[s] || { color: "muted" }).color;

  D.indexlight = (host) =>
    Machine(host, {
      width: 640, height: 200, start: "fresh",
      intro: "The light's states, with the cockpit's own words and colours. Click what happens next.",
      states: [
        { id: "fresh", word: word("index_fresh"), color: colorOf("index_fresh"), x: 90, y: 100 },
        { id: "stale", word: word("stale"), color: colorOf("stale"), x: 300, y: 40 },
        { id: "indexing", word: word("indexing"), color: colorOf("indexing"), x: 300, y: 160 },
        { id: "failed", word: word("index_failed"), color: colorOf("index_failed"), x: 540, y: 160 },
      ],
      moves: [
        { from: "fresh", to: "stale", by: { say: "A file is saved" }, say: "The watcher saw a save. The light goes amber, and the changed file's cards are marked." },
        { from: "stale", to: "indexing", by: { a: "index.changed" }, say: "Only the changed files are read again." },
        { from: "fresh", to: "indexing", by: { a: "index.rebuild" }, bend: -1, say: "Everything is read again, from scratch." },
        { from: "indexing", to: "fresh", by: { say: "It finishes" }, bend: 1, say: "Views that showed changed things refetch on their own." },
        { from: "indexing", to: "failed", by: { say: "It fails" }, say: "The light says why. Fix it and re-read." },
        { from: "failed", to: "indexing", by: { a: "index.changed" }, bend: 1, say: "Try again." },
        { from: "stale", to: "indexing", by: { t: "ingest" }, bend: 1, say: "The agent re-reads after it edits code." },
      ],
    });

  D.lifecycle = (host) => {
    const S = (stage) => colorOf("stage_" + stage);
    Machine(host, {
      width: 680, height: 190, start: "declared",
      intro: "Stages, in the cockpit's own words and colours (the details card shows a component's stage). Click what moves it on.",
      states: [
        { id: "declared", word: word("stage_declared"), color: S("declared"), x: 70, y: 60 },
        { id: "arch_approved", word: word("stage_arch_approved"), color: S("arch_approved"), x: 205, y: 140 },
        { id: "plan_approved", word: word("stage_plan_approved"), color: S("plan_approved"), x: 340, y: 60 },
        { id: "skeleton", word: word("stage_skeleton"), color: S("skeleton"), x: 475, y: 140 },
        { id: "implemented", word: word("stage_implemented"), color: S("implemented"), x: 610, y: 60 },
      ],
      moves: [
        { from: "declared", to: "arch_approved", by: { a: "queue.approve" }, say: "You approved the architecture. Only you can." },
        { from: "arch_approved", to: "plan_approved", by: { a: "queue.approve" }, say: "You approved the detailed plan, once every review point was accepted." },
        { from: "plan_approved", to: "skeleton", by: { t: "verify" }, say: "The structure is in the code. Placeholders are tracked." },
        { from: "skeleton", to: "implemented", by: { t: "verify" }, say: "A clean check: right place, right shape, calls made, no placeholders, tools and reviewers pass." },
        { from: "implemented", to: "skeleton", by: { say: "Later code breaks it" }, bend: -1, say: "A check that fails later demotes it again." },
      ],
    });
  };

  D.review = (host) =>
    Machine(host, {
      width: 640, height: 200, start: "open",
      intro: "One review point's life. You and the agent take turns.",
      states: [
        { id: "open", word: "open", color: colorOf("open_comments"), x: 90, y: 100 },
        { id: "asked", word: "agent asks back", color: "info", x: 300, y: 36 },
        { id: "answered", word: "answered", color: "info", x: 300, y: 164 },
        { id: "accepted", word: "accepted", color: "ok", x: 540, y: 100 },
      ],
      moves: [
        { from: "open", to: "answered", by: { t: "answer_comment" }, say: "The agent changed the plan and answered." },
        { from: "open", to: "asked", by: { t: "reply_comment" }, say: "The agent needs more from you." },
        { from: "asked", to: "open", by: { a: "spec.reply" }, bend: -1, say: "You replied. It's the agent's turn again." },
        { from: "answered", to: "accepted", by: { a: "spec.accept" }, say: "Done. Approval waits until every point is here." },
        { from: "answered", to: "open", by: { a: "spec.reopen" }, bend: 1, say: "Not right. You said why; it's open again." },
      ],
    });

  D.queue = (host) =>
    Machine(host, {
      width: 680, height: 200, start: "queued",
      intro: "One job. Words and colours are the cockpit's.",
      states: [
        { id: "queued", word: word("job_queued"), color: colorOf("job_queued"), x: 70, y: 100 },
        { id: "running", word: word("job_running"), color: colorOf("job_running"), x: 230, y: 100 },
        { id: "waiting", word: word("job_awaiting_approval"), color: colorOf("job_awaiting_approval"), x: 420, y: 36 },
        { id: "failed", word: word("job_failed"), color: colorOf("job_failed"), x: 420, y: 164 },
        { id: "done", word: word("job_done"), color: colorOf("job_done"), x: 610, y: 100 },
      ],
      moves: [
        { from: "queued", to: "running", by: { a: "queue.start" }, say: "Its turn, in dependency order. A session opens with a brief." },
        { from: "running", to: "waiting", by: { say: "Design job ends" }, say: "Design work waits for you." },
        { from: "running", to: "done", by: { t: "verify" }, say: "Verified. On to the next job." },
        { from: "running", to: "failed", by: { say: "Out of rounds" }, say: "Each failed round ran again with its findings, until the limit." },
        { from: "waiting", to: "done", by: { a: "queue.approve" }, say: "Approved by you." },
        { from: "failed", to: "queued", by: { a: "queue.retry" }, bend: 1.4, say: "Another round." },
      ],
    });

  D.chat = (host) =>
    Flow(host, [
      { title: "You type", say: "A prompt in the chat view." },
      { title: "Cockpit", say: "Sends it to the server for the current session." },
      { title: "Server", say: "Passes it to the harness (Antigravity, over ACP). Every session NAH opens is given NAH's MCP endpoint." },
      { title: "Agent", say: "Works, using NAH's tools to read the graph, plan and check." },
      { title: "Back to you", say: "Messages, tool calls and file changes stream into the chat. Links open in the code view. Permission requests wait for your answer." },
    ]);

  D.agenttools = (host) => {
    const areas = { Explore: ["ingest", "list_types", "search", "inspect", "view", "navigate"], Plan: ["plan_list", "plan_create", "plan_import", "plan_edit", "plan_validate", "contract"], Build: ["verify", "run_tools", "review"], Review: ["review_comments", "answer_comment", "reply_comment"] };
    const tools = Object.fromEntries(window.DATA.tools.map((t) => [t.name, t]));
    const detail = h("div", { class: "demo-caption" }, "Click a tool.");
    const grid = h("div", { class: "areas" });
    for (const [area, names] of Object.entries(areas)) {
      grid.append(h("div", { class: "area" }, h("div", { class: "area-title" }, area), names.filter((n) => tools[n]).map((n) => h("button", { class: "chip tool", type: "button", onclick: () => { detail.innerHTML = `<b>${n}</b>: ${esc(tools[n].description)}<br><span class="muted">Takes: ${tools[n].params.map((p) => p.name + (p.required ? "" : "?")).join(", ") || "nothing"}</span>`; } }, n))));
    }
    const missing = window.DATA.tools.filter((t) => !Object.values(areas).flat().includes(t.name));
    if (missing.length) grid.append(h("div", { class: "area" }, h("div", { class: "area-title" }, "Other"), missing.map((t) => h("span", { class: "chip tool" }, t.name))));
    grid.append(h("div", { class: "area none" }, h("div", { class: "area-title" }, "Approve"), h("span", { class: "muted" }, "No tool. Only you, in the cockpit.")));
    host.append(grid, detail);
  };

  D.events = (host) => {
    const evs = [
      ["Changed", "Code or plans changed.", ["graph", "spec", "tree"], "ask for the same tree (or item) again, where they are"],
      ["Validated", "A check ran.", ["spec"], "shows the result"],
      ["ViewRequested", "The agent wants to show you something.", ["graph"], "goes there and rings the card it pointed at"],
      ["QueueUpdated", "A job moved.", ["queue"], "updates the job"],
      ["Index", "Reading started, moved on, finished or failed; or files changed.", ["light", "graph", "code"], "light colour; stale marks"],
    ];
    const targets = ["light", "graph", "spec", "tree", "queue", "code"];
    const wrap = h("div", { class: "events" });
    const left = h("div", { class: "ev-list" });
    const right = h("div", { class: "ev-targets" });
    const cap = caption("Click an event to see who listens.");
    function fire(e) {
      right.querySelectorAll(".ev-t").forEach((el) => el.classList.toggle("on", e[2].includes(el.dataset.t)));
      left.querySelectorAll(".ev").forEach((el) => el.classList.toggle("on", el.dataset.e === e[0]));
      cap.innerHTML = `<b>${e[0]}</b>: ${e[1]} Listeners ${e[3]}.`;
    }
    for (const e of evs) left.append(h("button", { class: "ev chip", type: "button", "data-e": e[0], onclick: () => fire(e) }, e[0]));
    for (const t of targets) right.append(h("div", { class: "ev-t", "data-t": t }, t === "light" ? "index light" : t + " view"));
    wrap.append(h("div", { class: "ev-side" }, h("div", { class: "area-title" }, "Server sends"), left), h("div", { class: "ev-wire" }, "→ every open cockpit →"), h("div", { class: "ev-side" }, h("div", { class: "area-title" }, "Cockpit"), right));
    host.append(wrap, cap);
  };

  D.uifeedback = (host) =>
    Flow(host, [
      { title: "Turn it on", say: `The element inspector (${esc(keyOf("inspector.toggle"))}). Parts of the screen outline as you hover.` },
      { title: "Pick a part", say: "Click the part you mean: a button, a panel, a card." },
      { title: "Say what's wrong", say: "A category and a comment, saved by the server with what you clicked." },
      { title: "Track it", say: `The issues window (${esc(keyOf("issues.toggle"))}) lists them; mark each one done.` },
    ]);


  // ── The tree demo: the real graph, from the server ──────────────────
  // It sends the server the same request the cockpit sends (POST
  // /api/graph/tree) and draws what comes back where the server placed
  // it. Nothing is laid out here, just as nothing is laid out in the
  // cockpit.
  const VIEWS = ["contains", "calls", "data_flow"];
  const viewWord = (v) => v.replace("_", " ");
  const shortName = (key) => (key.startsWith("type:") ? key.slice(5) : key.split(/::|\.|:/).pop());
  let treeIds = 0;

  // A card's lines of text, as the server sized the card for them (the
  // cockpit draws exactly these too).
  function cardLines(card, view) {
    const L = window.DATA.views.layout;
    let kind = card.kind;
    if (card.more > 0) kind += ` · ${card.more} ${view === "contains" ? "inside" : "more"}`;
    const lines = [[card.label, L.name_size], [kind, L.small_size]];
    if (card.detail) lines.push([card.detail, L.small_size]);
    return lines;
  }

  function wayWord(view, way) {
    return {
      contains: { down: "what's inside", up: "what holds it" },
      calls: { down: "what it calls", up: "what calls it" },
      data_flow: { down: "where it goes", up: "where it came from" },
    }[view][way];
  }

  function TreeDemo(host, init = {}) {
    const uid = ++treeIds;
    const st = Object.assign({ view: "calls", plan: null, root: null, way: "down", depth: null, opened: [], closed: [], full_code: false, selection: null, pending: null, height: 340, agent: null, columns: false }, init);
    const trail = [];
    let tree = null, cam = null, err = null, loading = false, said = "";
    const wrapEl = h("div", { class: "mg tree-demo" });
    const status = h("div", { class: "tree-status" });
    const svgEl = document.createElementNS(NS, "svg");
    svgEl.setAttribute("class", "mg-svg");
    svgEl.style.height = st.height + "px";
    wrapEl.append(status, svgEl);
    host.append(wrapEl);
    const listeners = [];
    const byKey = (k) => tree && tree.cards.find((c) => c.key === k);

    function request() {
      const r = { view: st.view, source: st.plan ? { plan: st.plan } : "code", way: st.way, opened: st.opened, closed: st.closed, full_code: st.full_code };
      if (st.root) r.root = st.root;
      if (st.depth != null) r.depth = st.depth;
      return r;
    }
    async function load() {
      loading = true;
      drawStatus();
      try {
        const res = await fetch("/api/graph/tree", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(request()) });
        const body = await res.json().catch(() => ({}));
        if (!res.ok) {
          if (res.status === 404 && st.root) {
            const gone = st.root;
            st.root = null;
            await load();
            say(`${esc(shortName(gone))} isn't in this view, so it shows where the view starts.`);
            return;
          }
          throw new Error(body.error || `the server answered ${res.status}`);
        }
        tree = body;
        err = null;
        st.depth = tree.request.depth;
        st.opened = st.opened.filter((o) => !(tree.dropped || []).includes(o.card));
        if (st.pending && byKey(st.pending)) st.selection = st.pending;
        st.pending = null;
        if (st.selection && !byKey(st.selection)) st.selection = null;
        fit();
      } catch (e) {
        tree = null;
        err = `The demo couldn't get a tree from the server (${e.message}). It needs the NAH server this page came from.`;
      }
      loading = false;
      draw();
      listeners.forEach((f) => f(api));
    }
    function fit() {
      const aspect = (svgEl.clientWidth || 680) / st.height;
      const [w, hgt] = tree.size;
      const W = Math.max(w, 200) + 40, H = Math.max(hgt, 100) + 40;
      // The whole tree when it reads at that size; else readable, from the top left.
      const vw = Math.min(Math.max(W, H * aspect), 1500);
      cam = { x: -20, y: -20, w: vw, h: vw / aspect };
      const sel = byKey(st.selection);
      if (sel && (sel.rect.x + sel.rect.w > cam.x + cam.w || sel.rect.y + sel.rect.h > cam.y + cam.h)) {
        cam.x = sel.rect.x - 40;
        cam.y = sel.rect.y + sel.rect.h / 2 - cam.h / 2;
      }
    }
    function say(text) {
      said = text;
      drawStatus();
    }

    // With something selected: it, every card a line joins it to, what it
    // hangs from, and (boxes) what holds it and what's inside it.
    function related() {
      if (!st.selection || !tree) return null;
      const s = st.selection;
      const out = new Set([s]);
      for (const l of tree.lines || []) {
        if (l.from === s) out.add(l.to);
        if (l.to === s) out.add(l.from);
      }
      const card = byKey(s);
      if (card && card.parent) out.add(card.parent);
      for (const c of tree.cards) if (c.parent === s) out.add(c.key);
      if (tree.request.view === "contains") {
        for (let p = card && card.parent; p; p = (byKey(p) || {}).parent) out.add(p);
        const stack = [s];
        while (stack.length) {
          const k = stack.pop();
          for (const c of tree.cards) if (c.parent === k) { out.add(c.key); stack.push(c.key); }
        }
      }
      return out;
    }

    function drawStatus() {
      status.innerHTML = "";
      if (loading) return status.append(h("span", { class: "muted" }, "Asking the server…"));
      if (!tree) return;
      const source = st.plan ? (st.full_code ? `plan ${st.plan} in the full code` : `plan ${st.plan}`) : "code";
      const parts = [`${source} · ${viewWord(st.view)} · ${wayWord(st.view, st.way)}`];
      const crumbs = trail.map((t) => (t.root ? shortName(t.root) : "start")).concat(st.root ? [shortName(st.root)] : ["start"]);
      parts.push(crumbs.join(" › "));
      parts.push(`${tree.request.depth} levels · ${tree.cards.length} cards`);
      status.append(h("span", {}, parts.join(" · ")));
      if (tree.cut) {
        const c = tree.cut;
        const text = c.shown < c.asked ? `showing ${c.shown} of ${c.asked} levels: ${c.hidden} more` : `${tree.roots.length} of ${tree.roots.length + c.starts_left_out} starting points`;
        status.append(h("span", { class: "tree-cut" }, " · " + text));
      }
      if (tree.dropped && tree.dropped.length) status.append(h("span", { class: "tree-cut" }, ` · closed ${tree.dropped.map(shortName).join(", ")} again: it didn't fit`));
      if (said) {
        const note = h("div", { class: "tree-said" });
        note.innerHTML = said;
        status.append(note);
      }
    }

    function draw() {
      drawStatus();
      if (!tree) {
        svgEl.setAttribute("viewBox", `0 0 680 ${st.height}`);
        svgEl.innerHTML = `<text x="20" y="40" class="mg-sub">${esc(err || "Loading the tree…")}</text>`;
        return;
      }
      svgEl.setAttribute("viewBox", `${cam.x} ${cam.y} ${cam.w} ${cam.h}`);
      const view = tree.request.view;
      const L = window.DATA.views.layout;
      const rel = related();
      const parts = [`<defs><marker id="tah${uid}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 z" class="mk"/></marker></defs>`];
      if (st.columns && view !== "contains") {
        const cols = new Map();
        for (const c of tree.cards) {
          const col = cols.get(c.level) || { x: Infinity, r: -Infinity };
          col.x = Math.min(col.x, c.rect.x);
          col.r = Math.max(col.r, c.rect.x + c.rect.w);
          cols.set(c.level, col);
        }
        for (const [level, col] of cols) parts.push(`<rect class="mg-col" x="${col.x - 6}" y="${-10}" width="${col.r - col.x + 12}" height="${tree.size[1] + 20}"/><text class="mg-sub" x="${col.x}" y="-14">level ${level}</text>`);
      }
      for (const l of tree.lines || []) {
        const lit = st.selection && (l.from === st.selection || l.to === st.selection);
        const cls = `mg-line k-${l.relation} m-${l.mark} b-${l.kind}${rel && !lit ? " dim" : ""}${lit ? " hot" : ""}`;
        // Down, the arrow points at the child; up, the child calls (or
        // makes) the card it hangs from.
        const head = st.way === "up" && l.kind === "tree" ? `marker-start="url(#tah${uid})"` : `marker-end="url(#tah${uid})"`;
        parts.push(`<polyline class="${cls}" points="${l.path.map((p) => p.join(",")).join(" ")}" ${head}/>`);
      }
      const cards = tree.cards.slice();
      if (view === "contains") cards.sort((a, b) => a.level - b.level);
      for (const c of cards) {
        const r = c.rect;
        const isBox = view === "contains" && c.open;
        const cls = ["mg-card", "m-" + c.mark, isBox ? "box" : "", c.key === st.selection ? "sel" : "", c.outside_plan ? "outside" : "", rel && !rel.has(c.key) && !isBox ? "dim" : "", st.agent === c.key ? "agent" : ""].filter(Boolean).join(" ");
        let y = L.pad;
        const lines = (isBox ? cardLines(c, view).slice(0, 1) : cardLines(c, view)).map(([t, size], k) => {
          y += size;
          const out = `<text x="${L.pad}" y="${y}" class="${k ? "mg-sub" : "mg-name"}" style="font-size:${size}px">${esc(t)}</text>`;
          y += size * (L.line_height - 1);
          return out;
        });
        parts.push(`<g class="${cls}" data-key="${esc(c.key)}"><rect x="${r.x}" y="${r.y}" width="${r.w}" height="${r.h}" rx="6"/><svg x="${r.x}" y="${r.y}" width="${r.w}" height="${r.h}">${lines.join("")}</svg></g>`);
        if (c.more > 0 && !c.open) {
          const w = 18 + String(c.more).length * 7;
          parts.push(`<g class="mg-more" data-more="${esc(c.key)}"><rect x="${r.x + r.w + 6}" y="${r.y + r.h / 2 - 10}" width="${w}" height="20" rx="10"/><text x="${r.x + r.w + 6 + w / 2}" y="${r.y + r.h / 2 + 4}" text-anchor="middle">+${c.more}</text></g>`);
        }
        if (c.other_plans && c.other_plans.length) parts.push(`<text class="mg-badge" x="${r.x + 6}" y="${r.y - 4}">also ${esc(c.other_plans.join(", "))}</text>`);
        if (c.links_left_out) parts.push(`<text class="mg-sub" x="${r.x + r.w + 6}" y="${r.y + r.h}">⇉ ${c.links_left_out} not drawn</text>`);
      }
      // The pills on the selection's lines.
      for (const l of tree.lines || []) {
        if (!st.selection || (l.from !== st.selection && l.to !== st.selection) || !l.pills || !l.pills.length) continue;
        const mid = l.path[Math.floor(l.path.length / 2)];
        let py = mid[1] - (l.pills.length * 18) / 2;
        for (const p of l.pills) {
          const w = 14 + p.text.length * 6.2;
          parts.push(`<g class="mg-pill p-${p.role}"><rect x="${mid[0] - w / 2}" y="${py}" width="${w}" height="16" rx="8"/><text x="${mid[0]}" y="${py + 12}" text-anchor="middle">${esc(p.text)}</text></g>`);
          py += 18;
        }
      }
      svgEl.innerHTML = parts.join("");
    }

    // The moves, the same as the cockpit's.
    function select(key) {
      st.selection = key;
      draw();
      listeners.forEach((f) => f(api));
    }
    function goIn(key) {
      key = key || st.selection;
      if (!key) return say("Select a card first.");
      if (key === st.root) return say("It's the root already.");
      trail.push({ view: st.view, root: st.root, way: st.way, depth: st.depth, plan: st.plan, full_code: st.full_code, selection: st.selection });
      Object.assign(st, { root: key, opened: [], closed: [], pending: key });
      said = "";
      return load();
    }
    function back() {
      const step = trail.pop();
      if (step) {
        const pending = step.selection;
        Object.assign(st, step, { opened: [], closed: [], pending });
        said = "";
        return load();
      }
      if (!st.root) return say("Already at the view's starting points.");
      const holder = st.view === "contains" && tree && tree.path && tree.path.length ? tree.path[tree.path.length - 1].key : null;
      Object.assign(st, { pending: st.root, root: holder, opened: [], closed: [] });
      said = "";
      return load();
    }
    function depth(delta) {
      st.depth = Math.max(0, (st.depth == null ? 0 : st.depth) + delta);
      st.opened = [];
      st.closed = [];
      said = "";
      return load();
    }
    function flip() {
      st.way = st.way === "down" ? "up" : "down";
      st.opened = [];
      st.closed = [];
      said = "";
      return load();
    }
    function setView(view) {
      const card = byKey(st.selection || st.root);
      const key = st.selection || st.root;
      let root = null;
      if (view === "data_flow") root = key && key.startsWith("type:") ? key : null;
      else root = card && card.component ? card.component : key && !key.includes("type:") ? key : null;
      Object.assign(st, { view, root, opened: [], closed: [] });
      said = "";
      return load();
    }
    function open(key) {
      const card = byKey(key || st.selection);
      if (!card) return say("Select a card first.");
      if (card.open || !card.more) return say(`${esc(card.label)} has nothing more to show.`);
      st.closed = st.closed.filter((c) => c !== card.key);
      st.opened = st.opened.filter((o) => o.card !== card.key).concat([{ card: card.key, levels: 1 }]);
      said = "";
      return load();
    }
    function close(key) {
      const card = byKey(key || st.selection);
      if (!card) return say("Select a card first.");
      if (!card.open) return say(`${esc(card.label)} is closed already.`);
      st.opened = st.opened.filter((o) => o.card !== card.key);
      if (!st.closed.includes(card.key)) st.closed.push(card.key);
      said = "";
      return load();
    }
    function zoom(f) {
      if (!cam) return;
      const cx = cam.x + cam.w / 2, cy = cam.y + cam.h / 2;
      cam.w /= f;
      cam.h /= f;
      cam.x = cx - cam.w / 2;
      cam.y = cy - cam.h / 2;
      draw();
    }
    function refit() {
      if (!tree) return;
      fit();
      draw();
    }

    // Pointer: click selects (or opens a "+N"), double click goes in, drag pans.
    let drag = null, moved = false;
    svgEl.addEventListener("mousedown", (e) => { drag = { x: e.clientX, y: e.clientY }; moved = false; });
    window.addEventListener("mousemove", (e) => {
      if (!drag || !cam) return;
      const k = cam.w / (svgEl.clientWidth || 1);
      const dx = (e.clientX - drag.x) * k, dy = (e.clientY - drag.y) * k;
      if (Math.abs(dx) + Math.abs(dy) > 2) moved = true;
      cam.x -= dx;
      cam.y -= dy;
      drag = { x: e.clientX, y: e.clientY };
      if (moved) svgEl.setAttribute("viewBox", `${cam.x} ${cam.y} ${cam.w} ${cam.h}`);
    });
    window.addEventListener("mouseup", () => (drag = null));
    svgEl.addEventListener("click", (e) => {
      if (moved) return;
      const more = e.target.closest("[data-more]");
      if (more) return open(more.getAttribute("data-more"));
      const card = e.target.closest("[data-key]");
      select(card ? card.getAttribute("data-key") : null);
    });
    svgEl.addEventListener("dblclick", (e) => {
      const card = e.target.closest("[data-key]");
      if (card) goIn(card.getAttribute("data-key"));
    });

    const api = {
      st, el: wrapEl,
      get tree() { return tree; },
      selected: () => byKey(st.selection),
      set(patch) { Object.assign(st, patch); return load(); },
      onChange(f) { listeners.push(f); },
      select, goIn, back, depth, flip, setView, open, close, zoom, refit, say, draw, load,
      trail: () => trail,
    };
    load();
    return api;
  }

  // The keys every tree demo understands, as buttons with the real keys.
  function moves(t, also = []) {
    return [
      { a: "nav.enter", run: () => t.goIn() },
      { a: "nav.leave", run: () => t.back() },
      { a: "graph.depth_less", run: () => t.depth(-1) },
      { a: "graph.depth_more", run: () => t.depth(1) },
      { a: "graph.flip", run: () => t.flip() },
      { a: "graph.next_view", run: () => t.setView(VIEWS[(VIEWS.indexOf(t.st.view) + 1) % VIEWS.length]) },
      ...also,
    ];
  }
  const ROOT = "rust:arch_service::service::ArchitectureService::tree";

  D.views = (host, demo) => {
    const cap = caption("");
    const holder = h("div");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 2 });
    t.onChange(() => (cap.innerHTML = `<b>${viewWord(t.st.view)}</b>: ${esc((window.DATA.views.views[t.st.view] || {}).description || "")}`));
    host.append(controls(demo, [
      ...VIEWS.map((v) => ({ a: "graph.view", label: viewWord(v), key: "", run: () => t.setView(v) })),
      { a: "graph.next_view", run: () => t.setView(VIEWS[(VIEWS.indexOf(t.st.view) + 1) % VIEWS.length]) },
      { a: "graph.refresh", run: () => t.load() },
    ]), holder, cap);
  };

  D.goin = (host, demo) => {
    const holder = h("div");
    const cap = caption("Select a box and go in. The path above the drawing grows; back walks it.");
    const t = TreeDemo(holder, { view: "contains", depth: 1 });
    t.onChange(() => { if (t.tree && t.tree.path && t.tree.path.length) cap.innerHTML = `The root sits inside <b>${t.tree.path.map((c) => esc(c.label)).join(" › ")}</b>. With no way back left, back goes out to the box holding it.`; });
    host.append(controls(demo, [
      { a: "nav.enter", run: () => t.goIn() },
      { a: "graph.go_in", run: () => t.goIn() },
      { a: "nav.leave", run: () => t.back() },
    ]), holder, cap);
  };

  D.depth = (host, demo) => {
    const holder = h("div");
    const cap = caption("");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 1 });
    t.onChange(() => (cap.innerHTML = t.tree ? `${t.tree.request.depth} levels below the root: <b>${t.tree.cards.length}</b> cards. Each card at the edge says how many more it hides.` : ""));
    host.append(controls(demo, [
      { a: "graph.depth_less", run: () => t.depth(-1) },
      { a: "graph.depth_more", run: () => t.depth(1) },
    ]), holder, cap);
  };

  D.flip = (host, demo) => {
    const holder = h("div");
    const cap = caption("");
    const t = TreeDemo(holder, { view: "calls", root: "rust:arch_graph::tree::build", way: "up", depth: 2 });
    t.onChange(() => (cap.innerHTML = `Showing <b>${wayWord(t.st.view, t.st.way)}</b>. Flip shows the other way from the same root.`));
    host.append(controls(demo, [{ a: "graph.flip", run: () => t.flip() }, { a: "graph.next_view", run: () => t.setView(VIEWS[(VIEWS.indexOf(t.st.view) + 1) % VIEWS.length]) }]), holder, cap);
  };

  D.openclose = (host, demo) => {
    const holder = h("div");
    const cap = caption("Select a card with a “+N” count (or click the count) and open it. Only that branch grows.");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 1 });
    host.append(controls(demo, [
      { a: "nav.fold_open", run: () => t.open() },
      { a: "nav.fold_close", run: () => t.close() },
      { a: "nav.fold_toggle", run: () => { const c = t.selected(); return c && c.open ? t.close() : t.open(); } },
      { a: "nav.fold_open_all", run: () => t.depth(1) },
      { a: "nav.fold_close_all", run: () => t.set({ opened: [], closed: [] }) },
    ]), holder, cap);
  };

  D.honest = (host, demo) => {
    const holder = h("div");
    const cap = caption("");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 3 });
    t.onChange(() => {
      if (!t.tree) return;
      const lines = t.tree.lines || [];
      const n = (k) => lines.filter((l) => l.kind === k).length;
      const hidden = t.tree.cards.reduce((a, c) => a + (c.open ? 0 : c.more), 0);
      const left = t.tree.cards.reduce((a, c) => a + (c.links_left_out || 0), 0);
      cap.innerHTML = `${t.tree.cards.length} cards, each drawn once. <b>${n("repeat")}</b> dashed lines point at a card already shown elsewhere; <b>${n("loop")}</b> go back up their own path. <b>${hidden}</b> more cards are counted on the cards they hang from.${left ? ` ${left} lines to cards shown elsewhere went over the line limit and are counted instead.` : ""}${t.tree.cut ? " The answer was cut: the line above says what." : ""}`;
    });
    host.append(controls(demo, [{ a: "graph.depth_less", run: () => t.depth(-1) }, { a: "graph.depth_more", run: () => t.depth(1) }]), holder, cap);
  };

  D.selection = (host, demo) => {
    const holder = h("div");
    const cap = caption("Click a card.");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 2, selection: ROOT });
    t.onChange((api) => {
      const c = api.selected();
      cap.innerHTML = c ? `<b>${esc(c.label)}</b> and what its lines join stay bright; the rest dims. Its lines show their pills: what goes in and what comes back.` : "Nothing selected: nothing dims, no pills.";
    });
    host.append(controls(demo, [{ a: "graph.close_inspector", run: () => t.select(null) }]), holder, cap);
  };

  D.details = (host, demo) => {
    const row = h("div", { class: "inspector-demo" });
    const gHost = h("div");
    const drawer = h("div", { class: "drawer" }, h("div", { class: "muted" }, "Select a card."));
    row.append(gHost, drawer);
    const t = TreeDemo(gHost, { view: "calls", root: ROOT, depth: 1, height: 280 });
    let asked = null;
    t.onChange(async (api) => {
      const c = api.selected();
      if (!c) { drawer.innerHTML = '<div class="muted">Select a card.</div>'; asked = null; return; }
      if (asked === c.key) return;
      asked = c.key;
      drawer.innerHTML = `<div class="drawer-name">${esc(c.label)}</div><div class="muted">Fetching its details…</div>`;
      const q = `/api/graph/card?key=${encodeURIComponent(c.key)}${api.st.plan ? "&plan=" + encodeURIComponent(api.st.plan) : ""}`;
      const res = await fetch(q).catch(() => null);
      if (asked !== c.key) return;
      if (!res || !res.ok) { drawer.innerHTML = `<div class="drawer-name">${esc(c.label)}</div><div class="muted">No details: ${res ? res.status : "the server didn't answer"}.</div>`; return; }
      const d = await res.json();
      const comp = d.component;
      const where = comp.anchor ? `${comp.anchor.file}:${comp.anchor.span.start_line}–${comp.anchor.span.end_line}` : comp.proposed ? `planned in ${comp.proposed.file}` : "not placed yet";
      const sig = comp.signature ? `(${comp.signature.params.map((p) => (p.ty ? `${p.name}: ${p.ty}` : p.name)).join(", ")})${comp.signature.returns ? " -> " + comp.signature.returns : ""}` : "";
      const count = (list, kind) => list.filter((r) => r.kind === kind).length;
      drawer.innerHTML = "";
      drawer.append(
        h("div", { class: "drawer-name" }, comp.name),
        h("div", { class: "mono muted" }, comp.id),
        h("div", { class: "muted" }, `${comp.type} · ${comp.status} · ${comp.stage}`),
        h("div", { class: "mono" }, where),
        sig ? h("div", { class: "mono muted" }, sig) : null,
        h("div", { class: "drawer-links" }, h("span", { class: "chip" }, `calls ${count(d.outgoing, "calls")}`), h("span", { class: "chip" }, `called by ${count(d.incoming, "calls")}`), h("span", { class: "chip" }, h("kbd", {}, keyOf("goto.code")), " code"), h("span", { class: "chip" }, h("kbd", {}, keyOf("goto.spec")), " spec")),
      );
    });
    host.append(controls(demo, [{ a: "graph.close_inspector", run: () => t.select(null) }, { a: "drawer.close", run: () => t.select(null) }]), row);
  };

  D.plansource = async (host, demo) => {
    const holder = h("div");
    const cap = caption("");
    const plans = await fetch("/api/arch/plans").then((r) => (r.ok ? r.json() : [])).catch(() => []);
    const first = plans.length ? plans[0].id : null;
    const t = TreeDemo(holder, { view: "contains", plan: first });
    t.onChange(() => {
      if (!t.tree) return;
      const marks = {};
      for (const c of t.tree.cards) marks[c.mark] = (marks[c.mark] || 0) + 1;
      const outside = t.tree.cards.filter((c) => c.outside_plan).length;
      cap.innerHTML = t.st.plan
        ? `Plan <b>${esc(t.st.plan)}</b>${t.st.full_code ? " in the full code" : ": its own items, and only the code they plug into (faint)"}. Marks: ${Object.entries(marks).map(([m, n]) => `${n} ${m}`).join(", ")}${outside ? `; ${outside} cards are code it plugs into` : ""}.`
        : first ? "The code alone: no plan anywhere." : "The code alone. This repo has no plans yet, so there's nothing to lay over it.";
    });
    host.append(controls(demo, [
      { a: "graph.plan", label: "code", key: "", run: () => t.set({ plan: null, full_code: false, root: null, opened: [], closed: [] }) },
      ...plans.slice(0, 3).map((p) => ({ a: "graph.plan", label: `plan ${p.id}`, key: "", run: () => t.set({ plan: p.id, full_code: false, root: null, opened: [], closed: [] }) })),
      { a: "graph.full_code", run: () => (t.st.plan ? t.set({ full_code: !t.st.full_code, opened: [], closed: [] }) : t.say("Pick a plan first: “full code” is about showing a plan.")) },
      { a: "graph.next_view", run: () => t.setView(VIEWS[(VIEWS.indexOf(t.st.view) + 1) % VIEWS.length]) },
    ]), holder, cap);
  };

  D.zoom = (host, demo) => {
    const holder = h("div");
    const cap = caption(`Zoom and drag only move the camera: the same cards, nothing asked of the server. Fitting never goes below ${look().readable_zoom}× (text stays readable); smaller than ${look().min_text_px} px, text isn't drawn.`);
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 3 });
    host.append(controls(demo, [{ a: "graph.zoom_in", run: () => t.zoom(1.25) }, { a: "graph.zoom_out", run: () => t.zoom(0.8) }, { a: "graph.fit", run: () => t.refit() }]), holder, cap);
  };

  D.lines = (host, demo) => {
    const G = look().graph_style;
    const tok = (name) => `var(--t-${name})`;
    const row = (svg, label) => {
      const sw = h("span", { class: "legend-sw" });
      sw.innerHTML = svg;
      return h("div", { class: "legend-row" }, sw, label);
    };
    const line = (color, l) => `<svg width="40" height="12"><line x1="2" y1="6" x2="38" y2="6" stroke="${color}" stroke-width="${l.width}" ${l.dashed ? 'stroke-dasharray="5 3"' : ""}/></svg>`;
    const box = (color, l) => `<svg width="40" height="16"><rect x="2" y="2" width="36" height="12" rx="3" fill="none" stroke="${color}" stroke-width="${l.width}" ${l.dashed ? 'stroke-dasharray="4 3"' : ""}/></svg>`;
    const words = { code: "code (the plan doesn't touch it)", new: "new: planned, not in the code yet", changed: "changed: the plan changes it", done: "done: the code matches the plan", mismatch: "doesn't match: the code differs from the plan" };
    const legend = h("div", { class: "legend-grid" },
      h("div", { class: "area-title" }, "Lines"),
      row(line(tok(G.kinds.calls), G.lines.code), "calls"),
      row(line(tok(G.kinds.data_flow), G.lines.code), "data flow"),
      row(line(tok(G.back.repeat.color || G.kinds.calls), G.back.repeat), "to a card shown elsewhere"),
      row(line(tok(G.back.loop.color || G.kinds.calls), G.back.loop), "back up its own path (a loop)"),
      h("div", { class: "area-title" }, "Plan marks on lines"),
      ...Object.keys(words).map((m) => row(line(tok(G.lines[m].color || G.kinds.calls), G.lines[m]), words[m])),
      h("div", { class: "area-title" }, "Cards"),
      ...Object.keys(words).map((m) => row(box(tok(G.nodes.outline[m].color), G.nodes.outline[m]), words[m])),
      h("div", { class: "area-title" }, "On the selection's lines"),
      row(`<span class="pill-sw" style="border-color:${tok(G.pills.in)}">in</span>`, "goes in"),
      row(`<span class="pill-sw" style="border-color:${tok(G.pills.out)}">out</span>`, "comes back"),
      row(`<span class="pill-sw" style="border-color:${tok(G.pills.step)}">step</span>`, "the step that turns one data type into the next"));
    host.append(controls(demo, [{ a: "graph.legend", run: () => (legend.hidden = !legend.hidden) }]), legend, caption("Drawn from the same style table the cockpit's legend uses."));
  };

  D.outline = (host) =>
    Flow(host, [
      { title: "Open the outline", say: "The tree view shows the Contains view as a list you can fold." },
      { title: "What's loaded", say: "It holds what the server sent: the view's depth, plus the rows you opened further." },
      { title: "Unfold", say: "Unfolding a row whose parts aren't here yet asks the server for just that branch." },
      { title: "Fold", say: "Folding only hides rows; nothing is asked." },
      { title: "Enter", say: "Selects the row's component in the graph; focus stays in the outline." },
    ]);

  D.agentmoves = (host, demo) => {
    const banner = h("div", { class: "agent-banner", hidden: true });
    const holder = h("div");
    const cap = caption("");
    const t = TreeDemo(holder, { view: "contains", depth: 1 });
    let before = null;
    host.append(controls(demo, [
      { label: "Agent: navigate (calls up to build, select it)", key: "", run: () => { before = { view: t.st.view, root: t.st.root, way: t.st.way, depth: t.st.depth, selection: t.st.selection, plan: t.st.plan }; t.set({ view: "calls", root: "rust:arch_graph::tree::build", way: "up", depth: 2, pending: "rust:arch_graph::tree::build", agent: "rust:arch_graph::tree::build", opened: [], closed: [] }); banner.hidden = false; banner.innerHTML = `The agent moved the view: calls view · from build · what calls it. <span class="muted">Return: ${esc(keyOf("graph.agent_return"))}</span>`; cap.innerHTML = "Your graph went where the agent pointed. A ring marks the card."; } },
      { a: "graph.agent_return", run: () => { if (!before) return; const sel = before.selection; t.set({ ...before, agent: null, pending: sel, opened: [], closed: [] }); before = null; banner.hidden = true; cap.innerHTML = "Back exactly where you were."; } },
    ]), banner, holder, cap);
  };

  // ── Under the hood ──────────────────────────────────────────────────
  D.treebuild = (host) =>
    Flow(host, [
      { title: "Request", say: "View, source (the code or one plan), root, way, depth, and the cards opened or closed by hand. From your keys, or the agent's view and navigate tools." },
      { title: "The graph", say: "The server reads from one shared copy of the code's graph (and the plan laid over it, built once per plan). Nothing is copied per request." },
      { title: "Starting points", say: "The root; or, with no root, the view's natural starts (only the first few are shown; the rest are counted). With a plan, the plan's own top-level items." },
      { title: "Walk", flag: "can cut", say: "Level by level from the roots. A card met again is a line to it; one above on the same path is a loop. If the next level would go over the card budget, the walk stops at a whole level and says so." },
      { title: "Counts", say: "Every card that isn't open counts the cards opening it would add (the “+N”)." },
      { title: "Layout", say: "Positions for every card and a path for every line, on the server, in time that grows with the number of cards." },
      { title: "Answer", say: "A few hundred kilobytes at most, gzipped. The cockpit only draws it." },
    ]);

  D.layout = (host, demo) => {
    const holder = h("div");
    const cap = caption("Each level is a column. Every card is centred on the block its children fill, so cards never overlap. Lines to a card further away run in lanes above the drawing.");
    const t = TreeDemo(holder, { view: "calls", root: ROOT, depth: 3, columns: true });
    host.append(controls(demo, [
      { label: "Show columns", key: "", run: () => { t.st.columns = true; t.draw(); } },
      { label: "Hide columns", key: "", run: () => { t.st.columns = false; t.draw(); } },
      { a: "graph.next_view", run: () => t.setView(VIEWS[(VIEWS.indexOf(t.st.view) + 1) % VIEWS.length]) },
    ]), holder, cap);
  };

  D.perf = (host) => {
    // From spec/perf-baseline.md: the old views against the trees, on tokio
    // (159k lines) and on this repo.
    const rows = [
      ["Calls, no root", "5.4–6.0 MB, 2,991–3,323 nodes", "about 66 KB, 73 cards"],
      ["Data flow, no root", "5.0–7.7 MB, 2,818–4,285 nodes", "about 100 KB, up to 400 cards"],
      ["Structure / Contains, depth 12", "11.6 MB, 7,977 nodes", "the budget: at most 400 cards"],
      ["Slowest answer", "183 ms on the server, then the browser laid it all out", "about 20 ms, already laid out"],
    ];
    host.append(h("table", { class: "ref-table" }, h("thead", {}, h("tr", {}, h("th", {}, "On tokio"), h("th", {}, "Old views"), h("th", {}, "Trees now"))), h("tbody", {}, rows.map((r) => h("tr", {}, r.map((c) => h("td", { class: "small" }, c)))))), caption("The performance tests check every view, both ways, at depths 1, 4 and 12 on this repo and on tokio, and fail if any answer goes over its time or size limit."));
  };

  D.indexing = (host) =>
    Flow(host, [
      { title: "Files", say: "Every source file in the repo. After the first read, only files that changed." },
      { title: "Language packs", say: "Data files (TOML) of ast-grep patterns: what a module, type, function, call, return or data flow looks like in Rust, Python or TypeScript." },
      { title: "Extract", say: "Finds components, containment, calls (with their order), implements, data flow and placeholders in each file." },
      { title: "Resolve", flag: "gaps", say: "Links each call to what it calls by name, only on evidence. <b>Gap:</b> a method called on a value whose type can't be proven stays unresolved, and same-named functions can be mixed up, until the compiler-backed calls (rust-analyzer's SCIP) land." },
      { title: "Graph", say: "Components and relations, kept in memory as one shared copy. Data flows whose type isn't known can't be placed in the data flow view (its cards are types)." },
      { title: "Cache", say: "Saved under .nah/cache. The watcher marks files that change; re-reading reads just those, then links the whole repo again." },
    ]);

  D.actions = (host) =>
    Flow(host, [
      { title: "View state", say: "What the view has right now: a selection, a plan, the agent having moved it, a running job." },
      { title: "Actions", say: "Each view lists what it can do in that state: id, label, group, and whether it takes a choice." },
      { title: "Keymap", say: "Binds keys by action id, from config. Leader paths are fixed, or the next free letter." },
      { title: "Everywhere", say: "The same list feeds the key bar, the leader menu, the palette, toolbars, touch chips and this page's reference." },
    ]);

  D.config = (host) =>
    Flow(host, [
      { title: "Built in", say: "Each crate's config folder: the three views and the layout sizes (trees.toml), the server's limits (limits.toml), types, packs, queue, the cockpit's keys and look." },
      { title: "Project", say: "The repo's .nah folder adds to or overrides them, key by key: trees.toml, limits.toml, types.toml, packs, rules, tools, reviewers, queue." },
      { title: "You", say: "Your own cockpit file: keys, theme, a preset. Only what you set changes." },
      { title: "Live", say: "The cockpit reads the merged result, and asks the server for the layout sizes so its text fits the cards the server sized." },
    ]);

  window.DEMOS = D;
  window.DEMO_KIT = { h, Flow, Machine, Lanes, controls, TreeDemo };
})();
