//! The connection doctor: what the app has been saying to the outside world (Gmail over
//! IMAP, the assistant's model, other assistants over MCP) and whether each of those works
//! right now. Kept in memory only: commands, sizes and timings, never the mail that came back.

use crate::imap::Imap;
use crate::sync::App;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many lines of activity are remembered.
const KEEP: usize = 3000;
/// The longest a logged command may be; the rest is a long list of numbers anyway.
const LONGEST: usize = 180;

#[derive(Default)]
struct State {
    next: u64,
    lines: VecDeque<Value>,
    /// The latest result of a check asked for from the window, by what was checked.
    checks: HashMap<String, Value>,
    /// (name of the last assistant to connect over MCP, when it last asked, how many requests)
    mcp: (String, i64, u64),
    /// The last real answer from the assistant's model: (when, whether it worked, what it said)
    model: Option<(i64, bool, String)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(STATE.lock().unwrap().get_or_insert_with(State::default))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn shorten(s: &str) -> String {
    match s.char_indices().nth(LONGEST) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

pub fn size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// Record one thing that happened. `channel` is "imap", "mcp", "model" or "app"; `who` is
/// the account or program it concerned; `outcome` is what came of it.
pub fn note(channel: &str, who: &str, what: &str, outcome: &str, ok: bool, took: Option<Duration>) {
    with(|s| {
        s.next += 1;
        let line = json!({ "id": s.next, "at": now_ms(), "channel": channel, "who": who, "what": shorten(what),
                           "outcome": shorten(outcome), "ok": ok, "ms": took.map(|t| t.as_millis() as u64) });
        s.lines.push_back(line);
        if s.lines.len() > KEEP {
            s.lines.pop_front();
        }
    });
}

/// An assistant said hello over MCP.
pub fn mcp_client(name: &str) {
    with(|s| s.mcp.0 = name.to_string());
}

/// Count a request that arrived over MCP; returns who is thought to have sent it.
pub fn mcp_request() -> String {
    with(|s| {
        s.mcp.1 = now_ms();
        s.mcp.2 += 1;
        s.mcp.0.clone()
    })
}

/// Remember how the assistant's model last answered a real request.
pub fn model_answered(ok: bool, text: &str) {
    with(|s| s.model = Some((now_ms(), ok, shorten(text))));
}

fn set_check(key: &str, state: &str, ms: Option<u64>, detail: &str) {
    with(|s| s.checks.insert(key.to_string(), json!({ "state": state, "ms": ms, "detail": detail, "at": now_ms() })));
}

impl App {
    /// Everything the doctor window shows: the state of each connection, and the activity
    /// recorded after line `after`.
    pub fn doctor(&self, after: u64) -> anyhow::Result<Value> {
        let (lines, checks, mcp, model, last) = with(|s| {
            let lines: Vec<Value> = s.lines.iter().filter(|l| l["id"].as_u64().unwrap_or(0) > after).cloned().collect();
            (lines, s.checks.clone(), s.mcp.clone(), s.model.clone(), s.next)
        });
        let c = self.db.conn()?;
        let watching = self.watching();
        let mut accounts = Vec::new();
        for a in self.db.accounts()? {
            let count = |sql: &str| c.query_row(sql, [a.id], |r| r.get::<_, i64>(0)).unwrap_or(0);
            accounts.push(json!({
                "id": a.id, "email": a.email, "color": a.color,
                "watching": watching.get(&a.id).copied().unwrap_or(false),
                "last_sync": a.last_sync, "last_error": a.last_error, "listing": a.indexing,
                "messages": count("SELECT COUNT(*) FROM messages WHERE account_id=?"),
                "waiting": count("SELECT COUNT(*) FROM pending WHERE account_id=?"),
                "check": checks.get(&format!("imap:{}", a.id)),
            }));
        }
        let (endpoint, model_name, has_key) = self.agent_config();
        let helper = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("unibox-mcp.exe").exists())).unwrap_or(false);
        let file = |name: &str| std::fs::metadata(self.dir.join(name)).map(|m| m.len()).unwrap_or(0);
        Ok(json!({
            "accounts": accounts,
            "server": self.imap_server,
            "sync": self.sync_status(),
            "model": { "endpoint": endpoint, "model": model_name, "has_key": has_key, "check": checks.get("model"),
                       "last": model.map(|(at, ok, text)| json!({ "at": at, "ok": ok, "text": text })) },
            "mcp": { "client": mcp.0, "last": mcp.1, "requests": mcp.2, "helper": helper },
            "store": { "bytes": file("mail.db") + file("mail.db-wal"),
                       "search_ready": self.db.meta("search_index").ok().flatten().is_some() },
            "lines": lines,
            "last": last,
        }))
    }

    /// Try each connection afresh, in the background: sign in to every account, and ask the
    /// assistant's model for one word. The results appear in `doctor`.
    pub fn doctor_check(self: &Arc<App>) {
        for a in self.db.accounts().unwrap_or_default() {
            let key = format!("imap:{}", a.id);
            set_check(&key, "running", None, "");
            let app = self.clone();
            std::thread::spawn(move || {
                let began = Instant::now();
                let tried = app.connect(&a.email).and_then(|mut imap: Imap| {
                    imap.run("NOOP")?;
                    imap.logout();
                    Ok(())
                });
                let ms = Some(began.elapsed().as_millis() as u64);
                match tried {
                    Ok(()) => set_check(&key, "ok", ms, ""),
                    Err(e) => set_check(&key, "failed", ms, &format!("{e:#}")),
                }
            });
        }
        if self.agent_config().2 {
            set_check("model", "running", None, "");
            let app = self.clone();
            std::thread::spawn(move || {
                let began = Instant::now();
                let tried = app.model_check();
                let ms = Some(began.elapsed().as_millis() as u64);
                match tried {
                    Ok(()) => set_check("model", "ok", ms, ""),
                    Err(e) => set_check("model", "failed", ms, &format!("{e:#}")),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_bounded_record() {
        let before = with(|s| s.next);
        note("imap", "a@example.com", &"UID FETCH ".repeat(60), "OK", true, Some(Duration::from_millis(12)));
        let line = with(|s| s.lines.back().cloned()).unwrap();
        assert!(line["id"].as_u64().unwrap() > before);
        assert!(line["what"].as_str().unwrap().chars().count() <= LONGEST + 1);
        assert_eq!(line["ms"], 12);
        for _ in 0..KEEP + 5 {
            note("app", "", "x", "", true, None);
        }
        assert_eq!(with(|s| s.lines.len()), KEEP);
        assert_eq!(size(900), "900 B");
        assert_eq!(size(1536), "1.5 KB");
    }
}
