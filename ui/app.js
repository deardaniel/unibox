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
  split: store.get("layout") !== "list",
};
const key = t => t.account_id + ":" + t.thrid;
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
      b.append(rm);
    }
    b.onclick = () => { setFilter(id); b.blur(); };
    nav.append(b);
  };
  item(null, "All inboxes", "#77736b", state.accounts.reduce((n, a) => n + a.unread, 0));
  for (const a of state.accounts) item(a.id, a.email, a.color, a.unread, a);

  const errs = state.accounts.filter(a => a.last_error);
  const last = Math.max(0, ...state.accounts.map(a => a.last_sync || 0));
  $("#status").textContent = errs.length ? `Can't sync ${errs.map(a => a.email).join(", ")}`
    : last ? "Synced " + fmtDate(last) : state.accounts.length ? "Syncing…" : "";
}

async function removeAccount(a) {
  if (!confirm(`Remove ${a.email} from Unibox?\n\nThis deletes the copy of its mail on this PC and the saved app password. Nothing changes in Gmail.`)) return;
  await api("/api/accounts/" + a.id, { method: "DELETE" });
  if (state.filter === a.id) state.filter = null;
  hideView();
  refresh();
}

function setFilter(id) {
  state.filter = id;
  state.cursor = 0;
  state.selected.clear();
  hideView();
  loadThreads(true);
  renderAccounts();
  $("#list").focus();
}

// ---- conversation list -------------------------------------------------------
async function loadThreads(reset) {
  const offset = reset ? 0 : state.threads.length;
  const p = new URLSearchParams({ q: state.q, offset });
  if (state.filter != null) p.set("account", state.filter);
  const rows = await api("/api/threads?" + p);
  state.threads = reset ? rows : state.threads.concat(rows);
  state.more = rows.length === 60;
  renderList();
}

let hoverTimer;
function renderList() {
  const list = $("#list");
  const top = list.scrollTop;
  list.textContent = "";
  state.cursor = Math.max(0, Math.min(state.cursor, state.threads.length - 1));
  if (!state.threads.length) {
    list.append(el("div", "empty",
      !state.accounts.length ? "Add a Gmail account to get started."
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
  const rows = $("#list").children;
  for (let r = 0; r < rows.length; r++) rows[r].classList.toggle("cursor", r === state.cursor);
  rows[state.cursor]?.scrollIntoView({ block: "nearest" });
  if (state.more && state.cursor > n - 15) loadThreads(false);
}

let showTimer;
function moveCursor(delta) {
  if (!state.threads.length) return;
  // The first j in the reading pane opens the row the cursor is already on.
  const stay = state.split && !state.open && delta > 0;
  setCursor(state.cursor + (stay ? 0 : delta));
  const t = state.threads[state.cursor];
  if (!t) return;
  if (state.split) {
    // Holding j should not render every conversation it passes.
    clearTimeout(showTimer);
    showTimer = setTimeout(() => show(t), state.views.has(key(t)) ? 0 : 60);
  } else {
    ensureView(t);
  }
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
  // Paging past a conversation in the reading pane should not mark it read; resting on it does.
  clearTimeout(readTimer);
  if (t.unread) {
    const mark = () => { if (state.open === v) act("read", [t]); };
    state.split ? (readTimer = setTimeout(mark, READ_AFTER_MS)) : mark();
  }
}

function hideView() {
  clearTimeout(readTimer);
  state.open?.node.classList.remove("on");
  state.open = null;
  document.body.classList.remove("reading");
  markOpenRow();
}

function markOpenRow() {
  const rows = $("#list").children;
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
  const more = el("button", "more", "⋯");
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
  f.addEventListener("load", () => {
    const doc = f.contentDocument;
    if (!doc) return;
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
    doc.addEventListener("click", e => {
      const a = e.target.closest?.("a[href]");
      if (!a) return;
      e.preventDefault();
      if (/^(https?:|mailto:)/i.test(a.href)) openExternal(a.href);
    });
    // Keyboard shortcuts keep working after clicking inside a message.
    doc.addEventListener("keydown", e => onKey(e));
  });
  const view = node.dataset.view || "html";
  f.src = `/api/message/${m.id}/html?v=${BUILD}` + (view === "html" ? "" : "&view=" + view);
  content.append(f);
  if (m.attachments.length) {
    const files = el("div", "files");
    for (const a of m.attachments) {
      const link = el("a", "file");
      link.href = `/api/message/${m.id}/attachment/${a.index}`;
      link.download = a.name;
      link.title = `${a.name} (${a.mime}): save to Downloads`;
      // Saved straight into Downloads and shown there; a plain download is the fallback.
      link.onclick = e => {
        e.preventDefault();
        api(link.href + "/save", { method: "POST" })
          .then(r => toast("Saved to " + r.path))
          .catch(() => { link.onclick = null; link.click(); });
      };
      link.append(el("span", "fn", "📎 " + a.name), el("span", "sz", fmtSize(a.size)));
      files.append(link);
    }
    content.append(files);
  }
}

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
// Which conversations an action applies to: the selection, else the open one, else the cursor row.
function targets() {
  if (state.selected.size) return state.threads.filter(t => state.selected.has(key(t)));
  if (state.open) return [state.open];
  const t = state.threads[state.cursor];
  return t ? [t] : [];
}

function act(op, list = targets()) {
  if (!list.length) return;
  // Reflect it immediately; the server confirms (or the next sync corrects) afterwards.
  for (const t of list) {
    const k = key(t);
    const row = state.threads.find(x => key(x) === k);
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
    api(`/api/thread/${t.account_id}/${t.thrid}/action`, { body: { op } })
      .catch(e => toast("Gmail didn't accept that: " + e.message));
  }
  state.selected.clear();
  renderList();
}

/** After archiving or marking unread: in the pane move on to the next conversation,
 *  in the full-width list go back to the list. */
function leaveConversation() {
  if (SOLO) return closeWindow();
  const next = state.threads[state.cursor];
  if (state.split && next) show(next);
  else hideView();
  $("#list").focus();
}

function archive() { act("archive"); leaveConversation(); }

function markUnread() {
  clearTimeout(readTimer);
  act("unread");
  if (SOLO) closeWindow();
  else if (!state.split) hideView();
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

function stepThread(delta) {
  const i = state.threads.findIndex(t => key(t) === state.open?.key);
  const next = state.threads[(i < 0 ? state.cursor : i) + delta];
  if (!next) return hideView();
  setCursor(state.threads.indexOf(next));
  show(next);
}

$("#thread-bar").addEventListener("click", e => {
  const a = e.target.dataset?.act;
  if (a === "back") { hideView(); $("#list").focus(); }
  if (a === "archive") archive();
  if (a === "unread") markUnread();
  if (a === "star") toggleStar();
  if (a === "reply") replyInGmail();
  if (a === "window") openInWindow();
});

function setLayout(split) {
  state.split = split;
  store.set("layout", split ? "split" : "list");
  document.body.classList.toggle("split", split);
  $("#layout").textContent = split ? "Full list" : "Reading pane";
  setCursor(state.cursor);
}
$("#layout").onclick = () => setLayout(!state.split);

// ---- keyboard --------------------------------------------------------------
let gPending = 0;
function onKey(e) {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName);
  if (document.querySelector("dialog[open]")) return;
  if (typing) {
    if (e.key === "Escape") { document.activeElement.blur(); $("#list").focus(); }
    return;
  }
  const k = e.key;
  if (SOLO) {
    // One conversation, no list: only the keys that make sense here.
    const solo = { n: () => focusMessage(state.open.focus + 1), p: () => focusMessage(state.open.focus - 1),
      e: archive, s: toggleStar, U: markUnread, r: replyInGmail, Escape: closeWindow, u: closeWindow,
      "?": () => $("#help-dialog").showModal() }[k];
    if (solo && state.open) { solo(); e.preventDefault(); }
    return;
  }
  if (k === "Enter" && e.shiftKey) { openInWindow(); return e.preventDefault(); }
  // "Inside" a conversation only exists in the full-width layout; in the pane the list stays live.
  const inside = !!state.open && !state.split;
  if (gPending && Date.now() - gPending < 1500 && k === "i") { gPending = 0; setFilter(null); return e.preventDefault(); }
  gPending = 0;
  switch (k) {
    case "j": inside ? stepThread(1) : moveCursor(1); break;
    case "k": inside ? stepThread(-1) : moveCursor(-1); break;
    case "ArrowDown": if (inside) return; moveCursor(1); break;
    case "ArrowUp": if (inside) return; moveCursor(-1); break;
    case "o": case "Enter": if (!inside && state.threads[state.cursor]) show(state.threads[state.cursor]); break;
    case "u": case "Escape":
      if (state.selected.size) { state.selected.clear(); renderList(); }
      else if (inside) { hideView(); $("#list").focus(); }
      break;
    case "n": if (state.open) focusMessage(state.open.focus + 1); break;
    case "p": if (state.open) focusMessage(state.open.focus - 1); break;
    case "e": archive(); break;
    case "s": toggleStar(); break;
    case "I": act("read"); break;
    case "U": markUnread(); break;
    case "x": {
      const t = state.threads[state.cursor];
      if (inside || !t) break;
      state.selected.has(key(t)) ? state.selected.delete(key(t)) : state.selected.add(key(t));
      renderList();
      break;
    }
    case "r": replyInGmail(); break;
    case "/": $("#search").focus(); $("#search").select(); break;
    case "?": $("#help-dialog").showModal(); break;
    case "g": gPending = Date.now(); break;
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
  searchTimer = setTimeout(() => { state.q = e.target.value.trim(); state.cursor = 0; hideView(); loadThreads(true); }, 200);
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
  b.querySelector(".what").textContent = busy ? "Syncing…" : "Sync";
  // The border fills as the wait runs down; while syncing it is empty and the arrow spins.
  b.style.setProperty("--p", busy ? 0 : Math.min(1, 1 - (left - 1) / sync.every));
  const s = Math.ceil(left);
  b.title = busy ? "Checking Gmail for new mail and changes"
    : `Next automatic check in ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}. Click to check now.`;
}
function setSync(s) { Object.assign(sync, s); renderSync(); }
setInterval(renderSync, 1000);
$("#sync").onclick = () => { setSync({ syncing: true }); api("/api/sync", { method: "POST" }); };
$("#help").onclick = () => $("#help-dialog").showModal();
$("#add").onclick = () => { $("#add").blur(); $("#add-error").textContent = ""; $("#add-dialog").showModal(); };
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
    toast("Account added. Fetching the inbox…");
    refresh();
  } catch (err) {
    $("#add-error").textContent = /login failed/i.test(err.message)
      ? "Gmail rejected that address and app password. Check both and try again."
      : err.message;
  }
  btn.disabled = false;
  btn.textContent = "Add account";
});

// ---- live updates ------------------------------------------------------------
async function refresh() {
  state.accounts = await api("/api/accounts");
  renderAccounts();
  // Re-read as many rows as are on screen, so paging and the cursor survive a refresh.
  const want = Math.max(60, state.threads.length);
  const p = new URLSearchParams({ q: state.q, offset: 0 });
  if (state.filter != null) p.set("account", state.filter);
  let rows = [], page;
  do {
    p.set("offset", rows.length);
    page = await api("/api/threads?" + p);
    rows = rows.concat(page);
  } while (page.length === 60 && rows.length < want);
  const at = state.open?.key || (state.threads[state.cursor] && key(state.threads[state.cursor]));
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
    state.split = false;
    state.accounts = await api("/api/accounts");
    const t = { account_id: +account_id, thrid, subject: "" };
    show(t);
    await state.open.ready;
    if (state.open.messages.some(m => !m.seen)) act("read", [t]);
    listen();
    return;
  }
  setLayout(state.split);
  await refresh();
  listen();
  api("/api/sync").then(setSync).catch(() => {});
  $("#list").focus();
  if (!state.accounts.length) $("#add-dialog").showModal();
  // Have the top of the inbox rendered before it is asked for.
  state.threads.slice(0, 2).forEach(t => ensureView(t));
})();
