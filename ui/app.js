"use strict";
const $ = s => document.querySelector(s);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch {} },
};
const BUILD = document.documentElement.dataset.build || "0";
/** How many rendered conversations to keep ready, beyond the one on screen. */
const KEEP_VIEWS = 10;
/** In the reading pane a conversation counts as read after it has been up this long. */
const READ_AFTER_MS = 1500;

const state = {
  accounts: [],
  filter: null,          // account id, or null for all inboxes
  q: "",
  threads: [],
  cursor: 0,
  selected: new Set(),   // thread keys
  views: new Map(),      // thread key -> rendered conversation, oldest first
  open: null,            // the view on screen
  collection: null,      // { title }: the list is showing a set the assistant picked
  unreadOnly: false,     // list only conversations with something unread
  kept: new Set(),       // thread keys opened while unreadOnly is on; they stay listed once read
  kind: "",              // "people" or "automated": only mail a person wrote, or only what systems sent
  from: "",              // only mail from this sender: an address, part of one, or a domain
  all: false,            // list All Mail (archived and sent mail too), not only the inbox
};
const key = t => t.account_id + ":" + t.thrid;

// Small icons are drawn rather than typed: a text glyph looks however the font happens to
// draw it (the "more" ellipsis came out as three dashes).
const ICONS = {
  archive: '<path d="M2 3.5h12v3H2zM3 6.5v6h10v-6M6.3 9.2h3.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" stroke-linecap="round"/>',
  more: '<circle cx="3" cy="8" r="1.5"/><circle cx="8" cy="8" r="1.5"/><circle cx="13" cy="8" r="1.5"/>',
  key: '<circle cx="5" cy="11" r="2.6" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M7 9l6-6M10.5 5.5l2 2M12.6 3.4l1.2 1.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  verified: '<path d="M8 1.5 13.5 3.6v4.1c0 3.3-2.2 5.7-5.5 6.8-3.3-1.1-5.5-3.5-5.5-6.8V3.6z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M5.6 8.1 7.4 9.9 10.6 6.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  unverified: '<circle cx="8" cy="8" r="6.2" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M6.2 6.4a1.9 1.9 0 1 1 2.6 1.8c-.5.3-.8.6-.8 1.3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="11.6" r=".9"/>',
  failed: '<path d="M8 2 14.5 13.5h-13z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M8 6.4v3.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="8" cy="11.6" r=".9"/>',
  person: '<circle cx="8" cy="5.2" r="2.7"/><path d="M2.8 14c.4-3 2.5-4.7 5.2-4.7s4.800 1.700 5.200 4.700z"/>',
  save: '<path d="M8 2v8M4.5 7 8 10.5 11.5 7M3 13.5h10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>',
};
function icon(name) {
  const t = document.createElement("template");
  t.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">${ICONS[name]}</svg>`;
  return t.content.firstChild;
}
const account = id => state.accounts.find(a => a.id === id);
const dark = matchMedia("(prefers-color-scheme: dark)");
/** Set when this window shows a single conversation: "account:thread". */
const SOLO = new URLSearchParams(location.search).get("thread");

async function api(path, opts = {}) {
  if (opts.body) { opts.method ??= "POST"; opts.headers = { "Content-Type": "application/json" }; opts.body = JSON.stringify(opts.body); }
  const r = await fetch(path, opts);
  if (!r.ok) throw new Error((await r.text()) || r.statusText);
  return r.status === 204 || r.status === 202 ? null : r.json();
}

function toast(text) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.add("show");
  clearTimeout(toast.h);
  toast.h = setTimeout(() => t.classList.remove("show"), 4000);
}

// Links leave the app: they go to the default browser, where you are signed in.
function openExternal(url) {
  api("/api/open", { body: { url } }).catch(() => window.open(url, "_blank", "noopener"));
}

function fmtDate(ts, long) {
  const d = new Date(ts * 1000), now = new Date();
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (long) return d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric", year: "numeric" }) + ", " + time;
  if (d.toDateString() === now.toDateString()) return time;
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString([], { month: "short", day: "numeric" });
  return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
}
const fmtSize = n => n < 1024 ? n + " B" : n < 1048576 ? Math.round(n / 1024) + " KB" : (n / 1048576).toFixed(1) + " MB";

// ---- sender images -----------------------------------------------------------
// Gmail's own profile photos are not available over IMAP. For mail from an organisation
// we show its site icon (looked up by domain); people on webmail get coloured initials.
const WEBMAIL = new Set(("gmail.com googlemail.com yahoo.com yahoo.co.jp yahoo.co.uk ymail.com outlook.com outlook.jp "
  + "hotmail.com hotmail.co.jp live.com live.jp msn.com icloud.com me.com mac.com aol.com proton.me protonmail.com "
  + "docomo.ne.jp ezweb.ne.jp au.com softbank.ne.jp i.softbank.jp nifty.com eircom.net").split(" "));
const AV_COLORS = ["#5b8def", "#e0723c", "#3fae7a", "#b667d6", "#c9961a", "#d8587e", "#4aa8b8", "#8a7f6a"];
const icons = new Map(Object.entries(JSON.parse(store.get("icons:" + BUILD) || "{}")));  // domain -> url or ""
const iconLookups = new Map();

function domainsFor(addr) {
  const d = (addr.split("@")[1] || "").toLowerCase();
  if (!d || WEBMAIL.has(d)) return [];
  const parts = d.split(".");
  // news.example.com -> example.com; mail.example.co.jp -> example.co.jp
  const second = parts[parts.length - 2];
  const keep = parts[parts.length - 1].length === 2 && /^(co|ne|or|ac|go|com|net|org|gr|ed|lg)$/.test(second) ? 3 : 2;
  const base = parts.slice(-keep).join(".");
  return base === d ? [d] : [base, d];
}

function probeIcon(domain) {
  return new Promise(resolve => {
    const img = new Image();
    const url = `https://www.google.com/s2/favicons?domain=${encodeURIComponent(domain)}&sz=64`;
    // A site with no icon comes back as a 16px placeholder globe: not worth showing.
    img.onload = () => resolve(img.naturalWidth >= 32 ? url : "");
    img.onerror = () => resolve("");
    img.src = url;
  });
}

function iconFor(addr) {
  const candidates = domainsFor(addr);
  if (!candidates.length) return Promise.resolve("");
  const id = candidates[candidates.length - 1];
  if (icons.has(id)) return Promise.resolve(icons.get(id));
  if (!iconLookups.has(id)) {
    iconLookups.set(id, (async () => {
      let found = "";
      for (const d of candidates) if ((found = await probeIcon(d))) break;
      icons.set(id, found);
      store.set("icons:" + BUILD, JSON.stringify(Object.fromEntries(icons)));
      return found;
    })());
  }
  return iconLookups.get(id);
}

function avatar(name, addr, cls = "") {
  const label = (name || addr || "?").replace(/^["'\s]+/, "");
  const node = el("span", "av " + cls, (label[0] || "?").toUpperCase());
  let h = 0;
  for (const ch of addr || label) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  node.style.background = AV_COLORS[h % AV_COLORS.length];
  iconFor(addr || "").then(url => {
    if (!url) return;
    const img = el("img");
    img.alt = "";
    img.src = url;
    node.prepend(img);
  });
  return node;
}

// ---- sidebar ---------------------------------------------------------------
/** Show how many inbox conversations are unread on the app's taskbar button (a red badge
 *  over the icon) and in the window title. The badge is drawn here, where there is a font,
 *  and handed to the window as raw pixels; over 99 it is a plain dot. */
let badged = null;
function taskbarBadge(n) {
  if (SOLO || n === badged) return;
  badged = n;
  document.title = n ? `Unibox (${n.toLocaleString()})` : "Unibox";
  if (!window.ipc) return;
  if (!n) return window.ipc.postMessage("badge:");
  const s = Math.max(16, Math.min(64, Math.round(16 * devicePixelRatio)));
  const c = Object.assign(document.createElement("canvas"), { width: s, height: s }), g = c.getContext("2d");
  g.fillStyle = "#d92d20";
  // a little smaller than the slot Windows gives it, like other apps' badges
  g.beginPath(); g.arc(s / 2, s / 2, s * 0.4, 0, 2 * Math.PI); g.fill();
  g.fillStyle = "#fff";
  if (n > 99) { g.beginPath(); g.arc(s / 2, s / 2, s * 0.15, 0, 2 * Math.PI); g.fill(); }
  else {
    const text = String(n);
    g.font = `700 ${s * (text.length > 1 ? 0.5 : 0.62)}px "Segoe UI", sans-serif`;
    g.textAlign = "center";
    const m = g.measureText(text);
    g.fillText(text, s / 2, s / 2 + (m.actualBoundingBoxAscent - m.actualBoundingBoxDescent) / 2);
  }
  const hex = Array.from(g.getImageData(0, 0, s, s).data, b => b.toString(16).padStart(2, "0")).join("");
  window.ipc.postMessage(`badge:${s}:${hex}`);
}

function renderAccounts() {
  const nav = $("#accounts");
  if (nav.querySelector(".dragging")) return;   // don't rebuild the rows under a drag
  nav.textContent = "";
  const item = (id, name, color, unread, acct) => {
    const b = el("button", "acct" + (state.filter === id ? " on" : "") + (acct?.last_error ? " err" : "") + (unread ? "" : " zero"));
    b.style.setProperty("--c", color);
    const dot = el("span", "dot");
    b.append(dot, el("span", "n", unread || ""), el("span", "name", name));
    b.title = acct ? (acct.last_error ? `${acct.email}: sync problem: ${acct.last_error}` : acct.email) : "All inboxes";
    if (acct) {
      const rm = el("span", "rm", "×");
      rm.title = "Remove this account from Unibox";
      rm.onclick = e => { e.stopPropagation(); removeAccount(acct); };
      const pw = el("span", "pw");
      pw.append(icon("key"));
      pw.title = "Change the app password";
      pw.onclick = e => { e.stopPropagation(); openAccountDialog(acct); };
      b.append(pw, rm);
    }
    b.onclick = () => { if (b.dataset.dragged) return delete b.dataset.dragged; setFilter(id); b.blur(); };
    if (acct) { b.dataset.id = id; b.onpointerdown = e => dragAccount(e, b); }
    nav.append(b);
  };
  const unread = state.accounts.reduce((n, a) => n + a.unread, 0);
  item(null, "All inboxes", "#77736b", unread);
  taskbarBadge(unread);
  for (const a of state.accounts) item(a.id, a.email, a.color, a.unread, a);

  const errs = state.accounts.filter(a => a.last_error);
  const last = Math.max(0, ...state.accounts.map(a => a.last_sync || 0));
  $("#status").textContent = errs.length ? `Can't sync ${errs.map(a => a.email).join(", ")}`
    : last ? "Synced " + fmtDate(last) : state.accounts.length ? "Syncing…" : "";
}

// Drag an account up or down the sidebar to reorder it. Nothing moves in the DOM while
// dragging (that would drop the pointer capture): the row follows the pointer and the others
// slide out of its way with transforms. The order is saved on release (it also decides which
// account the 1-9 keys pick).
function dragAccount(down, b) {
  if (down.button !== 0 || down.target.closest(".rm, .pw")) return;
  const nav = $("#accounts");
  const rows = [...nav.querySelectorAll(".acct[data-id]")];
  const from = rows.indexOf(b);
  if (rows.length < 2) return;
  const step = rows[1].getBoundingClientRect().top - rows[0].getBoundingClientRect().top;
  let dragging = false, to = from;
  const move = e => {
    if (!dragging) {
      if (Math.abs(e.clientY - down.clientY) < 5) return;
      dragging = true;
      b.setPointerCapture(down.pointerId);
      b.classList.add("dragging");
      nav.classList.add("sorting");
    }
    const dy = Math.max(-from * step, Math.min((rows.length - 1 - from) * step, e.clientY - down.clientY));
    b.style.transform = `translateY(${dy}px)`;
    to = from + Math.round(dy / step);
    rows.forEach((o, j) => {
      if (o === b) return;
      const shift = j > from && j <= to ? -step : j < from && j >= to ? step : 0;
      o.style.transform = shift ? `translateY(${shift}px)` : "";
    });
  };
  const up = () => {
    b.removeEventListener("pointermove", move);
    b.removeEventListener("pointerup", up);
    b.removeEventListener("pointercancel", up);
    if (!dragging) return;
    b.dataset.dragged = 1;                      // swallow the click that follows the release
    setTimeout(() => delete b.dataset.dragged, 0);
    // let go: the row settles into its slot, then the list is rebuilt in the new order
    b.classList.add("settling");
    b.style.transform = `translateY(${(to - from) * step}px)`;
    setTimeout(() => {
      b.classList.remove("dragging");
      nav.classList.remove("sorting");
      if (to !== from) {
        const moved = state.accounts.splice(from, 1)[0];
        state.accounts.splice(to, 0, moved);
        api("/api/accounts/order", { body: { ids: state.accounts.map(a => a.id) } }).catch(() => refresh());
      }
      renderAccounts();
    }, 140);
  };
  b.addEventListener("pointermove", move);
  b.addEventListener("pointerup", up);
  b.addEventListener("pointercancel", up);
}

async function removeAccount(a) {
  if (!confirm(`Remove ${a.email} from Unibox?\n\nThis deletes the copy of its mail on this PC and the saved app password. Nothing changes in Gmail.`)) return;
  await api("/api/accounts/" + a.id, { method: "DELETE" });
  if (state.filter === a.id) state.filter = null;
  hideView();
  refresh();
}

function setFilter(id) {
  leaveCollection();
  state.filter = id;
  state.cursor = 0;
  state.selected.clear();
  hideView();
  loadThreads(true);
  renderAccounts();
  $("#list").focus();
}

// ---- conversation list -------------------------------------------------------
/** What to ask the server for: the inbox (filtered, searched), or the assistant's set. */
function listParams() {
  if (state.collection) return new URLSearchParams({ collection: true });
  const p = new URLSearchParams({ q: state.q });
  if (state.filter != null) p.set("account", state.filter);
  if (state.unreadOnly) { p.set("unread", true); p.set("keep", [...state.kept].join(",")); }
  if (state.kind) p.set("kind", state.kind);
  if (state.all) p.set("all", true);
  if (state.from) p.set("from", state.from);
  return p;
}

/** What the list is narrowed to, shown in the bar above it. Nothing switched on means no
 *  filter. Where it looks (the inbox, or all mail) is the label at the start of the search
 *  field; each filter that is on is a removable piece inside the field as well as a lit
 *  button: only unread (conversations opened meanwhile stay listed), only people or only
 *  automated mail (one or neither), and one sender. */
const KINDS = ["", "people", "automated"];
function showListFilters() {
  $("#unread-only").setAttribute("aria-pressed", state.unreadOnly);
  for (const b of $("#kind").children) b.setAttribute("aria-pressed", b.dataset.kind === state.kind);
  const scope = $("#scope");
  scope.firstElementChild.textContent = state.all ? "All mail" : "Inbox";
  scope.setAttribute("aria-pressed", state.all);
  scope.title = state.all ? "Looking in all mail, archived and sent too. Click for just the inbox (g then i)"
    : "Looking in the inbox. Click for all mail, archived and sent too (g then a)";
  const box = $("#tokens");
  box.textContent = "";
  const piece = (text, off) => {
    const t = el("span", "token", text), x = el("button", "", "×");
    x.title = x.ariaLabel = "Remove " + text;
    x.onclick = e => { e.stopPropagation(); off(); $("#search").focus(); };
    t.append(x);
    box.append(t);
  };
  if (state.unreadOnly) piece("is:unread", () => setListFilter("unreadOnly", false));
  if (state.kind) piece("from:" + state.kind, () => setListFilter("kind", ""));
  if (state.from) piece("from:" + state.from, () => setListFilter("from", ""));
}

/** Take the filters out of what was typed in the search field, leaving the words to search
 *  for. A piece counts once it is finished: followed by a space, or everything when `done`
 *  (Enter, or a suggestion chosen). Returns { text, changed }. */
function takeFilters(text, done) {
  const words = text.split(/\s+/).filter(Boolean), keep = [];
  const typing = !done && !/\s$/.test(text) ? words.pop() : null;   // the word still being typed
  let changed = false;
  for (const w of words) {
    const m = /^(is|in|from):(.+)$/i.exec(w), what = m && m[1].toLowerCase(), value = m && m[2].toLowerCase();
    if (what === "is" && value === "unread") state.unreadOnly = true;
    else if (what === "in" && (value === "all" || value === "inbox")) state.all = value === "all";
    else if (what === "from" && KINDS.includes(value)) state.kind = value;
    else if (what === "from") state.from = value;
    else { keep.push(w); continue; }
    changed = true;
  }
  if (typing != null) keep.push(typing);
  return { text: changed ? keep.join(" ") : text, changed };
}

function setListFilter(name, on) {
  leaveCollection();
  state[name] = on;
  state.kept = new Set();
  state.cursor = 0;
  state.selected.clear();
  showListFilters();
  hideView();
  loadThreads(true);
}

/** Stop showing the assistant's set and go back to the ordinary list. */
function leaveCollection() {
  if (!state.collection) return;
  state.collection = null;
  api("/api/agent/collection/clear", { method: "POST" }).catch(() => {});
}

// The list arrives as one array per conversation, in this order (see list_threads in main.rs).
const THREAD_COLUMNS = ["account_id", "thrid", "date", "subject", "snippet", "senders", "count", "unread", "starred",
  "has_attach", "from_name", "from_addr", "in_inbox", "people"];
const THREAD_FLAGS = new Set(["starred", "has_attach", "in_inbox", "people"]);
async function fetchThreads(params) {
  const rows = await api("/api/threads?" + params);
  return rows.map(r => {
    const t = {};
    THREAD_COLUMNS.forEach((name, i) => { t[name] = THREAD_FLAGS.has(name) ? !!r[i] : r[i]; });
    if (!t.senders) t.senders = t.from_name || t.from_addr;
    return t;
  });
}

/** Fetch the list: all of it, every time. Only the rows in view are ever drawn (see renderRows). */
async function loadThreads() {
  const asked = listParams().toString();
  const rows = await fetchThreads(asked);
  if (asked !== listParams().toString()) return;   // the list has changed since; a newer answer is coming
  state.threads = applyOverrides(rows);
  renderList();
}

/** The strip under the list: what it is showing and how much of it there is. */
function renderListFoot() {
  let foot = $("#list-foot");
  if (!foot) { foot = el("div"); foot.id = "list-foot"; }
  $("#list").append(foot);
  const c = { total: state.threads.length, unread: state.threads.filter(t => t.unread).length }, n = x => x.toLocaleString();
  // what is showing, in words: where, then each filter that is on
  const where = state.filter == null ? (state.all ? "All mail" : "All inboxes") : (account(state.filter)?.email || "Inbox") + (state.all ? ", all mail" : "");
  const what = state.collection ? state.collection.title
    : [where, state.q && `“${state.q}”`, state.unreadOnly && "unread only", state.kind === "people" && "from people",
       state.kind === "automated" && "automated only", state.from && `from ${state.from}`].filter(Boolean).join(" · ");
  // All Mail is listed in the background, newest first; say so while older mail is still coming
  const loading = state.all && !state.collection && state.accounts.some(a => a.indexing && (state.filter == null || a.id === state.filter));
  foot.textContent = "";
  foot.append(el("span", "what", what));
  foot.append(el("span", "n", `${n(c.total)} conversation${c.total === 1 ? "" : "s"}` + (c.unread && !state.unreadOnly ? ` · ${n(c.unread)} unread` : "")
    + (loading ? " · still listing older mail…" : "")));
  if (state.selected.size) foot.append(el("span", "sel", `${n(state.selected.size)} selected`));
}

// The whole list is held in memory, but only the rows in view exist as elements: each sits at
// its own place in a box as tall as the full list, and rows are made and dropped as it scrolls.
let hoverTimer, rowHeight = 0;
const rowNodes = new Map();   // index in state.threads -> the row's element, for rows currently drawn

function renderList() {
  const list = $("#list");
  const top = list.scrollTop;
  list.textContent = "";
  rowNodes.clear();
  if (state.collection) {
    const bar = el("div");
    bar.id = "set-bar";
    const back = el("button", "", "✕");
    back.title = "Back to the inbox";
    back.onclick = () => goInbox();
    bar.append(el("span", "name", state.collection.title), back);
    list.append(bar);
  }
  state.cursor = Math.max(0, Math.min(state.cursor, state.threads.length - 1));
  if (!state.threads.length) {
    list.append(el("div", "empty",
      !state.accounts.length ? "Add a Gmail account to get started."
        : state.q || state.unreadOnly || state.kind || state.from ? `Nothing in ${state.all ? "all mail" : "the inbox"} matches.`
        : state.accounts.some(a => !a.last_sync) ? "Fetching your inbox…" : "Inbox zero."));
    renderListFoot();
    return;
  }
  const box = el("div");
  box.id = "rows";
  list.append(box);
  // every row is the same height; measure one rather than assume (it follows the font and zoom)
  const probe = rowNode(0);
  box.append(probe);
  rowHeight = probe.offsetHeight || rowHeight || 70;
  probe.remove();
  box.style.height = state.threads.length * rowHeight + "px";
  box.style.setProperty("--row-h", rowHeight + "px");
  renderListFoot();
  list.scrollTop = top;
  renderRows();
}

/** Draw the rows that are in view (and a few either side), and drop the ones that no longer are. */
function renderRows() {
  const list = $("#list"), box = $("#rows");
  if (!box || !rowHeight) return;
  const y = list.scrollTop - box.offsetTop;
  // a screenful or so spare each way, so ordinary scrolling never outruns the drawing
  const spare = Math.ceil(list.clientHeight / rowHeight) + 10;
  const first = Math.max(0, Math.floor(y / rowHeight) - spare);
  const last = Math.min(state.threads.length, Math.ceil((y + list.clientHeight) / rowHeight) + spare);
  for (const [i, node] of rowNodes) if (i < first || i >= last) { node.remove(); rowNodes.delete(i); }
  for (let i = first; i < last; i++) {
    if (rowNodes.has(i)) continue;
    const node = rowNode(i);
    node.style.top = i * rowHeight + "px";
    rowNodes.set(i, node);
    box.append(node);
  }
}

function rowNode(i) {
  const t = state.threads[i];
  const row = el("div", "row-t" + (t.unread ? " unread" : "") + (i === state.cursor ? " cursor" : "")
    + (state.selected.has(key(t)) ? " sel" : "") + (state.open?.key === key(t) ? " open" : ""));
  const av = avatar(t.from_name, t.from_addr);
  const dot = el("span", "who-dot");
  dot.style.background = account(t.account_id)?.color || "gray";
  av.append(dot);
  av.title = account(t.account_id)?.email || "";
  const who = el("span", "who", t.senders || "(unknown)");
  if (t.people) {
    const p = el("span", "person");
    p.append(icon("person"));
    p.title = "A person wrote this";
    who.prepend(p);
  }
  if (t.count > 1) who.append(el("span", "c", t.count));
  const text = el("span", "text");
  text.append(el("span", "subj", t.subject), el("span", "snip", t.snippet));
  const marks = el("span", "marks");
  if (t.has_attach) marks.append("📎");
  if (t.starred) marks.append(el("span", "star", "★"));
  // in All Mail most things are archived, so there it is the inbox that gets the label
  if (state.all && !state.collection) { if (t.in_inbox) marks.append(el("span", "gone", "Inbox")); }
  else if (t.in_inbox === false) marks.append(el("span", "gone", "Archived"));
  const date = el("span", "date");
  date.append(el("span", "d", fmtDate(t.date)));
  row.append(el("span", "bar"), av, who, text, marks, date);
  row.onclick = e => {
    if (e.ctrlKey || e.metaKey) return openInWindow(t);
    setCursor(i);
    show(t);
  };
  // Start rendering as soon as the pointer rests on a row, so a click opens it instantly.
  row.onmouseenter = () => { clearTimeout(hoverTimer); hoverTimer = setTimeout(() => ensureView(t), 70); };
  row.onmouseleave = () => clearTimeout(hoverTimer);
  return row;
}

function setCursor(i) {
  const n = state.threads.length;
  state.cursor = Math.max(0, Math.min(n - 1, i));
  // bring the row into view, clear of the bars that stay put at the top and bottom of the list
  const list = $("#list"), box = $("#rows");
  if (box) {
    const top = box.offsetTop + state.cursor * rowHeight;
    const above = $("#set-bar")?.offsetHeight || 0, below = $("#list-foot")?.offsetHeight || 0;
    if (top < list.scrollTop + above) list.scrollTop = top - above;
    else if (top + rowHeight > list.scrollTop + list.clientHeight - below) list.scrollTop = top + rowHeight - list.clientHeight + below;
    renderRows();
  }
  for (const [r, node] of rowNodes) node.classList.toggle("cursor", r === state.cursor);
}

let showTimer;
function moveCursor(delta) {
  if (!state.threads.length) return;
  // With nothing open yet, the first j opens the row the cursor is already on.
  const stay = !state.open && delta > 0;
  setCursor(state.cursor + (stay ? 0 : delta));
  const t = state.threads[state.cursor];
  if (!t) return;
  // Holding j should not render every conversation it passes.
  clearTimeout(showTimer);
  showTimer = setTimeout(() => show(t), state.views.has(key(t)) ? 0 : 60);
  // ...and get the one after it ready too.
  const next = state.threads[state.cursor + (delta < 0 ? -1 : 1)];
  if (next) setTimeout(() => ensureView(next), 150);
}

// Keep the drawn rows up with the scroll position, in the same frame as the scroll (waiting
// for the next frame is what left a blank pane behind a dragged scrollbar).
$("#list").addEventListener("scroll", renderRows, { passive: true });
new ResizeObserver(renderRows).observe($("#list"));

// ---- rendered conversations ----------------------------------------------------
// A view is a conversation laid out in the pane, with its message frames loaded. Views are
// made ahead of time (hover, cursor) and kept for a while, so showing one is just a class change.
function ensureView(t) {
  const k = key(t);
  let v = state.views.get(k);
  if (v) {
    state.views.delete(k);       // move to the fresh end
    state.views.set(k, v);
    return v;
  }
  v = { key: k, account_id: t.account_id, thrid: t.thrid, messages: [], focus: 0, placed: 0, node: el("div", "view") };
  v.node.append(el("h1", "", t.subject), el("div", "messages"));
  v.node.addEventListener("scroll", () => {
    if (state.open !== v) return;
    clearTimeout(placeTimer);
    placeTimer = setTimeout(() => { placeTimer = 0; notePlace(v); }, 250);
  }, { passive: true });
  $("#views").append(v.node);
  state.views.set(k, v);
  for (const old of state.views.values()) {
    if (state.views.size <= KEEP_VIEWS + 1) break;
    if (old !== state.open && old !== v) dropView(old);
  }
  v.ready = loadView(v, true).catch(() => {});
  // Give the frames a moment to load and lay out, then stop tracking window resizes.
  v.ready.then(() => setTimeout(() => v.node.classList.add("parked"), 1500));
  return v;
}

function dropView(v) {
  v.node.remove();
  state.views.delete(v.key);
  if (state.open === v) hideView();
}

/** Read the conversation and bring the view's messages up to date. `peek` answers from the
 *  local cache only; without it the app also re-checks Gmail for messages it has not seen. */
async function loadView(v, peek) {
  const data = await api(`/api/thread/${v.account_id}/${v.thrid}${peek ? "?peek=true" : ""}`);
  if (state.views.get(v.key) !== v) return;
  const first = !v.messages.length;
  const before = new Map(v.messages.map(m => [m.id, m]));
  const held = v.messages[v.focus]?.id;
  v.messages = data.messages;
  // older messages turning up are added above: stay on the same message, not the same position
  const again = v.messages.findIndex(m => m.id === held);
  if (again >= 0) v.focus = again;
  // a conversation opens at its first unread message, or at the latest one when all are read
  if (first) { const unread = v.messages.findIndex(m => !m.seen); v.focus = unread >= 0 ? unread : Math.max(0, v.messages.length - 1); }
  const box = v.node.querySelector(".messages");
  // Keep frames that are already showing; only add or fill in what changed.
  v.messages.forEach((m, i) => {
    const old = before.get(m.id);
    const node = box.querySelector(`[data-id="${m.id}"]`);
    if (node && old && old.has_body === m.has_body && old.attachments.length === m.attachments.length) return;
    const open = node ? !node.classList.contains("closed") : (!m.seen || i === v.messages.length - 1);
    const fresh = renderMessage(v, m, open);
    if (node) node.replaceWith(fresh);
    else box.insertBefore(fresh, box.children[i] || null);
  });
  if (v.messages[0]) v.node.querySelector("h1").textContent = v.messages[0].subject.replace(/^(re|fwd?):\s*/i, "") || v.messages[0].subject;
  if (SOLO) document.title = v.node.querySelector("h1").textContent + " – Unibox";
  if (state.open === v) updateThreadBar();
}

// ---- where you were in a conversation --------------------------------------
// Scroll somewhere in a conversation and it opens there next time, also after a restart.
// The place is kept as a message and a distance into it, so it holds whatever the messages
// above it do. A new message arriving, or scrolling back to where the conversation opens
// by itself, forgets it.
const KEEP_PLACES = 300;
let placeTimer = 0;
function places() { try { return JSON.parse(store.get("places")) || {}; } catch { return {}; } }

function notePlace(v) {
  if (state.open !== v || !v.messages.length) return;
  const all = places(), pane = v.node.getBoundingClientRect().top;
  if (Math.abs(v.node.scrollTop - v.placed) <= 2) {
    // put there by the app: either the remembered place itself, or the usual opening position
    if (v.restored || !all[v.key]) return;
    delete all[v.key];
  } else {
    const node = [...v.node.querySelectorAll(".msg")].find(n => n.getBoundingClientRect().bottom > pane + 1);
    if (!node) return;
    all[v.key] = { id: +node.dataset.id, off: Math.round(pane - node.getBoundingClientRect().top), last: v.messages.at(-1).id, at: Date.now() };
    for (const k of Object.keys(all).sort((a, b) => all[b].at - all[a].at).slice(KEEP_PLACES)) delete all[k];
  }
  store.set("places", JSON.stringify(all));
}

/** Note the place now if a scroll is still waiting to be noted (the conversation is about to go). */
function flushPlace() {
  if (!placeTimer) return;
  clearTimeout(placeTimer);
  placeTimer = 0;
  if (state.open) notePlace(state.open);
}

/** Scroll the open conversation back to its remembered place; false if it has none. */
function restorePlace(v) {
  const p = places()[v.key];
  if (!p || p.last !== v.messages.at(-1)?.id) return false;
  const node = v.node.querySelector(`.msg[data-id="${p.id}"]`), i = v.messages.findIndex(m => m.id === p.id);
  if (!node || i < 0) return false;
  if (node.classList.contains("closed") && p.off > node.offsetHeight) { node.classList.remove("closed"); fill(node, v.messages[i]); updateThreadBar(); }
  v.focus = i;
  v.node.scrollTop += node.getBoundingClientRect().top - v.node.getBoundingClientRect().top + p.off;
  // a message still loading may be too short to reach the place yet; that is not the reader moving
  v.placed = v.node.scrollTop;
  v.restored = true;
  return true;
}

let readTimer;
function show(t) {
  clearTimeout(showTimer);
  flushPlace();
  if (state.unreadOnly) state.kept.add(key(t));
  const v = ensureView(t);
  if (state.open !== v) {
    state.open?.node.classList.remove("on");
    v.node.classList.add("on");
    state.open = v;
    document.body.classList.add("reading");
    updateThreadBar();
    markOpenRow();
    v.ready.then(() => {
      if (state.open !== v) return;
      // where the reader left it, or else at its latest message
      const place = () => restorePlace(v) || focusMessage(v.focus, false);
      place();
      // the message is still being laid out and grows as it loads; keep it in place meanwhile,
      // unless the reader has scrolled somewhere themselves
      let at = v.node.scrollTop;
      for (const ms of [120, 350, 800, 1500, 2500]) setTimeout(() => {
        if (state.open !== v || Math.abs(v.node.scrollTop - at) > 2) return;
        place();
        at = v.node.scrollTop;
      }, ms);
      loadView(v, false).catch(() => {});   // now also ask Gmail whether there is more
    });
  }
  // Paging past a conversation should not mark it read; resting on it does.
  clearTimeout(readTimer);
  if (t.unread) readTimer = setTimeout(() => { if (state.open === v) act("read", [t], { record: false }); }, READ_AFTER_MS);
}

function hideView() {
  flushPlace();
  clearTimeout(readTimer);
  state.open?.node.classList.remove("on");
  state.open = null;
  document.body.classList.remove("reading");
  markOpenRow();
}

function markOpenRow() {
  for (const [i, node] of rowNodes) node.classList.toggle("open", state.open?.key === key(state.threads[i]));
}

function renderMessage(v, m, open) {
  const node = el("article", "msg" + (open ? "" : " closed"));
  node.dataset.id = m.id;
  const head = el("header");
  // the name may be cut short to fit; the badge after it never is
  const from = el("div", "from");
  const name = el("span", "name", m.from_name || m.from_addr || "(unknown)");
  if (m.from_name) name.append(el("span", "addr", "  " + m.from_addr));
  from.append(name);
  // Gmail's verdict on whether the sender is who the From line says.
  const group = m.facts?.group, shown = senderTrust(m);
  const tr = shown && shown.level !== "none" ? shown : null;
  if (tr) {
    const badge = el("button", "trust " + tr.level);
    badge.append(icon(tr.level));
    badge.title = tr.summary + " Click for details.";
    badge.onclick = e => { e.stopPropagation(); senderDetails(m); };
    from.append(badge);
  }
  // Second line: who it went to when open, a preview of the text when collapsed.
  // The date shares the sender's line, so the two sit on one baseline.
  const top = el("div", "top");
  const more = el("button", "more");
  more.append(icon("more"));
  more.title = "View as plain text, or see the original message";
  more.onclick = e => { e.stopPropagation(); messageMenu(more, node, m); };
  top.append(from, el("span", "when", fmtDate(m.date, true)), more);
  const who = el("div", "who");
  const to = el("div", "to", "to " + [m.to_addrs, m.cc_addrs].filter(Boolean).join(", "));
  if (group) to.append(el("span", "via", " · via the group " + (group.address || "it was sent to")));
  // (a group that rewrites the sender points replies back at the person, which is as it should be)
  if (m.trust?.reply_to && m.trust.reply_to !== group?.sender_addr)
    to.append(el("span", "replies" + (tr?.level === "verified" ? "" : " warn"), " · replies go to " + m.trust.reply_to));
  const un = unsubscribeButton(m);
  if (un) to.append(" · ", un);
  who.append(top, to, el("div", "meta", m.snippet));
  head.append(avatar(m.from_name, m.from_addr, "big"), who);
  head.onclick = () => { node.classList.toggle("closed"); if (!node.classList.contains("closed")) fill(node, m); updateThreadBar(); };
  node.append(head);
  // Say it in words when something is wrong; a small icon is too easy to miss.
  const problems = [];
  if (tr?.level === "failed") problems.push(tr.summary);
  if (m.trust?.name_address && !group?.rewritten) problems.push(`The sender's name shows ${m.trust.name_address}, but this was sent from ${m.from_addr}.`);
  if (problems.length) node.append(el("div", "caution", problems.join(" ")));
  node.append(el("div", "content"));
  if (open) fill(node, m);
  return node;
}

/** A message's alternative views. Most mail carries both a formatted and a plain-text version. */
async function messageMenu(anchor, node, m) {
  document.querySelector(".menu")?.remove();
  const menu = el("div", "menu");
  const current = node.dataset.view || "html";
  const parts = m.has_body ? await api(`/api/message/${m.id}/parts`).catch(() => null) : null;
  const item = (view, label, available) => {
    const b = el("button", current === view ? "on" : "", label);
    b.disabled = !available;
    if (!available) b.title = "This message doesn't include one";
    b.onclick = () => { menu.remove(); setMessageView(node, m, view); };
    menu.append(b);
  };
  item("html", "Formatted", !!parts);
  item("text", "Plain text", !!parts?.text);
  item("source", "Original message", !!parts);
  // not on your own mail: recognised by the account's address, or (for an alias you send as)
  // by Gmail having recorded no check, which only happens to mail sent from the account
  const mine = state.accounts.some(a => a.email.toLowerCase() === m.from_addr.toLowerCase()) || (m.trust && m.trust.level === "none");
  if (!mine) {
    const from = m.from_name || m.from_addr;
    const b = el("button", "sep", `Archive everything from ${from}…`);
    b.title = `Archive every inbox conversation with mail from ${m.from_addr}`;
    b.onclick = () => { menu.remove(); archiveAllFrom(m, from); };
    menu.append(b);
  }
  if (m.has_body && current === "html" && !picturesOn(node, m)) {
    const b = el("button", "sep", "Show pictures");
    b.onclick = () => { menu.remove(); showPictures(node, m); };
    menu.append(b);
  }
  if (m.pictures) {
    const b = el("button", "sep", "Stop showing this sender's pictures");
    b.onclick = () => { menu.remove(); alwaysPictures(m, false); };
    menu.append(b);
  }
  if (m.trust) {
    const b = el("button", "sep", "Sender details…");
    b.onclick = () => { menu.remove(); senderDetails(m); };
    menu.append(b);
  }
  document.body.append(menu);
  const r = anchor.getBoundingClientRect();
  menu.style.top = r.bottom + 4 + "px";
  menu.style.left = Math.max(8, r.right - menu.offsetWidth) + "px";
  // Close on a click anywhere else. A click inside a message body lands in its frame and never
  // reaches this page; what the page does see is itself losing focus to the frame.
  const done = () => { menu.remove(); removeEventListener("pointerdown", close, true); removeEventListener("blur", done); removeEventListener("keydown", esc, true); };
  const close = e => { if (!menu.contains(e.target)) done(); };
  const esc = e => { if (e.key === "Escape") { e.stopPropagation(); done(); } };
  addEventListener("pointerdown", close, true);
  addEventListener("blur", done);
  addEventListener("keydown", esc, true);
}

/** Whose verdict the badge shows. A Google Group that puts its own name in the From line is
 *  what Gmail checked; what matters is the check the group made on the person who wrote. */
const senderTrust = m => (m.facts?.group?.rewritten ? m.facts.group.trust : m.trust);

/** A way off a mailing list, when the sender offers one and is who it says it is. */
function unsubscribeButton(m) {
  const u = m.facts?.unsubscribe;
  if (!u || m.facts.group || m.trust?.level === "failed") return null;
  const b = el("button", "unsub", m.unsubscribed ? "Unsubscribed" : "Unsubscribe");
  const who = m.from_name || m.from_addr;
  b.disabled = !!m.unsubscribed;
  b.title = m.unsubscribed ? "You asked this sender to stop."
    : u.one_click ? `Tell ${who} to stop sending these. Nothing opens; the request goes straight to the sender.`
    : u.url ? "Open this sender's unsubscribe page in your browser."
    : "Open a ready-made unsubscribe email in Gmail, for you to send.";
  b.onclick = async e => {
    e.stopPropagation();
    if (u.one_click) {
      b.disabled = true;
      b.textContent = "Unsubscribing…";
      try {
        await api(`/api/message/${m.id}/unsubscribe`, { method: "POST" });
        for (const v of state.views.values()) for (const x of v.messages) if (x.from_addr === m.from_addr) x.unsubscribed = true;
        b.textContent = "Unsubscribed";
        b.title = "You asked this sender to stop.";
        toast(`Unsubscribed from ${who}.`);
      } catch (err) {
        b.disabled = false;
        b.textContent = "Unsubscribe";
        toast("Couldn't unsubscribe: " + err.message);
      }
    } else if (u.url) {
      openExternal(u.url);
      toast("Opened the sender's unsubscribe page in your browser.");
    } else {
      // an address to write to: hand Gmail a filled-in message from the right account
      const mail = new URL(u.mailto), a = account(m.account_id ?? state.open?.account_id);
      const p = new URLSearchParams({ view: "cm", fs: 1, to: decodeURIComponent(mail.pathname),
        su: mail.searchParams.get("subject") || "unsubscribe", body: mail.searchParams.get("body") || "unsubscribe" });
      if (a) p.set("authuser", a.email);
      openExternal("https://mail.google.com/mail/?" + p);
      toast("Opened an unsubscribe email in Gmail. Send it to leave the list.");
    }
  };
  return b;
}

// ---- the app's colour ------------------------------------------------------------
// One colour, picked by the person, from which the style sheet shades everything (--pick).
// It is kept for next time, shared with other open windows, and the window's own icon and
// the page icon are redrawn in it.
const PURPLE = "#5b2ee8";
const COLOURS = [PURPLE, "#3b4fe0", "#1a73e8", "#0b8a8f", "#1e8e3e", "#c77700", "#e8590c", "#d93025", "#d6338a", "#55585e"];
const pickedColour = () => (/^#[0-9a-f]{6}$/i.test(store.get("accent") || "") ? store.get("accent").toLowerCase() : PURPLE);
function applyColour(hex) {
  document.documentElement.style.setProperty("--pick", hex);
  // the shade the light theme actually uses, as #rrggbb, for the icons
  const c = Object.assign(document.createElement("canvas"), { width: 1, height: 1 }).getContext("2d");
  c.fillStyle = hex;
  c.fillStyle = `oklch(from ${hex} clamp(0.38, l, 0.58) c h)`;   // ignored where unsupported
  c.fillRect(0, 0, 1, 1);
  const shade = "#" + [...c.getImageData(0, 0, 1, 1).data.slice(0, 3)].map(v => v.toString(16).padStart(2, "0")).join("");
  const link = document.querySelector('link[rel="icon"]');
  if (link) link.href = link.href.replace(/fill='%23[0-9a-f]{6}'/i, `fill='%23${shade.slice(1)}'`);
  window.ipc?.postMessage("accent:" + shade);
}
function setColour(hex) {
  store.set("accent", hex);
  applyColour(hex);
  showColour();
}
function showColour() {
  const hex = pickedColour();
  for (const b of $("#swatches").children) b.classList.toggle("on", b.dataset.c === hex);
  $("#colour-any").value = hex;
  if (document.activeElement !== $("#colour-hex")) $("#colour-hex").value = hex;
}
applyColour(pickedColour());
addEventListener("storage", e => { if (e.key === "accent") { applyColour(pickedColour()); showColour(); } });
for (const c of COLOURS) {
  const b = el("button");
  b.dataset.c = c;
  b.style.background = c;
  b.title = c === PURPLE ? "Purple (the original)" : c;
  b.onclick = () => setColour(c);
  $("#swatches").append(b);
}
$("#colour-any").oninput = e => setColour(e.target.value.toLowerCase());
$("#colour-hex").oninput = e => { const v = e.target.value.trim().toLowerCase(); if (/^#?[0-9a-f]{6}$/.test(v)) setColour(v.startsWith("#") ? v : "#" + v); };
$("#colour-reset").onclick = () => setColour(PURPLE);
$("#colour-done").onclick = () => $("#colour-dialog").close();
const openColour = () => { showColour(); $("#colour-dialog").showModal(); };
$("#brand .logo").onclick = openColour;
$("#brand .logo").onkeydown = e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openColour(); } };

// ---- subscriptions -------------------------------------------------------------
/** Every sender whose mail says how to unsubscribe, with how much it sends, and the buttons
 *  to do something about it: unsubscribe, look at its mail, clear it out of the inbox. */
async function openSubscriptions() {
  const box = $("#subs-list"), dialog = $("#subs-dialog");
  box.textContent = "";
  box.append(el("div", "empty", "Looking…"));
  if (!dialog.open) dialog.showModal();
  const p = new URLSearchParams();
  if (state.filter != null) p.set("account", state.filter);
  const rows = await api("/api/subscriptions?" + p).catch(e => (toast(e.message), []));
  box.textContent = "";
  if (!rows.length) box.append(el("div", "empty", "No senders with an unsubscribe link yet."));
  const n = x => x ? x.toLocaleString() : "";
  // A table: who, and how much they send. The row itself opens the sender's mail; the two
  // things that change anything sit quietly at the end of it.
  const head = el("div", "sub head");
  head.append(el("span"), el("span", "", "Sender"), el("span", "num", "Last 3 months"), el("span", "num", "In all"),
    el("span", "num", "In inbox"), el("span"), el("span"));
  if (rows.length) box.append(head);
  for (const r of rows) {
    const row = el("div", "sub" + (r.unsubscribed ? " done" : ""));
    row.tabIndex = 0;
    row.title = "Show this sender's mail";
    const av = avatar(r.from_name, r.from_addr);
    if (state.accounts.length > 1) {
      const dot = el("span", "who-dot");
      dot.style.background = account(r.account_id)?.color || "gray";
      av.append(dot);
      av.title = account(r.account_id)?.email || "";
    }
    const who = el("div", "who");
    who.append(el("div", "name", r.from_name || r.from_addr), el("div", "addr", r.from_addr));
    const inbox = el("span", "num", n(r.in_inbox));
    const m = { id: r.id, account_id: r.account_id, from_name: r.from_name, from_addr: r.from_addr, unsubscribed: r.unsubscribed, facts: { unsubscribe: r.unsubscribe } };
    const clear = el("span", "clear"), leave = el("span", "leave");
    if (r.in_inbox) {
      const b = el("button", "quiet");
      b.append(icon("archive"));
      b.title = `Archive the ${n(r.in_inbox)} in the inbox from this sender`;
      b.onclick = async e => {
        e.stopPropagation();
        if (!await archiveAllFrom(m, r.from_name || r.from_addr)) return;   // said no, or nothing was there
        b.remove();
        inbox.textContent = "";
      };
      clear.append(b);
    }
    const unsub = unsubscribeButton(m);
    if (unsub) { unsub.addEventListener("click", () => row.classList.add("done")); leave.append(unsub); }
    const show = () => {
      dialog.close();
      leaveCollection();
      state.all = true; state.unreadOnly = false; state.kind = ""; state.filter = r.account_id;
      state.q = $("#search").value = "";
      state.from = r.from_addr.toLowerCase();
      state.cursor = 0; state.selected.clear();
      showListFilters(); renderAccounts(); hideView(); loadThreads(true);
    };
    row.onclick = show;
    row.onkeydown = e => { if (e.key === "Enter" && e.target === row) show(); };
    row.append(av, who, el("span", "num", n(r.recent)), el("span", "num", n(r.total)), inbox, clear, leave);
    box.append(row);
  }
}
$("#subs").onclick = () => { $("#subs").blur(); openSubscriptions(); };
$("#subs-close").onclick = () => $("#subs-dialog").close();

// ---- pictures ------------------------------------------------------------------
// Fetching a picture from the sender's server tells them the message was opened, and when and
// roughly where. Senders Gmail verified get that; from anyone else nothing is fetched until
// asked, once for a message or always for a sender.
const picturesOn = (node, m) => m.pictures || node.dataset.pictures === "1" || !["unverified", "failed"].includes(senderTrust(m)?.level);
function showPictures(node, m) {
  node.dataset.pictures = "1";
  node.querySelector(".content").textContent = "";
  fill(node, m);
}
function alwaysPictures(m, allow) {
  api("/api/pictures", { body: { sender: m.from_addr, allow } }).catch(e => toast(e.message));
  for (const v of state.views.values()) {
    v.messages.forEach((x, i) => {
      if (x.from_addr !== m.from_addr) return;
      x.pictures = allow;
      const node = v.node.querySelector(`.msg[data-id="${x.id}"]`);
      if (!node) return;
      delete node.dataset.pictures;
      node.querySelector(".content").textContent = "";
      if (!node.classList.contains("closed")) fill(node, x);
    });
  }
  toast(allow ? `Pictures from ${m.from_addr} will always be shown.` : `Pictures from ${m.from_addr} are hidden again.`);
}

// ---- events and bookings ---------------------------------------------------------
/** The moment an invitation's time means. Its time zone is by name, which the browser knows. */
function eventMoment(w) {
  const p = w.value.match(/^(\d{4})(\d\d)(\d\d)(?:T(\d\d)(\d\d))?/);
  if (!p) return null;
  const [y, mo, d, h, mi] = [+p[1], p[2] - 1, +p[3], +(p[4] || 0), +(p[5] || 0)];
  if (w.all_day || !w.tz) return new Date(y, mo, d, h, mi);
  const wall = Date.UTC(y, mo, d, h, mi);
  if (w.tz === "UTC") return new Date(wall);
  try {
    const f = new Intl.DateTimeFormat("en-US", { timeZone: w.tz, hourCycle: "h23", year: "numeric", month: "numeric",
      day: "numeric", hour: "numeric", minute: "numeric" });
    const there = t => { const q = Object.fromEntries(f.formatToParts(new Date(t)).map(x => [x.type, +x.value])); return Date.UTC(q.year, q.month - 1, q.day, q.hour, q.minute); };
    let t = wall - (there(wall) - wall);
    t = wall - (there(t) - t);      // once more, in case that crossed a clock change
    return new Date(t);
  } catch { return new Date(y, mo, d, h, mi); }   // a zone name the browser doesn't know: read it as local
}

function eventWhen(e) {
  const start = eventMoment(e.start);
  if (!start) return "";
  let end = e.end && eventMoment(e.end);
  const day = { weekday: "short", month: "short", day: "numeric", year: "numeric" }, time = { hour: "numeric", minute: "2-digit" };
  if (e.start.all_day) {
    if (end) end = new Date(end.getFullYear(), end.getMonth(), end.getDate() - 1);   // the end day is not included
    const a = start.toLocaleDateString([], day);
    return end && end > start ? `${a} – ${end.toLocaleDateString([], day)}` : `${a}, all day`;
  }
  const a = `${start.toLocaleDateString([], day)}, ${start.toLocaleTimeString([], time)}`;
  if (!end || end <= start) return a;
  return start.toDateString() === end.toDateString() ? `${a} – ${end.toLocaleTimeString([], time)}`
    : `${a} – ${end.toLocaleDateString([], day)}, ${end.toLocaleTimeString([], time)}`;
}

function cardRows(card, rows) {
  const table = el("table");
  for (const [k, v] of rows) {
    if (!v) continue;
    const r = el("tr");
    r.append(el("td", "", k), el("td", "", v));
    table.append(r);
  }
  card.append(table);
}

function eventCard(e) {
  const card = el("div", "card event" + (e.method === "cancel" ? " cancelled" : ""));
  card.append(el("div", "kind", { request: "Invitation", cancel: "Cancelled", reply: "Reply to an invitation" }[e.method] || "Event"),
    el("div", "title", e.summary || "(no title)"));
  const name = p => p.name || p.email;
  const mark = { accepted: " ✓", declined: " ✗", tentative: " ?" };
  const going = e.attendees.slice(0, 8).map(p => name(p) + (mark[p.status] || "")).join(", ")
    + (e.attendees.length > 8 ? `, and ${e.attendees.length - 8} more` : "");
  const zone = e.start.tz && e.start.tz !== "UTC" && !e.start.all_day ? ` (your time; set in ${e.start.tz})` : "";
  cardRows(card, [["When", eventWhen(e) + (e.recurs ? ", repeating" : "") + zone], ["Where", e.location],
    ["Organiser", name(e.organizer)], ["Invited", going]]);
  if (e.method === "request") {
    const b = el("button", "", "Respond in Gmail");
    b.title = "Open this conversation in Gmail, where you can accept or decline";
    b.onclick = () => replyInGmail();
    card.append(b);
  }
  return card;
}

/** A date the sender wrote, shown as the clock there reads: a flight leaves at local time. */
function wallTime(v) {
  const p = v.match(/^(\d{4})-(\d\d)-(\d\d)(?:T(\d\d):(\d\d))?/);
  if (!p) return v;
  const d = new Date(Date.UTC(+p[1], p[2] - 1, +p[3], +(p[4] || 0), +(p[5] || 0)));
  const o = { timeZone: "UTC", weekday: "short", month: "short", day: "numeric", year: "numeric" };
  return p[4] ? d.toLocaleString([], { ...o, hour: "numeric", minute: "2-digit" }) : d.toLocaleDateString([], o);
}

function bookingCard(c) {
  const card = el("div", "card");
  if (c.title) card.append(el("div", "title", c.title));
  cardRows(card, c.rows.map(r => [r.label, r.time ? wallTime(r.value) : r.value]));
  for (const a of c.actions) {
    const b = el("button", "", a.label);
    b.append(el("span", "host", host(a.url)));
    b.title = a.url;
    b.onclick = () => openExternal(a.url);
    card.append(b);
  }
  return card;
}

/** Everything known about whether a message's sender is genuine. */
function senderDetails(m) {
  const group = m.facts?.group;
  const t = senderTrust(m) || { level: "none", dkim: [], raw: "" }, d = $("#trust-dialog");
  const words = { pass: "Passed", fail: "Failed", softfail: "Failed (soft)", none: "Not set up by the sender", neutral: "Inconclusive",
    temperror: "Couldn't be checked", permerror: "Sender's record is broken", policy: "Rejected by policy" };
  const say = r => words[r] || r || "No result";
  const body = d.querySelector(".body");
  body.textContent = "";
  const none = t.level === "none";
  body.append(el("p", "verdict " + t.level, none
    ? "Gmail recorded no check for this message. That is normal for mail you sent yourself."
    : t.summary));
  const table = el("table");
  const row = (k, v, cls) => { const r = el("tr"); r.append(el("td", "", k), el("td", cls || "", v)); table.append(r); };
  row("From", m.from_name ? `${m.from_name} <${m.from_addr}>` : m.from_addr);
  if (m.facts?.why) row("Written by", `${m.facts.automated ? "A system, probably" : "A person, probably"}: it ${m.facts.why}.`);
  if (group) row("Sent through", `the Google Group ${group.address}` + (group.rewritten ? ". The checks below are the ones the group made on the sender." : ""));
  if (t.reply_to) row("Replies go to", t.reply_to, "warn");
  if (t.name_address) row("Name claims", t.name_address, "warn");
  if (!none) {
    const policy = { none: "the domain asks for failing mail to be delivered anyway", quarantine: "the domain asks for failing mail to be treated as spam",
      reject: "the domain asks for failing mail to be refused" }[t.dmarc_policy];
    row("DMARC", say(t.dmarc) + (t.dmarc ? (t.dmarc === "fail" && policy ? `; ${policy}` : "") : " (the domain publishes no policy)"),
      t.dmarc === "fail" && !t.forwarded ? "bad" : "");
    if (t.forwarded) row("Forwarded", "Yes. It passed these checks where it first arrived.");
    for (const k of t.dkim) row("DKIM signature", `${say(k.result)}, signed by ${k.domain || "?"}`, k.result === "fail" ? "bad" : "");
    if (!t.dkim.length) row("DKIM signature", "None");
    row("SPF", say(t.spf) + (t.spf_domain ? `, for ${t.spf_domain}` : ""), /fail/.test(t.spf) ? "bad" : "");
  }
  body.append(table);
  if (!none) {
    body.append(el("p", "note", "These are Gmail's own checks, recorded when the message arrived. A pass means the message "
      + `really came from ${t.domain || "that domain"}; it says nothing about whether that domain is one you should trust, so read it carefully.`));
    const raw = el("details");
    raw.append(el("summary", "", group?.rewritten ? "The group's record" : "Gmail's record"), el("pre", "", t.raw.replace(/;\s*/g, ";\n  ")));
    body.append(raw);
  }
  d.showModal();
}

// Where a link really goes, shown along the bottom of the reading pane while pointing at it
// (email code never runs, so the page itself cannot fake this). A link whose text is a web
// address for somewhere else is called out: that is the classic phishing trick.
const host = u => { try { return new URL(u).hostname.replace(/^www\./, ""); } catch { return ""; } };
function linkStatus(a) {
  const bar = $("#link-status");
  if (!a) { bar.hidden = true; return; }
  const text = a.textContent.trim();
  const looksLikeUrl = /^(https?:\/\/)?[\w-]+(\.[\w-]+)+(\/\S*)?$/i.test(text);
  const shown = looksLikeUrl ? host(/^https?:/i.test(text) ? text : "https://" + text) : "";
  const real = /^https?:/i.test(a.href) ? host(a.href) : "";
  const lie = shown && real && shown !== real && !real.endsWith("." + shown) && !shown.endsWith("." + real);
  bar.textContent = lie ? `Says ${shown} but goes to ${a.href}` : a.href.replace(/^mailto:/i, "Email ");
  bar.classList.toggle("lie", !!lie);
  bar.hidden = false;
}

function setMessageView(node, m, view) {
  node.dataset.view = view;
  node.querySelector(".content").textContent = "";
  fill(node, m);
}

function fill(node, m) {
  const content = node.querySelector(".content");
  if (content.childElementCount) return;
  if (!m.has_body) { content.append(el("div", "pending", "Loading message…")); return; }
  const f = el("iframe", "body");
  // Same-origin so its height can be measured, but scripts stay off: email code never runs.
  f.setAttribute("sandbox", "allow-same-origin allow-popups allow-popups-to-escape-sandbox");
  // Set the frame up as soon as its text is there. Waiting for "load" would leave a message
  // squeezed into a short box for as long as one slow remote image took to arrive.
  let ready = false;
  const setup = () => {
    const doc = f.contentDocument;
    if (ready || !doc || !doc.body || doc.location.href === "about:blank" || doc.readyState === "loading") return;
    ready = true;
    clearInterval(poll);
    const root = doc.documentElement;
    // Mail laid out for a fixed width can be a little wider than the pane. Shrink it to fit
    // rather than cut it off; if it would need to shrink a lot, it scrolls sideways instead.
    let fittedFor = 0;
    const fit = () => {
      if (root.clientWidth && root.clientWidth !== fittedFor) {
        fittedFor = root.clientWidth;
        doc.body.style.zoom = "";
        const scale = root.clientWidth / root.scrollWidth;
        if (scale < 1 && scale > 0.6) doc.body.style.zoom = scale;
      }
      f.style.height = Math.ceil(root.getBoundingClientRect().height) + "px";
    };
    fit();
    new ResizeObserver(fit).observe(root);
    root.classList.toggle("dk", dark.matches);
    // An image that failed to load (usually a blocked tracker) leaves a broken-picture icon; drop it.
    for (const img of doc.images) {
      if (img.complete && !img.naturalWidth) img.style.display = "none";
      else img.addEventListener("error", () => (img.style.display = "none"));
    }
    doc.addEventListener("click", e => {
      const a = e.target.closest?.("a[href]");
      if (!a) return;
      e.preventDefault();
      if (/^(https?:|mailto:)/i.test(a.href)) openExternal(a.href);
    });
    doc.addEventListener("mouseover", e => linkStatus(e.target.closest?.("a[href]")));
    root.addEventListener("mouseleave", () => linkStatus(null));
    // Keyboard shortcuts keep working after clicking inside a message.
    doc.addEventListener("keydown", e => onKey(e));
  };
  const poll = setInterval(() => (f.isConnected ? setup() : clearInterval(poll)), 40);
  f.addEventListener("load", setup);
  const view = node.dataset.view || "html";
  const pictures = picturesOn(node, m);
  f.src = `/api/message/${m.id}/html?v=${BUILD}` + (view === "html" ? (pictures ? "&pictures=true" : "") : "&view=" + view);
  if (view === "html") {
    if (m.facts?.remote_images && !pictures) {
      const bar = el("div", "picbar");
      const once = el("button", "", "Show"), always = el("button", "", "Always from " + (m.from_name || m.from_addr));
      once.onclick = () => showPictures(node, m);
      always.onclick = () => alwaysPictures(m, true);
      always.title = `Always show pictures in mail from ${m.from_addr}`;
      bar.append(el("span", "", "Pictures hidden: Gmail couldn't verify this sender."), once, always);
      content.append(bar);
    }
    // what the message says about itself in a form a program can read, unless it may be forged
    if (senderTrust(m)?.level !== "failed") {
      if (m.facts?.event) content.append(eventCard(m.facts.event));
      for (const c of m.facts?.cards || []) content.append(bookingCard(c));
    }
  }
  content.append(f);
  if (m.attachments.length) {
    const files = el("div", "files");
    m.attachments.forEach((a, i) => {
      // Click to look at it here; the arrow saves it.
      const chip = el("div", "file");
      chip.title = `${a.name} (${a.mime})`;
      if (a.kind === "image") {
        const thumb = el("img", "thumb");
        thumb.loading = "lazy";
        thumb.alt = "";
        thumb.src = attachmentUrl(m, a) + "/preview";
        chip.append(thumb);
      }
      const dl = el("button", "dl");
      dl.append(icon("save"));
      dl.title = "Save to Downloads";
      dl.onclick = e => { e.stopPropagation(); saveAttachment(m, a); };
      chip.append(el("span", "fn", (a.kind === "image" ? "" : "📎 ") + a.name), el("span", "sz", fmtSize(a.size)), dl);
      chip.onclick = () => openPreview(m, i);
      files.append(chip);
    });
    content.append(files);
  }
}

const attachmentUrl = (m, a) => `/api/message/${m.id}/attachment/${a.index}`;

/** Saved straight into Downloads and shown there; a plain browser download is the fallback. */
function saveAttachment(m, a) {
  api(attachmentUrl(m, a) + "/save", { method: "POST" })
    .then(r => toast("Saved to " + r.path))
    .catch(() => {
      const link = el("a");
      link.href = attachmentUrl(m, a);
      link.download = a.name;
      link.click();
    });
}

// ---- attachment preview -------------------------------------------------------
const preview = { m: null, i: 0 };
function openPreview(m, i) {
  preview.m = m;
  preview.i = i;
  const a = m.attachments[i], box = $("#preview"), stage = box.querySelector(".stage");
  box.hidden = false;
  box.querySelector(".name").textContent = a.name;
  box.querySelector(".count").textContent =
    fmtSize(a.size) + (m.attachments.length > 1 ? ` · ${i + 1} of ${m.attachments.length}` : "");
  box.querySelector('[data-pv="prev"]').disabled = i === 0;
  box.querySelector('[data-pv="next"]').disabled = i === m.attachments.length - 1;
  stage.textContent = "";
  const url = attachmentUrl(m, a) + "/preview";
  if (a.kind === "image") {
    const img = el("img");
    img.alt = a.name;
    img.src = url;
    stage.append(img);
  } else if (a.kind === "pdf") {
    const f = el("iframe");
    f.src = url;
    stage.append(f);
  } else if (a.kind === "text") {
    const pre = el("pre", "", "Loading…");
    stage.append(pre);
    fetch(url).then(r => r.text()).then(t => (pre.textContent = t)).catch(() => (pre.textContent = "Couldn't load this file."));
  } else {
    stage.append(el("div", "none", "No preview for this kind of file. Use Save to put it in Downloads."));
  }
}
function closePreview() {
  $("#preview").hidden = true;
  $("#preview .stage").textContent = "";
  preview.m = null;
}
function stepPreview(d) {
  const n = preview.i + d;
  if (preview.m && n >= 0 && n < preview.m.attachments.length) openPreview(preview.m, n);
}
$("#preview").addEventListener("click", e => {
  const act = e.target.dataset?.pv;
  if (act === "close" || e.target.classList.contains("stage")) closePreview();
  if (act === "prev") stepPreview(-1);
  if (act === "next") stepPreview(1);
  if (act === "save") saveAttachment(preview.m, preview.m.attachments[preview.i]);
});

dark.addEventListener("change", () => {
  for (const f of document.querySelectorAll("iframe.body")) f.contentDocument?.documentElement.classList.toggle("dk", dark.matches);
});

function focusMessage(i, scroll = true) {
  const v = state.open;
  if (!v || !v.messages.length) return;
  v.focus = Math.max(0, Math.min(v.messages.length - 1, i));
  const nodes = v.node.querySelector(".messages").children;
  // the outline marks where n/p has taken you; it isn't drawn just for opening a conversation
  for (let n = 0; n < nodes.length; n++) nodes[n].classList.toggle("focus", scroll && n === v.focus && nodes.length > 1);
  const node = nodes[v.focus];
  if (!node) return;
  if (node.classList.contains("closed")) { node.classList.remove("closed"); fill(node, v.messages[v.focus]); updateThreadBar(); }
  // put it exactly where the first message of a conversation sits: below the room the subject takes
  // (a subject long enough to wrap pushes the first message down; go by a one-line subject)
  const h1 = v.node.querySelector("h1");
  const wrapped = Math.max(0, h1.offsetHeight - parseFloat(getComputedStyle(h1).lineHeight));
  const top = v.focus === 0 ? 0 : node.getBoundingClientRect().top - nodes[0].getBoundingClientRect().top + wrapped;
  // A short message at the end leaves too little below it to scroll that far, so room is added
  // under the conversation: as much as this needs, and never less than the current scroll
  // position is already using (taking that away would make the pane jump).
  const box = v.node.querySelector(".messages");
  const had = +box.dataset.room || 0;
  const end = box.getBoundingClientRect().bottom - v.node.getBoundingClientRect().top + v.node.scrollTop - had;
  const room = Math.max(0, Math.ceil(Math.max(top, v.node.scrollTop) + v.node.clientHeight - end));
  if (room !== had) {
    box.dataset.room = room;
    box.style.paddingBottom = room ? `calc(60px + ${room}px)` : "";
  }
  v.placed = top;
  v.restored = false;
  v.node.scrollTo({ top, behavior: scroll ? "smooth" : "auto" });
  // a message opened by this step is still growing while the pane glides; land exactly afterwards
  clearTimeout(focusMessage.settle);
  const at = v.focus;
  if (scroll) focusMessage.settle = setTimeout(() => { if (state.open === v && v.focus === at) { focusMessage(at, false); node.classList.toggle("focus", nodes.length > 1); } }, 500);
}

function updateThreadBar() {
  // a conversation that is not in the inbox can be put back there instead of archived
  const listed = state.open && state.threads.find(t => key(t) === state.open.key);
  const move = $("#thread-bar [data-act=archive], #thread-bar [data-act=inbox]"), out = listed?.in_inbox === false;
  move.dataset.act = out ? "inbox" : "archive";
  move.textContent = out ? "Move to inbox" : "Archive";
  move.title = out ? "Put this conversation back in the inbox" : "Archive (e)";
  const starred = state.open?.messages.some(m => m.starred);
  $('#thread-bar [data-act="star"]').textContent = starred ? "★ Starred" : "Star";
  // one button opens every message, or closes them again once all are open
  const b = $('#thread-bar [data-act="expand"]'), nodes = [...(state.open?.node.querySelectorAll(".msg") || [])];
  const allOpen = nodes.every(n => !n.classList.contains("closed"));
  b.hidden = nodes.length < 2;
  b.textContent = allOpen ? "Collapse all" : "Expand all";
  b.title = allOpen ? "Close every message but the last (:)" : "Open every message in this conversation (;)";
}

/** Open every message of the conversation on screen, or close all but the last. */
function expandAll(open = [...state.open?.node.querySelectorAll(".msg") || []].some(n => n.classList.contains("closed"))) {
  const v = state.open;
  if (!v) return;
  const nodes = [...v.node.querySelectorAll(".msg")];
  nodes.forEach((n, i) => {
    n.classList.toggle("closed", !open && i < nodes.length - 1);
    if (!n.classList.contains("closed")) fill(n, v.messages[i]);
  });
  updateThreadBar();
}

function openInWindow(t = state.open || state.threads[state.cursor]) {
  if (!t) return;
  api("/api/window", { body: { account_id: t.account_id, thrid: t.thrid } })
    // in a plain browser (development) a popup stands in for the app window
    .catch(() => window.open(`/?thread=${t.account_id}:${t.thrid}`, "_blank", "popup,width=920,height=860"));
}

function closeWindow() {
  window.ipc ? window.ipc.postMessage("close") : window.close();
}

function replyInGmail() {
  const o = state.open || state.threads[state.cursor];
  const a = o && account(o.account_id);
  if (!a) return;
  openExternal(`https://mail.google.com/mail/?authuser=${encodeURIComponent(a.email)}#all/${BigInt(o.thrid).toString(16)}`);
}

// ---- actions ---------------------------------------------------------------
// A change shows at once, but a list refresh already on its way back from the server can still
// carry the old state and would undo it on screen. Each change is remembered here until the
// server has certainly caught up, and every list that arrives is corrected against it.
const overrides = new Map();   // thread key -> { archive, unread, starred, until }
function remember(k, change) {
  overrides.set(k, { ...overrides.get(k), ...change, until: Infinity });
}
function settled(k) {
  const o = overrides.get(k);
  if (o) o.until = Date.now() + 3000;
}
function applyOverrides(rows) {
  const now = Date.now();
  for (const [k, o] of overrides) if (o.until < now) overrides.delete(k);
  if (!overrides.size) return rows;
  // an archived conversation leaves the inbox list; in All Mail it stays, no longer marked Inbox
  return rows.filter(t => state.all || !overrides.get(key(t))?.archive).map(t => {
    const o = overrides.get(key(t));
    if (o?.archive != null) t.in_inbox = !o.archive;
    if (o?.unread != null) t.unread = o.unread;
    if (o?.starred != null) t.starred = o.starred;
    return t;
  });
}

// Which conversations an action applies to: the selection, else the open one, else the cursor row.
function targets() {
  if (state.selected.size) return state.threads.filter(t => state.selected.has(key(t)));
  if (state.open) return [state.open];
  const t = state.threads[state.cursor];
  return t ? [t] : [];
}

/** The last thing done, so z can take it back: { op, items: [{ t, index, sent }] }. */
let lastAction = null;

/** `record: false` is for changes the user did not make by hand (an undo, or marking a
 *  conversation read because it was opened), which must not become the thing z undoes. */
function act(op, list = targets(), { record = true } = {}) {
  // archiving what is already archived, or moving to the inbox what is already there, is nothing
  const inInbox = t => (state.threads.find(x => key(x) === key(t)) || t).in_inbox !== false;
  if (op === "archive") list = list.filter(inInbox);
  if (op === "inbox") list = list.filter(t => !inInbox(t));
  if (!list.length) return;
  const undoing = !record;
  if (record) lastAction = { op, items: [] };
  // Reflect it immediately; the server confirms (or the next sync corrects) afterwards.
  for (const t of list) {
    const k = key(t);
    const index = state.threads.findIndex(x => key(x) === k);
    const row = state.threads[index];
    remember(k, { archive: op === "archive" ? true : op === "inbox" ? false : undefined,
      unread: op === "read" ? 0 : op === "unread" ? 1 : undefined,
      starred: op === "star" ? true : op === "unstar" ? false : undefined });
    if (op === "archive" && state.all) {
      if (row) row.in_inbox = false;
    } else if (op === "archive") {
      state.threads = state.threads.filter(x => key(x) !== k);
      const v = state.views.get(k);
      if (v) dropView(v);
    } else if (row) {
      if (op === "inbox") row.in_inbox = true;
      if (op === "read") row.unread = 0;
      if (op === "unread") row.unread = 1;
      if (op === "star") row.starred = true;
      if (op === "unstar") row.starred = false;
    }
    // the server answers with the messages it changed, which is what an undo needs
    const sent = api(`/api/thread/${t.account_id}/${t.thrid}/action`, { body: { op } })
      .catch(e => { overrides.delete(k); refreshSoon(); toast("That didn't go through: " + e.message); return null; })
      .finally(() => settled(k));
    if (!undoing) lastAction.items.push({ t: row || t, index, sent });
  }
  state.selected.clear();
  renderList();
  updateThreadBar();
}

const INVERSE = { read: "unread", unread: "read", star: "unstar", unstar: "star", inbox: "archive" };
async function undo() {
  const last = lastAction;
  if (!last) return toast("Nothing to undo.");
  lastAction = null;
  if (last.op !== "archive") {
    act(INVERSE[last.op], last.items.map(i => i.t), { record: false });
    return toast("Undone.");
  }
  // Put the rows back where they were straight away, then tell the server.
  for (const { t, index } of [...last.items].sort((a, b) => a.index - b.index)) {
    overrides.delete(key(t));
    if (!state.threads.some(x => key(x) === key(t))) state.threads.splice(Math.min(Math.max(index, 0), state.threads.length), 0, t);
    t.in_inbox = true;
  }
  renderList();
  updateThreadBar();
  const first = last.items[0];
  if (first) { setCursor(state.threads.findIndex(x => key(x) === key(first.t))); show(first.t); }
  toast(last.items.length > 1 ? `${last.items.length} conversations back in the inbox.` : "Back in the inbox.");
  for (const { t, sent } of last.items) {
    const done = await sent;   // the archive request itself has to have landed first
    if (!done?.msgids?.length) continue;
    api("/api/unarchive", { body: { account_id: t.account_id, msgids: done.msgids } })
      .catch(e => { toast("Couldn't undo: " + e.message); refreshSoon(); });
  }
}

/** After archiving: move on to the conversation that took its place in the list. */
function leaveConversation() {
  if (SOLO) return closeWindow();
  const next = state.threads[state.cursor];
  if (next) show(next);
  else hideView();
  $("#list").focus();
}

function archive() {
  act("archive");
  if (!state.all) leaveConversation();   // in All Mail the conversation is still listed: stay on it
}

/** Clear out everything in the inbox from one sender. One z brings it all back. */
async function archiveAllFrom(m, from) {
  const rows = await api(`/api/message/${m.id}/same-source`).catch(e => (toast(e.message), []));
  if (!rows.length) return false;
  if (rows.length > 1 && !confirm(`Archive ${rows.length} conversations from ${from} (${m.from_addr})?`)) return false;
  const wasOpen = rows.some(t => key(t) === state.open?.key);
  act("archive", rows);
  if (wasOpen) leaveConversation();
  toast(rows.length > 1 ? `Archived ${rows.length} conversations from ${from}.` : "Archived.");
  return true;
}

function markUnread() {
  clearTimeout(readTimer);
  act("unread");
  if (SOLO) closeWindow();
}

function toggleStar() {
  const list = targets();
  if (!list.length) return;
  const on = list.every(t => state.threads.find(x => key(x) === key(t))?.starred);
  for (const t of list) {
    const v = state.views.get(key(t));
    v?.messages.forEach((m, i, all) => (m.starred = !on && i === all.length - 1));
  }
  act(on ? "unstar" : "star", list);
  updateThreadBar();
}

$("#thread-bar").addEventListener("click", e => {
  const a = e.target.dataset?.act;
  if (a === "archive") archive();
  if (a === "inbox") act("inbox");
  if (a === "unread") markUnread();
  if (a === "star") toggleStar();
  if (a === "expand") expandAll();
  if (a === "reply") replyInGmail();
  if (a === "window") openInWindow();
});

// ---- list width ------------------------------------------------------------
const LIST_DEFAULT = 430, LIST_MIN = 260;
function setListWidth(px, save) {
  // leave the reading pane at least as much room as the narrowest list
  const max = Math.max(LIST_MIN, $("#work").clientWidth - 360);
  const w = Math.round(Math.max(LIST_MIN, Math.min(max, px)));
  document.documentElement.style.setProperty("--list-w", w + "px");
  if (save) store.set("list-width", w);
}
{
  const d = $("#divider");
  d.addEventListener("pointerdown", e => {
    e.preventDefault();
    try { d.setPointerCapture(e.pointerId); } catch {}   // keeps the drag alive over the message frames
    d.classList.add("dragging");
    const left = $("#list").getBoundingClientRect().left;
    const move = ev => setListWidth(ev.clientX - left, false);
    const up = ev => {
      setListWidth(ev.clientX - left, true);
      d.classList.remove("dragging");
      d.removeEventListener("pointermove", move);
      d.removeEventListener("pointerup", up);
      d.removeEventListener("pointercancel", up);
    };
    d.addEventListener("pointermove", move);
    d.addEventListener("pointerup", up);
    d.addEventListener("pointercancel", up);
  });
  d.addEventListener("dblclick", () => setListWidth(LIST_DEFAULT, true));
  // a window made narrower must not leave the list wider than it allows
  addEventListener("resize", () => setListWidth($("#list").getBoundingClientRect().width, false));
}

// ---- keyboard --------------------------------------------------------------
let gPending = 0, starPending = 0;

/** g i: back to the plain inbox, out of any search or account filter. */
function goInbox() {
  $("#search").value = "";
  state.q = "";
  state.unreadOnly = state.all = false; state.kind = state.from = "";
  state.kept = new Set();
  showListFilters();
  setFilter(null);   // also leaves any set the assistant was showing
}

function selectAll(on) {
  state.selected = new Set(on ? state.threads.map(key) : []);
  renderList();
  if (on) toast(`${state.selected.size} conversations selected. Esc to clear.`);
}
// Space, PageUp/PageDown, Home and End always move the open conversation, wherever focus is,
// so j/k can pick one and the same hand can read it.
function scrollKeys(e) {
  const k = e.key;
  if (![" ", "PageDown", "PageUp", "Home", "End"].includes(k)) return false;
  const v = document.querySelector("body.reading .view.on");
  if (!v) return true;   // nothing open: still keep these keys off the list
  const page = Math.max(80, v.clientHeight - 60);
  if (k === "Home") v.scrollTop = 0;
  else if (k === "End") v.scrollTop = v.scrollHeight;
  else v.scrollTop += (k === "PageUp" || (k === " " && e.shiftKey)) ? -page : page;
  return true;
}
function onKey(e) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k" && !SOLO) { toggleAssistant(true); return e.preventDefault(); }
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  if (preview.m) {
    // while an attachment is up, the keyboard belongs to it
    if (e.key === "Escape") closePreview();
    else if (e.key === "ArrowLeft") stepPreview(-1);
    else if (e.key === "ArrowRight") stepPreview(1);
    else return;
    return e.preventDefault();
  }
  const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName);
  if (document.querySelector("dialog[open]")) return;
  if (typing) {
    if (e.key === "Escape") { document.activeElement.blur(); $("#list").focus(); }
    return;
  }
  const k = e.key;
  if (scrollKeys(e)) return e.preventDefault();
  if (SOLO) {
    // One conversation, no list: only the keys that make sense here.
    const solo = { n: () => focusMessage(state.open.focus + 1), p: () => focusMessage(state.open.focus - 1),
      e: archive, s: toggleStar, U: markUnread, ";": () => expandAll(true), ":": () => expandAll(false), r: replyInGmail, Escape: closeWindow, u: closeWindow,
      "?": () => $("#help-dialog").showModal() }[k];
    if (solo && state.open) { solo(); e.preventDefault(); }
    return;
  }
  if (k === "Enter" && e.shiftKey) { openInWindow(); return e.preventDefault(); }
  if (gPending && Date.now() - gPending < 1500 && k === "i") { gPending = 0; goInbox(); return e.preventDefault(); }
  if (gPending && Date.now() - gPending < 1500 && k === "s") { gPending = 0; openSubscriptions(); return e.preventDefault(); }
  if (gPending && Date.now() - gPending < 1500 && k === "a") { gPending = 0; if (!state.all) setListFilter("all", true); return e.preventDefault(); }
  gPending = 0;
  // * then a selects every conversation in the list, * then n none (as in Gmail)
  if (starPending && Date.now() - starPending < 1500 && (k === "a" || k === "n")) {
    starPending = 0;
    selectAll(k === "a");
    return e.preventDefault();
  }
  starPending = 0;
  switch (k) {
    case "j": case "ArrowDown": moveCursor(1); break;
    case "k": case "ArrowUp": moveCursor(-1); break;
    case "o": case "Enter": if (state.threads[state.cursor]) show(state.threads[state.cursor]); break;
    case "u": case "Escape":
      if (state.selected.size) { state.selected.clear(); renderList(); }
      break;
    case "n": if (state.open) focusMessage(state.open.focus + 1); break;
    case "p": if (state.open) focusMessage(state.open.focus - 1); break;
    case "e": archive(); break;
    case ";": expandAll(true); break;
    case ":": expandAll(false); break;
    case "s": toggleStar(); break;
    case "z": undo(); break;
    case "I": act("read"); break;
    case "U": markUnread(); break;
    case "x": {
      const t = state.threads[state.cursor];
      if (!t) break;
      state.selected.has(key(t)) ? state.selected.delete(key(t)) : state.selected.add(key(t));
      renderList();
      break;
    }
    case "r": replyInGmail(); break;
    case "f": setListFilter("unreadOnly", !state.unreadOnly); break;
    case "F": setListFilter("kind", KINDS[(KINDS.indexOf(state.kind) + 1) % KINDS.length]); break;
    case "/": $("#search").focus(); $("#search").select(); break;
    case "?": $("#help-dialog").showModal(); break;
    case "g": gPending = Date.now(); break;
    case "*": starPending = Date.now(); break;
    default:
      if (/^[0-9]$/.test(k)) {
        const a = k === "0" ? null : state.accounts[+k - 1];
        if (k === "0" || a) setFilter(a ? a.id : null);
        break;
      }
      return;
  }
  e.preventDefault();
}
document.addEventListener("keydown", onKey);

// ---- the search field -------------------------------------------------------------
let searchTimer;
function searchNow() {
  clearTimeout(searchTimer);
  leaveCollection();
  // a filter still being typed ("from:eco") is not something to search the text for
  state.q = $("#search").value.replace(/(^|\s)(is|in|from):\S*$/i, "").trim();
  state.cursor = 0;
  state.kept = new Set();
  hideView();
  loadThreads(true);
}
/** Read the field after it changed: filters typed in full become pieces, the rest is searched for. */
function readSearch(done) {
  const field = $("#search"), took = takeFilters(field.value, done);
  if (took.changed) { field.value = took.text; showListFilters(); }
  clearTimeout(searchTimer);
  if (took.changed || done) searchNow();
  else searchTimer = setTimeout(searchNow, 200);
}

// Suggestions while a filter is being typed: the fixed ones, and after "from:" the senders
// that match. Up and Down choose, Tab or Enter takes one, Esc puts the list away.
const FILTER_WORDS = [["is:unread", "Only unread"], ["from:people", "Only what people wrote"],
  ["from:automated", "Only newsletters, receipts, notices"], ["in:all", "Look in all mail"], ["in:inbox", "Look in the inbox"]];
const suggest = { items: [], at: 0, asked: 0 };
function closeSuggest() { suggest.items = []; $("#suggest").hidden = true; }
function drawSuggest() {
  const box = $("#suggest");
  box.textContent = "";
  box.hidden = !suggest.items.length;
  let senders = false;
  suggest.items.forEach((it, i) => {
    if (it.sender && !senders) { senders = true; box.append(el("div", "group", "Senders")); }
    const row = el("div", "opt" + (i === suggest.at ? " at" : ""));
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", i === suggest.at);
    const cut = it.word.indexOf(":") + 1, word = el("span", "word");
    word.append(el("b", "", it.word.slice(0, cut)), it.word.slice(cut));
    row.append(word, el("span", "hint", it.hint));
    row.onpointerdown = e => { e.preventDefault(); takeSuggestion(i); };
    box.append(row);
  });
  if (suggest.items.length) box.append(el("div", "keys", "↑↓ choose   Tab take   Esc close"));
}
async function updateSuggest() {
  const field = $("#search");
  // only for the word at the end of the field, and only once it names a kind of filter
  const word = field.selectionStart === field.value.length ? (/(?:^|\s)(\S+)$/.exec(field.value)?.[1] || "") : "";
  const lower = word.toLowerCase();
  if (!/^(is|in|from):/.test(lower)) return closeSuggest();
  const fixed = FILTER_WORDS.filter(([w]) => w.startsWith(lower) && w !== lower).map(([w, hint]) => ({ word: w, hint }));
  // Until the senders for what is typed now arrive, keep the ones already showing that still
  // fit, so the list narrows in place as letters are added and never blinks shut.
  const typed = lower.slice(5);
  const still = lower.startsWith("from:")
    ? suggest.items.filter(it => it.sender && (it.word.slice(5).includes(typed) || it.name.includes(typed))) : [];
  const held = suggest.items[suggest.at]?.word;
  suggest.items = [...fixed, ...still];
  suggest.at = Math.max(0, suggest.items.findIndex(it => it.word === held));
  drawSuggest();
  if (!lower.startsWith("from:")) return;
  const asked = ++suggest.asked;
  const found = await api("/api/senders?" + new URLSearchParams({ q: word.slice(5) })).catch(() => []);
  if (asked !== suggest.asked || $("#search").value !== field.value || document.activeElement !== field) return;
  const chosen = suggest.items[suggest.at]?.word;
  suggest.items = [...fixed, ...found.map(f => ({ word: "from:" + f.addr, name: (f.name || "").toLowerCase(), hint: f.name && f.name.toLowerCase() !== f.addr ? f.name : `${f.n.toLocaleString()} message${f.n === 1 ? "" : "s"}`, sender: true }))];
  suggest.at = Math.max(0, suggest.items.findIndex(it => it.word === chosen));
  drawSuggest();
}
function takeSuggestion(i) {
  const it = suggest.items[i], field = $("#search");
  if (!it) return;
  field.value = field.value.replace(/\S+$/, it.word);
  closeSuggest();
  readSearch(true);
  field.focus();
}
$("#search").addEventListener("input", () => { readSearch(false); updateSuggest(); });
$("#search").addEventListener("blur", closeSuggest);
$("#search").addEventListener("keydown", e => {
  const field = e.target, open = suggest.items.length > 0;
  if (open && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    suggest.at = (suggest.at + (e.key === "ArrowDown" ? 1 : suggest.items.length - 1)) % suggest.items.length;
    drawSuggest();
    e.preventDefault();
  } else if (open && (e.key === "Tab" || e.key === "Enter")) {
    takeSuggestion(suggest.at);
    e.preventDefault();
  } else if (open && e.key === "Escape") {
    closeSuggest();
    e.preventDefault();
    e.stopPropagation();
  } else if (e.key === "Enter") {
    readSearch(true);
    field.blur();
    $("#list").focus();
  } else if (e.key === "Backspace" && field.selectionStart === 0 && field.selectionEnd === 0) {
    // at the very start: take off the piece before the cursor
    if (state.from) setListFilter("from", "");
    else if (state.kind) setListFilter("kind", "");
    else if (state.unreadOnly) setListFilter("unreadOnly", false);
    else return;
    field.focus();
    e.preventDefault();
  }
});
$("#query").addEventListener("pointerdown", e => { if (e.target === e.currentTarget || e.target.id === "tokens") { e.preventDefault(); $("#search").focus(); } });

// ---- toolbar and dialogs -------------------------------------------------------
// The sync button says what the background sync is doing: working, or counting down to the next check.
const sync = { syncing: true, next: 0, every: 120 };
function renderSync() {
  // background work, said quietly under the sync time while it lasts
  const doing = $("#activity");
  if (doing.textContent !== (sync.activity || "")) { doing.textContent = sync.activity || ""; doing.hidden = !sync.activity; }
  const left = Math.max(0, sync.next - Date.now() / 1000);
  // a countdown that has run out means the next pass is about to start
  const busy = sync.syncing || left === 0;
  const b = $("#sync");
  b.classList.toggle("busy", busy);
  // Every account has an open connection that Gmail pushes changes down: no countdown to
  // show, the button just glows.
  const live = !!sync.live && !busy;
  b.classList.toggle("live", live);
  // Otherwise the border fills as the wait runs down; while syncing it is empty and the arrow spins.
  b.style.setProperty("--p", busy || live ? 0 : Math.min(1, 1 - (left - 1) / sync.every));
  const s = Math.ceil(left);
  b.title = busy ? "Syncing: checking Gmail for new mail and changes"
    : live ? "Connected to Gmail: new mail shows up as it arrives. Click to check now."
    : `Next automatic check in ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}. Click to check now.`;
}
function setSync(s) { Object.assign(sync, s); renderSync(); }
setInterval(renderSync, 1000);
$("#sync").onclick = () => { setSync({ syncing: true }); api("/api/sync", { method: "POST" }); };
$("#scope").onclick = e => { e.currentTarget.blur(); setListFilter("all", !state.all); };
// people or automated: one of them, or neither
$("#kind").onclick = e => {
  const b = e.target.closest("button");
  if (!b) return;
  b.blur();
  setListFilter("kind", state.kind === b.dataset.kind ? "" : b.dataset.kind);
};
$("#unread-only").onclick = e => { e.currentTarget.blur(); setListFilter("unreadOnly", !state.unreadOnly); };
$("#help").onclick = () => $("#help-dialog").showModal();
// The same dialog adds an account or, given one, replaces its saved app password.
function openAccountDialog(acct) {
  const form = $("#add-form");
  form.reset();
  form.dataset.change = acct ? acct.email : "";
  form.email.value = acct ? acct.email : "";
  form.email.readOnly = !!acct;
  $("#add-title").textContent = acct ? "Change app password" : "Add a Gmail account";
  $("#add-submit").textContent = acct ? "Save password" : "Add account";
  $("#add-error").textContent = "";
  $("#add-dialog").showModal();
  if (acct) form.password.focus();
}
$("#add").onclick = () => { $("#add").blur(); openAccountDialog(); };
$("#add-cancel").onclick = () => $("#add-dialog").close();
document.addEventListener("click", e => {
  const a = e.target.closest?.("a[data-external]");
  if (a) { e.preventDefault(); openExternal(a.href); }
});

$("#add-form").addEventListener("submit", async e => {
  e.preventDefault();
  const form = e.target, btn = $("#add-submit");
  btn.disabled = true;
  btn.textContent = "Checking with Gmail…";
  $("#add-error").textContent = "";
  try {
    await api("/api/accounts", { body: { email: form.email.value, password: form.password.value } });
    form.reset();
    $("#add-dialog").close();
    toast(form.dataset.change ? "App password changed. Syncing…" : "Account added. Fetching the inbox…");
    refresh();
  } catch (err) {
    $("#add-error").textContent = /login failed/i.test(err.message)
      ? "Gmail rejected that address and app password. Check both and try again."
      : err.message;
  }
  btn.disabled = false;
  btn.textContent = form.dataset.change ? "Save password" : "Add account";
});

// ---- assistant ---------------------------------------------------------------
const assistant = { data: null, setup: false };

function toggleAssistant(focus) {
  const panel = $("#assistant");
  // Ctrl+K on an open panel just puts the cursor in it; the button and Esc close it
  if (focus && !panel.hidden) return $("#as-input").focus();
  panel.hidden = !panel.hidden;
  $("#ask").classList.toggle("on", !panel.hidden);
  store.set("assistant", panel.hidden ? "closed" : "open");
  if (!panel.hidden) loadAssistant().then(() => focus && $("#as-input").focus());
}
$("#ask").onclick = () => toggleAssistant(true);

async function loadAssistant() {
  if ($("#assistant").hidden) return;
  assistant.data = await api("/api/agent");
  renderAssistant();
}

/** The model writes light Markdown. Build it as elements, never as HTML text. */
function richText(text) {
  const out = document.createDocumentFragment();
  const inline = (node, line) => {
    for (const part of line.split(/(\*\*[^*]+\*\*|`[^`]+`)/)) {
      if (part.startsWith("**") && part.length > 4) node.append(el("b", "", part.slice(2, -2)));
      else if (part.startsWith("`") && part.length > 2) node.append(el("code", "", part.slice(1, -1)));
      else if (part) node.append(part);
    }
    return node;
  };
  let list = null, fence = null;
  for (const line of text.split("\n")) {
    if (line.trim().startsWith("```")) {
      if (fence) { out.append(fence); fence = null; } else { fence = el("pre"); list = null; }
      continue;
    }
    if (fence) { fence.append(line + "\n"); continue; }
    const item = line.match(/^\s*(?:[-*•]|(\d+)[.)])\s+(.*)$/);
    if (item) {
      const tag = item[1] ? "OL" : "UL";
      if (!list || list.tagName !== tag) { list = el(tag.toLowerCase()); out.append(list); }
      list.append(inline(el("li"), item[2]));
    } else if (line.trim()) {
      list = null;
      out.append(inline(el("p"), line.replace(/^#+\s*/, "")));
    } else list = null;
  }
  if (fence) out.append(fence);
  return out;
}

function renderAssistant() {
  const d = assistant.data;
  if (!d) return;
  const log = $("#as-log");
  const atEnd = log.scrollHeight - log.scrollTop - log.clientHeight < 60;
  log.textContent = "";
  const setup = !d.configured || assistant.setup;
  $("#as-setup").hidden = !setup;
  $("#as-form").hidden = setup;
  if (setup) {
    const f = $("#as-setup");
    if (!f.dataset.filled) { f.model.value = d.model; f.base_url.value = d.base_url; f.dataset.filled = "1"; }
    f.api_key.placeholder = d.configured ? "(saved; leave empty to keep it)" : "sk-ant-…";
    f.querySelector('[data-as="cancel-setup"]').hidden = !d.configured;
  }
  if (!d.messages.length && !setup) {
    const hint = el("div", "hint");
    hint.append(el("b", "", "Ask about your mail, or tell it what to do."),
      "“Find all my delivery emails”", el("br"), "“Archive the ones from before this year”", el("br"),
      "“What did they say about the refund?”", el("br"), "“Draft a reply saying Saturday works”");
    log.append(hint);
  }
  for (const m of d.messages) {
    if (m.role === "user") { log.append(el("div", "as-msg user", m.text)); continue; }
    if (m.role === "error") { log.append(el("div", "as-error", m.text)); continue; }
    const node = el("div", "as-msg");
    if (m.did.length) {
      const did = el("div", "as-did");
      for (const line of m.did) did.append(el("span", "", line));
      node.append(did);
    }
    node.append(richText(m.text));
    if (node.childNodes.length) log.append(node);
  }
  if (d.busy) log.append(el("div", "as-status", d.status || "Working…"));
  $("#as-send").textContent = d.busy ? "Stop" : "Send";
  if (atEnd || d.busy) log.scrollTop = log.scrollHeight;
}

/** What the assistant is told about the window with each message, so "this" and "these" mean something. */
function appState() {
  const brief = t => ({ account_id: t.account_id, thread_id: t.thrid, subject: t.subject });
  const now = new Date();
  const open = state.open && state.threads.find(t => key(t) === state.open.key);
  return {
    now: now.toLocaleString("sv") + " " + now.toLocaleDateString("en", { weekday: "long" }),
    accounts: state.accounts.map(a => ({ account_id: a.id, address: a.email })),
    list_is_showing: state.collection ? `the set you showed, titled "${state.collection.title}"`
      : (state.q ? `${state.all ? "all mail" : "inbox"} search results for "${state.q}"` : state.all ? "All Mail (archived and sent mail too)" : "the inbox")
        + (state.unreadOnly ? ", unread only" : "") + (state.kind ? `, only mail from ${state.kind === "people" ? "people" : "automated senders"}` : "")
        + (state.from ? `, only from ${state.from}` : ""),
    account_filter: state.filter == null ? null : account(state.filter)?.email,
    conversations_listed: state.threads.length,
    open: state.open ? brief(open || { ...state.open, subject: state.open.messages[0]?.subject }) : null,
    selected: state.threads.filter(t => state.selected.has(key(t))).slice(0, 100).map(brief),
  };
}

$("#as-form").addEventListener("submit", async e => {
  e.preventDefault();
  if (assistant.data?.busy) return api("/api/agent/stop", { method: "POST" });
  const input = $("#as-input");
  const text = input.value.trim();
  if (!text) return;
  input.value = "";
  input.style.height = "";
  try { await api("/api/agent", { body: { text, state: appState() } }); }
  catch (err) { input.value = text; toast(err.message); }
});
$("#as-input").addEventListener("keydown", e => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); $("#as-form").requestSubmit(); }
  if (e.key === "Escape") { e.stopPropagation(); e.target.blur(); $("#list").focus(); }
});
$("#as-input").addEventListener("input", e => { e.target.style.height = ""; e.target.style.height = Math.min(160, e.target.scrollHeight + 2) + "px"; });

$("#assistant").addEventListener("click", async e => {
  const act = e.target.dataset?.as;
  if (act === "close") toggleAssistant(false);
  if (act === "settings") { assistant.setup = !assistant.setup; renderAssistant(); }
  if (act === "cancel-setup") { assistant.setup = false; renderAssistant(); }
  if (act === "new") {
    if (assistant.data?.messages.length && !confirm("Start a new conversation? The assistant will forget this one.")) return;
    api("/api/agent/reset", { method: "POST" }).catch(err => toast(err.message));
  }
});
$("#as-setup").addEventListener("submit", async e => {
  e.preventDefault();
  const f = e.target;
  $("#as-setup-error").textContent = "";
  try {
    await api("/api/agent/config", { body: { base_url: f.base_url.value, model: f.model.value, api_key: f.api_key.value || null } });
    f.api_key.value = "";
    assistant.setup = false;
    loadAssistant();
  } catch (err) { $("#as-setup-error").textContent = err.message; }
});

/** The assistant picked a set of conversations: put it in the list. */
function showCollection(title) {
  state.collection = { title };
  state.unreadOnly = state.all = false; state.kind = state.from = "";
  showListFilters();
  state.q = "";
  $("#search").value = "";
  state.filter = null;
  state.cursor = 0;
  state.selected.clear();
  hideView();
  renderAccounts();
  loadThreads(true);
}

/** The assistant changed some mail: make it the thing z undoes, exactly as if done by hand. */
function assistantActed({ op, items }) {
  if (!items.length) return;
  lastAction = {
    op,
    items: items.map(i => {
      const index = state.threads.findIndex(t => t.account_id === i.account_id && t.thrid === i.thrid);
      return { t: state.threads[index] || { account_id: i.account_id, thrid: i.thrid }, index, sent: Promise.resolve({ msgids: i.msgids }) };
    }),
  };
  const what = { archive: "archived", read: "marked read", unread: "marked unread", star: "starred", unstar: "unstarred" }[op];
  toast(`Assistant ${what} ${items.length} conversation${items.length > 1 ? "s" : ""}.`);
  if (op === "archive") {
    for (const { t } of lastAction.items) { const v = state.views.get(key(t)); if (v) dropView(v); }
  }
  refreshSoon();
}

// ---- live updates ------------------------------------------------------------
async function refresh() {
  state.accounts = await api("/api/accounts");
  renderAccounts();
  const asked = listParams().toString();
  let rows = await fetchThreads(asked);
  if (asked !== listParams().toString()) return;
  const at = state.open?.key || (state.threads[state.cursor] && key(state.threads[state.cursor]));
  rows = applyOverrides(rows);
  state.threads = rows;
  const again = rows.findIndex(t => key(t) === at);
  if (again >= 0) state.cursor = again;
  renderList();
  // A rendered conversation that has gained messages (or bodies) since is brought up to date.
  for (const v of state.views.values()) {
    const row = rows.find(t => key(t) === v.key);
    if ((row && row.count !== v.messages.length) || v.messages.some(m => !m.has_body)) loadView(v, true).catch(() => {});
  }
}

let refreshTimer, indexedTimer = 0;
// (All Mail is a long list to fetch again: there, changes are gathered up for longer)
function refreshSoon() { clearTimeout(refreshTimer); refreshTimer = setTimeout(() => refresh().catch(() => {}), state.all ? 1500 : 250); }

function listen() {
  const es = new EventSource("/api/events");
  es.onmessage = e => {
    const ev = JSON.parse(e.data);
    if (ev.kind === "error") return toast(ev.detail);
    if (ev.kind === "sync") return setSync(JSON.parse(ev.detail));
    if (ev.kind === "agent") return SOLO ? null : loadAssistant();
    if (ev.kind === "agent-show") return SOLO ? null : showCollection(ev.detail);
    if (ev.kind === "agent-acted") { if (!SOLO) assistantActed(JSON.parse(ev.detail)); return; }
    if (SOLO) { if (state.open) loadView(state.open, true).catch(() => {}); return; }
    if (ev.kind === "indexed") {
      // older mail being listed in the background: only All Mail shows it, and not every chunk
      if (!state.all || indexedTimer) return;
      indexedTimer = setTimeout(() => { indexedTimer = 0; refresh().catch(() => {}); }, 4000);
      return;
    }
    if (ev.kind === "thread") {
      for (const v of state.views.values()) if (v.thrid === ev.detail) loadView(v, true).catch(() => {});
    }
    refreshSoon();
  };
}

(async () => {
  if (SOLO) {
    const [account_id, thrid] = SOLO.split(":");
    document.body.classList.add("solo");
    state.accounts = await api("/api/accounts");
    const t = { account_id: +account_id, thrid, subject: "" };
    show(t);
    await state.open.ready;
    if (state.open.messages.some(m => !m.seen)) act("read", [t], { record: false });
    listen();
    return;
  }
  setListWidth(+store.get("list-width") || LIST_DEFAULT, false);
  showListFilters();
  await refresh();
  listen();
  api("/api/sync").then(setSync).catch(() => {});
  if (store.get("assistant") === "open") toggleAssistant(false);
  $("#list").focus();
  if (!state.accounts.length) openAccountDialog();
  // Have the top of the inbox rendered before it is asked for.
  state.threads.slice(0, 2).forEach(t => ensureView(t));
})();
