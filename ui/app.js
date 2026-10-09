"use strict";
const $ = s => document.querySelector(s);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};

const state = {
  accounts: [],
  filter: null,          // account id, or null for all inboxes
  q: "",
  threads: [],
  more: true,
  cursor: 0,
  selected: new Set(),   // thread keys
  open: null,            // { account_id, thrid, messages, focus }
};
const key = t => t.account_id + ":" + t.thrid;
const account = id => state.accounts.find(a => a.id === id);
const dark = matchMedia("(prefers-color-scheme: dark)");

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

// ---- sidebar ---------------------------------------------------------------
function renderAccounts() {
  const nav = $("#accounts");
  nav.textContent = "";
  const item = (id, name, color, unread, acct) => {
    const b = el("button", "acct" + (state.filter === id ? " on" : "") + (acct?.last_error ? " err" : ""));
    const dot = el("span", "dot");
    dot.style.background = color;
    b.append(dot, el("span", "name", name), el("span", "n", unread || ""));
    if (acct) {
      b.title = acct.last_error ? "Sync problem: " + acct.last_error : acct.email;
      const rm = el("span", "rm", "×");
      rm.title = "Remove this account from Unibox";
      rm.onclick = e => { e.stopPropagation(); removeAccount(acct); };
      b.append(rm);
    }
    b.onclick = () => setFilter(id);
    nav.append(b);
  };
  item(null, "All inboxes", "var(--muted)", state.accounts.reduce((n, a) => n + a.unread, 0));
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
  refresh();
}

function setFilter(id) {
  state.filter = id;
  state.cursor = 0;
  state.selected.clear();
  showList();
  loadThreads(true);
  renderAccounts();
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
      + (state.selected.has(key(t)) ? " sel" : ""));
    const dot = el("span", "dot");
    dot.style.background = account(t.account_id)?.color || "gray";
    dot.title = account(t.account_id)?.email || "";
    const who = el("span", "who", t.senders || "(unknown)");
    if (t.count > 1) who.append(el("span", "c", t.count));
    const text = el("span", "text");
    text.append(el("span", "subj", t.subject), el("span", "snip", t.snippet ? " — " + t.snippet : ""));
    const marks = el("span", "marks");
    if (t.has_attach) marks.append("📎");
    if (t.starred) marks.append(el("span", "star", "★"));
    row.append(el("span", "bar"), dot, who, text, marks, el("span", "date", fmtDate(t.date)));
    row.onclick = () => { state.cursor = i; openThread(t); };
    list.append(row);
  });
  list.scrollTop = top;
}

function moveCursor(delta) {
  const n = state.threads.length;
  if (!n) return;
  state.cursor = Math.max(0, Math.min(n - 1, state.cursor + delta));
  const rows = $("#list").children;
  for (let i = 0; i < rows.length; i++) rows[i].classList.toggle("cursor", i === state.cursor);
  rows[state.cursor]?.scrollIntoView({ block: "nearest" });
  if (state.more && state.cursor > n - 15) loadThreads(false);
}

$("#list").addEventListener("scroll", e => {
  const l = e.target;
  if (state.more && l.scrollTop + l.clientHeight > l.scrollHeight - 600 && !loadThreads.busy) {
    loadThreads.busy = true;
    loadThreads(false).finally(() => (loadThreads.busy = false));
  }
});

// ---- actions ---------------------------------------------------------------
// Which conversations an action applies to: the open one, else the selection, else the cursor row.
function targets() {
  if (state.open) return [state.open];
  if (state.selected.size) return state.threads.filter(t => state.selected.has(key(t)));
  const t = state.threads[state.cursor];
  return t ? [t] : [];
}

async function act(op, list = targets()) {
  if (!list.length) return;
  // Reflect it immediately; the server confirms (or the next sync corrects) afterwards.
  for (const t of list) {
    const row = state.threads.find(x => key(x) === key(t));
    if (op === "archive") state.threads = state.threads.filter(x => key(x) !== key(t));
    else if (row) {
      if (op === "read") row.unread = 0;
      if (op === "unread") row.unread = 1;
      if (op === "star") row.starred = true;
      if (op === "unstar") row.starred = false;
    }
  }
  state.selected.clear();
  renderList();
  for (const t of list) {
    api(`/api/thread/${t.account_id}/${t.thrid}/action`, { body: { op } })
      .catch(e => toast("Gmail didn't accept that: " + e.message));
  }
}

function toggleStar() {
  const list = targets();
  if (!list.length) return;
  const on = state.open ? state.open.messages.some(m => m.starred)
    : list.every(t => state.threads.find(x => key(x) === key(t))?.starred);
  if (state.open) state.open.messages.forEach((m, i, all) => (m.starred = !on && i === all.length - 1));
  act(on ? "unstar" : "star", list);
  if (state.open) updateThreadBar();
}

// ---- conversation view -------------------------------------------------------
function showList() {
  state.open = null;
  $("#thread").hidden = true;
  $("#list").hidden = false;
  $("#messages").textContent = "";
}

async function openThread(t) {
  state.open = { account_id: t.account_id, thrid: t.thrid, messages: [], focus: 0 };
  $("#list").hidden = true;
  $("#thread").hidden = false;
  $("#thread").scrollTop = 0;
  $("#thread-subject").textContent = t.subject;
  $("#messages").textContent = "";
  await loadThread(true);
  if (t.unread) act("read", [t]);
}

async function loadThread(first) {
  const o = state.open;
  if (!o) return;
  const data = await api(`/api/thread/${o.account_id}/${o.thrid}`);
  if (state.open !== o) return;
  const before = new Map(o.messages.map(m => [m.id, m]));
  o.messages = data.messages;
  if (first) o.focus = Math.max(0, o.messages.findIndex(m => !m.seen));
  const box = $("#messages");
  // Keep frames that are already showing; only add or fill in what changed.
  o.messages.forEach((m, i) => {
    const old = before.get(m.id);
    const node = box.querySelector(`[data-id="${m.id}"]`);
    if (node && old && old.has_body === m.has_body && old.attachments.length === m.attachments.length) return;
    const open = node ? !node.classList.contains("closed") : (!m.seen || i === o.messages.length - 1);
    const fresh = renderMessage(m, open);
    if (node) node.replaceWith(fresh);
    else box.insertBefore(fresh, box.children[i] || null);
  });
  if (o.messages[0]) $("#thread-subject").textContent = o.messages[0].subject;
  updateThreadBar();
  if (first) focusMessage(o.focus, false);
}

function renderMessage(m, open) {
  const node = el("article", "msg" + (open ? "" : " closed"));
  node.dataset.id = m.id;
  const head = el("header");
  const from = el("span", "from", m.from_name || m.from_addr || "(unknown)");
  if (m.from_name) from.append(el("span", "addr", "  " + m.from_addr));
  head.append(from, el("span", "meta", m.snippet), el("span", "when", fmtDate(m.date, true)));
  head.onclick = () => { node.classList.toggle("closed"); if (!node.classList.contains("closed")) fill(node, m); };
  node.append(head, el("div", "to", "to " + [m.to_addrs, m.cc_addrs].filter(Boolean).join(", ")), el("div", "content"));
  if (open) fill(node, m);
  return node;
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
    const fit = () => { f.style.height = Math.ceil(root.getBoundingClientRect().height) + "px"; };
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
  f.src = `/api/message/${m.id}/html`;
  content.append(f);
  if (m.attachments.length) {
    const files = el("div", "files");
    for (const a of m.attachments) {
      const link = el("a", "file");
      link.href = `/api/message/${m.id}/attachment/${a.index}`;
      link.download = a.name;
      link.title = `${a.name} (${a.mime})`;
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
  const o = state.open;
  if (!o || !o.messages.length) return;
  o.focus = Math.max(0, Math.min(o.messages.length - 1, i));
  const nodes = $("#messages").children;
  for (let n = 0; n < nodes.length; n++) nodes[n].classList.toggle("focus", n === o.focus && nodes.length > 1);
  const node = nodes[o.focus];
  if (!node) return;
  if (node.classList.contains("closed")) { node.classList.remove("closed"); fill(node, o.messages[o.focus]); }
  if (scroll || o.focus > 0) node.scrollIntoView({ block: "start", behavior: scroll ? "smooth" : "auto" });
}

function updateThreadBar() {
  const starred = state.open?.messages.some(m => m.starred);
  $('#thread-bar [data-act="star"]').textContent = starred ? "★ Starred" : "Star";
}

function replyInGmail() {
  const o = state.open || state.threads[state.cursor];
  const a = o && account(o.account_id);
  if (!a) return;
  openExternal(`https://mail.google.com/mail/?authuser=${encodeURIComponent(a.email)}#all/${BigInt(o.thrid).toString(16)}`);
}

// After archiving or leaving a conversation, land back in the list.
function closeThread() { showList(); renderList(); $("#list").focus(); }

function stepThread(delta) {
  const o = state.open;
  const i = state.threads.findIndex(t => key(t) === key(o));
  const next = state.threads[(i < 0 ? state.cursor : i) + delta];
  if (!next) return closeThread();
  state.cursor = state.threads.indexOf(next);
  openThread(next);
}

$("#thread-bar").addEventListener("click", e => {
  const a = e.target.dataset?.act;
  if (a === "back") closeThread();
  if (a === "archive") { act("archive"); closeThread(); }
  if (a === "unread") { act("unread"); closeThread(); }
  if (a === "star") toggleStar();
  if (a === "reply") replyInGmail();
});

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
  const inThread = !!state.open;
  const handled = () => { e.preventDefault(); };
  if (gPending && Date.now() - gPending < 1500 && k === "i") { gPending = 0; setFilter(null); return handled(); }
  gPending = 0;
  switch (k) {
    case "j": inThread ? stepThread(1) : moveCursor(1); break;
    case "k": inThread ? stepThread(-1) : moveCursor(-1); break;
    case "ArrowDown": if (inThread) return; moveCursor(1); break;
    case "ArrowUp": if (inThread) return; moveCursor(-1); break;
    case "o": case "Enter": if (!inThread && state.threads[state.cursor]) openThread(state.threads[state.cursor]); break;
    case "u": case "Escape": if (inThread) closeThread(); else if (state.selected.size) { state.selected.clear(); renderList(); } break;
    case "n": if (inThread) focusMessage(state.open.focus + 1); break;
    case "p": if (inThread) focusMessage(state.open.focus - 1); break;
    case "e": act("archive"); if (inThread) closeThread(); break;
    case "s": toggleStar(); break;
    case "I": act("read"); break;
    case "U": act("unread"); if (inThread) closeThread(); break;
    case "x": {
      const t = state.threads[state.cursor];
      if (inThread || !t) break;
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
  handled();
}
document.addEventListener("keydown", onKey);

let searchTimer;
$("#search").addEventListener("input", e => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { state.q = e.target.value.trim(); state.cursor = 0; showList(); loadThreads(true); }, 200);
});
$("#search").addEventListener("keydown", e => { if (e.key === "Enter") { e.target.blur(); $("#list").focus(); } });

// ---- toolbar and dialogs -------------------------------------------------------
$("#sync").onclick = () => { api("/api/sync", { method: "POST" }); $("#status").textContent = "Syncing…"; };
$("#help").onclick = () => $("#help-dialog").showModal();
$("#add").onclick = () => { $("#add-error").textContent = ""; $("#add-dialog").showModal(); };
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
  const at = state.threads[state.cursor] && key(state.threads[state.cursor]);
  state.threads = rows;
  state.more = page.length === 60;
  const again = rows.findIndex(t => key(t) === at);
  if (again >= 0) state.cursor = again;
  if (!state.open) renderList();
}

let refreshTimer;
function refreshSoon() { clearTimeout(refreshTimer); refreshTimer = setTimeout(() => refresh().catch(() => {}), 250); }

function listen() {
  const es = new EventSource("/api/events");
  es.onmessage = e => {
    const ev = JSON.parse(e.data);
    if (ev.kind === "error") return toast(ev.detail);
    if (ev.kind === "thread") { if (state.open?.thrid === ev.detail) loadThread(false); return; }
    refreshSoon();
    if (state.open && state.open.messages.some(m => !m.has_body)) loadThread(false);
  };
}

(async () => {
  await refresh();
  listen();
  $("#list").focus();
  if (!state.accounts.length) $("#add-dialog").showModal();
})();
