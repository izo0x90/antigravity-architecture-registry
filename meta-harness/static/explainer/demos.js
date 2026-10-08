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

  // ── The make-believe codebase ───────────────────────────────────────
  const WORLD = (() => {
    const packages = [
      { id: "server", name: "server" },
      { id: "arch-service", name: "arch-service" },
      { id: "arch-index", name: "arch-index" },
    ];
    const modules = [
      { id: "http", name: "http", pkg: "server" },
      { id: "mcp", name: "mcp", pkg: "server" },
      { id: "service", name: "service", pkg: "arch-service" },
      { id: "store", name: "store", pkg: "arch-service" },
      { id: "extract", name: "extract", pkg: "arch-index" },
      { id: "resolve", name: "resolve", pkg: "arch-index" },
    ];
    const f = (id, mod, shape, type = "function") => ({ id, name: id, mod, shape, type });
    const fns = [
      f("serve", "http", "Config → ()"),
      f("get_view", "http", "Query → View"),
      f("post_plan", "http", "Ops → Plan"),
      f("view_tool", "mcp", "Args → Text"),
      f("navigate_tool", "mcp", "Args → Event"),
      f("view", "service", "Spec → View"),
      f("ingest", "service", "Path → Graph"),
      f("plan_edit", "service", "Ops → Plan"),
      f("publish", "service", "Event → ()"),
      f("load_index", "store", "() → Graph"),
      f("save_index", "store", "Graph → ()"),
      f("save_plan", "store", "Plan → ()"),
      f("parse_file", "extract", "Path → RawFile"),
      f("capture", "extract", "Node → Flow"),
      f("link_calls", "resolve", "RawFile → Graph"),
      f("folders", "resolve", "Graph → Graph"),
    ];
    const calls = [
      ["serve", "get_view", 0], ["serve", "post_plan", 1], ["serve", "ingest", 2],
      ["get_view", "view", 0], ["view_tool", "view", 0], ["navigate_tool", "publish", 0],
      ["post_plan", "plan_edit", 0], ["view", "load_index", 0],
      ["ingest", "parse_file", 0], ["ingest", "link_calls", 1], ["ingest", "save_index", 2], ["ingest", "publish", 3],
      ["parse_file", "capture", 0], ["link_calls", "folders", 0], ["folders", "link_calls", 0],
      ["plan_edit", "save_plan", 0], ["plan_edit", "publish", 1],
    ];
    const flows = [
      ["parse_file", "link_calls", "RawFile"], ["link_calls", "folders", "Graph"], ["folders", "save_index", "Graph"],
      ["load_index", "view", "Graph"], ["view", "get_view", "View"], ["view", "view_tool", "View"],
      ["post_plan", "plan_edit", "Ops"], ["plan_edit", "save_plan", "Plan"], ["navigate_tool", "publish", "Event"],
    ];
    const fn = Object.fromEntries(fns.map((x) => [x.id, x]));
    const mod = Object.fromEntries(modules.map((x) => [x.id, x]));
    const pkg = Object.fromEntries(packages.map((x) => [x.id, x]));
    const holder = (id, grain) => (grain === "fn" ? id : grain === "module" ? fn[id].mod : mod[fn[id].mod].pkg);
    const nameOf = (id) => (fn[id] || mod[id] || pkg[id] || { name: id }).name;
    const typeOf = (id) => (fn[id] ? fn[id].type : mod[id] ? "module" : pkg[id] ? "package" : "");
    const partsOf = (group) => fns.filter((x) => x.mod === group || mod[x.mod].pkg === group).map((x) => x.id);
    return { packages, modules, fns, calls, flows, fn, mod, pkg, holder, nameOf, typeOf, partsOf };
  })();

  // ── The graph engine ────────────────────────────────────────────────
  // state: kinds, grain ("fn" | "module" | "package"), wrap (draw units
  // inside their holder at this grain), open (groups shown with their parts),
  // only (function filter), roots + depth + direction, selection, trace,
  // maxCards / showAll, labels ("selection" | "all"), plan (overlay),
  // detail, camera (null: fit), markers.
  function MiniGraph(host, init) {
    const st = Object.assign({ kinds: ["calls"], grain: "fn", wrap: null, open: new Set(), only: null, roots: null, depth: 99, direction: "out", selection: null, trace: null, maxCards: Infinity, showAll: false, labels: "selection", plan: null, detail: false, camera: null, markers: false, height: 300, columns: false, onSelect: null, onChange: null, order: false }, init);
    const wrapEl = h("div", { class: "mg" });
    const svgEl = document.createElementNS(NS, "svg");
    svgEl.setAttribute("class", "mg-svg");
    svgEl.style.height = st.height + "px";
    wrapEl.append(svgEl);
    host.append(wrapEl);
    let scene = null;
    let shown = new Map(); // id -> {x,y,w,h} as drawn now
    let anim = null;
    let view = { x: 0, y: 0, w: 600, h: st.height };

    const kindRel = () => {
      const out = [];
      if (st.kinds.includes("calls")) for (const [a, b, o] of WORLD.calls) out.push({ kind: "calls", from: a, to: b, order: o });
      if (st.kinds.includes("data_flow")) for (const [a, b, c] of WORLD.flows) out.push({ kind: "data_flow", from: a, to: b, carries: c });
      if (st.plan) for (const r of st.plan.relations) out.push(r);
      return out;
    };
    const allFns = () => WORLD.fns.concat(st.plan ? st.plan.components : []);
    const fnById = (id) => WORLD.fn[id] || (st.plan && st.plan.components.find((c) => c.id === id));
    const holderOf = (id, grain) => {
      const f = fnById(id);
      if (grain === "fn") return id;
      if (grain === "module") return f.mod;
      return WORLD.mod[f.mod].pkg;
    };

    function compute() {
      const rels = kindRel();
      // Which functions: from the roots, so many hops; else every one with a relation.
      let fns = allFns().map((x) => x.id);
      if (st.only) fns = fns.filter(st.only);
      if (st.roots) {
        const keep = new Set(st.roots);
        let edge = [...st.roots];
        for (let d = 0; d < st.depth && edge.length; d++) {
          const next = [];
          for (const r of rels) {
            const fwd = st.direction !== "in" && edge.includes(r.from) && !keep.has(r.to);
            const back = st.direction !== "out" && edge.includes(r.to) && !keep.has(r.from);
            if (fwd) { keep.add(r.to); next.push(r.to); }
            if (back) { keep.add(r.from); next.push(r.from); }
          }
          edge = next;
        }
        fns = fns.filter((x) => keep.has(x));
      } else {
        const linked = new Set(rels.flatMap((r) => [r.from, r.to]));
        fns = fns.filter((x) => linked.has(x));
      }
      const fset = new Set(fns);
      // Units: a function's holder at the level, unless that holder is open.
      const unitOf = (id) => {
        const g = holderOf(id, st.grain);
        if (st.grain !== "fn" && st.open.has(g)) return id;
        return g;
      };
      const parentOf = (u) => {
        if (fnById(u) && st.grain !== "fn" && st.open.has(holderOf(u, st.grain))) return holderOf(u, st.grain);
        if (st.wrap) return holderOf(fnById(u) ? u : WORLD.partsOf(u)[0], st.wrap);
        return null;
      };
      let units = [...new Set(fns.map(unitOf))];
      // Lines: one per pair, merged with counts.
      const pairs = new Map();
      for (const r of rels) {
        if (!fset.has(r.from) || !fset.has(r.to)) continue;
        const a = unitOf(r.from), b = unitOf(r.to);
        if (a === b) continue;
        const key = a < b ? a + "|" + b : b + "|" + a;
        let p = pairs.get(key);
        if (!p) pairs.set(key, (p = { id: key, from: a, to: b, kinds: new Set(), count: 0, carries: new Set(), back: false, fwd: false, order: r.order, state: r.state || null }));
        p.kinds.add(r.kind);
        p.count++;
        if (r.carries) p.carries.add(r.carries);
        if (a === p.from) p.fwd = true; else p.back = true;
        if (r.state) p.state = r.state;
      }
      let lines = [...pairs.values()];
      // Crowded: keep the selection's neighbourhood, hold the rest.
      let held = [];
      if (units.length > st.maxCards && !st.showAll) {
        const near = new Set(st.selection ? [st.selection] : units.slice(0, st.maxCards));
        if (st.selection) for (const l of lines) if (l.from === st.selection || l.to === st.selection) { near.add(l.from); near.add(l.to); }
        held = units.filter((u) => !near.has(u));
        units = units.filter((u) => near.has(u));
        lines = lines.filter((l) => near.has(l.from) && near.has(l.to));
      }
      // Containers: open groups and wrapping holders.
      const parent = new Map();
      for (const u of units) {
        const p = parentOf(u);
        if (p && p !== u) parent.set(u, p);
      }
      const containers = [...new Set(parent.values())];
      // Columns by dependency, on top-level boxes.
      const top = (u) => (parent.has(u) ? top(parent.get(u)) : u);
      const tops = [...new Set(units.map(top))];
      const succ = new Map(tops.map((t) => [t, new Set()]));
      for (const l of lines) {
        const a = top(l.from), b = top(l.to);
        if (a !== b) (l.fwd || !l.back ? succ.get(a).add(b) : succ.get(b).add(a));
      }
      const col = new Map();
      const visiting = new Set();
      const depthOf = (t) => {
        if (col.has(t)) return col.get(t);
        if (visiting.has(t)) return 0;
        visiting.add(t);
        let d = 0;
        for (const [p, s] of succ) if (s.has(t)) d = Math.max(d, depthOf(p) + 1);
        visiting.delete(t);
        col.set(t, d);
        return d;
      };
      tops.forEach(depthOf);
      // Sizes: cards fit their text; containers fit their parts.
      const CW = (u) => Math.max(92, Math.min(260, 22 + WORLD.nameOf(u).length * 7.4 + (fnById(u) ? 0 : WORLD.typeOf(u).length * 6.6 + 12)));
      const CH = st.detail ? 54 : 30;
      const size = new Map();
      const kidsOf = (c) => units.filter((u) => parent.get(u) === c).concat(containers.filter((k) => parent.get(k) === c));
      const measure = (u) => {
        if (size.has(u)) return size.get(u);
        const kids = kidsOf(u);
        let s;
        if (kids.length) {
          const ks = kids.map(measure);
          const w = Math.max(CW(u) + 10, ...ks.map((k) => k.w)) + 20;
          const hgt = 28 + ks.reduce((a, k) => a + k.h + 8, 0) + 4;
          s = { w, h: hgt };
        } else s = { w: CW(u), h: CH };
        size.set(u, s);
        return s;
      };
      tops.forEach(measure);
      // Place columns left to right, boxes top to bottom (ordered by the
      // average row of what leads to them, to cut crossings).
      const byCol = new Map();
      for (const t of tops) {
        const c = col.get(t);
        if (!byCol.has(c)) byCol.set(c, []);
        byCol.get(c).push(t);
      }
      const pos = new Map();
      const cols = [...byCol.keys()].sort((a, b) => a - b);
      let x = 20;
      const GAPX = 96, GAPY = 16;
      const rowOf = new Map();
      const colRects = [];
      for (const c of cols) {
        const list = byCol.get(c);
        list.sort((a, b) => bary(a) - bary(b) || WORLD.nameOf(a).localeCompare(WORLD.nameOf(b)));
        function bary(t) {
          const preds = [...succ].filter(([, s]) => s.has(t)).map(([p]) => rowOf.get(p)).filter((r) => r != null);
          return preds.length ? preds.reduce((a, b) => a + b, 0) / preds.length : 0;
        }
        let y = 20;
        let w = 0;
        list.forEach((t, i) => {
          rowOf.set(t, i);
          const s = size.get(t);
          pos.set(t, { x, y, w: s.w, h: s.h });
          y += s.h + GAPY;
          w = Math.max(w, s.w);
        });
        colRects.push({ x, w });
        x += w + GAPX;
      }
      // Parts inside their containers.
      const placeKids = (c) => {
        const p = pos.get(c);
        let y = p.y + 28;
        for (const k of kidsOf(c)) {
          const s = size.get(k);
          pos.set(k, { x: p.x + 10, y, w: p.w - 20, h: s.h });
          y += s.h + 8;
          placeKids(k);
        }
      };
      tops.forEach(placeKids);
      const more = held.length ? { id: "__more", n: held.length } : null;
      if (more) {
        const maxY = Math.max(...[...pos.values()].map((p) => p.y + p.h));
        pos.set("__more", { x: 20, y: maxY + 20, w: 120, h: 30 });
      }
      return { units, lines, parent, containers, pos, held, more, colRects };
    }

    function related() {
      if (!st.selection || !scene) return null;
      const out = new Set([st.selection]);
      for (const l of scene.lines) if (l.from === st.selection || l.to === st.selection) { out.add(l.from); out.add(l.to); }
      let p = scene.parent.get(st.selection);
      while (p) { out.add(p); p = scene.parent.get(p); }
      for (const [k, v] of scene.parent) if (v === st.selection) out.add(k);
      return out;
    }

    function bounds() {
      const ps = [...scene.pos.values()];
      if (!ps.length) return { x: 0, y: 0, w: 400, h: st.height };
      const x0 = Math.min(...ps.map((p) => p.x)) - 16, y0 = Math.min(...ps.map((p) => p.y)) - 16;
      const x1 = Math.max(...ps.map((p) => p.x + p.w)) + 16, y1 = Math.max(...ps.map((p) => p.y + p.h)) + 16;
      return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    }

    function update() {
      const before = new Map(shown);
      const prevScene = scene;
      scene = compute();
      // Where each box starts: where it was, or where its group / parts were.
      const from = new Map();
      for (const [id, p] of scene.pos) {
        if (before.has(id)) { from.set(id, before.get(id)); continue; }
        const f = fnById(id);
        const grp = f && [holderOf(id, "module"), holderOf(id, "package")].find((g) => before.has(g));
        if (grp) { const g = before.get(grp); from.set(id, { x: g.x, y: g.y, w: Math.min(p.w, g.w), h: p.h, fade: true }); continue; }
        const parts = WORLD.partsOf(id).filter((k) => before.has(k));
        if (parts.length) {
          const ps = parts.map((k) => before.get(k));
          const cx = ps.reduce((a, q) => a + q.x, 0) / ps.length, cy = ps.reduce((a, q) => a + q.y, 0) / ps.length;
          from.set(id, { x: cx, y: cy, w: p.w, h: p.h, fade: true });
          continue;
        }
        from.set(id, { ...p, fade: true });
      }
      let target = st.camera ? { ...st.camera } : bounds();
      const cw = svgEl.clientWidth, chh = svgEl.clientHeight;
      if (cw && chh) {
        const ar = cw / chh;
        const cx = target.x + target.w / 2, cy = target.y + target.h / 2;
        if (st.camera) {
          // The camera is the box on screen: same shape as it.
          target = { ...target, w: target.h * ar, x: cx - (target.h * ar) / 2 };
        } else {
          // Fitting never magnifies: a small drawing stays at its own size.
          const w = Math.max(target.w, cw, target.h * ar), hh = Math.max(target.h, chh, w / ar);
          const w2 = Math.max(w, hh * ar);
          target = { x: cx - w2 / 2, y: cy - hh / 2, w: w2, h: hh };
        }
      }
      const fromView = { ...view };
      const secs = prevScene ? (look().look.move_seconds || 0.35) : 0;
      const t0 = performance.now();
      cancelAnimationFrame(anim);
      const step = (now) => {
        const k = secs ? Math.min(1, (now - t0) / (secs * 1000)) : 1;
        const e = k < 0.5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
        shown = new Map();
        for (const [id, p] of scene.pos) {
          const s = from.get(id) || p;
          shown.set(id, { x: s.x + (p.x - s.x) * e, y: s.y + (p.y - s.y) * e, w: s.w + (p.w - s.w) * e, h: s.h + (p.h - s.h) * e, alpha: s.fade ? e : 1 });
        }
        view = { x: fromView.x + (target.x - fromView.x) * e, y: fromView.y + (target.y - fromView.y) * e, w: fromView.w + (target.w - fromView.w) * e, h: fromView.h + (target.h - fromView.h) * e };
        draw();
        if (k < 1) anim = requestAnimationFrame(step);
      };
      anim = requestAnimationFrame(step);
      if (st.onChange) st.onChange(api);
    }

    function curve(a, b) {
      const ac = { x: a.x + a.w / 2, y: a.y + a.h / 2 }, bc = { x: b.x + b.w / 2, y: b.y + b.h / 2 };
      let p1, p2, dx;
      if (b.x >= a.x + a.w - 4) {
        p1 = { x: a.x + a.w, y: ac.y }; p2 = { x: b.x, y: bc.y }; dx = Math.max(30, (p2.x - p1.x) / 2);
        return { d: `M${p1.x},${p1.y} C${p1.x + dx},${p1.y} ${p2.x - dx},${p2.y} ${p2.x},${p2.y}`, mid: { x: (p1.x + p2.x) / 2, y: (p1.y + p2.y) / 2 }, p1, p2 };
      }
      if (a.x >= b.x + b.w - 4) {
        p1 = { x: a.x, y: ac.y }; p2 = { x: b.x + b.w, y: bc.y }; dx = Math.max(30, (p1.x - p2.x) / 2);
        return { d: `M${p1.x},${p1.y} C${p1.x - dx},${p1.y} ${p2.x + dx},${p2.y} ${p2.x},${p2.y}`, mid: { x: (p1.x + p2.x) / 2, y: (p1.y + p2.y) / 2 }, p1, p2 };
      }
      // Same column: bow out to the right.
      const down = bc.y > ac.y;
      p1 = { x: a.x + a.w, y: ac.y + (down ? 4 : -4) }; p2 = { x: b.x + b.w, y: bc.y };
      const bow = Math.max(a.x + a.w, b.x + b.w) + 40;
      return { d: `M${p1.x},${p1.y} C${bow},${p1.y} ${bow},${p2.y} ${p2.x},${p2.y}`, mid: { x: bow - 10, y: (p1.y + p2.y) / 2 }, p1, p2 };
    }

    function draw() {
      const L = look();
      const dim = st.selection ? L.look.dim : 0;
      const rel = related();
      const parts = [];
      const vb = `${view.x} ${view.y} ${view.w} ${view.h}`;
      svgEl.setAttribute("viewBox", vb);
      svgEl.setAttribute("preserveAspectRatio", "xMidYMid meet");
      parts.push(`<defs>${["calls", "data_flow", "contains", "implements", "planned", "missing", "unconfirmed"].map((k) => `<marker id="ah-${k}-${uid}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="10" markerHeight="10" markerUnits="userSpaceOnUse" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 z" class="mk mk-${k}"/></marker>`).join("")}</defs>`);
      if (st.columns && scene.colRects) for (const c of scene.colRects) parts.push(`<rect class="mg-col" x="${c.x - 8}" y="${view.y}" width="${c.w + 16}" height="${view.h}"/>`);
      // Containers first (behind), deepest last.
      const depthIn = (u) => { let d = 0, p = scene.parent.get(u); while (p) { d++; p = scene.parent.get(p); } return d; };
      for (const c of [...scene.containers].sort((a, b) => depthIn(a) - depthIn(b))) {
        const p = shown.get(c);
        if (!p) continue;
        const faded = rel && !rel.has(c) ? 1 - dim : 1;
        parts.push(`<g class="mg-box ${st.selection === c ? "sel" : ""}" data-id="${esc(c)}" style="opacity:${faded * (p.alpha ?? 1)}"><rect x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="7"/><text x="${p.x + 10}" y="${p.y + 18}" class="mg-box-title">${esc(WORLD.nameOf(c))}<tspan class="mg-sub"> ${esc(WORLD.typeOf(c))}</tspan></text></g>`);
      }
      // Lines.
      const labels = [];
      for (const l of scene.lines) {
        const a = shown.get(l.from), b = shown.get(l.to);
        if (!a || !b) continue;
        const kind = l.kinds.has("calls") ? "calls" : l.kinds.has("data_flow") ? "data_flow" : [...l.kinds][0];
        const touches = st.selection && (l.from === st.selection || l.to === st.selection);
        const carriesTrace = st.trace && l.carries.has(st.trace);
        let op = 1;
        if (st.trace) op = carriesTrace ? 1 : (1 - L.look.dim) * (1 - L.look.fade);
        else if (st.selection && !touches) op = 1 - dim;
        const c = curve(a, b);
        const w = 1.6 * Math.min(L.look.line_count_max, 1 + L.look.line_count_gain * Math.log2(l.count));
        const style = l.state || kind;
        const ends = `${l.fwd ? `marker-end="url(#ah-${style}-${uid})"` : ""} ${l.back ? `marker-start="url(#ah-${style}-${uid})"` : ""}`;
        parts.push(`<path class="mg-line k-${kind} s-${l.state || "none"} ${touches ? "hot" : ""}" d="${c.d}" style="stroke-width:${w};opacity:${op * Math.min(a.alpha ?? 1, b.alpha ?? 1)}" ${ends}/>`);
        const wantLabel = st.labels === "all" || touches || carriesTrace;
        if (wantLabel) {
          const ordered = !l.carries.size && l.count === 1 && st.order && l.order != null;
          const text = l.carries.size ? [...l.carries].join(", ") : l.count > 1 ? `${l.count} calls` : ordered ? `${l.order + 1}` : "";
          // A call's position sits just before the card it calls; other labels mid-line.
          const at = ordered ? { x: c.p2.x + (c.p2.x > c.p1.x ? -22 : 22), y: c.p2.y - 11 } : c.mid;
          if (text) labels.push(`<g class="mg-lab ${ordered ? "order" : ""}" style="opacity:${op}"><rect x="${at.x - text.length * 3.4 - 5}" y="${at.y - 9}" width="${text.length * 6.8 + 10}" height="16" rx="${ordered ? 8 : 4}"/><text x="${at.x}" y="${at.y + 3}" text-anchor="middle">${esc(text)}</text></g>`);
        }
      }
      // Cards.
      for (const u of scene.units) {
        if (scene.containers.includes(u)) continue;
        const p = shown.get(u);
        if (!p) continue;
        const faded = rel && !rel.has(u) ? 1 - dim : 1;
        const comp = fnById(u);
        const plan = st.plan && st.plan.marks[u];
        const stage = st.plan && st.plan.stages[u];
        const sub = st.detail && comp ? `<text x="${p.x + 10}" y="${p.y + 36}" class="mg-sub">${esc(comp.shape)}</text>` : !comp ? `<text x="${p.x + p.w - 8}" y="${p.y + 19}" text-anchor="end" class="mg-sub">${esc(WORLD.typeOf(u))}</text>` : "";
        parts.push(`<rect class="mg-back" x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="6" style="opacity:${p.alpha ?? 1}"/>`);
        parts.push(`<g class="mg-card ${st.selection === u ? "sel" : ""} ${plan ? "plan-" + plan : ""} ${st.agent === u ? "agent" : ""}" data-id="${esc(u)}" style="opacity:${faded * (p.alpha ?? 1)}"><rect x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="6"/>${stage ? `<rect class="stripe st-${stage}" x="${p.x}" y="${p.y + 3}" width="4" height="${p.h - 6}" rx="2"/>` : ""}<text x="${p.x + 10}" y="${p.y + 19}">${esc(WORLD.nameOf(u))}</text>${sub}</g>`);
      }
      if (scene.more) {
        const p = shown.get("__more");
        if (p) parts.push(`<g class="mg-more" data-more="1"><rect x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="15"/><text x="${p.x + p.w / 2}" y="${p.y + 19}" text-anchor="middle">+${scene.more.n} more</text></g>`);
      }
      parts.push(labels.join(""));
      // Markers where the selection's lines leave the camera.
      if (st.markers && st.selection && shown.get(st.selection)) {
        const s = shown.get(st.selection);
        const sc = { x: s.x + s.w / 2, y: s.y + s.h / 2 };
        const pad = 6;
        const box = { x0: view.x + pad, y0: view.y + pad, x1: view.x + view.w - pad, y1: view.y + view.h - pad };
        const inside = (q) => q.x >= box.x0 && q.x <= box.x1 && q.y >= box.y0 && q.y <= box.y1;
        const out = [];
        for (const l of scene.lines) {
          if (l.from !== st.selection && l.to !== st.selection) continue;
          const other = l.from === st.selection ? l.to : l.from;
          const o = shown.get(other);
          if (!o) continue;
          const oc = { x: o.x + o.w / 2, y: o.y + o.h / 2 };
          const onScreen = o.x + o.w > box.x0 && o.x < box.x1 && o.y + o.h > box.y0 && o.y < box.y1;
          if (onScreen) continue;
          // Where the line from the selection towards it meets the edge.
          const from = inside(sc) ? sc : { x: Math.min(Math.max(sc.x, box.x0), box.x1), y: Math.min(Math.max(sc.y, box.y0), box.y1) };
          let t = 1;
          const dx = oc.x - from.x, dy = oc.y - from.y;
          if (dx > 0) t = Math.min(t, (box.x1 - from.x) / dx);
          if (dx < 0) t = Math.min(t, (box.x0 - from.x) / dx);
          if (dy > 0) t = Math.min(t, (box.y1 - from.y) / dy);
          if (dy < 0) t = Math.min(t, (box.y0 - from.y) / dy);
          out.push({ other, x: from.x + dx * t, y: from.y + dy * t, ang: Math.atan2(dy, dx) });
        }
        const max = L.levels.max_markers;
        for (const m of out.slice(0, max)) {
          const name = WORLD.nameOf(m.other);
          const w = name.length * 6.6 + 26;
          const mx = Math.min(Math.max(m.x - w / 2, view.x + 4), view.x + view.w - w - 4);
          const my = Math.min(Math.max(m.y - 10, view.y + 4), view.y + view.h - 24);
          const arrow = Math.abs(m.ang) < Math.PI / 4 ? "→" : Math.abs(m.ang) > (3 * Math.PI) / 4 ? "←" : m.ang > 0 ? "↓" : "↑";
          parts.push(`<g class="mg-marker" data-go="${esc(m.other)}"><rect x="${mx}" y="${my}" width="${w}" height="20" rx="10"/><text x="${mx + w / 2}" y="${my + 14}" text-anchor="middle">${arrow} ${esc(name)}</text></g>`);
        }
        if (out.length > max) parts.push(`<g class="mg-marker more"><text x="${view.x + view.w - 8}" y="${view.y + view.h - 8}" text-anchor="end">+${out.length - max} more off screen</text></g>`);
      }
      svgEl.innerHTML = parts.join("");
    }
    const uid = Math.random().toString(36).slice(2, 7);

    // Lay out again once the box has its real size (and when it changes).
    let seen = "";
    new ResizeObserver(() => {
      const size = `${svgEl.clientWidth}x${svgEl.clientHeight}`;
      if (size !== seen && svgEl.clientWidth) { seen = size; update(); }
    }).observe(svgEl);
    svgEl.addEventListener("click", (e) => {
      const go = e.target.closest("[data-go]");
      if (go) { api.panTo(go.getAttribute("data-go")); return; }
      const more = e.target.closest("[data-more]");
      if (more) { st.showAll = true; update(); return; }
      const card = e.target.closest("[data-id]");
      const id = card ? card.getAttribute("data-id") : null;
      if (st.onSelect) st.onSelect(id, api);
      else api.select(id);
    });

    const api = {
      st, world: WORLD,
      set(changes) { Object.assign(st, changes); update(); return api; },
      select(id) { st.selection = id; update(); },
      panTo(id) {
        const p = scene.pos.get(id);
        if (!p || !st.camera) return;
        const hgt = Math.min(st.camera.h, 150), w = st.camera.w * (hgt / st.camera.h);
        st.camera = { w, h: hgt, x: p.x + p.w / 2 - w / 2, y: p.y + p.h / 2 - hgt / 2 };
        st.selection = id;
        update();
      },
      scene: () => scene,
      bounds: () => bounds(),
      redraw: update,
      el: wrapEl,
    };
    update();
    return api;
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
      if (state.slot === "spec" && state.spec) body.append(...["▸ " + state.spec, "  " + WORLD.fn[state.spec].shape, "  Calls: " + WORLD.calls.filter((c) => c[0] === state.spec).map((c) => c[1]).join(", ")].map((t) => h("div", { class: "frame-line" }, t)));
      else {
        const g = h("div", { class: "goto-graph" });
        body.append(g);
        const near = state.graphSel ? [state.graphSel, ...WORLD.calls.filter((c) => c[0] === state.graphSel || c[1] === state.graphSel).flatMap((c) => [c[0], c[1]])] : [];
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
        const callees = WORLD.calls.filter((c) => c[0] === f).map((c) => c[1]);
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
      "a graph card": ["goto.code", "goto.spec", "goto.callers", "goto.callees", "graph.focus", "graph.story", "graph.follow", "nav.fold_open", "graph.coarser"],
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
    let on = true, line = 3, timer = null;
    const ms = look().links.follow_debounce_ms;
    const frame = h("div", { class: "frame", style: "grid-template-columns: 1.2fr 1fr" });
    const graphHost = h("div");
    const cap = caption("");
    let g;
    function fnAt() { let at = null; for (const [l, f] of Object.entries(CODE_DEFS)) if (+l <= line) at = f; return at; }
    function draw() {
      frame.innerHTML = "";
      frame.append(h("div", { class: "frame-slot on" }, h("div", { class: "frame-tab" }, "slot 1", h("b", {}, "CODE")), h("div", { class: "frame-body code" }, CODE.map((l, i) => h("div", { class: "frame-line" + (i === line ? " cur" : "") }, l)))), h("div", { class: "frame-slot" }, h("div", { class: "frame-tab" }, "slot 2", h("b", {}, "GRAPH")), graphHost));
      if (!g) g = MiniGraph(graphHost, { height: 180, roots: ["serve"], depth: 2 });
      else graphHost.append(g.el);
      cap.innerHTML = on ? `Linked. The graph selects what the code cursor rests on, after ${ms} ms.` : "Not linked. Moving the code cursor leaves the graph alone.";
    }
    const move = (d) => () => {
      line = Math.max(0, Math.min(CODE.length - 1, line + d));
      draw();
      clearTimeout(timer);
      if (on) timer = setTimeout(() => { const f = fnAt(); if (f) g.set({ roots: [f], selection: f }); }, ms);
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

  // Graph demos.
  D.views = (host, demo) => {
    const cap = caption("");
    const views = window.DATA.views.views;
    const set = {
      structure: { kinds: ["calls", "data_flow"], grain: "module", wrap: "package", labels: "all", roots: null, depth: 99, direction: "out", order: false },
      calls: { kinds: ["calls"], grain: "fn", wrap: null, labels: "selection", roots: ["serve"], depth: 4, direction: "out", order: true },
      data_flow: { kinds: ["data_flow"], grain: "fn", wrap: null, labels: "all", roots: null, depth: 99, direction: "out", order: false },
      data_path: { kinds: ["data_flow"], grain: "fn", wrap: null, labels: "all", roots: ["link_calls"], depth: 8, direction: "both", order: false },
      combined: { kinds: ["calls", "data_flow"], grain: "fn", wrap: null, labels: "selection", roots: ["serve"], depth: 3, direction: "out", order: false },
    };
    const graphHost = h("div");
    const storyHost = h("div", { hidden: true });
    const g = MiniGraph(graphHost, { height: 300, ...set.calls, selection: "ingest" });
    let story = null;
    let current = "calls";
    const names = Object.keys(views);
    function show(name) {
      current = name;
      const isStory = name === "story" || name === "reached_by";
      graphHost.hidden = isStory;
      storyHost.hidden = !isStory;
      if (isStory) {
        storyHost.innerHTML = "";
        story = Story(storyHost, { root: name === "story" ? "serve" : "publish", backwards: name === "reached_by" });
      } else g.set({ ...set[name], open: new Set(), showAll: false });
      cap.innerHTML = `<b>${name}</b>: ${esc(views[name].description)}`;
      tabs.querySelectorAll("button").forEach((b) => b.classList.toggle("primary", b.dataset.v === name));
    }
    const tabs = h("div", { class: "demo-controls" }, names.map((n) => h("button", { class: "demo-btn", type: "button", "data-v": n, onclick: () => show(n) }, n)));
    host.append(controls(demo, [{ a: "graph.next_view", run: () => show(names[(names.indexOf(current) + 1) % names.length]) }]), tabs, graphHost, storyHost, cap);
    show("calls");
  };

  D.structurelevels = (host, demo) => {
    let at = [];
    const crumbs = h("div", { class: "crumbs" });
    const gauge = h("div", { class: "gauge" });
    const cap = caption("");
    const L = look().levels;
    let fill = 0.2;
    const g = MiniGraph(h("div"), { height: 260 });
    const graphHost = h("div");
    graphHost.append(g.el);
    function level() {
      const where = at[at.length - 1];
      if (!where) return { grain: "package", only: null };
      if (WORLD.pkg[where]) return { grain: "module", only: (id) => WORLD.mod[WORLD.fn[id].mod].pkg === where };
      return { grain: "fn", only: (id) => WORLD.fn[id].mod === where };
    }
    function draw(said) {
      const lv = level();
      g.set({ kinds: ["calls"], grain: lv.grain, only: lv.only, labels: "all", open: new Set(), selection: g.st.selection && (lv.grain === "fn" ? WORLD.fn[g.st.selection] : lv.grain === "module" ? WORLD.mod[g.st.selection] : WORLD.pkg[g.st.selection]) ? g.st.selection : null });
      crumbs.innerHTML = "";
      crumbs.append(h("button", { class: "crumb", type: "button", onclick: () => { at = []; fill = 0.2; draw(); } }, "repo"));
      at.forEach((p, i) => crumbs.append(h("span", { class: "crumb-sep" }, "›"), h("button", { class: "crumb", type: "button", onclick: () => { at = at.slice(0, i + 1); fill = 0.2; draw(); } }, WORLD.nameOf(p))));
      const sel = g.st.selection;
      gauge.innerHTML = sel && !WORLD.fn[sel] ? `Zoom: <b>${sel}</b> fills ${Math.round(fill * 100)}% of the screen. At ${Math.round(L.enter_fill * 100)}% you go in.<span class="bar"><span style="width:${Math.min(100, (fill / L.enter_fill) * 100)}%"></span></span>` : at.length ? `Zoom out until this level fills under ${Math.round(L.leave_fill * 100)}% of the screen to go up.` : "Select a card, then go in or zoom in.";
      cap.innerHTML = said || `You stand ${at.length ? "inside <b>" + WORLD.nameOf(at[at.length - 1]) + "</b>" : "at the top"}. The view shows what's in here.`;
    }
    function enter() {
      const sel = g.st.selection;
      if (!sel || WORLD.fn[sel]) return draw("Select a crate or module to go into.");
      at.push(sel);
      g.st.selection = null;
      fill = 0.2;
      draw(`Went into <b>${sel}</b>. The breadcrumb grew.`);
    }
    function up() {
      if (!at.length) return draw("Already at the top.");
      const left = at.pop();
      fill = 0.2;
      g.st.selection = left;
      draw(`Up a level. <b>${left}</b> stays selected, so you see where you were.`);
    }
    g.st.onSelect = (id, api) => { api.st.selection = id; fill = 0.2; draw(); };
    host.append(controls(demo, [
      { a: "nav.enter", run: enter },
      { a: "nav.leave", run: up, alsoKeys: [keyOf("graph.up")].filter(Boolean) },
      { sep: true },
      { a: "graph.zoom_in", run: () => { fill += 0.12; if (fill >= L.enter_fill && g.st.selection && !WORLD.fn[g.st.selection]) return enter(); draw(); } },
      { a: "graph.zoom_out", run: () => { fill -= 0.12; if (fill < L.leave_fill && at.length) return up(); draw(); } },
    ]), crumbs, graphHost, gauge, cap);
    host.append();
    draw();
  };

  D.relationlevels = (host, demo) => {
    const order = ["fn", "module", "package"];
    const names = { fn: "the parts (functions)", module: "modules", package: "packages" };
    const cap = caption("");
    const g = MiniGraph(host, { height: 300, kinds: ["calls"], grain: "fn", selection: "view" });
    function to(grain) {
      const sel = g.st.selection;
      const keep = sel ? (WORLD.fn[sel] ? WORLD.holder(sel, grain) : grain === "fn" ? null : sel) : null;
      g.set({ grain, open: new Set(), selection: keep && (grain === "fn" || !WORLD.fn[keep]) ? keep : null });
      cap.innerHTML = `Seen by <b>${names[grain]}</b>. ${grain === "fn" ? "Each function is a card." : "Each line stands for every call between two groups. Its label counts them; it's thicker when there are more."}`;
    }
    host.prepend(controls(demo, [
      { a: "graph.coarser", run: () => to(order[Math.min(2, order.indexOf(g.st.grain) + 1)]) },
      { a: "graph.finer", run: () => to(order[Math.max(0, order.indexOf(g.st.grain) - 1)]) },
      { sep: true },
      ...order.map((o) => ({ label: o === "fn" ? "parts" : o + "s", key: "", run: () => to(o) })),
    ]));
    host.append(cap);
    to("fn");
  };

  D.openinplace = (host, demo) => {
    const cap = caption("");
    const g = MiniGraph(host, { height: 320, kinds: ["calls"], grain: "module", selection: "service" });
    const say = () => (cap.innerHTML = g.st.open.size ? `Open: <b>${[...g.st.open].join(", ")}</b>. Its lines now end at the functions they really connect. The rest stay modules.` : "Every module is shut. Select one and open it.");
    const sel = () => { const s = g.st.selection; return s && WORLD.fn[s] ? WORLD.fn[s].mod : s; };
    host.prepend(controls(demo, [
      { a: "nav.fold_open", run: () => { const s = sel(); if (s) g.st.open.add(s); g.redraw(); say(); } },
      { a: "nav.fold_close", run: () => { const s = sel(); if (s) { g.st.open.delete(s); g.st.selection = s; } g.redraw(); say(); } },
      { a: "nav.fold_toggle", run: () => { const s = sel(); if (!s) return; g.st.open.has(s) ? g.st.open.delete(s) : g.st.open.add(s); g.redraw(); say(); } },
      { a: "nav.fold_close_all", run: () => { g.st.open.clear(); g.redraw(); say(); } },
    ]));
    host.append(cap);
    say();
  };

  D.neighbourhood = (host, demo) => {
    const max = 7;
    const cap = caption(`This demo crowds at ${max} cards. The cockpit's limit is ${look().levels.max_cards}.`);
    const g = MiniGraph(host, { height: 300, kinds: ["calls"], grain: "fn", selection: "ingest", maxCards: max });
    host.prepend(controls(demo, [
      { a: "graph.show_all", label: "Show every part", run: () => g.set({ showAll: true }) },
      { label: "Hold them back again", key: "", run: () => g.set({ showAll: false }) },
      { a: "graph.coarser", run: () => g.set({ grain: "module", selection: "service", showAll: false }) },
      { a: "graph.finer", run: () => g.set({ grain: "fn", selection: "ingest", showAll: false }) },
    ]));
    host.append(cap);
  };

  D.selection = (host) => {
    const cap = caption("Click any card. Click empty space to clear.");
    MiniGraph(host, { height: 300, kinds: ["calls", "data_flow"], grain: "fn", selection: "view", onChange: (api) => (cap.innerHTML = api.st.selection ? `<b>${api.st.selection}</b> and everything it connects to stay bright. Only its lines are labelled.` : "Nothing selected: nothing dims.") });
    host.append(cap);
  };

  D.markers = (host, demo) => {
    const cap = caption("The box is the screen. Select a card: its partners off screen get markers. Click one to go there.");
    const g = MiniGraph(host, { height: 260, kinds: ["calls"], grain: "fn", selection: "ingest", markers: true, camera: { x: 260, y: 40, w: 300, h: 150 } });
    const pan = (dx, dy) => () => g.set({ camera: { ...g.st.camera, x: g.st.camera.x + dx, y: g.st.camera.y + dy } });
    g.panTo("ingest");
    host.prepend(controls(demo, [
      { label: "Pan left", key: "h", run: pan(-90, 0) },
      { label: "Pan right", key: "l", run: pan(90, 0) },
      { label: "Pan up", key: "k", run: pan(0, -60) },
      { label: "Pan down", key: "j", run: pan(0, 60) },
      { a: "graph.fit", run: () => { const b = g.bounds(); g.set({ camera: { x: b.x, y: b.y, w: b.w, h: b.h } }); } },
      { label: "Zoom back in", key: "", run: () => g.panTo(g.st.selection || "ingest") },
    ]));
    host.append(cap);
  };

  D.zoom = (host, demo) => {
    const L = look().detail;
    let z = 1.0;
    const stage = h("div", { class: "zoomstage" });
    const cap = caption("");
    function draw() {
      const w = 150 * z, hgt = 34 * z + (z >= L.mid ? 26 * z : 0) + (z >= L.near ? 26 * z : 0);
      const lines = [h("div", { class: "zc-name" }, "view")];
      if (z >= L.mid) lines.push(h("div", { class: "zc-line" }, "function · implemented"), h("div", { class: "zc-line muted" }, "service.rs:42"));
      if (z >= L.near) lines.push(h("div", { class: "zc-line" }, "fn view(spec: Spec) -> View"), h("div", { class: "zc-line ok" }, "planned and in code"), h("div", { class: "zc-line warn" }, "1 open review point"));
      stage.innerHTML = "";
      stage.append(h("div", { class: "zcard", style: `width:${w}px;min-height:${hgt}px` }, lines));
      cap.innerHTML = `Zoom ${z.toFixed(2)}. ${z < L.mid ? `The name only. More from ${L.mid}.` : z < L.near ? `Type, stage and place. More from ${L.near}.` : "Everything."} The text stays the same size; the card grows.`;
    }
    host.append(controls(demo, [
      { a: "graph.zoom_in", run: () => { z = Math.min(2.2, z * 1.2); draw(); } },
      { a: "graph.zoom_out", run: () => { z = Math.max(0.5, z / 1.2); draw(); } },
      { a: "graph.fit", label: "Reset", run: () => { z = 1; draw(); } },
    ]), stage, cap);
    draw();
  };

  D.depth = (host, demo) => {
    const cap = caption("");
    const g = MiniGraph(host, { height: 300, kinds: ["calls"], roots: ["serve"], depth: 1, selection: "serve", labels: "selection" });
    const say = () => (cap.innerHTML = `From <b>serve</b>, ${g.st.depth} hop${g.st.depth > 1 ? "s" : ""} out: ${g.scene().units.length} cards.`);
    host.prepend(controls(demo, [
      { a: "graph.depth_more", run: () => { g.set({ depth: Math.min(5, g.st.depth + 1) }); say(); } },
      { a: "graph.depth_less", run: () => { g.set({ depth: Math.max(1, g.st.depth - 1) }); say(); } },
    ]));
    host.append(cap);
    say();
  };

  D.lines = (host) => {
    const S = look().graph_style;
    const rows = [
      { cls: "k-calls", say: "calls", why: `colour: ${S.kinds.calls}` },
      { cls: "k-data_flow", say: "data flow, saying what it carries", label: "Graph", why: `colour: ${S.kinds.data_flow}` },
      { cls: "k-implements", say: "implements", why: `colour: ${S.kinds.implements}` },
      { cls: "k-contains", say: "contains", why: `colour: ${S.kinds.contains}` },
      { cls: "k-calls s-planned", say: "planned, not built yet", why: "dashed, its kind's colour" },
      { cls: "k-calls s-missing", say: "planned, missing in the code", why: `dashed, ${S.edges.missing.color}` },
      { cls: "k-data_flow s-unconfirmed", say: "a flow the types can't confirm", why: `dashed, ${S.unconfirmed.color}` },
      { cls: "k-calls", say: "12 calls merged into one line", label: "12 calls", width: 1.6 * Math.min(look().look.line_count_max, 1 + look().look.line_count_gain * Math.log2(12)), why: "wider with the count" },
      { cls: "k-calls", say: "calls both ways: one line, two heads", both: true, why: "never two lines for one pair" },
    ];
    const svgEl = document.createElementNS(NS, "svg");
    const H = rows.length * 34 + 10;
    svgEl.setAttribute("viewBox", `0 0 640 ${H}`);
    svgEl.setAttribute("class", "mg-svg");
    svgEl.style.height = H + "px";
    const id = Math.random().toString(36).slice(2, 7);
    svgEl.innerHTML = `<defs>${["calls", "data_flow", "contains", "implements", "planned", "missing", "unconfirmed"].map((k) => `<marker id="lh-${k}-${id}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,1 L9,5 L0,9 z" class="mk mk-${k}"/></marker>`).join("")}</defs>` +
      rows.map((r, i) => {
        const y = 22 + i * 34;
        const kind = r.cls.split(" ")[0].slice(2);
        const st = (r.cls.split(" ")[1] || "").slice(2) || kind;
        return `<path class="mg-line ${r.cls}" d="M20,${y} C80,${y - 10} 140,${y + 10} 200,${y}" style="stroke-width:${r.width || 1.6}" marker-end="url(#lh-${st}-${id})" ${r.both ? `marker-start="url(#lh-${st}-${id})"` : ""}/>` +
          (r.label ? `<g class="mg-lab"><rect x="${110 - r.label.length * 3.4 - 5}" y="${y - 9}" width="${r.label.length * 6.8 + 10}" height="16" rx="4"/><text x="110" y="${y + 3}" text-anchor="middle">${esc(r.label)}</text></g>` : "") +
          `<text x="224" y="${y + 4}" class="lines-say">${esc(r.say)}</text><text x="630" y="${y + 4}" text-anchor="end" class="mg-sub">${esc(r.why)}</text>`;
      }).join("");
    host.append(h("div", { class: "mg" }, svgEl), caption("These are the cockpit's own line styles, read from its config."));
  };

  function Story(host, { root, backwards }) {
    const shut = new Set();
    const list = h("div", { class: "story" });
    const cap = caption("");
    host.append(list, cap);
    function tell() {
      const next = new Map();
      for (const [a, b, o] of WORLD.calls) {
        const [from, to] = backwards ? [b, a] : [a, b];
        if (!next.has(from)) next.set(from, []);
        next.get(from).push([backwards ? 0 : o, to]);
      }
      for (const l of next.values()) l.sort((x, y) => x[0] - y[0] || x[1].localeCompare(y[1]));
      const rows = [], told = new Set(), loops = new Map(), inside = new Map(), path = [];
      const walk = (id, depth, under) => {
        if (path.includes(id)) { if (under) loops.set(under, (loops.get(under) || []).concat(id)); return; }
        if (told.has(id)) return;
        told.add(id);
        rows.push({ id, depth, under });
        if (shut.has(id)) {
          const seen = new Set(), stack = (next.get(id) || []).map((x) => x[1]);
          while (stack.length) { const s = stack.pop(); if (s !== id && !seen.has(s)) { seen.add(s); stack.push(...(next.get(s) || []).map((x) => x[1])); } }
          if (seen.size) inside.set(id, seen.size);
          seen.forEach((s) => told.add(s));
          return;
        }
        path.push(id);
        for (const [, s] of next.get(id) || []) walk(s, depth + 1, id);
        path.pop();
      };
      walk(root, 0, null);
      const also = new Map();
      for (const r of rows) {
        const n = rows.filter((p) => p.id !== r.under && p.id !== r.id && (next.get(p.id) || []).some((x) => x[1] === r.id)).length;
        if (n) also.set(r.id, n);
      }
      return { rows, loops, inside, also };
    }
    let cur = 0;
    function draw() {
      const s = tell();
      cur = Math.min(cur, s.rows.length - 1);
      list.innerHTML = "";
      s.rows.forEach((r, i) => {
        list.append(h("div", { class: "story-row" + (i === cur ? " cur" : ""), style: `--d:${r.depth}`, onclick: () => { cur = i; draw(); } },
          h("span", { class: "story-elbow" }, r.depth ? "└" : "●"),
          h("span", { class: "story-name" }, r.id),
          s.inside.has(r.id) ? h("span", { class: "tag" }, `${s.inside.get(r.id)} inside`) : null,
          s.also.has(r.id) ? h("span", { class: "tag muted" }, `also from ${s.also.get(r.id)} more place${s.also.get(r.id) > 1 ? "s" : ""}`) : null,
          s.loops.has(r.id) ? h("span", { class: "tag warn" }, `↩ loops back to ${s.loops.get(r.id).join(", ")}`) : null,
        ));
      });
      cap.innerHTML = backwards ? `What reaches <b>${root}</b>: its callers, then theirs, outwards.` : `What <b>${root}</b> does, in call order. Deeper calls are further right.`;
    }
    draw();
    return {
      fold(open) { const s = tell(); const id = s.rows[cur].id; open ? shut.delete(id) : shut.add(id); draw(); },
      move(d) { cur = Math.max(0, cur + d); draw(); },
    };
  }

  D.story = (host, demo) => {
    const area = h("div");
    let s;
    let back = false;
    const make = () => { area.innerHTML = ""; s = Story(area, back ? { root: "publish", backwards: true } : { root: "serve", backwards: false }); };
    host.append(controls(demo, [
      { a: "graph.story", label: "Story of serve", run: () => { back = false; make(); } },
      { a: "graph.reached_by", label: "What reaches publish", run: () => { back = true; make(); } },
      { sep: true },
      { a: "nav.down", run: () => s.move(1) },
      { a: "nav.up", run: () => s.move(-1) },
      { a: "nav.fold_close", run: () => s.fold(false) },
      { a: "nav.fold_open", run: () => s.fold(true) },
    ]), area);
    make();
  };

  D.callwalk = (host, demo) => {
    const caller = "ingest";
    const callees = WORLD.calls.filter((c) => c[0] === caller).sort((a, b) => a[2] - b[2]).map((c) => c[1]);
    let i = -1;
    const cap = caption("");
    const g = MiniGraph(host, { height: 260, kinds: ["calls"], roots: [caller], depth: 1, order: true, labels: "all" });
    const say = () => (cap.innerHTML = i < 0 ? `Nothing selected. Step into <b>${caller}</b>'s calls in order.` : `Call ${i + 1} of ${callees.length}: <b>${callees[i]}</b>.`);
    host.prepend(controls(demo, [
      { a: "graph.next_call", run: () => { i = Math.min(callees.length - 1, i + 1); g.set({ selection: callees[i] }); say(); } },
      { a: "graph.prev_call", run: () => { i = Math.max(0, i - 1); g.set({ selection: callees[i] }); say(); } },
    ]));
    host.append(cap);
    say();
  };

  D.followvalue = (host, demo) => {
    const cap = caption("");
    const chip = h("div", { class: "following", hidden: true });
    const g = MiniGraph(host, { height: 300, kinds: ["data_flow"], grain: "fn", labels: "all", selection: "link_calls" });
    host.prepend(chip);
    const choicesFor = (id) => [...new Set(WORLD.flows.filter((f) => f[0] === id || f[1] === id).map((f) => f[2]))];
    const ctr = h("div");
    function draw() {
      ctr.innerHTML = "";
      const sel = g.st.selection;
      const values = sel ? choicesFor(sel) : [];
      ctr.append(controls(demo, [
        ...values.map((v) => ({ a: "graph.follow", label: `Follow ${v}`, key: values.indexOf(v) ? "" : undefined, run: () => { g.set({ roots: [sel], direction: "both", depth: 8, trace: v }); chip.hidden = false; chip.innerHTML = `${esc(word("following"))} <b>${esc(v)}</b>`; cap.innerHTML = `The view switched to data_path from <b>${sel}</b>. Lines not carrying ${v} fade.`; draw(); } })),
        { a: "graph.unfollow", run: () => { g.set({ roots: null, trace: null }); chip.hidden = true; cap.innerHTML = "Back to the whole pipeline."; draw(); } },
      ]));
    }
    g.st.onChange = () => {};
    g.st.onSelect = (id, api) => { api.set({ selection: id }); draw(); };
    host.prepend(ctr);
    host.append(cap);
    cap.innerHTML = "Select a step, then follow one of the values it makes or takes.";
    draw();
  };

  D.planoverlay = (host, demo) => {
    const plan = {
      components: [{ id: "cache_view", name: "cache_view", mod: "service", shape: "View → ()", type: "function" }, { id: "warm_cache", name: "warm_cache", mod: "store", shape: "() → ()", type: "function" }],
      relations: [{ kind: "calls", from: "view", to: "cache_view", state: "planned" }, { kind: "calls", from: "cache_view", to: "warm_cache", state: "missing" }],
      marks: { view: "present", load_index: "present", cache_view: "planned", warm_cache: "missing" },
      stages: { view: "implemented", load_index: "implemented", cache_view: "plan_approved", warm_cache: "skeleton" },
    };
    WORLD.fn.cache_view = plan.components[0];
    WORLD.fn.warm_cache = plan.components[1];
    const cap = caption("");
    const g = MiniGraph(host, { height: 280, kinds: ["calls"], roots: ["get_view", "view_tool"], depth: 3 });
    const legend = h("div", { class: "legend-row" });
    function set(on) {
      g.set({ plan: on ? plan : null });
      legend.innerHTML = "";
      const marks = on ? ["observed", "planned", "missing", "present"] : ["observed"];
      for (const m of marks) legend.append(h("span", { class: "chip", style: `--c:var(--t-${colorOf(m)})` }, h("i", { class: "dot" }), word(m)));
      cap.innerHTML = on ? "A plan adds <b>cache_view</b> (approved, not built) and <b>warm_cache</b> (should be a skeleton, but the code doesn't have it). The stripe on each card is its stage." : "Code only: everything is just code.";
    }
    host.prepend(controls(demo, [
      { a: "graph.plan", label: "Lay the plan over the code", run: () => set(true) },
      { a: "graph.plan", label: "Code only", key: "", run: () => set(false) },
    ]));
    host.append(legend, cap);
    set(true);
  };

  D.inspector = (host) => {
    const row = h("div", { class: "inspector-demo" });
    const gHost = h("div");
    const drawer = h("div", { class: "drawer" });
    row.append(gHost, drawer);
    host.append(row);
    const fill = (id) => {
      drawer.innerHTML = "";
      if (!id) { drawer.append(h("div", { class: "muted" }, "Select a card.")); return; }
      const f = WORLD.fn[id];
      const callers = WORLD.calls.filter((c) => c[1] === id).map((c) => c[0]);
      const callees = WORLD.calls.filter((c) => c[0] === id).map((c) => c[1]);
      drawer.append(
        h("div", { class: "drawer-name" }, id),
        h("div", { class: "mono muted" }, `rust:${f ? f.mod + "::" + id : id}`),
        h("div", { class: "muted" }, `${f ? f.type : WORLD.typeOf(id)} · ${word("observed")}`),
        h("div", { class: "mono" }, f ? `▤ ${f.mod}.rs` : ""),
        h("div", { class: "drawer-links" }, [["goto.code", ""], ["goto.spec", ""], ["goto.callers", callers.length], ["goto.callees", callees.length]].map(([a, n]) => h("span", { class: "chip" }, h("kbd", {}, keyOf(a)), action(a).label.replace(/^Go to (a |its )?/, ""), n !== "" ? ` (${n})` : ""))),
      );
    };
    MiniGraph(gHost, { height: 240, kinds: ["calls"], roots: ["serve"], depth: 2, selection: "ingest", onChange: (api) => fill(api.st.selection) });
  };

  D.lifecycle = (host) => {
    const S = look().graph_style.nodes.stages;
    Machine(host, {
      width: 680, height: 190, start: "declared",
      intro: "Stages, coloured as the stripe on graph cards. Click what moves it on.",
      states: [
        { id: "declared", word: "declared", color: S.declared, x: 70, y: 60 },
        { id: "arch_approved", word: "arch approved", color: S.arch_approved, x: 205, y: 140 },
        { id: "plan_approved", word: "plan approved", color: S.plan_approved, x: 340, y: 60 },
        { id: "skeleton", word: "skeleton", color: S.skeleton, x: 475, y: 140 },
        { id: "implemented", word: "implemented", color: S.implemented, x: 610, y: 60 },
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

  D.agentmoves = (host, demo) => {
    const banner = h("div", { class: "agent-banner", hidden: true });
    const cap = caption("");
    const g = MiniGraph(h("div"), { height: 240, kinds: ["calls"], roots: ["serve"], depth: 2, selection: "get_view" });
    let before = null;
    host.append(controls(demo, [
      { label: "Agent: navigate to publish", key: "", run: () => { before = { roots: g.st.roots, selection: g.st.selection, depth: g.st.depth }; g.set({ roots: ["publish"], direction: "in", depth: 2, selection: "publish", agent: "publish" }); banner.hidden = false; banner.innerHTML = `The agent showed you <b>publish</b> and what reaches it. <span class="muted">Return: ${esc(keyOf("graph.agent_return"))}</span>`; cap.innerHTML = "Your view switched. A ring marks what the agent pointed at."; } },
      { a: "graph.agent_return", run: () => { if (!before) return; g.set({ ...before, direction: "out", agent: null }); before = null; banner.hidden = true; cap.innerHTML = "Back exactly where you were."; } },
    ]), banner, g.el, cap);
  };

  D.events = (host) => {
    const evs = [
      ["Changed", "Code or plans changed.", ["graph", "spec", "tree"], "refetch what they show"],
      ["Validated", "A check ran.", ["spec"], "shows the result"],
      ["ViewRequested", "The agent wants to show you something.", ["graph"], "moves and rings it"],
      ["QueueUpdated", "A job moved.", ["queue", "graph"], "updates the job; stages on cards"],
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

  D.indexing = (host) =>
    Flow(host, [
      { title: "Files", say: "Every source file in the repo. After the first read, only files that changed." },
      { title: "Language packs", say: "Data files (TOML) of ast-grep patterns: what a module, type, function, call, return or data flow looks like in Rust, Python or TypeScript." },
      { title: "Extract", say: "Finds components, containment, calls (with their order), implements, data flow and placeholders in each file." },
      { title: "Resolve", flag: "gaps", say: "Links each call to what it calls, only on evidence. <b>Gap:</b> a method called on a value whose type can't be proven stays unresolved (about 2,800 here). Same-named functions can be mixed up." },
      { title: "Graph", say: "Components and relations. Folders become containers. Data flows whose types don't match are kept, marked unconfirmed (about 780)." },
      { title: "Cache", say: "Saved under .nah/cache. The watcher marks files that change; re-reading reads just those." },
    ]);

  D.viewpipeline = (host) =>
    Flow(host, [
      { title: "Request", say: "View name, roots, depth, plan. From your keys, or the agent's navigate." },
      { title: "Walk", flag: "can hide", say: "The server walks out from the roots, so many hops, through the view's relation kinds. Anything further isn't sent." },
      { title: "Plan", say: "The plan, if any, is laid over: alignment and stages per card and line." },
      { title: "Level", say: "The cockpit regroups the parts at the chosen level, merging lines with counts." },
      { title: "Scene", flag: "can hide", say: "Cards sized, placed in columns, lines routed. A crowded level keeps the selection's neighbourhood and holds the rest in '+N more'." },
      { title: "Draw", say: "Dims what isn't related, labels the selection's lines, adds edge markers, picks card detail by zoom." },
    ]);

  D.layout = (host, demo) => {
    const cap = caption("Bands are columns. A card sits right of what calls it.");
    const g = MiniGraph(host, { height: 300, kinds: ["calls"], grain: "fn", columns: true });
    host.prepend(controls(demo, [
      { label: "Show columns", key: "", run: () => g.set({ columns: true }) },
      { label: "Hide columns", key: "", run: () => g.set({ columns: false }) },
    ]));
    host.append(cap);
  };

  D.actions = (host) =>
    Flow(host, [
      { title: "View state", say: "What the view has right now: a selection, a plan, levels, a running job." },
      { title: "Actions", say: "Each view lists what it can do in that state: id, label, group, and whether it takes a choice." },
      { title: "Keymap", say: "Binds keys by action id, from config. Leader paths are fixed, or the next free letter." },
      { title: "Everywhere", say: "The same list feeds the key bar, the leader menu, the palette, toolbars, touch chips and this page's reference." },
    ]);

  D.config = (host) =>
    Flow(host, [
      { title: "Built in", say: "Each crate's config folder: views, types, packs, queue, the cockpit's keys and look." },
      { title: "Project", say: "The repo's .nah folder adds to or overrides them: views.toml, types.toml, packs, rules, tools, reviewers, queue." },
      { title: "You", say: "Your own cockpit file: keys, theme, a preset. Only what you set changes." },
      { title: "Live", say: "The cockpit reads the merged result. This page reads the built-in look." },
    ], { vertical: false });

  window.DEMOS = D;
  window.DEMO_KIT = { h, MiniGraph, Flow, Machine, Lanes, controls, WORLD };
})();
