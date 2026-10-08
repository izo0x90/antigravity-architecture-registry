// The explainer page: loads the generated reference (data/*.json) and the
// user's notes, then lays out the map, the comparisons, every item with its
// demo, the workflows and the reference.

(function () {
  "use strict";
  const C = window.EXPLAINER;
  const { h } = window.DEMO_KIT;
  const DATA = (window.DATA = {});
  const notes = new Map(); // key -> [note]
  let drawerKey = null;

  // ── Text with references ────────────────────────────────────────────
  // {a:id} action, {t:name} tool, {v:name} view, {i:id} item, {c:path} setting.
  function rich(text) {
    const out = [];
    const re = /\{([atvic]):([^}]+)\}/g;
    let last = 0, m;
    while ((m = re.exec(text))) {
      if (m.index > last) out.push(text.slice(last, m.index));
      const [, kind, ref] = m;
      if (kind === "a") out.push(actionChip(ref, true));
      else if (kind === "t") out.push(toolChip(ref));
      else if (kind === "v") out.push(h("span", { class: "ref view", title: (DATA.views.views[ref] || {}).description || "" }, ref));
      else if (kind === "i") out.push(itemLink(ref));
      else if (kind === "c") out.push(h("b", { class: "ref setting", title: ref }, String(setting(ref))));
      last = re.lastIndex;
    }
    if (last < text.length) out.push(text.slice(last));
    return out;
  }
  function setting(path) {
    const v = path.split(".").reduce((o, k) => (o ? o[k] : undefined), DATA.look);
    return v === undefined ? "?" : v;
  }
  function keysOf(a) {
    const ks = a.keys.slice();
    if (a.leader) ks.push(a.leader);
    return ks;
  }
  function actionChip(id, small) {
    const a = DATA.actionsById[id];
    if (!a) return h("span", { class: "ref bad" }, `unknown action ${id}`);
    const ks = keysOf(a);
    return h("span", { class: "ref action" + (small ? " small" : ""), title: `${a.id} · ${a.group}${a.toolbar ? " · toolbar button" : ""}${a.choice ? " · asks you to pick" : ""}` }, a.label, ...ks.map((k) => h("kbd", {}, k)), !ks.length ? h("span", { class: "muted" }, " palette") : null);
  }
  function toolChip(name) {
    const t = DATA.toolsByName[name];
    if (!t) return h("span", { class: "ref bad" }, `unknown tool ${name}`);
    return h("span", { class: "ref tool", title: t.description }, name);
  }
  function itemLink(id) {
    const it = C.items.find((x) => x.id === id);
    if (!it) return h("span", { class: "ref bad" }, `unknown item ${id}`);
    return h("a", { class: "ref item", href: `#item-${id}`, onclick: (e) => { e.preventDefault(); goTo(id); } }, it.title);
  }
  function trigger(d) {
    if (d.a) return h("div", { class: "trig" + (d.say ? " wide" : "") }, actionChip(d.a), d.say ? h("span", { class: "trig-say" }, rich(d.say)) : null);
    if (d.t) return h("div", { class: "trig wide" }, toolChip(d.t), h("span", { class: "trig-say" }, d.say ? rich(d.say) : DATA.toolsByName[d.t] ? firstSentence(DATA.toolsByName[d.t].description) : ""));
    if (d.ev) return h("div", { class: "trig wide" }, h("span", { class: "ref event" }, d.ev), d.say ? h("span", { class: "trig-say" }, rich(d.say)) : null);
    return h("div", { class: "trig wide" }, h("span", { class: "trig-say plain" }, rich(d.say)));
  }
  const firstSentence = (s) => (s.match(/^[^.]*\./) || [s])[0];

  // ── Notes ───────────────────────────────────────────────────────────
  async function loadNotes() {
    notes.clear();
    try {
      const r = await fetch("/api/explainer/notes");
      if (r.ok) for (const n of await r.json()) { if (!notes.has(n.item)) notes.set(n.item, []); notes.get(n.item).push(n); }
    } catch (_) { /* the server is down: the page still reads */ }
    refreshCounts();
  }
  const openCount = (key) => (notes.get(key) || []).filter((n) => !n.resolved).length;
  function noteButton(key, label) {
    const b = h("button", { class: "note-btn", type: "button", "data-note": key, title: "Questions and notes", onclick: (e) => { e.stopPropagation(); openDrawer(key, label); } }, h("span", { class: "bubble" }), h("span", { class: "note-n" }, ""));
    setCount(b, key);
    return b;
  }
  function setCount(b, key) {
    const n = openCount(key), all = (notes.get(key) || []).length;
    b.querySelector(".note-n").textContent = all ? String(n || "✓") : "";
    b.classList.toggle("has", all > 0);
    b.classList.toggle("open", n > 0);
  }
  function refreshCounts() {
    document.querySelectorAll("[data-note]").forEach((b) => setCount(b, b.getAttribute("data-note")));
    const total = [...notes.values()].flat().filter((n) => !n.resolved).length;
    const t = document.getElementById("notes-total");
    if (t) t.textContent = total ? `${total} open` : "none open";
    document.querySelectorAll("[data-map-note]").forEach((el) => {
      const n = openCount(el.getAttribute("data-map-note"));
      el.querySelector("text").textContent = n ? String(n) : "";
      el.style.display = n ? "" : "none";
    });
    if (drawerKey) renderDrawer();
  }
  let drawerLabel = "";
  function openDrawer(key, label) {
    drawerKey = key;
    drawerLabel = label;
    document.body.classList.add("drawer-open");
    renderDrawer();
    setTimeout(() => { const t = document.querySelector("#drawer textarea"); if (t) t.focus(); }, 50);
  }
  function closeDrawer() {
    drawerKey = null;
    document.body.classList.remove("drawer-open");
  }
  function when(at) {
    const d = new Date(at * 1000);
    return d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }
  function labelFor(key) {
    const it = C.items.find((x) => x.id === key);
    if (it) return it.title;
    const [wf, step] = key.split("#");
    const w = C.workflows.find((x) => x.id === wf);
    if (w) return step ? `Step ${step} of “${w.title}”` : w.title;
    return key;
  }
  function renderDrawer() {
    const el = document.getElementById("drawer");
    el.innerHTML = "";
    const all = drawerKey === "*";
    el.append(h("div", { class: "drawer-head" }, h("div", {}, h("div", { class: "drawer-kicker" }, all ? "Every note" : "Notes on"), h("div", { class: "drawer-title" }, all ? "Questions and notes" : drawerLabel || labelFor(drawerKey))), h("button", { class: "icon-btn", type: "button", title: "Close", onclick: closeDrawer }, "×")));
    const list = h("div", { class: "thread" });
    const shown = all ? [...notes.entries()].flatMap(([k, ns]) => ns.map((n) => [k, n])) : (notes.get(drawerKey) || []).map((n) => [drawerKey, n]);
    shown.sort((a, b) => a[1].resolved - b[1].resolved || b[1].at - a[1].at);
    if (!shown.length) list.append(h("div", { class: "muted empty" }, all ? "No notes yet. Use the speech bubble on any item or workflow step." : "Nothing here yet. Ask a question or leave a note."));
    for (const [k, n] of shown) {
      list.append(h("div", { class: "note" + (n.resolved ? " resolved" : "") },
        h("div", { class: "note-meta" }, h("span", { class: "kind " + n.kind }, n.kind === "question" ? "Question" : "Note"), all ? h("a", { href: "#", class: "note-where", onclick: (e) => { e.preventDefault(); const id = k.split("#")[0]; goTo(id); openDrawer(k, labelFor(k)); } }, labelFor(k)) : null, h("span", { class: "muted" }, when(n.at))),
        h("div", { class: "note-text" }, n.text),
        h("div", { class: "note-actions" },
          h("button", { class: "link-btn", type: "button", onclick: () => change(n, { resolved: !n.resolved }) }, n.resolved ? "Reopen" : "Resolve"),
          h("button", { class: "link-btn danger", type: "button", onclick: () => remove(n) }, "Delete"))));
    }
    el.append(list);
    if (!all) {
      let kind = "question";
      const ta = h("textarea", { placeholder: "What's unclear, wrong, or worth changing?", rows: 4 });
      const kinds = h("div", { class: "seg" }, ["question", "note"].map((k) => h("button", { type: "button", class: k === kind ? "on" : "", onclick: (e) => { kind = k; kinds.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b === e.target)); } }, k === "question" ? "Question" : "Note")));
      const err = h("div", { class: "form-err", hidden: true });
      const save = async () => {
        const text = ta.value.trim();
        if (!text) return;
        const r = await fetch("/api/explainer/notes", { method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin", body: JSON.stringify({ item: drawerKey, kind, text }) }).catch(() => null);
        if (!r || !r.ok) {
          err.hidden = false;
          err.textContent = !r ? "The server didn't answer." : r.status === 403 ? "Notes save only from your own browser session. Open the cockpit once through its launch link (the server prints it), then come back." : `It didn't save (${r.status}).`;
          return;
        }
        ta.value = "";
        err.hidden = true;
        await loadNotes();
      };
      ta.addEventListener("keydown", (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); save(); } if (e.key === "Escape") closeDrawer(); });
      el.append(h("div", { class: "composer" }, kinds, ta, err, h("div", { class: "composer-foot" }, h("span", { class: "muted" }, "Ctrl+Enter saves"), h("button", { class: "demo-btn primary", type: "button", onclick: save }, "Save"))));
    }
  }
  async function change(n, body) {
    const r = await fetch(`/api/explainer/notes/${n.id}`, { method: "PUT", headers: { "Content-Type": "application/json" }, credentials: "same-origin", body: JSON.stringify(body) }).catch(() => null);
    if (r && r.ok) await loadNotes();
  }
  async function remove(n) {
    const r = await fetch(`/api/explainer/notes/${n.id}`, { method: "DELETE", credentials: "same-origin" }).catch(() => null);
    if (r && r.ok) await loadNotes();
  }

  // ── Navigation ──────────────────────────────────────────────────────
  function goTo(id) {
    const el = document.getElementById(`item-${id}`) || document.getElementById(id);
    if (!el) return;
    el.scrollIntoView({ behavior: "smooth", block: "start" });
    el.classList.remove("pulse");
    void el.offsetWidth;
    el.classList.add("pulse");
    history.replaceState(null, "", `#${el.id}`);
  }

  // ── The map ─────────────────────────────────────────────────────────
  function map() {
    const NS = "http://www.w3.org/2000/svg";
    const groups = C.groups;
    const avail = Math.max(900, (document.getElementById("main").clientWidth || 1100) - 90);
    const colW = Math.max(190, Math.floor(avail / groups.length)), rowH = 34, top = 44;
    const W = groups.length * colW + 10;
    const maxRows = Math.max(...groups.map((g) => C.items.filter((i) => i.group === g.id).length));
    const H = top + maxRows * rowH + 16;
    const pos = {};
    groups.forEach((g, gi) => C.items.filter((i) => i.group === g.id).forEach((it, k) => (pos[it.id] = { x: 10 + gi * colW, y: top + k * rowH, w: colW - 22, h: 26 })));
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svg.setAttribute("class", "map-svg");
    svg.style.width = "100%";
    svg.style.maxWidth = W + "px";
    const ties = [];
    for (const it of C.items) for (const [to] of it.ties) if (pos[to]) ties.push([it.id, to]);
    const edge = ([a, b]) => {
      const p = pos[a], q = pos[b];
      const ax = p.x + p.w, ay = p.y + p.h / 2, bx = q.x, by = q.y + q.h / 2;
      if (Math.abs(p.x - q.x) < 2) { const bow = p.x + p.w + 18; return `M${ax},${ay} C${bow},${ay} ${bow},${by} ${q.x + q.w},${by}`; }
      const [sx, sy, ex, ey] = q.x > p.x ? [ax, ay, bx, by] : [p.x, ay, q.x + q.w, by];
      const dx = (ex - sx) / 2;
      return `M${sx},${sy} C${sx + dx},${sy} ${ex - dx},${ey} ${ex},${ey}`;
    };
    svg.innerHTML =
      groups.map((g, gi) => `<text class="map-group" x="${10 + gi * colW}" y="24">${g.title.length * 7.2 > colW - 24 ? g.title.split(" ").slice(0, 2).join(" ") + "…" : g.title}<title>${g.title}</title></text>`).join("") +
      `<g class="map-ties">${ties.map((t) => `<path data-a="${t[0]}" data-b="${t[1]}" d="${edge(t)}"/>`).join("")}</g>` +
      C.items.map((it) => { const p = pos[it.id]; return `<g class="map-item st-${it.status[0]}" data-id="${it.id}"><rect x="${p.x}" y="${p.y}" width="${p.w}" height="${p.h}" rx="13"/><circle cx="${p.x + 13}" cy="${p.y + 13}" r="4"/><text x="${p.x + 24}" y="${p.y + 17}">${it.title.length > 25 ? it.title.slice(0, 24) + "…" : it.title}</text></g>`; }).join("");
    svg.insertAdjacentHTML("beforeend", C.items.map((it) => { const p = pos[it.id]; return `<g class="map-badge" data-map-note="${it.id}" style="display:none"><circle cx="${p.x + p.w - 4}" cy="${p.y + 2}" r="9"/><text x="${p.x + p.w - 4}" y="${p.y + 6}" text-anchor="middle"></text></g>`; }).join(""));
    const wrap = h("div", { class: "map-scroll" }, svg);
    const light = (id) => {
      const near = new Set(id ? [id] : []);
      svg.querySelectorAll(".map-ties path").forEach((p) => {
        const on = id && (p.dataset.a === id || p.dataset.b === id);
        p.classList.toggle("on", !!on);
        if (on) { near.add(p.dataset.a); near.add(p.dataset.b); }
      });
      svg.querySelectorAll(".map-item").forEach((g) => g.classList.toggle("dim", !!id && !near.has(g.dataset.id)));
      svg.classList.toggle("focused", !!id);
    };
    svg.addEventListener("mouseover", (e) => { const g = e.target.closest(".map-item"); light(g ? g.dataset.id : null); });
    svg.addEventListener("mouseleave", () => light(null));
    svg.addEventListener("click", (e) => { const g = e.target.closest(".map-item"); if (g) goTo(g.dataset.id); });
    return h("section", { class: "block", id: "map" },
      h("h2", {}, "The map"),
      h("p", { class: "lede" }, "Everything on this page, by area. Point at one to light up what it ties to, the way the graph does. Click to read it."),
      wrap);
  }

  // ── Comparisons ─────────────────────────────────────────────────────
  function ways() {
    const w = C.ways;
    const table = h("table", { class: "ways" },
      h("thead", {}, h("tr", {}, h("th", {}, "To see more or less…"), w.columns.map((c) => h("th", {}, c)), h("th", {}, "Keys"), h("th", {}, ""))),
      h("tbody", {}, w.rows.map((r) => {
        const it = C.items.find((x) => x.id === r.item);
        const keys = it.user.filter((u) => u.a && DATA.actionsById[u.a]).slice(0, 4).map((u) => { const a = DATA.actionsById[u.a]; const k = keysOf(a)[0]; return k ? h("kbd", { title: a.label }, k) : null; });
        return h("tr", {}, h("td", {}, itemLink(r.item), h("div", { class: "muted small" }, r.note)), r.does.map((d) => h("td", { class: "dotcell" }, d ? h("span", { class: "yes" }, "●") : h("span", { class: "no" }, "·"))), h("td", { class: "keys" }, keys), h("td", {}, h("span", { class: "pill st-" + it.status[0] }, statusWord(it.status[0]))));
      })));
    return h("section", { class: "block", id: "ways" },
      h("h2", {}, "Seven ways to see more or less"),
      h("p", { class: "lede" }, "This is the heart of the confusion. Each row changes something different. Rows that change the same columns are candidates to merge."),
      h("div", { class: "table-scroll" }, table),
      h("div", { class: "block-notes" }, noteButton("ways", "Seven ways to see more or less"), h("span", { class: "muted" }, "Ideas for merging these?")));
  }
  function clashes() {
    return h("section", { class: "block", id: "clashes" },
      h("h2", {}, "Words that mean more than one thing"),
      h("p", { class: "lede" }, "Each word below names several unrelated actions. Renaming or merging them is cheap and removes a lot of confusion."),
      h("div", { class: "clash-grid" }, C.clashes.map((c) => h("div", { class: "clash" },
        h("div", { class: "clash-word" }, `“${c.word}”`),
        h("div", { class: "clash-uses" }, c.uses.map((u) => (u.a ? actionChip(u.a, true) : u.t ? toolChip(u.t) : h("span", { class: "ref setting" }, u.say)))),
        h("p", {}, c.say),
        noteButton(`clash:${c.word}`, `The word “${c.word}”`)))));
  }

  // ── Items ───────────────────────────────────────────────────────────
  const STATUS = {
    works: "Works",
    clumsy: "Works, but clumsy",
    questionable: "Worth questioning",
    gaps: "Known gaps",
    unproven: "Not proven live",
  };
  const statusWord = (s) => STATUS[s] || s;
  function item(it) {
    const demoEl = h("div", { class: "demo", tabindex: "0", "aria-label": `Demo: ${it.title}. Click it, then use the keys shown.` });
    const card = h("article", { class: "item", id: `item-${it.id}` },
      h("header", { class: "item-head" },
        h("div", {}, h("h3", {}, it.title), h("p", { class: "intent" }, rich(it.intent))),
        h("div", { class: "item-tools" }, h("span", { class: "pill st-" + it.status[0], title: it.status[1] }, statusWord(it.status[0])), noteButton(it.id, it.title))),
      h("div", { class: "item-body" },
        h("div", { class: "item-demo" }, demoEl),
        h("div", { class: "facts" },
          fact("What happens", h("ol", { class: "steps" }, it.behaviour.map((b) => h("li", {}, rich(b))))),
          it.user.length ? fact("You", h("div", { class: "trigs" }, it.user.map(trigger))) : null,
          it.agent.length ? fact("The agent", h("div", { class: "trigs" }, it.agent.map(trigger))) : fact("The agent", h("div", { class: "muted small" }, "Nothing. This is yours alone.")),
          fact("Ties to", h("ul", { class: "ties" }, it.ties.map(([to, how]) => h("li", { class: "tie" }, itemLink(to), h("span", { class: "muted" }, ": " + how))))),
          fact("Honestly", h("div", { class: "small" }, it.status[1])),
          fact("Built in", h("div", { class: "files" }, it.files.map((f) => h("code", {}, f)))))));
    const run = window.DEMOS[it.demo];
    if (run) {
      try { run(demoEl, demoEl); } catch (e) { demoEl.append(h("div", { class: "form-err" }, `This demo failed: ${e.message}`)); console.error(it.id, e); }
    }
    return card;
  }
  function fact(title, body) {
    return h("section", { class: "fact" }, h("h4", {}, title), body);
  }

  function workflows() {
    return h("section", { class: "block", id: "workflows" },
      h("h2", {}, "Workflows"),
      h("p", { class: "lede" }, "Common jobs, step by step: who does what, and what triggers each step. Step through with the buttons or the arrow keys."),
      C.workflows.map((w) => {
        const host = h("div", { class: "wf-demo" });
        const el = h("article", { class: "item wf", id: w.id }, h("header", { class: "item-head" }, h("div", {}, h("h3", {}, w.title), h("p", { class: "intent" }, w.say)), h("div", { class: "item-tools" }, noteButton(w.id, w.title))), host);
        window.DEMO_KIT.Lanes(host, w, { noteKey: true });
        return el;
      }));
  }

  // ── Reference ───────────────────────────────────────────────────────
  function reference() {
    const usedIn = new Map();
    const note = (id, where) => { if (!usedIn.has(id)) usedIn.set(id, new Set()); usedIn.get(id).add(where); };
    for (const it of C.items) {
      for (const u of it.user) if (u.a) note(u.a, it.id);
      for (const u of it.agent) if (u.t) note("tool:" + u.t, it.id);
      for (const b of it.behaviour.concat(it.intent)) for (const m of b.matchAll(/\{([at]):([^}]+)\}/g)) note(m[1] === "a" ? m[2] : "tool:" + m[2], it.id);
    }
    for (const w of C.workflows) for (const s of w.steps) for (const d of s.do || []) { if (d.a) note(d.a, s.item); if (d.t) note("tool:" + d.t, s.item); }
    const filter = h("input", { class: "demo-input", placeholder: "Filter actions by name, id or key" });
    const rows = DATA.actions.map((a) => {
      const where = [...(usedIn.get(a.id) || [])];
      return h("tr", { "data-text": `${a.id} ${a.label} ${keysOf(a).join(" ")}`.toLowerCase(), class: where.length ? "" : "unexplained" },
        h("td", {}, a.label, h("div", { class: "mono muted small" }, a.id)),
        h("td", { class: "keys" }, keysOf(a).map((k) => h("kbd", {}, k)), keysOf(a).length ? null : h("span", { class: "muted small" }, "palette only")),
        h("td", { class: "small" }, a.views.join(", ")),
        h("td", { class: "small" }, a.group, a.choice ? h("div", { class: "muted" }, "asks you to pick") : null, a.toolbar ? h("div", { class: "muted" }, "toolbar button") : null),
        h("td", { class: "small" }, where.length ? where.map((w) => h("div", {}, itemLink(w))) : h("span", { class: "muted" }, "not explained above")));
    });
    filter.addEventListener("input", () => { const q = filter.value.toLowerCase(); rows.forEach((r) => (r.hidden = q && !r.dataset.text.includes(q))); });
    const unexplained = DATA.actions.filter((a) => !usedIn.has(a.id)).length;
    const toolRows = DATA.tools.map((t) => h("tr", {}, h("td", { class: "mono" }, t.name), h("td", { class: "small" }, t.description), h("td", { class: "small" }, t.params.map((p) => h("div", {}, h("span", { class: "mono" }, p.name), p.required ? "" : h("span", { class: "muted" }, " (optional)"))), t.params.length ? null : h("span", { class: "muted" }, "nothing")), h("td", { class: "small" }, [...(usedIn.get("tool:" + t.name) || [])].map((w) => h("div", {}, itemLink(w))))));
    const viewRows = Object.entries(DATA.views.views).map(([n, v]) => h("tr", {}, h("td", { class: "mono" }, n), h("td", { class: "small" }, v.description), h("td", { class: "small" }, v.kinds.join(", "), v.overlays && v.overlays.length ? h("div", { class: "muted" }, "over: " + v.overlays.join(", ")) : null, v.links ? h("div", { class: "muted" }, "counted: " + v.links.join(", ")) : null), h("td", { class: "small" }, `${v.layout}, ${v.direction}, ${v.depth} hops`)));
    const levels = DATA.views.grains.map((g) => h("span", { class: "chip" }, g.name, h("span", { class: "muted" }, ` (${g.types.join(", ")}, ${g.pick})`)));
    return h("section", { class: "block", id: "reference" },
      h("h2", {}, "Reference"),
      h("p", { class: "lede" }, "Generated from the cockpit and the server by their tests, so it can't drift. Rows marked in amber aren't explained by any item above."),
      h("h3", { class: "ref-h" }, `Cockpit actions (${DATA.actions.length}, ${unexplained} not explained above)`),
      filter,
      h("div", { class: "table-scroll" }, h("table", { class: "ref-table" }, h("thead", {}, h("tr", {}, h("th", {}, "Action"), h("th", {}, "Keys"), h("th", {}, "Where"), h("th", {}, "Kind"), h("th", {}, "Explained in"))), h("tbody", {}, rows))),
      h("h3", { class: "ref-h" }, `Agent tools (${DATA.tools.length})`),
      h("div", { class: "table-scroll" }, h("table", { class: "ref-table" }, h("thead", {}, h("tr", {}, h("th", {}, "Tool"), h("th", {}, "What it does"), h("th", {}, "Takes"), h("th", {}, "Explained in"))), h("tbody", {}, toolRows))),
      h("h3", { class: "ref-h" }, `Views (${Object.keys(DATA.views.views).length}) and levels`),
      h("div", { class: "table-scroll" }, h("table", { class: "ref-table" }, h("thead", {}, h("tr", {}, h("th", {}, "View"), h("th", {}, "What it shows"), h("th", {}, "Relations"), h("th", {}, "Shape"))), h("tbody", {}, viewRows))),
      h("p", { class: "small" }, "Levels relation views can be seen at, finest first: ", levels));
  }

  // ── Page ────────────────────────────────────────────────────────────
  function theme(look) {
    const root = document.documentElement.style;
    for (const [k, v] of Object.entries(look.theme)) root.setProperty(`--t-${k}`, v);
    for (const [k, v] of Object.entries(look.graph_style.kinds)) root.setProperty(`--k-${k}`, `var(--t-${v})`);
    root.setProperty("--k-missing", `var(--t-${look.graph_style.edges.missing.color})`);
    root.setProperty("--k-unconfirmed", `var(--t-${look.graph_style.unconfirmed.color})`);
    for (const [k, v] of Object.entries(look.graph_style.nodes.stages)) root.setProperty(`--stage-${k}`, `var(--t-${v})`);
    for (const [k, v] of Object.entries(look.graph_style.nodes.outline)) root.setProperty(`--plan-${k}`, `var(--t-${v.color})`);
    root.setProperty("--dim", look.look.dim);
  }
  function nav() {
    const list = h("nav", { class: "rail" },
      h("a", { href: "#map", class: "rail-top" }, "The map"),
      h("a", { href: "#ways", class: "rail-top" }, "Ways to see more"),
      h("a", { href: "#clashes", class: "rail-top" }, "Clashing words"),
      C.groups.map((g) => h("div", { class: "rail-group" }, h("a", { href: `#group-${g.id}`, class: "rail-top" }, g.title), C.items.filter((i) => i.group === g.id).map((i) => h("a", { href: `#item-${i.id}`, class: "rail-item" }, h("i", { class: "dot st-" + i.status[0] }), i.title)))),
      h("a", { href: "#workflows", class: "rail-top" }, "Workflows"),
      h("a", { href: "#reference", class: "rail-top" }, "Reference"));
    list.addEventListener("click", (e) => { const a = e.target.closest("a"); if (!a) return; e.preventDefault(); goTo(a.getAttribute("href").slice(1).replace(/^item-/, "")); });
    return list;
  }
  function search() {
    const input = h("input", { class: "search", type: "search", placeholder: "Find anything on this page" });
    input.addEventListener("input", () => {
      const q = input.value.trim().toLowerCase();
      document.querySelectorAll(".item:not(.wf)").forEach((el) => { el.hidden = q && !el.textContent.toLowerCase().includes(q); });
      document.querySelectorAll(".group-block").forEach((g) => { g.hidden = q && ![...g.querySelectorAll(".item")].some((i) => !i.hidden); });
    });
    return input;
  }

  async function boot() {
    const get = (f) => fetch(`data/${f}`).then((r) => { if (!r.ok) throw new Error(`${f}: ${r.status}`); return r.json(); });
    const [actions, tools, views, look] = await Promise.all([get("actions.json"), get("agent-tools.json"), get("views.json"), get("look.json")]);
    Object.assign(DATA, { actions, tools, views, look, actionsById: Object.fromEntries(actions.map((a) => [a.id, a])), toolsByName: Object.fromEntries(tools.map((t) => [t.name, t])) });
    theme(look);
    window.EXPLAIN = { trigger, itemLink, noteButton, rich };
    const main = document.getElementById("main");
    main.innerHTML = "";
    main.append(
      h("section", { class: "hero" },
        h("h1", {}, "How the cockpit works"),
        h("p", { class: "lede" }, "Every concept, action and moving part of the cockpit and its graph, as it is today. Each item has a live demo you can drive with the real keys, what triggers it for you and for the agent, how it ties to the rest, and an honest status."),
        h("div", { class: "legend" }, Object.entries(STATUS).map(([k, v]) => h("span", { class: "pill st-" + k }, v)), h("span", { class: "muted small" }, "Click a demo, then press its keys. Use the speech bubble to leave a question or note on anything."))),
      map(), ways(), clashes(),
      ...C.groups.map((g) => h("section", { class: "block group-block", id: `group-${g.id}` }, h("h2", {}, g.title), h("p", { class: "lede" }, g.say), C.items.filter((i) => i.group === g.id).map(item))),
      workflows(), reference());
    document.getElementById("rail").replaceWith(Object.assign(nav(), { id: "rail" }));
    document.getElementById("search-slot").replaceWith(search());
    document.getElementById("all-notes").addEventListener("click", () => openDrawer("*", "Every note"));
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && drawerKey && !e.target.closest("textarea")) closeDrawer(); });
    await loadNotes();
    if (location.hash) setTimeout(() => goTo(location.hash.slice(1).replace(/^item-/, "")), 400);
  }
  boot().catch((e) => {
    document.getElementById("main").innerHTML = `<div class="form-err">The page couldn't load its data: ${e.message}</div>`;
    console.error(e);
  });
})();
