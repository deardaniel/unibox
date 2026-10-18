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
/// What is fetched for each message when All Mail is listed: enough headers to tell who it is
/// from, whether a person or a system sent it, and how to unsubscribe (see `facts`).
pub const INDEX_ITEMS: &str = "(UID X-GM-MSGID X-GM-THRID FLAGS INTERNALDATE RFC822.SIZE \
    BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT MESSAGE-ID IN-REPLY-TO LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST \
    LIST-ID LIST-POST PRECEDENCE AUTO-SUBMITTED FEEDBACK-ID X-MAILER USER-AGENT THREAD-INDEX X-MS-HAS-ATTACH \
    AUTHENTICATION-RESULTS X-GOOGLE-GROUP-ID X-ORIGINAL-SENDER X-ORIGINAL-FROM X-ORIGINAL-AUTHENTICATION-RESULTS \
    MAILING-LIST)])";
const FULL_ITEMS: &str = "(UID X-GM-MSGID X-GM-THRID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])";
/// Everything in the inbox is downloaded ahead of time, except messages bigger than this,
/// which load when opened.
const MAX_PREFETCH_SIZE: i64 = 25 * 1024 * 1024;
/// All Mail is listed (headers only) this many UIDs at a time, newest first.
const INDEX_CHUNK: u32 = 2000;
/// Every pass re-checks read/star marks and deletions among this many of the newest UIDs...
const RECENT_UIDS: u32 = 3000;
/// ...and the whole folder this often.
const SWEEP_EVERY: i64 = 3600;
const SWEEP_CHUNK: u32 = 20_000;
const SYNC_EVERY: Duration = Duration::from_secs(120);
/// ...unless every account has a live IDLE connection telling us about inbox changes as
/// they happen. Then the timed pass is only a safety net.
const SYNC_EVERY_WATCHED: Duration = Duration::from_secs(900);
/// How long one IDLE lasts before it is renewed. Short enough that a connection that died
/// quietly (sleep, a network change) is noticed within a few minutes.
const IDLE_FOR: Duration = Duration::from_secs(240);
/// How long one pass may spend preloading before it goes back to check for new mail.
const PRELOAD_BUDGET: Duration = Duration::from_secs(40);
/// No key pressed or mouse moved for this long, and new mail waits for one toast on return.
const AWAY_AFTER: Duration = Duration::from_secs(300);
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
    /// Requests for a sync pass: one account, or `None` for all of them.
    wake: Mutex<Option<mpsc::Sender<Option<i64>>>>,
    /// Accounts that have a watcher thread, and whether its IDLE connection is up right now.
    watchers: Mutex<HashMap<i64, bool>>,
    push_wake: Mutex<Option<mpsc::Sender<()>>>,
    /// (a pass is running, Unix time of the next scheduled pass, seconds between passes)
    status: Mutex<(bool, i64, u64)>,
    /// Background work worth mentioning while it lasts ("Listing older mail"); empty when none.
    activity: Mutex<String>,
    /// The same, for each account that has such work going on.
    doing: Mutex<HashMap<i64, String>>,
    pub agent: crate::agent::AgentState,
    /// Accounts whose inbox has been listed since the app started. Mail found on that first
    /// listing arrived while the app was closed, and gets no notification.
    listed: Mutex<HashSet<i64>>,
    /// New mail that arrived while nobody was at the PC, newest first, kept for one toast
    /// when someone is back.
    held: Mutex<Vec<crate::notify::Arrival>>,
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
            watchers: Mutex::new(HashMap::new()),
            push_wake: Mutex::new(None),
            status: Mutex::new((true, 0, SYNC_EVERY.as_secs())),
            activity: Mutex::new(String::new()),
            doing: Mutex::new(HashMap::new()),
            agent: Default::default(),
            listed: Mutex::new(HashSet::new()),
            held: Mutex::new(Vec::new()),
        })
    }

    pub fn emit(&self, kind: &str, detail: &str) {
        let _ = self.events.send(serde_json::json!({ "kind": kind, "detail": detail }).to_string());
    }

    fn lock_for(&self, account_id: i64) -> Arc<Mutex<()>> {
        self.locks.lock().unwrap().entry(account_id).or_default().clone()
    }

    pub(crate) fn connect(&self, email: &str) -> Result<Imap> {
        let password = secrets::get(&self.dir, email)?;
        Imap::connect(&self.imap_server, email, &password)
    }

    /// Check that an address and app password really log in.
    pub fn verify_login(&self, email: &str, password: &str) -> Result<()> {
        Imap::connect(&self.imap_server, email, password)?.logout();
        Ok(())
    }

    /// What the sync button shows: whether a pass is running and when the next one is due.
    pub fn sync_status(&self) -> serde_json::Value {
        let (syncing, next, every) = *self.status.lock().unwrap();
        let activity = self.activity.lock().unwrap().clone();
        serde_json::json!({ "syncing": syncing, "next": next, "every": every, "live": self.all_watched(), "activity": activity })
    }

    /// Say what the app is busy with in the background, or "" when it has finished.
    fn set_activity(&self, what: &str) {
        {
            let mut now = self.activity.lock().unwrap();
            if *now == what {
                return;
            }
            *now = what.to_string();
        }
        self.emit("sync", &self.sync_status().to_string());
    }

    fn set_status(&self, syncing: bool, next: i64, every: u64) {
        *self.status.lock().unwrap() = (syncing, next, every);
        self.emit("sync", &self.sync_status().to_string());
    }

    /// Ask the background thread to sync now instead of waiting for its timer.
    pub fn sync_soon(&self) {
        self.request_sync(None);
    }

    fn request_sync(&self, account: Option<i64>) {
        if let Some(tx) = self.wake.lock().unwrap().as_ref() {
            let _ = tx.send(account);
        }
    }

    /// Forget which conversations were topped up lately, so the next one opened is checked again.
    pub(crate) fn forget_hydrated(&self) {
        self.hydrated.lock().unwrap().clear();
    }

    /// What each account is busy with in the background, in the words the window shows.
    pub(crate) fn doing(&self) -> HashMap<i64, String> {
        self.doing.lock().unwrap().clone()
    }

    /// Which accounts have a live IDLE connection right now.
    pub(crate) fn watching(&self) -> HashMap<i64, bool> {
        self.watchers.lock().unwrap().clone()
    }

    /// True when every account has a live IDLE connection.
    fn all_watched(&self) -> bool {
        let w = self.watchers.lock().unwrap();
        !w.is_empty() && w.values().all(|up| *up)
    }

    /// Keep an IDLE connection open on an account's inbox and ask for a sync of that account
    /// the moment Gmail reports a change, instead of waiting for the timer.
    fn watch(self: &Arc<App>, account_id: i64) {
        {
            let mut w = self.watchers.lock().unwrap();
            if w.contains_key(&account_id) {
                return; // already being watched
            }
            w.insert(account_id, false);
        }
        let app = self.clone();
        std::thread::spawn(move || {
            let set_up = |up: bool| {
                let was = app.watchers.lock().unwrap().insert(account_id, up);
                if was != Some(up) {
                    app.emit("sync", &app.sync_status().to_string()); // the sync button shows it
                }
            };
            let mut retry = Duration::from_secs(15);
            let mut dropped = false;
            loop {
                let Ok(Some(acct)) = app.db.account(account_id) else { break }; // account removed
                let result = (|| -> Result<()> {
                    let mut imap = app.connect(&acct.email)?;
                    imap.select("INBOX", true)?;
                    if dropped {
                        // whatever happened while the connection was down was missed
                        app.request_sync(Some(account_id));
                        app.log(&format!("{}: watching the inbox again", acct.email));
                    }
                    loop {
                        let changed = imap.idle(IDLE_FOR, || set_up(true))?;
                        dropped = false;
                        retry = Duration::from_secs(15);
                        if changed {
                            app.request_sync(Some(account_id));
                        }
                        if !matches!(app.db.account(account_id), Ok(Some(_))) {
                            imap.logout();
                            return Ok(());
                        }
                    }
                })();
                set_up(false);
                if let Err(e) = result {
                    if !dropped {
                        app.log(&format!("{}: stopped watching the inbox ({e:#}); retrying", acct.email));
                    }
                    dropped = true;
                    std::thread::sleep(retry);
                    retry = (retry * 2).min(Duration::from_secs(300));
                }
            }
            app.watchers.lock().unwrap().remove(&account_id);
        });
    }

    pub fn start(self: &Arc<App>) {
        // Watches for someone coming back to the PC, to tell them what arrived meanwhile.
        let watcher = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            watcher.welcome_back();
        });

        // The push thread sends queued changes (archive, read, star) to Gmail.
        let (push_tx, push_rx) = mpsc::channel();
        *self.push_wake.lock().unwrap() = Some(push_tx);
        let pusher = self.clone();
        std::thread::spawn(move || loop {
            pusher.push_pending(); // first pass replays anything left from the last run
            let _ = push_rx.recv_timeout(Duration::from_secs(60));
            // let a burst of key presses land, so they go out as one batch
            std::thread::sleep(Duration::from_millis(250));
            while push_rx.try_recv().is_ok() {}
        });

        let (tx, rx) = mpsc::channel();
        *self.wake.lock().unwrap() = Some(tx);
        let app = self.clone();
        std::thread::spawn(move || {
            app.set_activity("Building the search index…");
            if let Err(e) = app.db.build_search() {
                app.log(&format!("building the search index: {e:#}"));
            }
            app.set_activity("");
            app.refresh_summaries();
            // which accounts this pass covers; None means all of them
            let mut only: Option<Vec<i64>> = None;
            let mut next_full = 0i64;
            loop {
                let every = if app.all_watched() { SYNC_EVERY_WATCHED } else { SYNC_EVERY };
                app.set_status(true, 0, every.as_secs());
                let mut more = false;
                for acct in app.db.accounts().unwrap_or_default() {
                    app.watch(acct.id);
                    if only.as_ref().is_some_and(|ids| !ids.contains(&acct.id)) {
                        continue;
                    }
                    let result = app.sync_account(acct.id, &acct.email);
                    let err = result.as_ref().err().map(|e| format!("{e:#}"));
                    if let Some(e) = &err {
                        app.log(&format!("sync {}: {e}", acct.email));
                    }
                    more |= result.unwrap_or(false);
                    let _ = app.db.set_sync_result(acct.id, err.as_deref());
                    app.emit("synced", &acct.email);
                }
                // While there is still mail to preload, go straight round again.
                // A pass for one account (Gmail just told us something changed there) leaves
                // the timer for the next full pass where it was.
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
                let every = if app.all_watched() { SYNC_EVERY_WATCHED } else { SYNC_EVERY }.as_secs();
                if only.is_none() || next_full > now + every as i64 {
                    next_full = now + every as i64;
                }
                if !more {
                    app.set_activity("");
                }
                if more {
                    // While there is still mail to preload, go straight round again.
                    std::thread::sleep(Duration::from_millis(200));
                    while rx.try_recv().is_ok() {}
                    only = None;
                    continue;
                }
                app.set_status(false, next_full, every);
                // Wait for the timer or a request, looking up now and then: if a watcher has
                // dropped meanwhile, the long wait is cut back to the ordinary one.
                let mut asked: Vec<Option<i64>> = Vec::new();
                loop {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
                    let short = SYNC_EVERY.as_secs();
                    if !app.all_watched() && next_full > now + short as i64 {
                        next_full = now + short as i64;
                        app.set_status(false, next_full, short);
                    }
                    if next_full <= now {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_secs(((next_full - now) as u64).min(20))) {
                        Ok(a) => {
                            asked.push(a);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                // collapse a burst of requests into one pass
                asked.extend(rx.try_iter());
                only = match !asked.is_empty() && asked.iter().all(Option::is_some) {
                    true => Some(asked.into_iter().flatten().collect()),
                    false => None, // the timer ran out, or someone asked for everything
                };
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
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for &id in chunk {
                    let raw: Vec<u8> = tx.query_row("SELECT raw FROM bodies WHERE message_id=?", [id], |r| r.get(0))?;
                    let s = mime::summarize(&raw);
                    Db::set_summary(&tx, id, &s)?;
                }
                tx.commit()?;
            }
            self.db.set_meta("summary_version", mime::SUMMARY_VERSION)?;
            // conversations already open were read in the old form: the page shows them again
            self.emit("summaries", "");
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
        let first_listing = !self.listed.lock().unwrap().contains(&account_id);
        let mut arrived = Vec::new();
        for chunk in new_uids.chunks(200) {
            let fetched = imap.uid_fetch(&uid_set(chunk), HEADER_ITEMS)?;
            let c = self.db.conn()?;
            for f in &fetched {
                insert_fetched(&c, account_id, f, true)?;
                if !first_listing {
                    arrived.extend(arrival(account_id, email, f));
                }
            }
            self.emit("changed", email);
        }
        self.listed.lock().unwrap().insert(account_id);
        self.announce(email, &arrived);

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

        // Last, the list of everything else: All Mail, headers only.
        if !all_mail_open {
            let all_mail = imap.all_mail()?;
            imap.select(&all_mail, true)?;
        }
        let more = self.index_all_mail(&mut imap, account_id, email, deadline)?;
        imap.logout();
        Ok(more)
    }

    /// With All Mail selected, bring the local list of that folder up to date: headers of
    /// anything new, read/star marks and deletions among recent mail (and now and then all
    /// of it), then older mail a chunk at a time until `deadline`. Bodies are not fetched;
    /// they come when a conversation is opened. Returns whether older mail is still to come.
    fn index_all_mail(&self, imap: &mut Imap, account_id: i64, email: &str, deadline: Instant) -> Result<bool> {
        let Some(top) = imap.uid_fetch("*", "(UID)")?.iter().filter_map(fetched_uid).max() else {
            return Ok(false); // nothing there at all
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
        let mut at = self.db.all_mail(account_id)?.unwrap_or(crate::db::AllMail { low: top + 1, high: top, swept: now });
        if top > at.high {
            let mut from = at.high + 1;
            while from <= top {
                let to = top.min(from + INDEX_CHUNK - 1);
                self.index_range(imap, account_id, from, to)?;
                from = to + 1;
            }
            let new = at.high + 1;
            at.high = top;
            self.db.set_all_mail(account_id, &at)?;
            self.emit("changed", email);
            self.fill_new_replies(imap, account_id, new, top)?;
        }
        if at.low <= at.high {
            let whole = now - at.swept >= SWEEP_EVERY;
            let mut from = if whole { at.low } else { at.low.max(at.high.saturating_sub(RECENT_UIDS)) };
            let mut changed = false;
            while from <= at.high {
                let to = at.high.min(from + SWEEP_CHUNK - 1);
                changed |= self.sweep_range(imap, account_id, from, to)?;
                from = to + 1;
            }
            if whole {
                at.swept = now;
                self.db.set_all_mail(account_id, &at)?;
            }
            if changed {
                self.emit("changed", email);
            }
        }
        while at.low > 1 && Instant::now() < deadline {
            let from = at.low.saturating_sub(INDEX_CHUNK).max(1);
            let oldest = self.index_range(imap, account_id, from, at.low - 1)?;
            at.low = from;
            self.db.set_all_mail(account_id, &at)?;
            if let Some(oldest) = oldest {
                let what = format!("Listing older mail… back to {}", month_year(oldest));
                self.set_activity(&what);
                self.doing.lock().unwrap().insert(account_id, what);
            }
            self.emit("indexed", email);
        }
        if at.low <= 1 {
            self.doing.lock().unwrap().remove(&account_id);
        }
        Ok(at.low > 1)
    }

    /// With All Mail selected, fetch the bodies of messages just listed there (UIDs `from..=to`)
    /// that belong to a conversation in the inbox: a reply sent from another mail program
    /// joins a conversation that has already been topped up, and would otherwise wait to be
    /// opened afresh before it showed more than its headers.
    fn fill_new_replies(&self, imap: &mut Imap, account_id: i64, from: u32, to: u32) -> Result<()> {
        let want: Vec<(u32, String)> = {
            let c = self.db.conn()?;
            let mut st = c.prepare(
                "SELECT m.all_uid, m.gm_thrid FROM messages m
                  WHERE m.account_id=?1 AND m.all_uid BETWEEN ?2 AND ?3 AND m.has_body=0 AND m.size<=?4
                    AND EXISTS (SELECT 1 FROM messages i WHERE i.account_id=?1 AND i.gm_thrid=m.gm_thrid AND i.in_inbox=1)",
            )?;
            let rows = st.query_map(params![account_id, from, to, MAX_PREFETCH_SIZE], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let uids: Vec<u32> = want.iter().map(|w| w.0).collect();
        for chunk in uids.chunks(8) {
            let fetched = imap.uid_fetch(&uid_set(chunk), FULL_ITEMS)?;
            let c = self.db.conn()?;
            for f in &fetched {
                store_fetched_body(&c, account_id, f)?;
            }
        }
        let mut thrids: Vec<String> = want.into_iter().map(|w| w.1).collect();
        thrids.sort_unstable();
        thrids.dedup();
        for thrid in thrids {
            self.emit("thread", &thrid);
        }
        Ok(())
    }

    /// List the All Mail messages with UIDs `from..=to`. Drafts are left out. Returns when the
    /// oldest of them arrived.
    fn index_range(&self, imap: &mut Imap, account_id: i64, from: u32, to: u32) -> Result<Option<i64>> {
        let fetched = imap.uid_fetch(&format!("{from}:{to}"), INDEX_ITEMS)?;
        let mut c = self.db.conn()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for f in &fetched {
            // "n:*" style answers can include a message outside the range asked for
            let (Some(uid), Some(msgid)) = (fetched_uid(f), f.get("X-GM-MSGID")) else { continue };
            if uid < from || uid > to || is_draft(f) {
                continue;
            }
            insert_fetched(&tx, account_id, f, false)?;
            // what the headers say about it stands until the whole message is held
            let (said, automated) = f.get("BODY").and_then(|b| mime::header_facts(b.bytes())).unzip();
            tx.execute(
                "UPDATE messages SET all_uid=?1,
                        facts=CASE WHEN has_body=0 THEN COALESCE(?4, facts) ELSE facts END,
                        automated=CASE WHEN has_body=0 THEN COALESCE(?5, automated) ELSE automated END
                  WHERE account_id=?2 AND gm_msgid=?3",
                params![uid, account_id, msgid.text(), said, automated],
            )?;
        }
        tx.commit()?;
        Ok(fetched.iter().filter_map(|f| parse_internaldate(&f.get("INTERNALDATE")?.text())).min())
    }

    /// Check the listed All Mail messages with UIDs `from..=to` against the server: take on
    /// read/star marks changed elsewhere, and forget messages that are gone (deleted, or
    /// moved to Spam). Returns whether anything changed.
    fn sweep_range(&self, imap: &mut Imap, account_id: i64, from: u32, to: u32) -> Result<bool> {
        let listing: HashMap<u32, (bool, bool)> = imap
            .uid_fetch(&format!("{from}:{to}"), "(UID FLAGS)")?
            .iter()
            .filter_map(|f| Some((fetched_uid(f)?, fetched_flags(f))))
            .collect();
        let mut c = self.db.conn()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let local: Vec<(i64, u32, bool, bool, bool)> = {
            // a change of ours that is still on its way to Gmail wins
            let mut st = tx.prepare(
                "SELECT id, all_uid, seen, starred, in_inbox FROM messages
                  WHERE account_id=?1 AND all_uid BETWEEN ?2 AND ?3
                    AND gm_msgid NOT IN (SELECT gm_msgid FROM pending WHERE account_id=?1)",
            )?;
            let rows = st.query_map(params![account_id, from, to], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut changed = false;
        for (id, uid, seen, starred, in_inbox) in local {
            match listing.get(&uid) {
                Some(&(s, f)) if (s, f) != (seen, starred) => {
                    tx.execute("UPDATE messages SET seen=?, starred=? WHERE id=?", params![s, f, id])?;
                    changed = true;
                }
                Some(_) => {}
                // still in the inbox as far as we know: the inbox sync deals with it first
                None if in_inbox => {}
                None => {
                    tx.execute("DELETE FROM messages WHERE id=?", [id])?;
                    changed = true;
                }
            }
        }
        tx.commit()?;
        Ok(changed)
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
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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

    /// Apply a user action to a whole conversation. The local copy changes at once and this
    /// returns; the change is queued and sent to Gmail by the push thread.
    ///
    /// Transactions here and elsewhere in this file are opened IMMEDIATE. They read and then
    /// write, and a deferred transaction doing that fails outright with "database is locked"
    /// whenever another thread has written in between, which the busy timeout cannot wait out.
    pub fn act(&self, account_id: i64, thrid: &str, op: &str) -> Result<Vec<String>> {
        let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("no such account"))?;
        if op == "inbox" {
            // Move to inbox: the reverse of archiving, for every message of the conversation.
            let msgids: Vec<String> = {
                let c = self.db.conn()?;
                let mut st = c.prepare(
                    "SELECT gm_msgid FROM messages WHERE account_id=? AND gm_thrid=? AND in_inbox=0 AND all_uid IS NOT NULL",
                )?;
                let rows = st.query_map(params![account_id, thrid], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            if !msgids.is_empty() {
                self.unarchive(account_id, &msgids)?;
            }
            return Ok(msgids);
        }
        let local = match op {
            "archive" => "in_inbox=0",
            "read" => "seen=1",
            "unread" => "seen=0",
            "star" => "starred=1",
            "unstar" => "starred=0",
            _ => return Err(anyhow!("unknown action {op}")),
        };
        // Gmail stars, and marks unread, the latest message of a conversation: the latest in
        // the inbox when it has any there.
        let only_latest = if matches!(op, "star" | "unread") { " ORDER BY in_inbox DESC, date DESC LIMIT 1" } else { "" };
        // A message is addressed by its UID in the inbox while it is there, and otherwise by
        // its UID in All Mail (queued as "all:<op>"), where only marks can change: there is
        // nothing to archive, and no point re-marking what is already so.
        let elsewhere = match op {
            "archive" => "0",
            "read" => "seen=0",
            "unstar" => "starred=1",
            _ => "1",
        };
        let mut c = self.db.conn()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // A message put back in the inbox by an undo has no inbox UID until the next sync. To
        // archive it again, the undo is withdrawn if it has not been sent, or else the message
        // is looked up in the inbox when the archive is sent (uid 0).
        let unplaced = if op == "archive" { "1" } else { "0" };
        let targets: Vec<(String, u32, bool)> = {
            let mut st = tx.prepare(&format!(
                "SELECT gm_msgid, CASE WHEN in_inbox=1 AND uid IS NOT NULL THEN uid WHEN in_inbox=1 THEN 0 ELSE all_uid END,
                        in_inbox=1
                   FROM messages
                  WHERE account_id=? AND gm_thrid=?
                    AND ((in_inbox=1 AND uid IS NOT NULL) OR (in_inbox=1 AND uid IS NULL AND {unplaced})
                         OR (in_inbox=0 AND all_uid IS NOT NULL AND {elsewhere}))
                  {only_latest}"
            ))?;
            let rows = st.query_map(params![account_id, thrid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (msgid, uid, inbox) in &targets {
            let withdrawn = *uid == 0
                && tx.execute(
                    "DELETE FROM pending WHERE account_id=? AND gm_msgid=? AND op='unarchive'",
                    params![account_id, msgid],
                )? > 0;
            let queued = if *inbox { op.to_string() } else { format!("all:{op}") };
            if !withdrawn {
                tx.execute(
                    "INSERT INTO pending(account_id, gm_msgid, op, uid) VALUES(?,?,?,?)",
                    params![account_id, msgid, queued, uid],
                )?;
            }
            tx.execute(
                &format!("UPDATE messages SET {local} WHERE account_id=? AND gm_msgid=?"),
                params![account_id, msgid],
            )?;
        }
        tx.commit()?;
        if !targets.is_empty() {
            self.emit("changed", &acct.email);
            self.push_soon();
        }
        Ok(targets.into_iter().map(|(msgid, _, _)| msgid).collect())
    }

    /// Undo an archive: put these messages back in the inbox. An archive that has not been
    /// sent yet is simply withdrawn; one that has is reversed on Gmail.
    pub fn unarchive(&self, account_id: i64, msgids: &[String]) -> Result<()> {
        let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("no such account"))?;
        // Holding the account lock means the push thread is not half-way through these.
        let lock = self.lock_for(account_id);
        let _guard = lock.lock().unwrap();
        let mut c = self.db.conn()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for msgid in msgids {
            let withdrawn = tx.execute(
                "DELETE FROM pending WHERE account_id=? AND gm_msgid=? AND op='archive'",
                params![account_id, msgid],
            )?;
            if withdrawn == 0 {
                // already archived on Gmail; uid 0 = to be looked up in All Mail
                tx.execute(
                    "INSERT INTO pending(account_id, gm_msgid, op, uid) VALUES(?,?,'unarchive',0)",
                    params![account_id, msgid],
                )?;
            }
            tx.execute(
                "UPDATE messages SET in_inbox=1 WHERE account_id=? AND gm_msgid=?",
                params![account_id, msgid],
            )?;
        }
        tx.commit()?;
        self.emit("changed", &acct.email);
        self.push_soon();
        Ok(())
    }

    fn push_soon(&self) {
        if let Some(tx) = self.push_wake.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
    }

    /// Send every queued change to Gmail: one connection per account, consecutive changes
    /// of the same kind in one command. Ten quick archives become a single MOVE.
    fn push_pending(&self) {
        type Row = (i64, i64, String, String, u32); // (row id, account, message id, op, uid)
        let read = |account: Option<i64>| -> Result<Vec<Row>> {
            let c = self.db.conn()?;
            let mut st = c.prepare(
                "SELECT id, account_id, gm_msgid, op, uid FROM pending WHERE ?1 IS NULL OR account_id=?1 ORDER BY id",
            )?;
            let rows = st.query_map([account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        let mut accounts: Vec<i64> = match read(None) {
            Ok(rows) => rows.iter().map(|r| r.1).collect(),
            Err(e) => return self.log(&format!("reading queued changes: {e:#}")),
        };
        accounts.sort_unstable();
        accounts.dedup();
        for account_id in accounts {
            let lock = self.lock_for(account_id);
            let _guard = lock.lock().unwrap();
            // Read the queue only once the lock is held: an undo may just have withdrawn rows.
            let rows = match read(Some(account_id)) {
                Ok(rows) => rows,
                Err(e) => return self.log(&format!("reading queued changes: {e:#}")),
            };
            let mine: Vec<&Row> = rows.iter().collect();
            // consecutive changes of one kind form a batch
            let mut batches: Vec<(&str, Vec<&Row>)> = Vec::new();
            for r in mine {
                match batches.last_mut() {
                    Some((op, batch)) if *op == r.3 => batch.push(r),
                    _ => batches.push((r.3.as_str(), vec![r])),
                }
            }
            if batches.is_empty() {
                continue;
            }
            let mut done: Vec<i64> = Vec::new();
            let result = (|| -> Result<()> {
                let acct = self.db.account(account_id)?.ok_or_else(|| anyhow!("account removed"))?;
                let mut imap = self.connect(&acct.email)?;
                imap.select("INBOX", false)?;
                let mut all_mail = None;
                for (op, batch) in &batches {
                    let uids: Vec<u32> = batch.iter().map(|r| r.4).collect();
                    let set = uid_set(&uids);
                    match *op {
                        // Moving to All Mail is how Gmail spells "archive" over IMAP.
                        "archive" => {
                            if all_mail.is_none() {
                                all_mail = Some(imap.all_mail()?);
                            }
                            // uid 0: back in the inbox after an undo, under a UID not yet known
                            let mut uids: Vec<u32> = uids.iter().copied().filter(|&u| u != 0).collect();
                            for r in batch.iter().filter(|r| r.4 == 0) {
                                if r.2.chars().all(|c| c.is_ascii_digit()) {
                                    uids.extend(imap.uid_search(&format!("X-GM-MSGID {}", r.2))?);
                                }
                            }
                            if !uids.is_empty() {
                                imap.run(&format!("UID MOVE {} {}", uid_set(&uids), all_mail.as_deref().unwrap_or_default()))?;
                            }
                        }
                        // The reverse: find each message in All Mail and copy it to the inbox,
                        // which on Gmail just puts the Inbox label back.
                        "unarchive" => {
                            if all_mail.is_none() {
                                all_mail = Some(imap.all_mail()?);
                            }
                            imap.select(all_mail.as_deref().unwrap_or_default(), false)?;
                            let mut found = Vec::new();
                            for r in batch {
                                if r.2.chars().all(|c| c.is_ascii_digit()) {
                                    found.extend(imap.uid_search(&format!("X-GM-MSGID {}", r.2))?);
                                }
                            }
                            if !found.is_empty() {
                                imap.run(&format!("UID COPY {} INBOX", uid_set(&found)))?;
                            }
                            imap.select("INBOX", false)?;
                        }
                        // Marks on messages that are not in the inbox are set in All Mail.
                        marks if marks.starts_with("all:") => {
                            let change = match &marks[4..] {
                                "read" => "+FLAGS.SILENT (\\Seen)",
                                "unread" => "-FLAGS.SILENT (\\Seen)",
                                "star" => "+FLAGS.SILENT (\\Flagged)",
                                "unstar" => "-FLAGS.SILENT (\\Flagged)",
                                other => return Err(anyhow!("unknown queued change all:{other}")),
                            };
                            if all_mail.is_none() {
                                all_mail = Some(imap.all_mail()?);
                            }
                            imap.select(all_mail.as_deref().unwrap_or_default(), false)?;
                            imap.run(&format!("UID STORE {set} {change}"))?;
                            imap.select("INBOX", false)?;
                        }
                        "read" => drop(imap.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Seen)"))?),
                        "unread" => drop(imap.run(&format!("UID STORE {set} -FLAGS.SILENT (\\Seen)"))?),
                        "star" => drop(imap.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Flagged)"))?),
                        "unstar" => drop(imap.run(&format!("UID STORE {set} -FLAGS.SILENT (\\Flagged)"))?),
                        other => return Err(anyhow!("unknown queued change {other}")),
                    }
                    let c = self.db.conn()?;
                    for r in batch {
                        c.execute("DELETE FROM pending WHERE id=?", [r.0])?;
                        if *op == "archive" {
                            c.execute(
                                "UPDATE messages SET uid=NULL WHERE account_id=? AND gm_msgid=? AND in_inbox=0",
                                params![account_id, r.2],
                            )?;
                        }
                        done.push(r.0);
                    }
                    self.log(&format!("{}: {op} sent for {} message(s)", acct.email, batch.len()));
                }
                imap.logout();
                // messages put back in the inbox need their new inbox UIDs
                if batches.iter().any(|(op, _)| *op == "unarchive") {
                    self.sync_soon();
                }
                Ok(())
            })();
            if let Err(e) = result {
                // Give up on what is left for this account. The local copy is now wrong for
                // those messages; the next sync puts back what Gmail really has.
                self.log(&format!("account {account_id}: sending changes failed: {e:#}"));
                if let Ok(c) = self.db.conn() {
                    for (_, batch) in &batches {
                        for r in batch.iter().filter(|r| !done.contains(&r.0)) {
                            let _ = c.execute("DELETE FROM pending WHERE id=?", [r.0]);
                        }
                    }
                }
                self.emit("error", &format!("Gmail didn't accept a change, so it has been undone: {e:#}"));
                self.sync_soon();
            }
        }
    }

    /// Append a line to unibox.log in the data folder. The Windows build has no console.
    /// Whether new mail shows a notification (on unless turned off in Appearance).
    pub fn notify_on(&self) -> bool {
        self.db.meta("notify").ok().flatten().as_deref() != Some("off")
    }

    /// Show a notification for mail that just arrived, unless they are off or Unibox is in
    /// front. While nobody is at the PC it is held for one toast on their return instead:
    /// otherwise Windows saves up every toast and plays them back one by one.
    fn announce(&self, email: &str, arrived: &[crate::notify::Arrival]) {
        if arrived.is_empty() || !self.notify_on() || crate::notify::app_in_front() {
            return;
        }
        if crate::notify::idle() >= AWAY_AFTER {
            let mut held = self.held.lock().unwrap();
            held.splice(0..0, arrived.iter().cloned());
            held.truncate(500);
            crate::doctor::note("app", "Windows", &format!("New mail notification, {} new", arrived.len()), "held: nobody at the PC", true, None);
            return;
        }
        let lang = self.db.meta("notify-lang").ok().flatten().unwrap_or_default();
        for toast in crate::notify::toasts(arrived, email, &lang) {
            self.show_toast(&toast, arrived.len());
        }
    }

    /// Once someone is at the PC again, one toast for the mail held while they were away,
    /// leaving out what was read or archived meanwhile (on a phone, say).
    fn welcome_back(&self) {
        if self.held.lock().unwrap().is_empty() || crate::notify::idle() > Duration::from_secs(5) {
            return;
        }
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        if !self.notify_on() || crate::notify::app_in_front() {
            return;
        }
        let Ok(c) = self.db.conn() else { return };
        let unread: Vec<_> = held
            .into_iter()
            .filter(|a| {
                c.query_row(
                    "SELECT 1 FROM messages WHERE account_id=? AND gm_msgid=? AND seen=0 AND in_inbox=1",
                    params![a.account_id, a.gm_msgid],
                    |_| Ok(()),
                )
                .is_ok()
            })
            .collect();
        let lang = self.db.meta("notify-lang").ok().flatten().unwrap_or_default();
        if let Some(toast) = crate::notify::away_toast(&unread, &lang) {
            self.show_toast(&toast, unread.len());
        }
    }

    fn show_toast(&self, toast: &crate::notify::Toast, messages: usize) {
        let events = self.events.clone();
        // clicking opens the conversation in the main window
        let shown = crate::notify::show(toast, move |open| {
            if !open.is_empty() {
                let _ = events.send(serde_json::json!({ "kind": "open", "detail": open }).to_string());
            }
        });
        let outcome = shown.as_ref().map(|_| "shown".to_string()).unwrap_or_else(|e| e.to_string());
        crate::doctor::note("app", "Windows", &format!("New mail notification, {messages} new"), &outcome, shown.is_ok(), None);
    }

    pub fn log(&self, line: &str) {
        use std::io::Write;
        eprintln!("{line}");
        crate::doctor::note("app", "", line, "", !line.contains("fail") && !line.contains("stopped"), None);
        let path = self.dir.join("unibox.log");
        if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
            let _ = std::fs::remove_file(&path);
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(format!("{secs} {line}\n").as_bytes());
        }
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

/// Unix seconds as "March 2019" (UTC; near enough for saying how far back something goes).
fn month_year(secs: i64) -> String {
    // days since 1970 to a civil date, after Howard Hinnant's algorithm
    let z = secs.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const NAMES: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    format!("{} {year}", NAMES[(month - 1) as usize])
}

/// Mail worth a notification: unread, not a draft, not sent by the account itself, and from
/// the last day (so an old message moved back into the inbox stays quiet).
fn arrival(account_id: i64, email: &str, f: &Fetched) -> Option<crate::notify::Arrival> {
    let date = f.get("INTERNALDATE").and_then(|d| parse_internaldate(&d.text()))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    if fetched_flags(f).0 || is_draft(f) || date < now - 86_400 {
        return None;
    }
    let headers = mime::parse_headers(f.get("BODY").map(|b| b.bytes()).unwrap_or_default());
    if headers.from_addr.eq_ignore_ascii_case(email) {
        return None;
    }
    Some(crate::notify::Arrival {
        account_id,
        gm_msgid: f.get("X-GM-MSGID")?.text(),
        thrid: f.get("X-GM-THRID")?.text(),
        from: if headers.from_name.is_empty() { headers.from_addr } else { headers.from_name },
        subject: headers.subject,
    })
}

fn is_draft(f: &Fetched) -> bool {
    f.get("FLAGS").is_some_and(|v| v.list().iter().any(|a| a.text().eq_ignore_ascii_case("\\draft")))
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
    Db::store_body(c, account_id, &msgid.text(), raw, &s)?;
    Ok(true)
}

#[cfg(test)]
mod progress_tests {
    #[test]
    fn says_how_far_back() {
        assert_eq!(super::month_year(0), "January 1970");
        assert_eq!(super::month_year(1_551_398_400), "March 2019");
        assert_eq!(super::month_year(1_582_934_400), "February 2020"); // 29 Feb
        assert_eq!(super::month_year(1_798_761_599), "December 2026");
    }
}
