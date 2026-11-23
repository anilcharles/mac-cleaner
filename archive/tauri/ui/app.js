"use strict";

/* ================================================================
   Bridge: real Tauri backend, or mock data when opened in a browser
   ================================================================ */
const TAURI = window.__TAURI__;
const bus = new EventTarget();
const invoke = TAURI ? TAURI.core.invoke : mockInvoke;
const listen = TAURI
  ? TAURI.event.listen
  : async (name, cb) => {
      const h = (e) => cb({ payload: e.detail });
      bus.addEventListener(name, h);
      return () => bus.removeEventListener(name, h);
    };

/* ================================================================
   State
   ================================================================ */
const S = {
  view: "welcome", // welcome | scanning | results | done
  modal: null, // null | confirm | working
  sources: [],
  categories: [],
  byPath: new Map(), // path -> node {path, name, size, modified, isDir, parent, cat, ...}
  children: new Map(), // folder path -> {loading} | {error} | {entries, hidden}
  expanded: new Set(),
  // Selection roots: a selected folder covers everything inside it. Never holds a path and its ancestor together.
  selected: new Set(),
  active: null,
  sort: { key: "size", dir: "desc" },
  finished: new Set(),
  report: null,
  error: null,
};

const app = document.getElementById("app");
const modalRoot = document.getElementById("modal-root");

/* ================================================================
   Formatting helpers
   ================================================================ */
const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

// Decimal units, matching Finder.
function bytes(n) {
  if (!n) return "Zero KB";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  if (i === 0) return `${v} bytes`;
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);
function ago(ms) {
  if (!ms) return "—";
  return cap(agoRaw(ms));
}
function agoRaw(ms) {
  const days = Math.floor((Date.now() - ms) / 86400000);
  if (days < 1) return "Today";
  if (days < 7) return rtf.format(-days, "day");
  if (days < 30) return rtf.format(-Math.floor(days / 7), "week");
  if (days < 365) return rtf.format(-Math.floor(days / 30), "month");
  return rtf.format(-Math.floor(days / 365), "year");
}

const plural = (n, one, many = one + "s") => `${n.toLocaleString()} ${n === 1 ? one : many}`;
const tile = (cat, cls = "") => `<span class="tile ${cls}" data-cat="${esc(cat.id)}">${Icon(cat.icon)}</span>`;
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

/* ================================================================
   Derived values
   ================================================================ */
function selectionStats() {
  let size = 0;
  for (const p of S.selected) size += S.byPath.get(p)?.size || 0;
  return { count: S.selected.size, size };
}
function totalFound() {
  return S.categories.reduce((a, c) => a + c.total, 0);
}
function catSelection(cat) {
  let n = 0;
  let size = 0;
  for (const p of S.selected) {
    const node = S.byPath.get(p);
    if (node && node.cat === cat.id) {
      n++;
      size += node.size;
    }
  }
  const all = cat.items.length > 0 && cat.items.every((i) => S.selected.has(i.path));
  return { n, size, all, some: n > 0 && !all };
}
function sorted(list) {
  const { key, dir } = S.sort;
  const m = dir === "asc" ? 1 : -1;
  return [...list].sort((a, b) => {
    if (key === "name") return m * a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    if (key === "modified") return m * ((a.modified || 0) - (b.modified || 0));
    return m * (a.size - b.size);
  });
}

/* ---------- Tree selection ---------- */
// The selected path itself, or the nearest selected folder above it.
function coveringAncestor(path) {
  for (let p = path; p; p = S.byPath.get(p)?.parent) if (S.selected.has(p)) return p;
  return null;
}
function hasSelectedInside(path) {
  const pre = path + "/";
  for (const s of S.selected) if (s.startsWith(pre)) return true;
  return false;
}
function dropInside(path) {
  const pre = path + "/";
  for (const s of [...S.selected]) if (s.startsWith(pre)) S.selected.delete(s);
}
// When every child of a fully listed folder is selected, select the folder instead.
function collapseUp(path) {
  const parent = S.byPath.get(path)?.parent;
  const list = parent && S.children.get(parent);
  if (!list?.entries || list.hidden) return;
  if (list.entries.every((e) => S.selected.has(e.path))) {
    list.entries.forEach((e) => S.selected.delete(e.path));
    S.selected.add(parent);
    collapseUp(parent);
  }
}
function toggleNode(path) {
  const cover = coveringAncestor(path);
  if (cover === path) {
    S.selected.delete(path);
  } else if (cover) {
    // Unchecking something inside a selected folder: keep its siblings at every level instead.
    S.selected.delete(cover);
    for (let child = path; child !== cover; ) {
      const parent = S.byPath.get(child).parent;
      for (const sib of S.children.get(parent).entries) if (sib.path !== child) S.selected.add(sib.path);
      child = parent;
    }
  } else {
    dropInside(path);
    S.selected.add(path);
    collapseUp(path);
  }
}
function setCategorySelection(cat, on) {
  if (on) {
    for (const it of cat.items) {
      dropInside(it.path);
      S.selected.add(it.path);
    }
  } else {
    for (const p of [...S.selected]) if (S.byPath.get(p)?.cat === cat.id) S.selected.delete(p);
  }
}

/* ================================================================
   Views
   ================================================================ */
function viewWelcome() {
  const scope = S.sources
    .map(
      (s) => `<div class="scope-item">${tile(s)}<span class="label">${esc(s.title)}</span>${
        s.risk === "review" ? '<span class="badge review">Review</span>' : ""
      }</div>`
    )
    .join("");
  return `
    <section class="welcome">
      <div class="hero-icon">${Icon("sparkle")}</div>
      <h1>Mac Cleaner</h1>
      <p class="lead">Find caches, logs and developer leftovers taking up space. Nothing is removed until you review and confirm it.</p>
      <button class="btn btn-primary btn-large" data-action="scan" autofocus>Start Scan</button>
      <div class="scope">${scope}</div>
      <div class="safety-note">${Icon("shield")}Items are moved to the Trash, so you can always put them back.</div>
    </section>`;
}

function viewScanning() {
  const total = S.sources.length || 1;
  const pct = Math.round((S.finished.size / total) * 100);
  const C = 2 * Math.PI * 58;
  const current = S.sources.find((s) => !S.finished.has(s.id));
  const steps = S.sources
    .map((s) => {
      const done = S.finished.has(s.id);
      return `<div class="scan-step ${done ? "done" : ""}">${tile(s)}<span class="label">${esc(s.title)}</span>
        <span class="state">${done ? Icon("check") : '<span class="spinner"></span>'}</span></div>`;
    })
    .join("");
  return `
    <section class="scanning">
      <div class="ring">
        <svg viewBox="0 0 132 132"><circle class="track" cx="66" cy="66" r="58" stroke-width="8" fill="none"/>
          <circle class="bar" cx="66" cy="66" r="58" stroke-width="8" fill="none" stroke-linecap="round"
            stroke-dasharray="${C}" stroke-dashoffset="${C * (1 - pct / 100)}"/></svg>
        <div class="ring-label">${pct}%</div>
      </div>
      <h2>Scanning your Mac…</h2>
      <p class="sub">${current ? `Looking through ${esc(current.title)}` : "Finishing up"}</p>
      <div class="scan-steps">${steps}</div>
    </section>`;
}

function viewResults() {
  const found = totalFound();
  const sel = selectionStats();
  const pct = found ? Math.min(100, (sel.size / found) * 100) : 0;
  const cat = S.categories.find((c) => c.id === S.active) || S.categories[0];

  const catRows = S.categories
    .map((c) => {
      const cs = catSelection(c);
      const empty = c.items.length === 0;
      return `
      <div class="cat-row ${c.id === cat.id ? "active" : ""} ${empty ? "empty" : ""}" data-action="select-cat" data-id="${esc(c.id)}">
        <input type="checkbox" class="check" data-action="toggle-cat" data-id="${esc(c.id)}"
          ${cs.all ? "checked" : ""} ${empty ? "disabled" : ""} data-indeterminate="${cs.some}" aria-label="Select all in ${esc(c.title)}">
        ${tile(c)}
        <span class="cat-text"><span class="cat-title">${esc(c.title)}</span>
          <span class="cat-sub">${empty ? "Nothing found" : cs.n ? `${bytes(cs.size)} selected` : plural(c.items.length, "item")}</span></span>
        <span class="cat-size">${empty ? "" : bytes(c.total)}</span>
      </div>`;
    })
    .join("");

  return `
    <div class="results">
      <aside class="sidebar">
        <div class="summary">
          <div class="summary-label">Selected to clean</div>
          <div class="summary-value">${bytes(sel.size)}</div>
          <div class="meter"><div style="width:${pct}%"></div></div>
          <div class="summary-sub">${bytes(found)} found across ${plural(S.categories.filter((c) => c.items.length).length, "category", "categories")}</div>
        </div>
        <div class="section-label">Categories</div>
        <nav class="cat-list">${catRows}</nav>
      </aside>
      ${viewDetail(cat)}
      <footer class="action-bar">
        <div class="selection-info">${
          sel.count
            ? `<strong>${plural(sel.count, "item")}</strong> selected · <strong>${bytes(sel.size)}</strong>`
            : "Select items to clean"
        }</div>
        <button class="btn btn-secondary" data-action="scan">${Icon("refresh")}Rescan</button>
        <button class="btn btn-primary" data-action="review" ${sel.count ? "" : "disabled"}>Review &amp; Clean…</button>
      </footer>
    </div>`;
}

function treeRows(list, depth) {
  return list
    .map((node) => {
      let html = rowHtml(node, depth);
      if (node.isDir && S.expanded.has(node.path)) {
        const kids = S.children.get(node.path);
        const note = (text, cls = "") =>
          `<div class="row row-grid note-row ${cls}"><span></span><div class="cell-name"><div class="name-wrap" style="--depth:${depth + 1}"><span class="disclosure-spacer"></span>${text}</div></div></div>`;
        if (!kids || kids.loading) html += note('<span class="spinner"></span><span>Loading…</span>');
        else if (kids.error) html += note(`${Icon("alert")}<span>${esc(kids.error)}</span>`, "error");
        else if (!kids.entries.length) html += note("<span>This folder is empty</span>");
        else {
          html += treeRows(sorted(kids.entries), depth + 1);
          if (kids.hidden) html += note(`<span>${plural(kids.hidden, "smaller item")} not shown</span>`);
        }
      }
      return html;
    })
    .join("");
}

function rowHtml(it, depth) {
  const cover = coveringAncestor(it.path);
  const on = !!cover;
  const partial = !on && it.isDir && hasSelectedInside(it.path);
  const open = S.expanded.has(it.path);
  const toggle = it.isDir
    ? `<button class="disclosure ${open ? "open" : ""}" data-action="expand" data-path="${esc(it.path)}" aria-label="${open ? "Collapse" : "Expand"} ${esc(it.name)}" aria-expanded="${open}">${Icon("chevron")}</button>`
    : '<span class="disclosure-spacer"></span>';
  const sub = depth === 0 ? `<div class="path" title="${esc(it.path)}">${esc(it.location)}</div>` : "";
  return `
    <div class="row row-grid ${on ? "on" : ""} ${depth ? "child" : ""}" data-action="toggle-item" data-path="${esc(it.path)}">
      <input type="checkbox" class="check" ${on ? "checked" : ""} data-indeterminate="${partial}" tabindex="-1" aria-label="Select ${esc(it.name)}">
      <div class="cell-name">
        <div class="name-wrap" style="--depth:${depth}">
          ${toggle}
          <span class="kind ${it.isDir ? "folder" : "file"}">${Icon(it.isDir ? "folder" : "file")}</span>
          <div class="name-text">
            <div class="name-line"><span class="name" title="${esc(it.name)}">${esc(it.name)}</span>${
              it.note ? `<span class="pill">${esc(it.note)}</span>` : ""
            }</div>
            ${sub}
          </div>
        </div>
      </div>
      <span class="cell-date">${ago(it.modified)}</span>
      <span class="cell-size num">${bytes(it.size)}</span>
      <button class="icon-btn" data-action="reveal" data-path="${esc(it.path)}" title="Show in Finder" aria-label="Show in Finder">${Icon("reveal")}</button>
    </div>`;
}

function viewDetail(cat) {
  const badge =
    cat.risk === "review" ? '<span class="badge review">Review</span>' : '<span class="badge safe">Safe to remove</span>';
  const head = (key, label, right = false) => {
    const on = S.sort.key === key;
    const btn = `<button class="sort ${on ? "on" : ""} ${on && S.sort.dir === "asc" ? "asc" : ""}" data-action="sort" data-key="${key}">${label}${Icon("chevron")}</button>`;
    return right ? `<span class="th-right">${btn}</span>` : `<span>${btn}</span>`;
  };

  let body;
  if (!cat.items.length) {
    body = `<div class="empty-state">${Icon("empty")}<strong>All clean</strong><span>Nothing to remove in ${esc(cat.title)}.</span></div>`;
  } else {
    const rows = treeRows(sorted(cat.items), 0);
    body = `
      <div class="table-head row-grid"><span></span>${head("name", "Name")}${head("modified", "Modified")}${head("size", "Size", true)}<span></span></div>
      ${rows}`;
  }

  const cs = catSelection(cat);
  return `
    <section class="detail">
      <header class="detail-header">
        ${tile(cat, "lg")}
        <div class="detail-heading">
          <h2>${esc(cat.title)} ${badge}</h2>
          <p>${esc(cat.description)}</p>
        </div>
        <div class="detail-actions">
          <button class="btn btn-ghost" data-action="select-all" ${!cat.items.length || cs.all ? "disabled" : ""}>Select All</button>
          <button class="btn btn-ghost" data-action="select-none" ${cs.n ? "" : "disabled"}>Deselect All</button>
        </div>
      </header>
      <div class="table"><div class="table-scroll" data-scroll="table">${body}</div></div>
    </section>`;
}

function viewDone() {
  const r = S.report;
  const failed = r.failures.length;
  const failures = failed
    ? `<div class="failures"><div class="failures-head">${plural(failed, "item")} couldn't be moved</div>${r.failures
        .map(
          (f) => `<div class="failure"><div class="path" title="${esc(f.path)}">${esc(f.path)}</div><div class="why">${esc(f.reason)}</div></div>`
        )
        .join("")}</div>`
    : "";
  return `
    <section class="done-view">
      <div class="done-badge ${r.removed ? "" : "warn"}">${Icon(r.removed ? "check" : "alert")}</div>
      <div class="label">${r.removed ? "Space recovered" : "Nothing was removed"}</div>
      <div class="big">${bytes(r.freed)}</div>
      <p class="sub">${
        r.removed
          ? `${plural(r.removed, "item")} moved to the Trash. Empty the Trash to free the space for good, or put items back if you need them.`
          : "None of the selected items could be moved to the Trash."
      }</p>
      <div class="done-actions">
        <button class="btn btn-secondary" data-action="open-trash">${Icon("trash")}Open Trash</button>
        <button class="btn btn-primary" data-action="scan">${Icon("refresh")}Scan Again</button>
      </div>
      ${failures}
    </section>`;
}

function viewConfirm() {
  const sel = selectionStats();
  const groups = S.categories
    .map((c) => {
      const items = [...S.selected]
        .map((p) => S.byPath.get(p))
        .filter((n) => n && n.cat === c.id)
        .sort((a, b) => b.size - a.size);
      if (!items.length) return "";
      const size = items.reduce((a, i) => a + i.size, 0);
      return `
        <div class="group-head">${tile(c)}<span>${esc(c.title)}</span><span class="num">${plural(items.length, "item")} · ${bytes(size)}</span></div>
        ${items
          .map(
            (i) => `<div class="confirm-row"><span class="dot"></span>
              <div style="min-width:0"><div class="name" title="${esc(i.name)}">${esc(i.name)}</div><div class="path" title="${esc(i.path)}">${esc(i.path)}</div></div>
              <span class="num">${bytes(i.size)}</span></div>`
          )
          .join("")}`;
    })
    .join("");

  const reviewSelected = S.categories.some(
    (c) => [...S.selected].some((p) => {
      const n = S.byPath.get(p);
      if (n?.cat !== c.id) return false;
      let top = n;
      while (top.parent) top = S.byPath.get(top.parent);
      return c.risk === "review" || !!top.note;
    })
  );
  const callout = reviewSelected
    ? `<div class="callout">${Icon("alert")}<span>Your selection includes items marked <strong>Review</strong> or system caches. Make sure you no longer need them.</span></div>`
    : "";

  return `
    <div class="backdrop" data-action="backdrop">
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="confirm-title">
        <div class="modal-header">
          <div class="modal-icon">${Icon("trash")}</div>
          <div>
            <h3 id="confirm-title">Move ${plural(sel.count, "item")} to the Trash?</h3>
            <p>This frees about <strong>${bytes(sel.size)}</strong>. Review the list below. Items stay in the Trash until you empty it, so you can put them back.</p>
          </div>
        </div>
        <div class="modal-list">${groups}</div>
        ${callout}
        <div class="modal-footer">
          <span class="hint">Press Esc to cancel</span>
          <button class="btn btn-secondary" data-action="cancel">Cancel</button>
          <button class="btn btn-danger" data-action="confirm-clean">${Icon("trash")}Move to Trash</button>
        </div>
      </div>
    </div>`;
}

function viewWorking() {
  return `
    <div class="backdrop">
      <div class="modal" role="dialog" aria-modal="true" aria-busy="true" style="width:min(380px,100%)">
        <div class="working"><span class="spinner"></span><div><h3>Moving items to the Trash…</h3><p>This usually takes a few seconds.</p></div></div>
      </div>
    </div>`;
}

/* ================================================================
   Render
   ================================================================ */
function render() {
  // Keep scroll positions stable across re-renders.
  const scroll = {};
  app.querySelectorAll("[data-scroll], .cat-list").forEach((el) => {
    scroll[el.dataset.scroll || "cats"] = el.scrollTop;
  });

  const views = { welcome: viewWelcome, scanning: viewScanning, results: viewResults, done: viewDone };
  app.innerHTML = views[S.view]();

  app.querySelectorAll("[data-indeterminate='true']").forEach((el) => (el.indeterminate = true));
  app.querySelectorAll("[data-scroll], .cat-list").forEach((el) => {
    const k = el.dataset.scroll || "cats";
    if (scroll[k] != null) el.scrollTop = scroll[k];
  });

  modalRoot.innerHTML = S.modal === "confirm" ? viewConfirm() : S.modal === "working" ? viewWorking() : "";
  if (S.modal === "confirm") modalRoot.querySelector("[data-action='cancel']").focus();
}

function toast(msg) {
  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = msg;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 2600);
}

/* ================================================================
   Actions
   ================================================================ */
async function startScan() {
  S.view = "scanning";
  S.modal = null;
  S.finished = new Set();
  render();
  try {
    const [cats] = await Promise.all([invoke("scan"), delay(900)]);
    const order = new Map(S.sources.map((s, i) => [s.id, i]));
    S.categories = cats.sort((a, b) => order.get(a.id) - order.get(b.id));
    S.byPath = new Map();
    S.children = new Map();
    S.expanded = new Set();
    S.selected = new Set();
    for (const c of S.categories)
      for (const it of c.items) {
        it.parent = null;
        it.cat = c.id;
        S.byPath.set(it.path, it);
        if (it.selected) S.selected.add(it.path);
      }
    const firstWithItems = S.categories.find((c) => c.items.length);
    S.active = (firstWithItems || S.categories[0]).id;
    S.view = "results";
  } catch (e) {
    S.view = "welcome";
    toast(`Scan failed: ${e}`);
  }
  render();
}

async function runClean() {
  const paths = [...S.selected];
  S.modal = "working";
  render();
  try {
    const [report] = await Promise.all([invoke("clean", { paths }), delay(600)]);
    S.report = report;
    S.view = "done";
  } catch (e) {
    toast(`Cleaning failed: ${e}`);
  }
  S.modal = null;
  render();
}

async function toggleExpand(path) {
  if (S.expanded.has(path)) {
    S.expanded.delete(path);
    return render();
  }
  S.expanded.add(path);
  if (S.children.get(path)?.entries) return render();
  S.children.set(path, { loading: true });
  render();
  try {
    const listing = await invoke("list_children", { path });
    const parent = S.byPath.get(path);
    for (const e of listing.entries) {
      e.parent = path;
      e.cat = parent.cat;
      S.byPath.set(e.path, e);
    }
    S.children.set(path, listing);
  } catch (e) {
    S.children.set(path, { error: String(e) });
  }
  render();
}

const handlers = {
  expand: (el) => toggleExpand(el.dataset.path),
  scan: startScan,
  "select-cat": (el) => {
    if (S.active !== el.dataset.id) {
      S.active = el.dataset.id;
      render();
      const t = app.querySelector("[data-scroll='table']");
      if (t) t.scrollTop = 0;
    }
  },
  "toggle-cat": (el) => {
    const cat = S.categories.find((c) => c.id === el.dataset.id);
    setCategorySelection(cat, !catSelection(cat).all);
    render();
  },
  "toggle-item": (el) => {
    toggleNode(el.dataset.path);
    render();
  },
  "select-all": () => {
    setCategorySelection(S.categories.find((c) => c.id === S.active), true);
    render();
  },
  "select-none": () => {
    setCategorySelection(S.categories.find((c) => c.id === S.active), false);
    render();
  },
  sort: (el) => {
    const key = el.dataset.key;
    if (S.sort.key === key) S.sort.dir = S.sort.dir === "asc" ? "desc" : "asc";
    else S.sort = { key, dir: key === "name" ? "asc" : "desc" };
    render();
  },
  reveal: (el) => invoke("reveal", { path: el.dataset.path }).catch((e) => toast(String(e))),
  review: () => {
    if (!S.selected.size) return;
    S.modal = "confirm";
    render();
  },
  cancel: () => {
    S.modal = null;
    render();
  },
  backdrop: (el, ev) => {
    if (ev.target === el) handlers.cancel();
  },
  "confirm-clean": runClean,
  "open-trash": () => invoke("open_trash").catch((e) => toast(String(e))),
};

document.addEventListener("click", (ev) => {
  const el = ev.target.closest("[data-action]");
  if (!el || el.disabled) return;
  const action = el.dataset.action;
  if (action !== "backdrop") ev.stopPropagation();
  // Checkboxes render from state; stop the native toggle so state stays the single source of truth.
  if (el.matches("input[type=checkbox]") || el.querySelector(":scope > input[type=checkbox]") === ev.target) {
    ev.preventDefault();
  }
  handlers[action]?.(el, ev);
});

document.addEventListener("dblclick", (ev) => {
  const row = ev.target.closest(".row[data-path]");
  if (row && !ev.target.closest("button") && S.byPath.get(row.dataset.path)?.isDir) toggleExpand(row.dataset.path);
});

document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && S.modal === "confirm") handlers.cancel();
});

/* ================================================================
   Boot
   ================================================================ */
(async function boot() {
  await listen("scan-progress", (e) => {
    S.finished.add(e.payload.id);
    if (S.view === "scanning") render();
  });
  S.sources = await invoke("list_sources");

  // Preview helper: index.html?view=results|confirm|done|scanning (browser only).
  const preview = !TAURI && new URLSearchParams(location.search).get("view");
  if (preview && preview !== "welcome") {
    if (preview === "scanning") {
      S.view = "scanning";
      S.sources.slice(0, 3).forEach((s) => S.finished.add(s.id));
      return render();
    }
    MOCK_FAST = true;
    await startScan();
    if (preview.startsWith("tree")) {
      const first = S.categories[0].items[0];
      await toggleExpand(first.path);
      await toggleExpand(`${first.path}/Code Cache`);
      toggleNode(`${first.path}/Code Cache/GPUCache`);
      render();
    }
    if (preview === "confirm" || preview === "tree-confirm") handlers.review();
    if (preview === "done") await runClean();
    return;
  }
  render();
})();

/* ================================================================
   Mock backend (browser preview only)
   ================================================================ */
var MOCK_FAST = false;
async function mockInvoke(cmd, args) {
  const sources = [
    ["caches", "App Caches", "Temporary files apps keep to load faster. Apps rebuild them when needed.", "cache", "safe"],
    ["logs", "Log Files", "Activity and diagnostic logs written by apps. Only useful when troubleshooting.", "logs", "safe"],
    ["derived-data", "Xcode Derived Data", "Build products and indexes. Xcode recreates them on the next build.", "hammer", "safe"],
    ["simulator-caches", "Simulator Caches", "Cached data for iOS simulators. Recreated when a simulator next boots.", "phone", "safe"],
    ["package-caches", "Package Caches", "Downloaded packages from npm, Cargo, Gradle, pnpm and Bun. Fetched again on demand.", "package", "safe"],
    ["xcode-archives", "Xcode Archives", "Archived app builds. They may hold the only dSYMs for crash reports of shipped versions.", "archive", "review"],
    ["device-support", "Device Support", "Debug symbols for devices you have connected. Keep versions you still debug on.", "chip", "review"],
    ["installers", "Installers in Downloads", "Disk images and installer packages. Usually not needed once the app is installed.", "download", "review"],
  ].map(([id, title, description, icon, risk]) => ({ id, title, description, icon, risk }));

  if (cmd === "list_sources") return sources;
  if (cmd === "reveal" || cmd === "open_trash") return null;
  if (cmd === "list_children") {
    await delay(MOCK_FAST ? 0 : 250);
    const base = S.byPath.get(args.path);
    const names = ["Cache.db", "fsCachedData", "Code Cache", "GPUCache", "index-v2", "blob_storage", "preview.png", "manifest.json"];
    const entries = names.map((name, k) => ({
      path: `${args.path}/${name}`,
      name,
      size: Math.round(base.size * [0.42, 0.21, 0.14, 0.09, 0.06, 0.04, 0.02, 0.01][k]),
      modified: base.modified - k * 3 * 86400000,
      isDir: !name.includes("."),
    }));
    return { entries, hidden: base.size > 1e9 ? 37 : 0 };
  }
  if (cmd === "clean") {
    await delay(MOCK_FAST ? 0 : 800);
    const freed = args.paths.reduce((a, p) => a + (S.byPath.get(p)?.size || 0), 0);
    return {
      removed: args.paths.length - 1,
      freed,
      failures: [{ path: "/Users/you/Library/Caches/com.apple.Safari", reason: "macOS protects this item. Grant Full Disk Access to clean it." }],
    };
  }
  if (cmd === "scan") {
    const H = "/Users/you";
    const day = 86400000;
    const now = Date.now();
    const mk = (dir, name, size, ageDays, selected, note) => ({
      path: `${dir.replace("~", H)}/${name}`,
      name,
      location: dir,
      size,
      modified: now - ageDays * day,
      selected,
      note: note || null,
      isDir: !/\.(dmg|pkg|log)$/.test(name),
    });
    const data = {
      caches: [
        mk("~/Library/Caches", "com.spotify.client", 2_840_000_000, 0, true),
        mk("~/Library/Caches", "Google", 1_210_000_000, 1, true),
        mk("~/Library/Caches", "Homebrew", 864_000_000, 12, true),
        mk("~/Library/Caches", "com.apple.Safari", 412_000_000, 0, false, "macOS system cache"),
        mk("~/Library/Caches", "pip", 318_500_000, 40, true),
        mk("~/Library/Caches", "com.microsoft.VSCode.ShipIt", 254_000_000, 21, true),
        mk("~/Library/Caches", "Yarn", 198_000_000, 90, true),
        mk("~/Library/Caches", "org.swift.swiftpm", 96_400_000, 5, true),
        mk("~/Library/Caches", "com.figma.Desktop", 61_200_000, 3, true),
        mk("~/Library/Caches", "com.apple.python", 12_300_000, 200, false, "macOS system cache"),
      ],
      logs: [
        mk("~/Library/Logs", "DiagnosticReports", 88_200_000, 2, true),
        mk("~/Library/Logs", "JetBrains", 41_000_000, 14, true),
        mk("~/Library/Logs", "CoreSimulator", 12_800_000, 30, true),
      ],
      "derived-data": [
        mk("~/Library/Developer/Xcode/DerivedData", "Weather-bqzxkdlrfhqmd", 4_620_000_000, 2, true),
        mk("~/Library/Developer/Xcode/DerivedData", "Notes-gfhdkqoeuwnzpa", 1_930_000_000, 45, true),
        mk("~/Library/Developer/Xcode/DerivedData", "ModuleCache.noindex", 512_000_000, 2, true),
      ],
      "simulator-caches": [mk("~/Library/Developer/CoreSimulator/Caches", "dyld", 1_480_000_000, 9, true)],
      "package-caches": [
        mk("~/.npm", "_cacache", 3_120_000_000, 1, true),
        mk("~/.cargo/registry", "cache", 1_045_000_000, 6, true),
        mk("~/.gradle", "caches", 2_400_000_000, 60, true),
      ].map((i, k) => ({ ...i, name: ["npm cache", "Cargo registry cache", "Gradle caches"][k] })),
      "xcode-archives": [
        mk("~/Library/Developer/Xcode/Archives", "2025-03-14", 1_820_000_000, 250, false),
        mk("~/Library/Developer/Xcode/Archives", "2026-06-02", 940_000_000, 170, false),
      ],
      "device-support": [
        mk("~/Library/Developer/Xcode/iOS DeviceSupport", "iPhone16,2 18.4 (22E240)", 4_210_000_000, 220, false),
        mk("~/Library/Developer/Xcode/iOS DeviceSupport", "iPhone17,1 26.1 (23B85)", 4_480_000_000, 15, false),
      ],
      installers: [],
    };
    const out = [];
    for (const s of sources) {
      await delay(MOCK_FAST ? 0 : 140 + Math.random() * 220);
      const items = data[s.id];
      out.push({ ...s, items, total: items.reduce((a, i) => a + i.size, 0) });
      bus.dispatchEvent(new CustomEvent("scan-progress", { detail: { id: s.id } }));
    }
    return out;
  }
  throw new Error(`unknown command ${cmd}`);
}
