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
  more: true,
  cursor: 0,
  selected: new Set(),   // thread keys
  views: new Map(),      // thread key -> rendered conversation, oldest first
  open: null,            // the view on screen
  collection: null,      // { title }: the list is showing a set the assistant picked
  unreadOnly: false,     // list only conversations with something unread
  kept: new Set(),       // thread keys opened while unreadOnly is on; they stay listed once read
};
const key = t => t.account_id + ":" + t.thrid;

// Small icons are drawn rather than typed: a text glyph looks however the font happens to
// draw it (the "more" ellipsis came out as three dashes).
const ICONS = {
  more: '<circle cx="3" cy="8" r="1.5"/><circle cx="8" cy="8" r="1.5"/><circle cx="13" cy="8" r="1.5"/>',
  key: '<circle cx="5" cy="11" r="2.6" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M7 9l6-6M10.5 5.5l2 2M12.6 3.4l1.2 1.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
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
  item(null, "All inboxes", "#77736b", state.accounts.reduce((n, a) => n + a.unread, 0));
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
function listParams(offset) {
  if (state.collection) return new URLSearchParams({ collection: true, offset });
  const p = new URLSearchParams({ q: state.q, offset });
  if (state.filter != null) p.set("account", state.filter);
  if (state.unreadOnly) { p.set("unread", true); p.set("keep", [...state.kept].join(",")); }
  return p;
}

/** Show only conversations with something unread. Ones opened while it is on stay listed. */
function setUnreadOnly(on) {
  leaveCollection();
  state.unreadOnly = on;
  state.kept = new Set();
  state.cursor = 0;
  state.selected.clear();
  $("#unread-only").classList.toggle("on", on);
  $("#unread-only").setAttribute("aria-pressed", on);
  hideView();
  loadThreads(true);
}

/** Stop showing the assistant's set and go back to the ordinary list. */
function leaveCollection() {
  if (!state.collection) return;
  state.collection = null;
  api("/api/agent/collection/clear", { method: "POST" }).catch(() => {});
}

async function loadThreads(reset) {
  const offset = reset ? 0 : state.threads.length;
  const rows = await api("/api/threads?" + listParams(offset));
  state.threads = reset ? applyOverrides(rows) : state.threads.concat(applyOverrides(rows));
  state.more = rows.length === 60;
  renderList();
}

let hoverTimer;
function renderList() {
  const list = $("#list");
  const top = list.scrollTop;
  list.textContent = "";
  if (state.collection) {
    const bar = el("div");
    bar.id = "set-bar";
    const back = el("button", "", "✕");
    back.title = "Back to the inbox";
    back.onclick = () => goInbox();
    bar.append(el("span", "name", `${state.collection.title} · ${state.threads.length}${state.more ? "+" : ""}`), back);
    list.append(bar);
  }
  state.cursor = Math.max(0, Math.min(state.cursor, state.threads.length - 1));
  if (!state.threads.length) {
    list.append(el("div", "empty",
      !state.accounts.length ? "Add a Gmail account to get started."
        : state.unreadOnly ? (state.q ? "Nothing unread matches that search." : "Nothing unread.")
        : state.q ? "Nothing in the inbox matches that search."
        : state.accounts.some(a => !a.last_sync) ? "Fetching your inbox…" : "Inbox zero."));
    return;
  }
  state.threads.forEach((t, i) => {
    const row = el("div", "row-t" + (t.unread ? " unread" : "") + (i === state.cursor ? " cursor" : "")
      + (state.selected.has(key(t)) ? " sel" : "") + (state.open?.key === key(t) ? " open" : ""));
    const av = avatar(t.from_name, t.from_addr);
    const dot = el("span", "who-dot");
    dot.style.background = account(t.account_id)?.color || "gray";
    av.append(dot);
    av.title = account(t.account_id)?.email || "";
    const who = el("span", "who", t.senders || "(unknown)");
    if (t.count > 1) who.append(el("span", "c", t.count));
    const text = el("span", "text");
    text.append(el("span", "subj", t.subject), el("span", "snip", t.snippet));
    const marks = el("span", "marks");
    if (t.has_attach) marks.append("📎");
    if (t.starred) marks.append(el("span", "star", "★"));
    if (t.in_inbox === false) marks.append(el("span", "gone", "Archived"));
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
    list.append(row);
  });
  list.scrollTop = top;
}

function setCursor(i) {
  const n = state.threads.length;
  state.cursor = Math.max(0, Math.min(n - 1, i));
  const rows = $("#list").querySelectorAll(".row-t");
  for (let r = 0; r < rows.length; r++) rows[r].classList.toggle("cursor", r === state.cursor);
  rows[state.cursor]?.scrollIntoView({ block: "nearest" });
  if (state.more && state.cursor > n - 15) loadThreads(false);
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

$("#list").addEventListener("scroll", e => {
  const l = e.target;
  if (state.more && l.scrollTop + l.clientHeight > l.scrollHeight - 600 && !loadThreads.busy) {
    loadThreads.busy = true;
    loadThreads(false).finally(() => (loadThreads.busy = false));
  }
});

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
  v = { key: k, account_id: t.account_id, thrid: t.thrid, messages: [], focus: 0, node: el("div", "view") };
  v.node.append(el("h1", "", t.subject), el("div", "messages"));
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
  v.messages = data.messages;
  if (first) v.focus = Math.max(0, v.messages.findIndex(m => !m.seen));
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

let readTimer;
function show(t) {
  clearTimeout(showTimer);
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
      focusMessage(v.focus, false);
      loadView(v, false).catch(() => {});   // now also ask Gmail whether there is more
    });
  }
  // Paging past a conversation should not mark it read; resting on it does.
  clearTimeout(readTimer);
  if (t.unread) readTimer = setTimeout(() => { if (state.open === v) act("read", [t], { record: false }); }, READ_AFTER_MS);
}

function hideView() {
  clearTimeout(readTimer);
  state.open?.node.classList.remove("on");
  state.open = null;
  document.body.classList.remove("reading");
  markOpenRow();
}

function markOpenRow() {
  const rows = $("#list").querySelectorAll(".row-t");
  state.threads.forEach((t, i) => rows[i]?.classList.toggle("open", state.open?.key === key(t)));
}

function renderMessage(v, m, open) {
  const node = el("article", "msg" + (open ? "" : " closed"));
  node.dataset.id = m.id;
  const head = el("header");
  const from = el("div", "from", m.from_name || m.from_addr || "(unknown)");
  if (m.from_name) from.append(el("span", "addr", "  " + m.from_addr));
  // Second line: who it went to when open, a preview of the text when collapsed.
  // The date shares the sender's line, so the two sit on one baseline.
  const top = el("div", "top");
  const more = el("button", "more");
  more.append(icon("more"));
  more.title = "View as plain text, or see the original message";
  more.onclick = e => { e.stopPropagation(); messageMenu(more, node, m); };
  top.append(from, el("span", "when", fmtDate(m.date, true)), more);
  const who = el("div", "who");
  who.append(top, el("div", "to", "to " + [m.to_addrs, m.cc_addrs].filter(Boolean).join(", ")), el("div", "meta", m.snippet));
  head.append(avatar(m.from_name, m.from_addr, "big"), who);
  head.onclick = () => { node.classList.toggle("closed"); if (!node.classList.contains("closed")) fill(node, m); };
  node.append(head, el("div", "content"));
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
  document.body.append(menu);
  const r = anchor.getBoundingClientRect();
  menu.style.top = r.bottom + 4 + "px";
  menu.style.left = Math.max(8, r.right - menu.offsetWidth) + "px";
  const close = e => { if (!menu.contains(e.target)) { menu.remove(); removeEventListener("pointerdown", close, true); } };
  addEventListener("pointerdown", close, true);
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
    // Keyboard shortcuts keep working after clicking inside a message.
    doc.addEventListener("keydown", e => onKey(e));
  };
  const poll = setInterval(() => (f.isConnected ? setup() : clearInterval(poll)), 40);
  f.addEventListener("load", setup);
  const view = node.dataset.view || "html";
  f.src = `/api/message/${m.id}/html?v=${BUILD}` + (view === "html" ? "" : "&view=" + view);
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
  for (let n = 0; n < nodes.length; n++) nodes[n].classList.toggle("focus", n === v.focus && nodes.length > 1);
  const node = nodes[v.focus];
  if (!node) return;
  if (node.classList.contains("closed")) { node.classList.remove("closed"); fill(node, v.messages[v.focus]); }
  if (v.focus === 0) { if (scroll) v.node.scrollTo({ top: 0, behavior: "smooth" }); }
  else node.scrollIntoView({ block: "start", behavior: scroll ? "smooth" : "auto" });
}

function updateThreadBar() {
  const starred = state.open?.messages.some(m => m.starred);
  $('#thread-bar [data-act="star"]').textContent = starred ? "★ Starred" : "Star";
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
  return rows.filter(t => !overrides.get(key(t))?.archive).map(t => {
    const o = overrides.get(key(t));
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
  if (!list.length) return;
  const undoing = !record;
  if (record) lastAction = { op, items: [] };
  // Reflect it immediately; the server confirms (or the next sync corrects) afterwards.
  for (const t of list) {
    const k = key(t);
    const index = state.threads.findIndex(x => key(x) === k);
    const row = state.threads[index];
    remember(k, { archive: op === "archive" || undefined,
      unread: op === "read" ? 0 : op === "unread" ? 1 : undefined,
      starred: op === "star" ? true : op === "unstar" ? false : undefined });
    if (op === "archive") {
      state.threads = state.threads.filter(x => key(x) !== k);
      const v = state.views.get(k);
      if (v) dropView(v);
    } else if (row) {
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
}

const INVERSE = { read: "unread", unread: "read", star: "unstar", unstar: "star" };
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
  }
  renderList();
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
  const n = targets().length;
  act("archive");
  leaveConversation();
  if (n) toast((n > 1 ? `${n} conversations archived.` : "Archived.") + " Press z to undo.");
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
  if (a === "unread") markUnread();
  if (a === "star") toggleStar();
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
  state.unreadOnly = false;
  state.kept = new Set();
  $("#unread-only").classList.remove("on");
  $("#unread-only").setAttribute("aria-pressed", false);
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
      e: archive, s: toggleStar, U: markUnread, r: replyInGmail, Escape: closeWindow, u: closeWindow,
      "?": () => $("#help-dialog").showModal() }[k];
    if (solo && state.open) { solo(); e.preventDefault(); }
    return;
  }
  if (k === "Enter" && e.shiftKey) { openInWindow(); return e.preventDefault(); }
  if (gPending && Date.now() - gPending < 1500 && k === "i") { gPending = 0; goInbox(); return e.preventDefault(); }
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
    case "f": setUnreadOnly(!state.unreadOnly); break;
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

let searchTimer;
$("#search").addEventListener("input", e => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { leaveCollection(); state.q = e.target.value.trim(); state.cursor = 0; hideView(); loadThreads(true); }, 200);
});
$("#search").addEventListener("keydown", e => { if (e.key === "Enter") { e.target.blur(); $("#list").focus(); } });

// ---- toolbar and dialogs -------------------------------------------------------
// The sync button says what the background sync is doing: working, or counting down to the next check.
const sync = { syncing: true, next: 0, every: 120 };
function renderSync() {
  const left = Math.max(0, sync.next - Date.now() / 1000);
  // a countdown that has run out means the next pass is about to start
  const busy = sync.syncing || left === 0;
  const b = $("#sync");
  b.classList.toggle("busy", busy);
  // The border fills as the wait runs down; while syncing it is empty and the arrow spins.
  b.style.setProperty("--p", busy ? 0 : Math.min(1, 1 - (left - 1) / sync.every));
  const s = Math.ceil(left);
  b.title = busy ? "Syncing: checking Gmail for new mail and changes"
    : `Next automatic check in ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}. Click to check now.`;
}
function setSync(s) { Object.assign(sync, s); renderSync(); }
setInterval(renderSync, 1000);
$("#sync").onclick = () => { setSync({ syncing: true }); api("/api/sync", { method: "POST" }); };
$("#unread-only").onclick = e => { e.currentTarget.blur(); setUnreadOnly(!state.unreadOnly); };
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
  if (d.error) log.append(el("div", "as-error", d.error));
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
      : state.q ? `inbox search results for "${state.q}"` : "the inbox",
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
  state.unreadOnly = false;
  $("#unread-only").classList.remove("on");
  $("#unread-only").setAttribute("aria-pressed", false);
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
  toast(`Assistant ${what} ${items.length} conversation${items.length > 1 ? "s" : ""}. Press z to undo.`);
  if (op === "archive") {
    for (const { t } of lastAction.items) { const v = state.views.get(key(t)); if (v) dropView(v); }
  }
  refreshSoon();
}

// ---- live updates ------------------------------------------------------------
async function refresh() {
  state.accounts = await api("/api/accounts");
  renderAccounts();
  // Re-read as many rows as are on screen, so paging and the cursor survive a refresh.
  const want = Math.max(60, state.threads.length);
  let rows = [], page;
  do {
    page = await api("/api/threads?" + listParams(rows.length));
    rows = rows.concat(page);
  } while (page.length === 60 && rows.length < want);
  const at = state.open?.key || (state.threads[state.cursor] && key(state.threads[state.cursor]));
  rows = applyOverrides(rows);
  state.threads = rows;
  state.more = page.length === 60;
  const again = rows.findIndex(t => key(t) === at);
  if (again >= 0) state.cursor = again;
  renderList();
  // A rendered conversation that has gained messages (or bodies) since is brought up to date.
  for (const v of state.views.values()) {
    const row = rows.find(t => key(t) === v.key);
    if ((row && row.count !== v.messages.length) || v.messages.some(m => !m.has_body)) loadView(v, true).catch(() => {});
  }
}

let refreshTimer;
function refreshSoon() { clearTimeout(refreshTimer); refreshTimer = setTimeout(() => refresh().catch(() => {}), 250); }

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
  await refresh();
  listen();
  api("/api/sync").then(setSync).catch(() => {});
  if (store.get("assistant") === "open") toggleAssistant(false);
  $("#list").focus();
  if (!state.accounts.length) openAccountDialog();
  // Have the top of the inbox rendered before it is asked for.
  state.threads.slice(0, 2).forEach(t => ensureView(t));
})();
