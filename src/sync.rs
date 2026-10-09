//! Talking to Gmail: pulling the inbox down, filling in conversations, and
//! pushing archive / read / star changes back.

use crate::db::{Db, NewMessage};
use crate::imap::{parse_internaldate, uid_set, Fetched, Imap};
use crate::{mime, secrets};
use anyhow::{anyhow, Result};
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const HEADER_ITEMS: &str = "(UID X-GM-MSGID X-GM-THRID FLAGS INTERNALDATE RFC822.SIZE \
                            BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT)])";
const FULL_ITEMS: &str = "(UID X-GM-MSGID X-GM-THRID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])";
/// Bodies are fetched ahead of time for the newest inbox messages; the rest load when opened.
const PREFETCH_BODIES: i64 = 400;
const MAX_PREFETCH_SIZE: i64 = 8 * 1024 * 1024;
const SYNC_EVERY: Duration = Duration::from_secs(120);

pub struct App {
    pub db: Db,
    pub dir: PathBuf,
    pub imap_server: String,
    pub events: broadcast::Sender<String>,
    /// One lock per account: held while the inbox listing is reconciled and while
    /// a local change is pushed, so the two can never interleave.
    locks: Mutex<HashMap<i64, Arc<Mutex<()>>>>,
    hydrated: Mutex<HashMap<(i64, String), Instant>>,
    wake: Mutex<Option<mpsc::Sender<()>>>,
}

impl App {
    pub fn new(db: Db, dir: PathBuf, imap_server: String) -> Arc<App> {
        Arc::new(App {
            db,
            dir,
            imap_server,
            events: broadcast::channel(64).0,
            locks: Mutex::new(HashMap::new()),
            hydrated: Mutex::new(HashMap::new()),
            wake: Mutex::new(None),
        })
    }

    pub fn emit(&self, kind: &str, detail: &str) {
        let _ = self.events.send(serde_json::json!({ "kind": kind, "detail": detail }).to_string());
    }

    fn lock_for(&self, account_id: i64) -> Arc<Mutex<()>> {
        self.locks.lock().unwrap().entry(account_id).or_default().clone()
    }

    fn connect(&self, email: &str) -> Result<Imap> {
        let password = secrets::get(&self.dir, email)?;
        Imap::connect(&self.imap_server, email, &password)
    }

    /// Check that an address and app password really log in.
    pub fn verify_login(&self, email: &str, password: &str) -> Result<()> {
        Imap::connect(&self.imap_server, email, password)?.logout();
        Ok(())
    }

    /// Ask the background thread to sync now instead of waiting for its timer.
    pub fn sync_soon(&self) {
        if let Some(tx) = self.wake.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
    }

    pub fn start(self: &Arc<App>) {
        let (tx, rx) = mpsc::channel();
        *self.wake.lock().unwrap() = Some(tx);
        let app = self.clone();
        std::thread::spawn(move || loop {
            for acct in app.db.accounts().unwrap_or_default() {
                let result = app.sync_account(acct.id, &acct.email);
                let err = result.as_ref().err().map(|e| format!("{e:#}"));
                if let Some(e) = &err {
                    eprintln!("sync {}: {e}", acct.email);
                }
                let _ = app.db.set_sync_result(acct.id, err.as_deref());
                app.emit("synced", &acct.email);
            }
            let _ = rx.recv_timeout(SYNC_EVERY);
            while rx.try_recv().is_ok() {} // collapse a burst of requests into one pass
        });
    }

    fn sync_account(&self, account_id: i64, email: &str) -> Result<()> {
        let mut imap = self.connect(email)?;
        let exists = imap.select("INBOX", false)?;
        let new_uids = {
            let lock = self.lock_for(account_id);
            let _guard = lock.lock().unwrap();
            let listing = if exists == 0 { Vec::new() } else { imap.uid_fetch("1:*", "(UID X-GM-MSGID FLAGS)")? };
            self.reconcile(account_id, &listing)?
        };
        self.emit("changed", email);

        // Headers for anything new, newest first so the top of the list fills in first.
        for chunk in new_uids.chunks(200) {
            let fetched = imap.uid_fetch(&uid_set(chunk), HEADER_ITEMS)?;
            let c = self.db.conn()?;
            for f in &fetched {
                insert_fetched(&c, account_id, f, true)?;
            }
            self.emit("changed", email);
        }

        if !new_uids.is_empty() {
            let with_files = imap.uid_search("X-GM-RAW \"has:attachment\"")?;
            let c = self.db.conn()?;
            for chunk in with_files.chunks(500) {
                let marks = chunk.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
                c.execute(
                    &format!("UPDATE messages SET has_attach=1 WHERE account_id=? AND in_inbox=1 AND uid IN ({marks})"),
                    [account_id],
                )?;
            }
        }

        // Bodies, a few at a time, so snippets appear and opening a message is instant.
        loop {
            let want: Vec<u32> = {
                let c = self.db.conn()?;
                let mut st = c.prepare(
                    "SELECT uid FROM (SELECT uid, has_body, size FROM messages
                                       WHERE account_id=? AND in_inbox=1 AND uid IS NOT NULL
                                       ORDER BY date DESC LIMIT ?)
                      WHERE has_body=0 AND size<=? LIMIT 12",
                )?;
                let rows = st.query_map(params![account_id, PREFETCH_BODIES, MAX_PREFETCH_SIZE], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            if want.is_empty() {
                break;
            }
            let fetched = imap.uid_fetch(&uid_set(&want), FULL_ITEMS)?;
            let c = self.db.conn()?;
            let mut stored = 0;
            for f in &fetched {
                stored += store_fetched_body(&c, account_id, f)? as usize;
            }
            self.emit("changed", email);
            if stored == 0 {
                break; // the server gave us nothing for these; don't spin
            }
        }
        imap.logout();
        Ok(())
    }

    /// Bring local inbox membership and flags in line with the server's listing.
    /// Returns the UIDs of messages we have never seen, newest first.
    fn reconcile(&self, account_id: i64, listing: &[Fetched]) -> Result<Vec<u32>> {
        let mut c = self.db.conn()?;
        let tx = c.transaction()?;
        let pending: HashSet<String> = {
            let mut st = tx.prepare("SELECT gm_msgid FROM pending WHERE account_id=?")?;
            let rows = st.query_map([account_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let known: HashSet<String> = {
            let mut st = tx.prepare("SELECT gm_msgid FROM messages WHERE account_id=?")?;
            let rows = st.query_map([account_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut on_server = HashSet::new();
        let mut new_uids = Vec::new();
        for f in listing {
            let (Some(msgid), Some(uid)) = (f.get("X-GM-MSGID").map(|v| v.text()), fetched_uid(f)) else { continue };
            on_server.insert(msgid.clone());
            if pending.contains(&msgid) {
                continue;
            }
            if known.contains(&msgid) {
                let (seen, starred) = fetched_flags(f);
                tx.execute(
                    "UPDATE messages SET uid=?, in_inbox=1, seen=?, starred=? WHERE account_id=? AND gm_msgid=?",
                    params![uid, seen, starred, account_id, msgid],
                )?;
            } else {
                new_uids.push(uid);
            }
        }
        let gone: Vec<String> = {
            let mut st = tx.prepare("SELECT gm_msgid FROM messages WHERE account_id=? AND in_inbox=1")?;
            let rows = st.query_map([account_id], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).filter(|m| !on_server.contains(m) && !pending.contains(m)).collect()
        };
        for msgid in gone {
            tx.execute(
                "UPDATE messages SET in_inbox=0, uid=NULL WHERE account_id=? AND gm_msgid=?",
                params![account_id, msgid],
            )?;
        }
        tx.commit()?;
        new_uids.sort_unstable_by(|a, b| b.cmp(a));
        Ok(new_uids)
    }

    /// Fetch every message of a conversation from All Mail, so replies you sent and
    /// earlier archived messages show up alongside what is in the inbox.
    pub fn hydrate_thread(&self, account_id: i64, thrid: &str) -> Result<()> {
        let key = (account_id, thrid.to_string());
        {
            let mut seen = self.hydrated.lock().unwrap();
            if seen.get(&key).is_some_and(|t| t.elapsed() < Duration::from_secs(90)) {
                return Ok(());
            }
            seen.insert(key.clone(), Instant::now());
        }
        let result = self.hydrate_inner(account_id, thrid);
        if result.is_err() {
            self.hydrated.lock().unwrap().remove(&key);
        }
        result
    }

    fn hydrate_inner(&self, account_id: i64, thrid: &str) -> Result<()> {
        if !thrid.chars().all(|c| c.is_ascii_digit()) {
            return Err(anyhow!("bad thread id"));
        }
        let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("no such account"))?;
        let mut imap = self.connect(&acct.email)?;
        let all_mail = imap.all_mail()?;
        imap.select(&all_mail, true)?;
        let uids = imap.uid_search(&format!("X-GM-THRID {thrid}"))?;
        if uids.is_empty() {
            imap.logout();
            return Ok(());
        }
        // Find out which of these we already hold in full before downloading anything big.
        let listing = imap.uid_fetch(&uid_set(&uids), "(UID X-GM-MSGID)")?;
        let have: HashSet<String> = {
            let c = self.db.conn()?;
            let mut st =
                c.prepare("SELECT gm_msgid FROM messages WHERE account_id=? AND gm_thrid=? AND has_body=1")?;
            let rows = st.query_map(params![account_id, thrid], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let need: Vec<u32> = listing
            .iter()
            .filter(|f| f.get("X-GM-MSGID").is_some_and(|m| !have.contains(&m.text())))
            .filter_map(fetched_uid)
            .collect();
        for chunk in need.chunks(8) {
            let fetched = imap.uid_fetch(&uid_set(chunk), FULL_ITEMS)?;
            let c = self.db.conn()?;
            for f in &fetched {
                insert_fetched(&c, account_id, f, false)?;
                store_fetched_body(&c, account_id, f)?;
            }
            self.emit("thread", thrid);
        }
        imap.logout();
        Ok(())
    }

    /// Apply a user action to a whole conversation, locally at once and then on Gmail.
    pub fn act(&self, account_id: i64, thrid: &str, op: &str) -> Result<()> {
        let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("no such account"))?;
        let local = match op {
            "archive" => "in_inbox=0",
            "read" => "seen=1",
            "unread" => "seen=0",
            "star" => "starred=1",
            "unstar" => "starred=0",
            _ => return Err(anyhow!("unknown action {op}")),
        };
        // Only messages sitting in the inbox have a UID we can address.
        let (msgids, uids): (Vec<String>, Vec<u32>) = {
            let c = self.db.conn()?;
            let filter = match op {
                "star" => " ORDER BY date DESC LIMIT 1", // Gmail stars the latest message
                "unread" => " ORDER BY date DESC LIMIT 1",
                _ => "",
            };
            let mut st = c.prepare(&format!(
                "SELECT gm_msgid, uid FROM messages
                  WHERE account_id=? AND gm_thrid=? AND in_inbox=1 AND uid IS NOT NULL{filter}"
            ))?;
            let rows = st.query_map(params![account_id, thrid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<(String, u32)>>>()?.into_iter().unzip()
        };
        if uids.is_empty() {
            return Ok(());
        }
        {
            let c = self.db.conn()?;
            for m in &msgids {
                c.execute("INSERT OR IGNORE INTO pending(account_id, gm_msgid) VALUES(?,?)", params![account_id, m])?;
                c.execute(
                    &format!("UPDATE messages SET {local} WHERE account_id=? AND gm_msgid=?"),
                    params![account_id, m],
                )?;
            }
        }
        self.emit("changed", &acct.email);

        let lock = self.lock_for(account_id);
        let _guard = lock.lock().unwrap();
        let pushed = (|| -> Result<()> {
            let mut imap = self.connect(&acct.email)?;
            imap.select("INBOX", false)?;
            let set = uid_set(&uids);
            match op {
                // Moving to All Mail is how Gmail spells "archive" over IMAP.
                "archive" => {
                    let all_mail = imap.all_mail()?;
                    imap.run(&format!("UID MOVE {set} {all_mail}"))?;
                }
                "read" => drop(imap.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Seen)"))?),
                "unread" => drop(imap.run(&format!("UID STORE {set} -FLAGS.SILENT (\\Seen)"))?),
                "star" => drop(imap.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Flagged)"))?),
                "unstar" => drop(imap.run(&format!("UID STORE {set} -FLAGS.SILENT (\\Flagged)"))?),
                _ => unreachable!(),
            }
            imap.logout();
            Ok(())
        })();
        let c = self.db.conn()?;
        for m in &msgids {
            c.execute("DELETE FROM pending WHERE account_id=? AND gm_msgid=?", params![account_id, m])?;
            if op == "archive" && pushed.is_ok() {
                c.execute("UPDATE messages SET uid=NULL WHERE account_id=? AND gm_msgid=?", params![account_id, m])?;
            }
        }
        if pushed.is_err() {
            // Local state is now wrong; the next sync puts back what Gmail really has.
            self.sync_soon();
        }
        pushed
    }
}

fn fetched_uid(f: &Fetched) -> Option<u32> {
    f.get("UID")?.text().parse().ok()
}

fn fetched_flags(f: &Fetched) -> (bool, bool) {
    let flags: Vec<String> =
        f.get("FLAGS").map(|v| v.list().iter().map(|a| a.text().to_ascii_lowercase()).collect()).unwrap_or_default();
    (flags.iter().any(|x| x == "\\seen"), flags.iter().any(|x| x == "\\flagged"))
}

/// Insert a message from a FETCH carrying headers or a full body.
/// `inbox` says whether the UID in the response is an INBOX UID.
fn insert_fetched(c: &rusqlite::Connection, account_id: i64, f: &Fetched, inbox: bool) -> Result<()> {
    let (Some(msgid), Some(thrid)) = (f.get("X-GM-MSGID"), f.get("X-GM-THRID")) else { return Ok(()) };
    let headers = mime::parse_headers(f.get("BODY").map(|b| b.bytes()).unwrap_or_default());
    let (seen, starred) = fetched_flags(f);
    Db::insert_message(
        c,
        account_id,
        &NewMessage {
            gm_msgid: &msgid.text(),
            gm_thrid: &thrid.text(),
            uid: if inbox { fetched_uid(f) } else { None },
            in_inbox: inbox,
            seen,
            starred,
            date: f.get("INTERNALDATE").and_then(|d| parse_internaldate(&d.text())).unwrap_or(0),
            size: f.get("RFC822.SIZE").and_then(|s| s.text().parse().ok()).unwrap_or(0),
            from_name: &headers.from_name,
            from_addr: &headers.from_addr,
            to_addrs: &headers.to,
            cc_addrs: &headers.cc,
            subject: &headers.subject,
        },
    )
}

fn store_fetched_body(c: &rusqlite::Connection, account_id: i64, f: &Fetched) -> Result<bool> {
    let (Some(msgid), Some(body)) = (f.get("X-GM-MSGID"), f.get("BODY")) else { return Ok(false) };
    let raw = body.bytes();
    let s = mime::summarize(raw);
    Db::store_body(c, account_id, &msgid.text(), raw, &s.snippet, &s.text, s.has_attach)?;
    Ok(true)
}
