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
/// Everything in the inbox is downloaded ahead of time, except messages bigger than this,
/// which load when opened.
const MAX_PREFETCH_SIZE: i64 = 25 * 1024 * 1024;
const SYNC_EVERY: Duration = Duration::from_secs(120);
/// How long one pass may spend preloading before it goes back to check for new mail.
const PRELOAD_BUDGET: Duration = Duration::from_secs(40);
/// How often opening a conversation re-checks All Mail for replies sent from elsewhere.
const REHYDRATE_AFTER: Duration = Duration::from_secs(600);

pub struct App {
    pub db: Db,
    pub dir: PathBuf,
    pub imap_server: String,
    pub events: broadcast::Sender<String>,
    /// One lock per account: held while the inbox listing is reconciled and while
    /// a local change is pushed, so the two can never interleave.
    locks: Mutex<HashMap<i64, Arc<Mutex<()>>>>,
    hydrated: Mutex<HashMap<(i64, String), Instant>>,
    /// Conversations opened by hand are topped up one at a time, so paging quickly
    /// through the list cannot open a pile of connections to Gmail.
    hydrating: Mutex<()>,
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
            hydrating: Mutex::new(()),
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
        std::thread::spawn(move || {
            app.refresh_summaries();
            loop {
                let mut more = false;
                for acct in app.db.accounts().unwrap_or_default() {
                    let result = app.sync_account(acct.id, &acct.email);
                    let err = result.as_ref().err().map(|e| format!("{e:#}"));
                    if let Some(e) = &err {
                        eprintln!("sync {}: {e}", acct.email);
                    }
                    more |= result.unwrap_or(false);
                    let _ = app.db.set_sync_result(acct.id, err.as_deref());
                    app.emit("synced", &acct.email);
                }
                // While there is still mail to preload, go straight round again.
                let _ = rx.recv_timeout(if more { Duration::from_millis(200) } else { SYNC_EVERY });
                while rx.try_recv().is_ok() {} // collapse a burst of requests into one pass
            }
        });
    }

    /// Previews, search text and attachment lists are worked out when a body is stored.
    /// When the way they are worked out changes, redo them for everything already held.
    fn refresh_summaries(&self) {
        let work = || -> Result<()> {
            if self.db.meta("summary_version")?.as_deref() == Some(mime::SUMMARY_VERSION) {
                return Ok(());
            }
            for chunk in self.db.stored_bodies()?.chunks(100) {
                let mut c = self.db.conn()?;
                let tx = c.transaction()?;
                for &id in chunk {
                    let raw: Vec<u8> = tx.query_row("SELECT raw FROM bodies WHERE message_id=?", [id], |r| r.get(0))?;
                    let s = mime::summarize(&raw);
                    Db::set_summary(&tx, id, &s.snippet, &s.text, &s.attachments)?;
                }
                tx.commit()?;
            }
            self.db.set_meta("summary_version", mime::SUMMARY_VERSION)?;
            self.emit("changed", "");
            Ok(())
        };
        if let Err(e) = work() {
            eprintln!("refreshing summaries: {e:#}");
        }
    }

    /// One pass over an account. Returns true if preloading ran out of time and wants another go.
    fn sync_account(&self, account_id: i64, email: &str) -> Result<bool> {
        let deadline = Instant::now() + PRELOAD_BUDGET;
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

        // Bodies, a few at a time, newest first, so snippets appear and opening is instant.
        let mut gave_nothing: HashSet<u32> = HashSet::new();
        let mut batches = 0;
        loop {
            let want: Vec<u32> = {
                let c = self.db.conn()?;
                let mut st = c.prepare(
                    "SELECT uid FROM messages
                      WHERE account_id=? AND in_inbox=1 AND uid IS NOT NULL AND has_body=0 AND size<=?
                      ORDER BY date DESC LIMIT 200",
                )?;
                let rows = st.query_map(params![account_id, MAX_PREFETCH_SIZE], |r| r.get::<_, u32>(0))?;
                rows.filter_map(|r| r.ok()).filter(|u| !gave_nothing.contains(u)).take(12).collect()
            };
            if want.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                imap.logout();
                self.emit("changed", email);
                return Ok(true);
            }
            let fetched = imap.uid_fetch(&uid_set(&want), FULL_ITEMS)?;
            let c = self.db.conn()?;
            let mut got = HashSet::new();
            for f in &fetched {
                if store_fetched_body(&c, account_id, f)? {
                    got.extend(fetched_uid(f));
                }
            }
            // a message that vanished mid-sync returns nothing; don't ask for it again this pass
            gave_nothing.extend(want.iter().filter(|u| !got.contains(u)));
            batches += 1;
            if batches % 4 == 0 {
                self.emit("changed", email);
            }
        }
        if batches > 0 {
            self.emit("changed", email);
        }

        // Then the rest of each conversation: replies you sent, earlier archived messages.
        let mut all_mail_open = false;
        loop {
            let thrids = self.db.unhydrated_threads(account_id, 25)?;
            if thrids.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                imap.logout();
                return Ok(true);
            }
            if !all_mail_open {
                let all_mail = imap.all_mail()?;
                imap.select(&all_mail, true)?;
                all_mail_open = true;
            }
            let added = self.fill_threads(&mut imap, account_id, &thrids, MAX_PREFETCH_SIZE)?;
            self.db.mark_hydrated(account_id, &thrids)?;
            if added > 0 {
                self.emit("changed", email);
            }
        }
        imap.logout();
        Ok(false)
    }

    /// With All Mail selected, download every message of these conversations that we
    /// do not hold yet. Returns how many were added.
    fn fill_threads(&self, imap: &mut Imap, account_id: i64, thrids: &[String], max_size: i64) -> Result<usize> {
        // "OR a OR b c" is how IMAP spells a three-way OR.
        let mut criteria = String::new();
        for (i, t) in thrids.iter().enumerate() {
            if !t.chars().all(|c| c.is_ascii_digit()) {
                return Err(anyhow!("bad thread id"));
            }
            if i + 1 < thrids.len() {
                criteria.push_str("OR ");
            }
            criteria.push_str(&format!("X-GM-THRID {t} "));
        }
        let uids = match imap.uid_search(criteria.trim_end()) {
            Ok(u) => u,
            // If Gmail balks at the combined search, ask one conversation at a time.
            Err(_) if thrids.len() > 1 => {
                let mut all = Vec::new();
                for t in thrids {
                    all.extend(imap.uid_search(&format!("X-GM-THRID {t}"))?);
                }
                all
            }
            Err(e) => return Err(e),
        };
        let have = self.db.bodies_held(account_id, thrids)?;
        let mut need = Vec::new();
        for chunk in uids.chunks(500) {
            for f in imap.uid_fetch(&uid_set(chunk), "(UID X-GM-MSGID RFC822.SIZE)")? {
                let size: i64 = f.get("RFC822.SIZE").and_then(|s| s.text().parse().ok()).unwrap_or(0);
                let held = f.get("X-GM-MSGID").is_some_and(|m| have.contains(&m.text()));
                if !held && size <= max_size {
                    need.extend(fetched_uid(&f));
                }
            }
        }
        let mut added = 0;
        for chunk in need.chunks(8) {
            let fetched = imap.uid_fetch(&uid_set(chunk), FULL_ITEMS)?;
            let c = self.db.conn()?;
            for f in &fetched {
                insert_fetched(&c, account_id, f, false)?;
                added += store_fetched_body(&c, account_id, f)? as usize;
            }
        }
        Ok(added)
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
            if seen.get(&key).is_some_and(|t| t.elapsed() < REHYDRATE_AFTER) {
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
        let _one_at_a_time = self.hydrating.lock().unwrap();
        let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("no such account"))?;
        let mut imap = self.connect(&acct.email)?;
        let all_mail = imap.all_mail()?;
        imap.select(&all_mail, true)?;
        let thrids = [thrid.to_string()];
        // No size limit here: the user is looking at this conversation.
        let added = self.fill_threads(&mut imap, account_id, &thrids, i64::MAX)?;
        imap.logout();
        self.db.mark_hydrated(account_id, &thrids)?;
        if added > 0 {
            self.emit("thread", thrid);
        }
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
    Db::store_body(c, account_id, &msgid.text(), raw, &s.snippet, &s.text, &s.attachments)?;
    Ok(true)
}
