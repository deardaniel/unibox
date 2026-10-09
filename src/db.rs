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
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let has_column = |name: &str| -> Result<bool> {
            let mut st = c.prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name=?")?;
            Ok(st.exists([name])?)
        };
        if !has_column("attachments")? {
            // JSON list of attachments, filled in when the body is stored
            c.execute("ALTER TABLE messages ADD COLUMN attachments TEXT", [])?;
        }
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
                      WHERE m.account_id=a.id AND m.in_inbox=1 AND m.seen=0)
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        snippet: &str,
        body_text: &str,
        attachments: &str,
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
        c.execute(
            "UPDATE messages SET has_body=1, snippet=?, body_text=?, attachments=?, has_attach=? WHERE id=?",
            params![snippet, body_text, attachments, attachments != "[]", id],
        )?;
        Ok(())
    }

    pub fn raw_body(&self, message_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn()?
            .query_row("SELECT raw FROM bodies WHERE message_id=?", [message_id], |r| r.get(0))
            .optional()?)
    }

    pub fn threads(&self, account: Option<i64>, query: &str, limit: i64, offset: i64) -> Result<Vec<ThreadRow>> {
        let c = self.conn()?;
        let like = format!("%{}%", query.replace('%', "").replace('_', " "));
        // SQLite fills the bare columns from the row that supplied MAX(date): the newest message.
        let mut st = c.prepare(
            "SELECT account_id, gm_thrid, MAX(date) AS newest, subject, snippet,
                    (SELECT GROUP_CONCAT(n, ', ') FROM (
                        SELECT CASE WHEN t.from_name='' THEN t.from_addr ELSE t.from_name END AS n
                          FROM messages t WHERE t.account_id=m.account_id AND t.gm_thrid=m.gm_thrid
                         GROUP BY n ORDER BY MIN(t.date))),
                    (SELECT COUNT(*) FROM messages t WHERE t.account_id=m.account_id AND t.gm_thrid=m.gm_thrid),
                    SUM(seen=0), SUM(starred)>0, SUM(has_attach)>0, from_name, from_addr
               FROM messages m
              WHERE in_inbox=1
                AND (?1 IS NULL OR account_id=?1)
                AND (?2='%%' OR EXISTS (SELECT 1 FROM messages s
                       WHERE s.account_id=m.account_id AND s.gm_thrid=m.gm_thrid
                         AND (s.subject LIKE ?2 OR s.from_name LIKE ?2 OR s.from_addr LIKE ?2 OR s.body_text LIKE ?2)))
              GROUP BY account_id, gm_thrid
              ORDER BY newest DESC
              LIMIT ?3 OFFSET ?4",
        )?;
        let rows = st.query_map(params![account, like, limit, offset], |r| {
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn thread_messages(&self, account_id: i64, thrid: &str) -> Result<Vec<MessageRow>> {
        let c = self.conn()?;
        let mut st = c.prepare(
            "SELECT id, date, from_name, from_addr, to_addrs, cc_addrs, subject, snippet, seen, starred, in_inbox,
                    has_body, attachments
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

    /// Replace what was derived from a stored body (preview, search text, attachment list).
    pub fn set_summary(c: &Connection, message_id: i64, snippet: &str, body_text: &str, attachments: &str) -> Result<()> {
        c.execute(
            "UPDATE messages SET snippet=?, body_text=?, attachments=?, has_attach=? WHERE id=?",
            params![snippet, body_text, attachments, attachments != "[]", message_id],
        )?;
        Ok(())
    }
}
