//! SQLite store. One connection per operation; WAL keeps readers and the sync thread apart.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Db {
    path: PathBuf,
}

#[derive(Serialize, Clone)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub color: String,
    pub unread: i64,
    pub last_sync: Option<i64>,
    pub last_error: Option<String>,
    /// Older mail is still being listed from All Mail.
    pub indexing: bool,
}

/// A message `m` in a conversation that has nothing in the inbox any more.
const SPARE: &str = "NOT EXISTS (SELECT 1 FROM messages i WHERE i.account_id=m.account_id AND i.gm_thrid=m.gm_thrid AND i.in_inbox=1)";

/// Bump to have the search index rebuilt from scratch on the next start.
const SEARCH_INDEX: &str = "1";

/// How to look `query` up in the search index: every word must appear in the message, as a
/// word or as part of one. `None` if the index cannot answer it, which is when a word is
/// shorter than the three characters it is built from.
fn search_terms(query: &str) -> Option<String> {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() || words.iter().any(|w| w.chars().count() < 3) {
        return None;
    }
    Some(words.iter().map(|w| format!("\"{}\"", w.replace('"', "\"\""))).collect::<Vec<_>>().join(" "))
}

/// Bump when listing All Mail starts to record more about each message, to have it listed again.
const ALL_MAIL_LISTING: &str = "2";

/// The part of an account's All Mail folder that is listed locally: every message with a
/// UID from `low` to `high`. `low` works its way down to 1. `swept` is when read/star marks
/// and deletions were last checked across the whole of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllMail {
    pub low: u32,
    pub high: u32,
    pub swept: i64,
}

impl AllMail {
    fn parse(v: &str) -> Option<AllMail> {
        let mut n = v.split(',');
        let at = AllMail { low: n.next()?.parse().ok()?, high: n.next()?.parse().ok()?, swept: n.next()?.parse().ok()? };
        // listed the way an earlier version did: start over, to pick up what it left out
        (n.next() == Some(ALL_MAIL_LISTING)).then_some(at)
    }
}

#[derive(Serialize)]
pub struct ThreadRow {
    pub account_id: i64,
    pub thrid: String,
    pub date: i64,
    pub subject: String,
    pub snippet: String,
    pub senders: String,
    pub count: i64,
    pub unread: i64,
    pub starred: bool,
    pub has_attach: bool,
    /// Sender of the newest message, for the avatar.
    pub from_name: String,
    pub from_addr: String,
    /// False for a conversation shown from outside the inbox (in a set the assistant picked).
    pub in_inbox: bool,
    /// A person wrote at least one of its messages (as far as the app can tell).
    pub people: bool,
}

#[derive(Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub date: i64,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub subject: String,
    pub snippet: String,
    pub seen: bool,
    pub starred: bool,
    pub in_inbox: bool,
    pub has_body: bool,
    pub attachments: serde_json::Value,
    /// What the message says about itself: mailing list, group, event, booking (see `facts`).
    pub facts: serde_json::Value,
}

/// Header-level facts about a message, as found on the server.
pub struct NewMessage<'a> {
    pub gm_msgid: &'a str,
    pub gm_thrid: &'a str,
    pub uid: Option<u32>,
    pub in_inbox: bool,
    pub seen: bool,
    pub starred: bool,
    pub date: i64,
    pub size: i64,
    pub from_name: &'a str,
    pub from_addr: &'a str,
    pub to_addrs: &'a str,
    pub cc_addrs: &'a str,
    pub subject: &'a str,
}

const COLORS: [&str; 6] = ["#5b8def", "#e0723c", "#3fae7a", "#b667d6", "#d9a521", "#d8587e"];

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let db = Db { path: path.to_path_buf() };
        let c = db.conn()?;
        // The first version of this table only marked messages; it could not replay anything.
        let old_pending: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('pending') WHERE name='gm_msgid'")?
            .exists([])?
            && !c.prepare("SELECT 1 FROM pragma_table_info('pending') WHERE name='op'")?.exists([])?;
        if old_pending {
            c.execute("DROP TABLE pending", [])?;
        }
        c.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS accounts(
               id INTEGER PRIMARY KEY,
               email TEXT NOT NULL UNIQUE,
               color TEXT NOT NULL,
               last_sync INTEGER,
               last_error TEXT);
             CREATE TABLE IF NOT EXISTS messages(
               id INTEGER PRIMARY KEY,
               account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
               gm_msgid TEXT NOT NULL,
               gm_thrid TEXT NOT NULL,
               uid INTEGER,                      -- UID in INBOX, while it is there
               in_inbox INTEGER NOT NULL DEFAULT 0,
               seen INTEGER NOT NULL DEFAULT 0,
               starred INTEGER NOT NULL DEFAULT 0,
               date INTEGER NOT NULL,
               size INTEGER NOT NULL DEFAULT 0,
               from_name TEXT NOT NULL DEFAULT '',
               from_addr TEXT NOT NULL DEFAULT '',
               to_addrs TEXT NOT NULL DEFAULT '',
               cc_addrs TEXT NOT NULL DEFAULT '',
               subject TEXT NOT NULL DEFAULT '',
               snippet TEXT NOT NULL DEFAULT '',
               body_text TEXT NOT NULL DEFAULT '',
               has_attach INTEGER NOT NULL DEFAULT 0,
               has_body INTEGER NOT NULL DEFAULT 0,
               UNIQUE(account_id, gm_msgid));
             CREATE INDEX IF NOT EXISTS messages_thread ON messages(account_id, gm_thrid);
             CREATE INDEX IF NOT EXISTS messages_inbox ON messages(in_inbox, date);
             CREATE TABLE IF NOT EXISTS bodies(
               message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
               raw BLOB NOT NULL);
             -- A local change that has not reached Gmail yet, in the order it was made. While a
             -- row exists, sync leaves that message's inbox/read/star state alone. Rows survive
             -- a restart, so a change made just before closing the app is still sent.
             CREATE TABLE IF NOT EXISTS pending(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               account_id INTEGER NOT NULL,
               gm_msgid TEXT NOT NULL,
               op TEXT NOT NULL,
               uid INTEGER NOT NULL);
             -- Conversations whose other messages (sent replies, archived mail) have been
             -- pulled from All Mail already.
             CREATE TABLE IF NOT EXISTS hydrated(
               account_id INTEGER NOT NULL,
               gm_thrid TEXT NOT NULL,
               PRIMARY KEY(account_id, gm_thrid));
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             -- The assistant's conversation, exactly as exchanged with the model: one row per
             -- message, content as the API's JSON. Append-only.
             CREATE TABLE IF NOT EXISTS agent_log(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               at INTEGER NOT NULL);
             -- Senders whose lists were left with the Unsubscribe button.
             CREATE TABLE IF NOT EXISTS unsubscribed(sender TEXT PRIMARY KEY, at INTEGER NOT NULL);",
        )?;
        let has_column = |name: &str| -> Result<bool> {
            let mut st = c.prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name=?")?;
            Ok(st.exists([name])?)
        };
        if !has_column("attachments")? {
            // JSON list of attachments, filled in when the body is stored
            c.execute("ALTER TABLE messages ADD COLUMN attachments TEXT", [])?;
        }
        if !has_column("facts")? {
            // JSON from `facts`, and whether a system rather than a person sent the message
            c.execute("ALTER TABLE messages ADD COLUMN facts TEXT", [])?;
            c.execute("ALTER TABLE messages ADD COLUMN automated INTEGER", [])?;
        }
        if !has_column("all_uid")? {
            // UID in All Mail, once the background index of that folder has reached the message
            c.execute("ALTER TABLE messages ADD COLUMN all_uid INTEGER", [])?;
        }
        c.execute("CREATE INDEX IF NOT EXISTS messages_all ON messages(account_id, all_uid)", [])?;
        // The search index: every run of three characters in the subject, sender and text of
        // each message, so a word or part of one is found without reading all the mail. It
        // holds no text of its own (it refers back to `messages`) and is kept in step by the
        // triggers. `build_search` fills it the first time.
        c.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
               subject, from_name, from_addr, body_text,
               content='messages', content_rowid='id', tokenize='trigram');
             CREATE TRIGGER IF NOT EXISTS search_insert AFTER INSERT ON messages BEGIN
               INSERT INTO search(rowid, subject, from_name, from_addr, body_text)
               VALUES(new.id, new.subject, new.from_name, new.from_addr, new.body_text);
             END;
             CREATE TRIGGER IF NOT EXISTS search_delete AFTER DELETE ON messages BEGIN
               INSERT INTO search(search, rowid, subject, from_name, from_addr, body_text)
               VALUES('delete', old.id, old.subject, old.from_name, old.from_addr, old.body_text);
             END;
             CREATE TRIGGER IF NOT EXISTS search_update AFTER UPDATE OF subject, from_name, from_addr, body_text ON messages BEGIN
               INSERT INTO search(search, rowid, subject, from_name, from_addr, body_text)
               VALUES('delete', old.id, old.subject, old.from_name, old.from_addr, old.body_text);
               INSERT INTO search(rowid, subject, from_name, from_addr, body_text)
               VALUES(new.id, new.subject, new.from_name, new.from_addr, new.body_text);
             END;",
        )?;
        Ok(db)
    }

    pub fn conn(&self) -> Result<Connection> {
        let c = Connection::open(&self.path)?;
        c.busy_timeout(std::time::Duration::from_secs(30))?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        Ok(c)
    }

    // ---- accounts --------------------------------------------------------
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.conn()?;
        let mut st = c.prepare(
            "SELECT a.id, a.email, a.color, a.last_sync, a.last_error,
                    (SELECT COUNT(DISTINCT gm_thrid) FROM messages m
                      WHERE m.account_id=a.id AND m.in_inbox=1 AND m.seen=0),
                    (SELECT value FROM meta WHERE key='allmail:' || a.id)
               FROM accounts a ORDER BY a.id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                email: r.get(1)?,
                color: r.get(2)?,
                last_sync: r.get(3)?,
                last_error: r.get(4)?,
                unread: r.get(5)?,
                indexing: r.get::<_, Option<String>>(6)?.and_then(|v| AllMail::parse(&v)).map_or(true, |a| a.low > 1),
            })
        })?;
        let mut accounts: Vec<Account> = rows.collect::<rusqlite::Result<_>>()?;
        // The order chosen in the sidebar; accounts it doesn't mention (new ones) go last.
        let order: Vec<i64> = c
            .query_row("SELECT value FROM meta WHERE key='account_order'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        accounts.sort_by_key(|a| (order.iter().position(|id| *id == a.id).unwrap_or(usize::MAX), a.id));
        Ok(accounts)
    }

    pub fn set_account_order(&self, ids: &[i64]) -> Result<()> {
        self.set_meta("account_order", &serde_json::to_string(ids)?)
    }

    pub fn account(&self, id: i64) -> Result<Option<Account>> {
        Ok(self.accounts()?.into_iter().find(|a| a.id == id))
    }

    pub fn add_account(&self, email: &str) -> Result<i64> {
        let c = self.conn()?;
        let n: i64 = c.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
        c.execute(
            "INSERT INTO accounts(email, color) VALUES(?,?)",
            params![email, COLORS[n as usize % COLORS.len()]],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn remove_account(&self, id: i64) -> Result<()> {
        let c = self.conn()?;
        c.execute("DELETE FROM pending WHERE account_id=?", [id])?;
        c.execute("DELETE FROM hydrated WHERE account_id=?", [id])?;
        c.execute("DELETE FROM accounts WHERE id=?", [id])?;
        Ok(())
    }

    pub fn set_sync_result(&self, id: i64, error: Option<&str>) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        self.conn()?.execute(
            "UPDATE accounts SET last_sync=?, last_error=? WHERE id=?",
            params![now, error, id],
        )?;
        Ok(())
    }

    // ---- messages --------------------------------------------------------
    /// Insert a message we have not seen before. Existing rows are left alone.
    pub fn insert_message(c: &Connection, account_id: i64, m: &NewMessage) -> Result<()> {
        c.execute(
            "INSERT OR IGNORE INTO messages(account_id, gm_msgid, gm_thrid, uid, in_inbox, seen, starred,
                                            date, size, from_name, from_addr, to_addrs, cc_addrs, subject)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                account_id, m.gm_msgid, m.gm_thrid, m.uid, m.in_inbox, m.seen, m.starred, m.date, m.size,
                m.from_name, m.from_addr, m.to_addrs, m.cc_addrs, m.subject
            ],
        )?;
        Ok(())
    }

    pub fn store_body(
        c: &Connection,
        account_id: i64,
        gm_msgid: &str,
        raw: &[u8],
        s: &crate::mime::Summary,
    ) -> Result<()> {
        let id: Option<i64> = c
            .query_row(
                "SELECT id FROM messages WHERE account_id=? AND gm_msgid=?",
                params![account_id, gm_msgid],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(()) };
        c.execute("INSERT OR REPLACE INTO bodies(message_id, raw) VALUES(?,?)", params![id, raw])?;
        c.execute("UPDATE messages SET has_body=1 WHERE id=?", [id])?;
        Db::set_summary(c, id, s)
    }

    pub fn raw_body(&self, message_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn()?
            .query_row("SELECT raw FROM bodies WHERE message_id=?", [message_id], |r| r.get(0))
            .optional()?)
    }

    /// `unread_only` narrows the list to conversations with something unread; `keep` is a JSON
    /// array of "account:thread" keys that stay listed regardless (the ones just opened, so
    /// reading a conversation doesn't pull it out from under the cursor).
    pub fn threads(
        &self,
        account: Option<i64>,
        query: &str,
        unread_only: bool,
        kind: &str,
        keep: &str,
        all_mail: bool,
        from: &str,
    ) -> Result<Vec<ThreadRow>> {
        let c = self.conn()?;
        // ?2 is what to look for: words for the search index when it can answer, otherwise a
        // pattern that every message is read against.
        let indexed = search_terms(query).filter(|_| self.meta("search_index").ok().flatten().as_deref() == Some(SEARCH_INDEX));
        let (wanted, matching) = match indexed {
            Some(terms) => (
                terms,
                "(m.account_id, m.gm_thrid) IN (SELECT x.account_id, x.gm_thrid FROM search JOIN messages x ON x.id=search.rowid
                                                 WHERE search MATCH ?2)",
            ),
            None => (
                format!("%{}%", query.replace('%', "").replace('_', " ")),
                "(?2='%%' OR EXISTS (SELECT 1 FROM messages s
                       WHERE s.account_id=m.account_id AND s.gm_thrid=m.gm_thrid
                         AND (s.subject LIKE ?2 OR s.from_name LIKE ?2 OR s.from_addr LIKE ?2 OR s.body_text LIKE ?2)))",
            ),
        };
        // SQLite fills the bare columns from the row that supplied MAX(date): the newest message.
        let mut st = c.prepare(&format!(
            "SELECT account_id, gm_thrid, MAX(date) AS newest, subject, snippet,
                    -- (All Mail groups whole conversations, so there the group itself answers
                    -- these two for the many conversations of one message)
                    CASE WHEN ?6=1 AND COUNT(*)=1 THEN CASE WHEN from_name='' THEN from_addr ELSE from_name END
                    ELSE (SELECT GROUP_CONCAT(n, ', ') FROM (
                        SELECT CASE WHEN t.from_name='' THEN t.from_addr ELSE t.from_name END AS n
                          FROM messages t WHERE t.account_id=m.account_id AND t.gm_thrid=m.gm_thrid
                         GROUP BY n ORDER BY MIN(t.date))) END,
                    CASE WHEN ?6=1 THEN COUNT(*)
                    ELSE (SELECT COUNT(*) FROM messages t WHERE t.account_id=m.account_id AND t.gm_thrid=m.gm_thrid) END,
                    SUM(seen=0), SUM(starred)>0, SUM(has_attach)>0, from_name, from_addr,
                    SUM(automated=0)>0, SUM(in_inbox)>0
               FROM messages m
              WHERE (?6=1 OR in_inbox=1)
                AND (?1 IS NULL OR account_id=?1)
                AND {matching}
                -- from a sender: part of an address, a whole one, or a domain
                AND (?7='' OR (m.account_id, m.gm_thrid) IN (
                       SELECT f.account_id, f.gm_thrid FROM messages f WHERE f.from_addr LIKE '%' || ?7 || '%' ESCAPE '\\'))
              GROUP BY account_id, gm_thrid
             HAVING (?3=0 OR SUM(seen=0)>0
                     OR (account_id || ':' || gm_thrid) IN (SELECT value FROM json_each(?4)))
                AND CASE ?5 WHEN 'people' THEN SUM(automated=0)>0
                            WHEN 'automated' THEN SUM(automated=1)>0 AND COALESCE(SUM(automated=0), 0)=0
                            ELSE 1 END
              ORDER BY newest DESC"
        ))?;
        let from = from.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let rows = st.query_map(params![account, wanted, unread_only, keep, kind, all_mail, from], |r| {
            Ok(ThreadRow {
                account_id: r.get(0)?,
                thrid: r.get(1)?,
                date: r.get(2)?,
                subject: r.get(3)?,
                snippet: r.get(4)?,
                senders: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                count: r.get(6)?,
                unread: r.get(7)?,
                starred: r.get(8)?,
                has_attach: r.get(9)?,
                from_name: r.get(10)?,
                from_addr: r.get(11)?,
                in_inbox: r.get(13)?,
                people: r.get::<_, Option<bool>>(12)?.unwrap_or(false),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- search ----------------------------------------------------------
    /// Fill the search index from the mail already held, once. Until it is done, searching
    /// reads through everything instead (slower, same answers).
    pub fn build_search(&self) -> Result<()> {
        if self.meta("search_index")?.as_deref() == Some(SEARCH_INDEX) {
            return Ok(());
        }
        self.conn()?.execute("INSERT INTO search(search) VALUES('rebuild')", [])?;
        self.set_meta("search_index", SEARCH_INDEX)
    }

    // ---- All Mail --------------------------------------------------------
    /// How far the index of an account's All Mail folder has got.
    pub fn all_mail(&self, account_id: i64) -> Result<Option<AllMail>> {
        Ok(self.meta(&format!("allmail:{account_id}"))?.and_then(|v| AllMail::parse(&v)))
    }

    pub fn set_all_mail(&self, account_id: i64, a: &AllMail) -> Result<()> {
        self.set_meta(&format!("allmail:{account_id}"), &format!("{},{},{},{ALL_MAIL_LISTING}", a.low, a.high, a.swept))
    }

    pub fn thread_messages(&self, account_id: i64, thrid: &str) -> Result<Vec<MessageRow>> {
        let c = self.conn()?;
        let mut st = c.prepare(
            "SELECT id, date, from_name, from_addr, to_addrs, cc_addrs, subject, snippet, seen, starred, in_inbox,
                    has_body, attachments, facts
               FROM messages WHERE account_id=? AND gm_thrid=? ORDER BY date, id",
        )?;
        let rows = st.query_map(params![account_id, thrid], |r| {
            Ok(MessageRow {
                id: r.get(0)?,
                date: r.get(1)?,
                from_name: r.get(2)?,
                from_addr: r.get(3)?,
                to_addrs: r.get(4)?,
                cc_addrs: r.get(5)?,
                subject: r.get(6)?,
                snippet: r.get(7)?,
                seen: r.get(8)?,
                starred: r.get(9)?,
                in_inbox: r.get(10)?,
                has_body: r.get(11)?,
                attachments: r
                    .get::<_, Option<String>>(12)?
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_else(|| serde_json::json!([])),
                facts: r
                    .get::<_, Option<String>>(13)?
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_else(|| serde_json::json!({})),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Inbox conversations not yet topped up from All Mail, newest first.
    pub fn unhydrated_threads(&self, account_id: i64, limit: i64) -> Result<Vec<String>> {
        let c = self.conn()?;
        let mut st = c.prepare(
            "SELECT gm_thrid FROM messages m
              WHERE account_id=?1 AND in_inbox=1
                AND NOT EXISTS (SELECT 1 FROM hydrated h WHERE h.account_id=?1 AND h.gm_thrid=m.gm_thrid)
              GROUP BY gm_thrid ORDER BY MAX(date) DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![account_id, limit], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The downloaded copies that can be let go: messages in conversations with nothing left
    /// in the inbox. Returns (how many, their size in bytes).
    pub fn spare_bodies(&self) -> Result<(i64, i64)> {
        let c = self.conn()?;
        Ok(c.query_row(&format!("SELECT COUNT(*), COALESCE(SUM(LENGTH(b.raw)), 0) FROM bodies b JOIN messages m ON m.id=b.message_id WHERE {SPARE}"), [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?)
    }

    /// Drop those copies, keeping what the list and search show (sender, subject, text). Each
    /// is fetched from Gmail again when its conversation is opened. Returns how many went.
    pub fn drop_spare_bodies(&self) -> Result<usize> {
        let mut c = self.conn()?;
        let tx = c.transaction()?;
        tx.execute_batch(&format!(
            "CREATE TEMP TABLE going AS SELECT m.id, m.account_id, m.gm_thrid FROM messages m WHERE m.has_body=1 AND {SPARE};
             DELETE FROM bodies WHERE message_id IN (SELECT id FROM going);
             UPDATE messages SET has_body=0 WHERE id IN (SELECT id FROM going);
             DELETE FROM hydrated WHERE (account_id, gm_thrid) IN (SELECT account_id, gm_thrid FROM going);"
        ))?;
        let gone: i64 = tx.query_row("SELECT COUNT(*) FROM going", [], |r| r.get(0))?;
        tx.execute_batch("DROP TABLE going")?;
        tx.commit()?;
        Ok(gone as usize)
    }

    /// Hand the space freed inside the file back to the disk.
    pub fn compact(&self) -> Result<()> {
        Ok(self.conn()?.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?)
    }

    pub fn mark_hydrated(&self, account_id: i64, thrids: &[String]) -> Result<()> {
        let c = self.conn()?;
        for t in thrids {
            c.execute("INSERT OR IGNORE INTO hydrated(account_id, gm_thrid) VALUES(?,?)", params![account_id, t])?;
        }
        Ok(())
    }

    /// Message ids in these conversations whose bodies we already hold.
    pub fn bodies_held(&self, account_id: i64, thrids: &[String]) -> Result<std::collections::HashSet<String>> {
        let c = self.conn()?;
        let mut out = std::collections::HashSet::new();
        let mut st = c.prepare("SELECT gm_msgid FROM messages WHERE account_id=? AND gm_thrid=? AND has_body=1")?;
        for t in thrids {
            for r in st.query_map(params![account_id, t], |r| r.get::<_, String>(0))? {
                out.insert(r?);
            }
        }
        Ok(out)
    }

    /// The first `len` bytes of a stored message: its headers, without loading a large body.
    pub fn message_head(&self, message_id: i64, len: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn()?
            .query_row("SELECT substr(raw, 1, ?) FROM bodies WHERE message_id=?", params![len, message_id], |r| r.get(0))
            .optional()?)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn()?.query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn()?.execute("INSERT OR REPLACE INTO meta(key, value) VALUES(?,?)", params![key, value])?;
        Ok(())
    }

    /// Ids of every message whose body is stored.
    pub fn stored_bodies(&self) -> Result<Vec<i64>> {
        let c = self.conn()?;
        let mut st = c.prepare("SELECT message_id FROM bodies")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replace what was derived from a stored body (preview, search text, attachment list, facts).
    pub fn set_summary(c: &Connection, message_id: i64, s: &crate::mime::Summary) -> Result<()> {
        c.execute(
            "UPDATE messages SET snippet=?, body_text=?, attachments=?, has_attach=?, facts=?, automated=? WHERE id=?",
            params![s.snippet, s.text, s.attachments, s.attachments != "[]", s.facts, s.automated, message_id],
        )?;
        // A Google Group puts its own name in the From line; list the person who wrote instead.
        let sender = serde_json::from_str::<serde_json::Value>(&s.facts).ok().map(|f| f["group"].clone());
        if let Some(g) = sender.filter(|g| g["rewritten"] == true) {
            let addr = g["sender_addr"].as_str().unwrap_or_default();
            let name = g["sender_name"].as_str().unwrap_or_default();
            c.execute("UPDATE messages SET from_name=?, from_addr=? WHERE id=?", params![name, addr, message_id])?;
        }
        Ok(())
    }

    /// A message's sender and the facts worked out about it.
    pub fn message_facts(&self, message_id: i64) -> Result<Option<(String, serde_json::Value)>> {
        let c = self.conn()?;
        let row: Option<(String, Option<String>)> = c
            .query_row("SELECT from_addr, facts FROM messages WHERE id=?", [message_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row.map(|(from, facts)| (from, facts.and_then(|f| serde_json::from_str(&f).ok()).unwrap_or_default())))
    }

    /// Senders whose mail says how to unsubscribe, busiest in the last three months first:
    /// one row per account and sender address, as JSON objects for the UI.
    pub fn subscriptions(&self, account: Option<i64>) -> Result<Vec<serde_json::Value>> {
        let c = self.conn()?;
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64
            - 90 * 86400;
        // (the bare columns come from each sender's newest message)
        let mut st = c.prepare(
            "SELECT m.account_id, m.from_addr, MAX(m.date), m.from_name, m.id, json_extract(m.facts, '$.unsubscribe'),
                    COUNT(*), SUM(m.date >= ?2) AS recent, SUM(m.in_inbox),
                    EXISTS (SELECT 1 FROM unsubscribed u WHERE u.sender = lower(m.from_addr))
               FROM messages m
              WHERE m.facts LIKE '%\"unsubscribe\":{%' AND json_extract(m.facts, '$.group') IS NULL
                AND m.from_addr <> ''
                AND (?1 IS NULL OR m.account_id=?1)
                AND lower(m.from_addr) NOT IN (SELECT lower(email) FROM accounts)
              GROUP BY m.account_id, lower(m.from_addr)
              ORDER BY recent DESC, COUNT(*) DESC, lower(m.from_addr)",
        )?;
        let rows = st.query_map(params![account, since], |r| {
            Ok(serde_json::json!({
                "account_id": r.get::<_, i64>(0)?,
                "from_addr": r.get::<_, String>(1)?,
                "latest": r.get::<_, i64>(2)?,
                "from_name": r.get::<_, String>(3)?,
                "id": r.get::<_, i64>(4)?,
                "unsubscribe": r.get::<_, Option<String>>(5)?.and_then(|u| serde_json::from_str::<serde_json::Value>(&u).ok()),
                "total": r.get::<_, i64>(6)?,
                "recent": r.get::<_, i64>(7)?,
                "in_inbox": r.get::<_, i64>(8)?,
                "unsubscribed": r.get::<_, bool>(9)?,
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The senders whose address or name contains `q`, the ones with most mail first:
    /// (address, name as last used, how many messages).
    pub fn senders(&self, q: &str, limit: i64) -> Result<Vec<(String, String, i64)>> {
        let c = self.conn()?;
        let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        // (the name comes from each sender's newest message)
        let mut st = c.prepare(
            "SELECT lower(from_addr), from_name, COUNT(*), MAX(date) FROM messages
              WHERE from_addr <> '' AND (from_addr LIKE ?1 ESCAPE '\\' OR from_name LIKE ?1 ESCAPE '\\')
                AND lower(from_addr) NOT IN (SELECT lower(email) FROM accounts)
              GROUP BY lower(from_addr) ORDER BY COUNT(*) DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![like, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- small lists the person builds up ----------------------------------
    pub fn mark_unsubscribed(&self, sender: &str) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        self.conn()?.execute("INSERT OR REPLACE INTO unsubscribed(sender, at) VALUES(?,?)", params![sender.to_lowercase(), now])?;
        Ok(())
    }

    pub fn unsubscribed(&self) -> Result<std::collections::HashSet<String>> {
        let c = self.conn()?;
        let mut st = c.prepare("SELECT sender FROM unsubscribed")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Senders whose pictures are always shown.
    pub fn picture_senders(&self) -> Result<Vec<String>> {
        Ok(self.meta("picture_senders")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
    }

    pub fn set_picture_sender(&self, sender: &str, allow: bool) -> Result<()> {
        let sender = sender.to_lowercase();
        let mut list = self.picture_senders()?;
        list.retain(|s| *s != sender);
        if allow {
            list.push(sender);
        }
        self.set_meta("picture_senders", &serde_json::to_string(&list)?)
    }

    // ---- the assistant ---------------------------------------------------
    pub fn agent_append(&self, role: &str, content: &serde_json::Value) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        self.conn()?.execute(
            "INSERT INTO agent_log(role, content, at) VALUES(?,?,?)",
            params![role, content.to_string(), now],
        )?;
        Ok(())
    }

    /// The whole conversation, oldest first, as (role, content).
    pub fn agent_history(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let c = self.conn()?;
        let mut st = c.prepare("SELECT role, content FROM agent_log ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (role, content) = r?;
            out.push((role, serde_json::from_str(&content)?));
        }
        Ok(out)
    }

    pub fn agent_clear(&self) -> Result<()> {
        self.conn()?.execute("DELETE FROM agent_log", [])?;
        Ok(())
    }

    /// Run a query the assistant wrote. It gets a connection that cannot write, sees mail
    /// through the `mail` view, is cut off after a few seconds, and gets at most `max_rows`
    /// rows with long text shortened. Returns (column names, rows, whether rows were left out).
    pub fn agent_query(
        &self,
        sql: &str,
        max_rows: usize,
        max_cell: usize,
    ) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>, bool)> {
        use rusqlite::types::ValueRef;
        use rusqlite::OpenFlags;
        // as whole words: the view has a column called has_attachments
        static BARRED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let barred = BARRED.get_or_init(|| regex::Regex::new(r"(?i)\b(attach|detach|pragma|load_extension)\b").unwrap());
        if barred.is_match(sql) {
            anyhow::bail!("only plain SELECT queries are allowed");
        }
        let c = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute_batch(
            "CREATE TEMP VIEW mail AS
               SELECT m.id AS message_id, a.email AS account, m.account_id, m.gm_thrid AS thread_id,
                      m.date, datetime(m.date, 'unixepoch', 'localtime') AS date_local,
                      m.from_name, m.from_addr, m.to_addrs, m.cc_addrs, m.subject, m.snippet, m.body_text,
                      m.in_inbox, (m.seen = 0) AS unread, m.starred, m.has_attach AS has_attachments, m.size,
                      m.automated, m.has_body AS downloaded,
                      COALESCE(m.facts LIKE '%\"unsubscribe\":{%', 0) AS can_unsubscribe
                 FROM main.messages m JOIN main.accounts a ON a.id = m.account_id;",
        )?;
        let mut st = c.prepare(sql)?;
        if !st.readonly() {
            anyhow::bail!("only SELECT queries are allowed");
        }
        // A runaway query is interrupted rather than left to hold the app up.
        let interrupt = c.get_interrupt_handle();
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = finished.clone();
        std::thread::spawn(move || {
            for _ in 0..80 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
            }
            interrupt.interrupt();
        });
        let columns: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut truncated = false;
        let result = (|| -> Result<()> {
            let mut rows = st.query([])?;
            while let Some(row) = rows.next()? {
                if out.len() >= max_rows {
                    truncated = true;
                    break;
                }
                let mut cells = Vec::with_capacity(columns.len());
                for i in 0..columns.len() {
                    cells.push(match row.get_ref(i)? {
                        ValueRef::Null => serde_json::Value::Null,
                        ValueRef::Integer(n) => n.into(),
                        ValueRef::Real(f) => serde_json::json!(f),
                        ValueRef::Text(t) => {
                            let t = String::from_utf8_lossy(t);
                            if t.chars().count() > max_cell {
                                format!("{}…", t.chars().take(max_cell).collect::<String>()).into()
                            } else {
                                t.into_owned().into()
                            }
                        }
                        ValueRef::Blob(_) => "<binary>".into(),
                    });
                }
                out.push(cells);
            }
            Ok(())
        })();
        finished.store(true, std::sync::atomic::Ordering::Relaxed);
        result?;
        Ok((columns, out, truncated))
    }

    /// The conversations these messages belong to, as (account id, thread id), without repeats.
    pub fn threads_of_messages(&self, message_ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let c = self.conn()?;
        let mut st = c.prepare("SELECT account_id, gm_thrid FROM messages WHERE id=?")?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for id in message_ids {
            if let Some(key) = st.query_row([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).optional()? {
                if seen.insert(key.clone()) {
                    out.push(key);
                }
            }
        }
        Ok(out)
    }

    /// Inbox conversations holding mail from the same sender address as this message, in any
    /// account. Mail that came through a Google Group counts by who wrote it, not by the group:
    /// a group is often just an address that many senders write to.
    pub fn threads_from_same_source(&self, message_id: i64) -> Result<Vec<ThreadRow>> {
        let c = self.conn()?;
        let from: Option<String> = c.query_row("SELECT from_addr FROM messages WHERE id=?", [message_id], |r| r.get(0)).optional()?;
        let Some(from) = from.filter(|f| !f.is_empty()) else { return Ok(vec![]) };
        // never "everything from me": that is every conversation the owner replied in
        if c.prepare("SELECT 1 FROM accounts WHERE email=? COLLATE NOCASE")?.exists([&from])? {
            return Ok(vec![]);
        }
        let mut st = c.prepare("SELECT DISTINCT account_id, gm_thrid FROM messages WHERE in_inbox=1 AND from_addr=? COLLATE NOCASE")?;
        let keys: Vec<(i64, String)> = st.query_map([&from], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        self.threads_by_keys(&keys)
    }

    /// List rows for specific conversations, wherever they are (inbox or not), newest first.
    pub fn threads_by_keys(&self, keys: &[(i64, String)]) -> Result<Vec<ThreadRow>> {
        let c = self.conn()?;
        let mut st = c.prepare(
            "SELECT account_id, gm_thrid, MAX(date) AS newest, subject, snippet,
                    (SELECT GROUP_CONCAT(n, ', ') FROM (
                        SELECT CASE WHEN t.from_name='' THEN t.from_addr ELSE t.from_name END AS n
                          FROM messages t WHERE t.account_id=m.account_id AND t.gm_thrid=m.gm_thrid
                         GROUP BY n ORDER BY MIN(t.date))),
                    COUNT(*), COALESCE(SUM(seen=0 AND in_inbox=1), 0), SUM(starred)>0, SUM(has_attach)>0,
                    from_name, from_addr, SUM(in_inbox)>0, SUM(automated=0)>0
               FROM messages m WHERE account_id=? AND gm_thrid=?
              GROUP BY account_id, gm_thrid",
        )?;
        let mut out = Vec::new();
        for (account_id, thrid) in keys {
            let row = st
                .query_row(params![account_id, thrid], |r| {
                    Ok(ThreadRow {
                        account_id: r.get(0)?,
                        thrid: r.get(1)?,
                        date: r.get(2)?,
                        subject: r.get(3)?,
                        snippet: r.get(4)?,
                        senders: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        count: r.get(6)?,
                        unread: r.get(7)?,
                        starred: r.get(8)?,
                        has_attach: r.get(9)?,
                        from_name: r.get(10)?,
                        from_addr: r.get(11)?,
                        in_inbox: r.get(12)?,
                        people: r.get::<_, Option<bool>>(13)?.unwrap_or(false),
                    })
                })
                .optional()?;
            out.extend(row);
        }
        out.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assistant_can_ask_about_attachments_but_not_attach_files() {
        let dir = std::env::temp_dir().join(format!("unibox-test-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("q.db")).unwrap();
        assert!(db.agent_query("SELECT message_id, has_attachments FROM mail", 5, 40).is_ok());
        // what the list's filters go by, and the search index, are there to be asked about
        assert!(db.agent_query("SELECT automated, downloaded, can_unsubscribe FROM mail WHERE in_inbox=0", 5, 40).is_ok());
        assert!(db.agent_query("SELECT message_id FROM mail WHERE message_id IN (SELECT rowid FROM search WHERE search MATCH '\"refund\"')", 5, 40).is_ok());
        assert!(db.agent_query("ATTACH DATABASE 'x.db' AS x", 5, 40).is_err());
        assert!(db.agent_query("SELECT 1; PRAGMA writable_schema=1", 5, 40).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloaded_copies_of_archived_conversations_can_be_dropped() {
        let dir = std::env::temp_dir().join(format!("unibox-test-spare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("spare.db")).unwrap();
        let id = db.add_account("a@example.com").unwrap();
        let c = db.conn().unwrap();
        // one archived conversation; one with an earlier archived message and one still in the inbox
        for (msgid, thrid, inbox) in [("1", "10", false), ("2", "20", false), ("3", "20", true)] {
            let m = NewMessage {
                gm_msgid: msgid, gm_thrid: thrid, uid: None, in_inbox: inbox, seen: true, starred: false, date: 1, size: 0,
                from_name: "A", from_addr: "a@b.example", to_addrs: "", cc_addrs: "", subject: "Hello",
            };
            Db::insert_message(&c, id, &m).unwrap();
            c.execute("INSERT INTO bodies(message_id, raw) SELECT id, x'0102030405' FROM messages WHERE gm_msgid=?", [msgid]).unwrap();
            c.execute("UPDATE messages SET has_body=1, body_text='kept for search' WHERE gm_msgid=?", [msgid]).unwrap();
        }
        assert_eq!(db.spare_bodies().unwrap(), (1, 5));
        assert_eq!(db.drop_spare_bodies().unwrap(), 1);
        db.compact().unwrap();
        let held: Vec<String> = c.prepare("SELECT gm_msgid FROM messages WHERE has_body=1 ORDER BY 1").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(held, ["2", "3"]);
        assert_eq!(c.query_row("SELECT COUNT(*) FROM bodies", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        // what the list and search go by is still there
        assert_eq!(c.query_row("SELECT body_text FROM messages WHERE gm_msgid='1'", [], |r| r.get::<_, String>(0)).unwrap(), "kept for search");
        assert_eq!(db.spare_bodies().unwrap(), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_words_go_to_the_index_unless_one_is_too_short() {
        assert_eq!(search_terms("stripe receipt").as_deref(), Some("\"stripe\" \"receipt\""));
        assert_eq!(search_terms("say \"hi there\"").as_deref(), Some("\"say\" \"\"\"hi\" \"there\"\"\""));
        assert_eq!(search_terms("予約確認").as_deref(), Some("\"予約確認\""));
        assert_eq!(search_terms("to do"), None);
        assert_eq!(search_terms("予約"), None);
        assert_eq!(search_terms("  "), None);
    }

    #[test]
    fn search_finds_parts_of_words_and_follows_changes() {
        let dir = std::env::temp_dir().join(format!("unibox-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("search.db");
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).unwrap();
        let id = db.add_account("a@example.com").unwrap();
        let c = db.conn().unwrap();
        let add = |msgid: &str, thrid: &str, subject: &str, inbox: bool| {
            let m = NewMessage {
                gm_msgid: msgid, gm_thrid: thrid, uid: None, in_inbox: inbox, seen: true, starred: false, date: 1, size: 0,
                from_name: "Castle Hire", from_addr: "receipts@pay.example", to_addrs: "", cc_addrs: "", subject,
            };
            Db::insert_message(&c, id, &m).unwrap();
        };
        add("1", "1", "Your receipt for the bouncy castle", true);
        add("2", "2", "Minutes of the meeting", false);
        let found = |q: &str, all: bool| db.threads(None, q, false, "", "[]", all, "").unwrap().len();
        // before the index is built, and after: the same answers
        for built in [false, true] {
            if built {
                db.build_search().unwrap();
            }
            assert_eq!(found("ouncy cast", false), 1, "built: {built}");
            assert_eq!(found("RECEIPT castle", false), built as usize, "words in any order need the index");
            assert_eq!(found("minutes", false), 0);
            assert_eq!(found("minutes", true), 1);
            assert_eq!(found("pay.example", true), 2);
            assert_eq!(found("", true), 2);
            let from = |who: &str| db.threads(None, "", false, "", "[]", true, who).unwrap().len();
            assert_eq!((from("pay.example"), from("RECEIPTS@pay"), from("pay_example"), from("nobody")), (2, 2, 0, 0));
        }
        c.execute("UPDATE messages SET body_text='the deposit was refunded' WHERE gm_msgid='2'", []).unwrap();
        assert_eq!(found("deposit refunded", true), 1);
        c.execute("DELETE FROM messages WHERE gm_msgid='2'", []).unwrap();
        assert_eq!(found("deposit", true), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
