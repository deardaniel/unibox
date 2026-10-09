//! A small IMAP client covering exactly what Unibox needs from Gmail.
//!
//! Gmail's extensions (X-GM-MSGID, X-GM-THRID, X-GM-LABELS, X-GM-RAW) are the
//! reason this is hand-rolled: responses are parsed into a generic value tree
//! so unknown attributes never break a FETCH.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Atom(String),
    Str(Vec<u8>),
    List(Vec<Val>),
}

impl Val {
    pub fn text(&self) -> String {
        match self {
            Val::Atom(a) => a.clone(),
            Val::Str(s) => String::from_utf8_lossy(s).into_owned(),
            Val::List(_) => String::new(),
        }
    }
    pub fn bytes(&self) -> &[u8] {
        match self {
            Val::Atom(a) => a.as_bytes(),
            Val::Str(s) => s,
            Val::List(_) => &[],
        }
    }
    pub fn list(&self) -> &[Val] {
        match self {
            Val::List(l) => l,
            _ => &[],
        }
    }
}

/// One response line, with any `{n}` literals already read off the wire.
enum Piece {
    Text(Vec<u8>),
    Lit(Vec<u8>),
}

struct Parser<'a> {
    pieces: &'a [Piece],
    pi: usize,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn cur(&self) -> Option<&'a [u8]> {
        match self.pieces.get(self.pi) {
            Some(Piece::Text(t)) => Some(t),
            _ => None,
        }
    }
    fn peek(&self) -> Option<u8> {
        self.cur().and_then(|t| t.get(self.pos).copied())
    }
    /// Move past the end of a text piece onto whatever follows it.
    fn advance_piece(&mut self) {
        self.pi += 1;
        self.pos = 0;
    }
    fn values(&mut self, in_list: bool) -> Vec<Val> {
        let mut out = Vec::new();
        loop {
            match self.pieces.get(self.pi) {
                None => return out,
                Some(Piece::Lit(l)) => {
                    out.push(Val::Str(l.clone()));
                    self.advance_piece();
                }
                Some(Piece::Text(t)) => {
                    let Some(&c) = t.get(self.pos) else {
                        self.advance_piece();
                        continue;
                    };
                    match c {
                        b' ' => self.pos += 1,
                        b'(' => {
                            self.pos += 1;
                            out.push(Val::List(self.values(true)));
                        }
                        b')' => {
                            self.pos += 1;
                            if in_list {
                                return out;
                            }
                        }
                        b'"' => out.push(self.quoted()),
                        _ => out.push(self.atom()),
                    }
                }
            }
        }
    }
    fn quoted(&mut self) -> Val {
        let t = self.cur().unwrap_or(&[]);
        let mut s = Vec::new();
        self.pos += 1;
        while let Some(&c) = t.get(self.pos) {
            self.pos += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    if let Some(&n) = t.get(self.pos) {
                        s.push(n);
                        self.pos += 1;
                    }
                }
                _ => s.push(c),
            }
        }
        Val::Str(s)
    }
    /// Atoms run to the next space or paren, except that a `[...]` section
    /// (as in `BODY[HEADER.FIELDS (FROM TO)]`) is swallowed whole.
    fn atom(&mut self) -> Val {
        let t = self.cur().unwrap_or(&[]);
        let start = self.pos;
        let mut depth = 0;
        while let Some(c) = self.peek() {
            match c {
                b'[' => depth += 1,
                b']' if depth > 0 => depth -= 1,
                b' ' | b'(' | b')' if depth == 0 => break,
                _ => {}
            }
            self.pos += 1;
        }
        Val::Atom(String::from_utf8_lossy(&t[start..self.pos]).into_owned())
    }
}

fn parse(pieces: &[Piece]) -> Vec<Val> {
    Parser { pieces, pi: 0, pos: 0 }.values(false)
}

/// Quote a string for use as an IMAP argument.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render a list of UIDs as a compact sequence set, e.g. `1:3,7,9:10`.
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        out.push(if j > i { format!("{}:{}", v[i], v[j]) } else { v[i].to_string() });
        i = j + 1;
    }
    out.join(",")
}

/// The attributes of one `* n FETCH (...)` response, keyed by upper-cased name.
/// Any `BODY[...]` section is stored under `BODY`.
pub type Fetched = HashMap<String, Val>;

pub struct Imap {
    io: BufReader<Box<dyn Stream>>,
    tag: u32,
}

impl Imap {
    /// Connect and log in. `server` is `host:port`; a `plain:` prefix skips TLS
    /// (only used against the local test server).
    pub fn connect(server: &str, user: &str, password: &str) -> Result<Imap> {
        let (plain, addr) = match server.strip_prefix("plain:") {
            Some(a) => (true, a),
            None => (false, server),
        };
        let tcp = TcpStream::connect(addr).with_context(|| format!("connecting to {addr}"))?;
        tcp.set_read_timeout(Some(Duration::from_secs(90)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(90)))?;
        let stream: Box<dyn Stream> = if plain {
            Box::new(tcp)
        } else {
            let host = addr.split(':').next().unwrap_or(addr).to_string();
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host)?;
            let conn = rustls::ClientConnection::new(Arc::new(config), name)?;
            Box::new(rustls::StreamOwned::new(conn, tcp))
        };
        let mut imap = Imap { io: BufReader::new(stream), tag: 0 };
        imap.read_response()?; // greeting
        imap.run(&format!("LOGIN {} {}", quote(user), quote(password)))
            .map_err(|e| anyhow!("login failed: {e}"))?;
        Ok(imap)
    }

    fn read_response(&mut self) -> Result<Vec<Val>> {
        let mut pieces = Vec::new();
        loop {
            let mut line = Vec::new();
            if self.io.read_until(b'\n', &mut line)? == 0 {
                bail!("connection closed by server");
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            // A trailing {n} means n raw bytes follow, then the line continues.
            let lit = line.ends_with(b"}").then(|| line.iter().rposition(|&c| c == b'{')).flatten().and_then(|i| {
                let n = std::str::from_utf8(&line[i + 1..line.len() - 1]).ok()?.parse::<usize>().ok()?;
                Some((i, n))
            });
            match lit {
                Some((i, n)) => {
                    line.truncate(i);
                    pieces.push(Piece::Text(line));
                    let mut buf = vec![0u8; n];
                    self.io.read_exact(&mut buf)?;
                    pieces.push(Piece::Lit(buf));
                }
                None => {
                    pieces.push(Piece::Text(line));
                    return Ok(parse(&pieces));
                }
            }
        }
    }

    /// Send one command and collect its untagged responses.
    pub fn run(&mut self, cmd: &str) -> Result<Vec<Vec<Val>>> {
        self.tag += 1;
        let tag = format!("U{}", self.tag);
        let w = self.io.get_mut();
        w.write_all(format!("{tag} {cmd}\r\n").as_bytes())?;
        w.flush()?;
        let mut out = Vec::new();
        loop {
            let resp = self.read_response()?;
            if resp.first().map(Val::text).as_deref() == Some(tag.as_str()) {
                let status = resp.get(1).map(Val::text).unwrap_or_default().to_uppercase();
                if status == "OK" {
                    return Ok(out);
                }
                let detail: Vec<String> = resp.iter().skip(1).map(Val::text).collect();
                let name = cmd.split(' ').take(2).collect::<Vec<_>>().join(" ");
                // never echo LOGIN arguments back in an error
                let name = if name.starts_with("LOGIN") { "LOGIN".to_string() } else { name };
                bail!("{name}: {}", detail.join(" "));
            }
            out.push(resp);
        }
    }

    /// Select a mailbox (already quoted). Returns the number of messages in it.
    pub fn select(&mut self, mailbox: &str, readonly: bool) -> Result<u32> {
        let verb = if readonly { "EXAMINE" } else { "SELECT" };
        let mut exists = 0;
        for r in self.run(&format!("{verb} {mailbox}"))? {
            if r.get(2).map(Val::text).as_deref() == Some("EXISTS") {
                exists = r[1].text().parse().unwrap_or(0);
            }
        }
        Ok(exists)
    }

    /// The quoted name of Gmail's "All Mail" folder, whatever the account's language.
    pub fn all_mail(&mut self) -> Result<String> {
        self.special_folder("\\All")
            .map_err(|_| anyhow!("this account has no All Mail folder visible over IMAP (enable it in Gmail's label settings)"))
    }

    /// The quoted name of the folder Gmail marks with a special-use flag such as `\\All` or `\\Drafts`.
    pub fn special_folder(&mut self, flag: &str) -> Result<String> {
        for r in self.run("LIST \"\" \"*\"")? {
            if r.get(1).map(Val::text).as_deref() != Some("LIST") {
                continue;
            }
            let has = r.get(2).map(|f| f.list().iter().any(|a| a.text().eq_ignore_ascii_case(flag)));
            if has == Some(true) {
                if let Some(name) = r.get(4) {
                    return Ok(quote(&name.text()));
                }
            }
        }
        bail!("no {flag} folder visible over IMAP")
    }

    /// Store a complete message in a mailbox (already quoted), with the given flags.
    pub fn append(&mut self, mailbox: &str, flags: &str, message: &[u8]) -> Result<()> {
        self.tag += 1;
        let tag = format!("U{}", self.tag);
        let w = self.io.get_mut();
        w.write_all(format!("{tag} APPEND {mailbox} ({flags}) {{{}}}\r\n", message.len()).as_bytes())?;
        w.flush()?;
        loop {
            let resp = self.read_response()?;
            match resp.first().map(Val::text).as_deref() {
                // the server is ready for the message itself
                Some("+") => {
                    let w = self.io.get_mut();
                    w.write_all(message)?;
                    w.write_all(b"\r\n")?;
                    w.flush()?;
                }
                Some(t) if t == tag => {
                    let status = resp.get(1).map(Val::text).unwrap_or_default().to_uppercase();
                    if status == "OK" {
                        return Ok(());
                    }
                    let detail: Vec<String> = resp.iter().skip(1).map(Val::text).collect();
                    bail!("APPEND: {}", detail.join(" "));
                }
                _ => {}
            }
        }
    }

    pub fn uid_fetch(&mut self, set: &str, items: &str) -> Result<Vec<Fetched>> {
        let mut out = Vec::new();
        for r in self.run(&format!("UID FETCH {set} {items}"))? {
            if r.get(2).map(Val::text).as_deref() != Some("FETCH") {
                continue;
            }
            let attrs = r.get(3).map(Val::list).unwrap_or(&[]);
            let mut f = Fetched::new();
            for pair in attrs.chunks(2) {
                if let [k, v] = pair {
                    let key = k.text().to_uppercase();
                    let key = if key.starts_with("BODY[") { "BODY".to_string() } else { key };
                    f.insert(key, v.clone());
                }
            }
            out.push(f);
        }
        Ok(out)
    }

    pub fn uid_search(&mut self, criteria: &str) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        for r in self.run(&format!("UID SEARCH {criteria}"))? {
            if r.get(1).map(Val::text).as_deref() == Some("SEARCH") {
                out.extend(r.iter().skip(2).filter_map(|v| v.text().parse::<u32>().ok()));
            }
        }
        Ok(out)
    }

    pub fn logout(mut self) {
        let _ = self.run("LOGOUT");
    }
}

/// Parse an INTERNALDATE such as `17-Jul-2026 02:44:25 -0700` into a Unix timestamp.
pub fn parse_internaldate(s: &str) -> Option<i64> {
    let s = s.trim();
    let mut parts = s.split(' ');
    let mut dmy = parts.next()?.split('-');
    let (hms, tz) = (parts.next()?, parts.next()?);
    let day: i64 = dmy.next()?.parse().ok()?;
    let mon = dmy.next()?.to_ascii_lowercase();
    let year: i64 = dmy.next()?.parse().ok()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        .position(|m| *m == mon)? as i64
        + 1;
    let mut t = hms.split(':').map(|x| x.parse::<i64>().ok());
    let (h, m, sec) = (t.next()??, t.next()??, t.next()??);
    let sign = if tz.starts_with('-') { -1 } else { 1 };
    let tzn: i64 = tz.get(1..)?.parse().ok()?;
    let offset = sign * ((tzn / 100) * 3600 + (tzn % 100) * 60);
    // days from civil (Howard Hinnant's algorithm)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + m * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Piece {
        Piece::Text(s.as_bytes().to_vec())
    }

    #[test]
    fn parses_gmail_fetch_with_literal() {
        let pieces = vec![
            text(r#"* 12 FETCH (X-GM-THRID 1278455344230334865 X-GM-MSGID 1278455344230334865 X-GM-LABELS ("\\Important" "Money Stuff" \Starred) UID 4827 FLAGS (\Seen \Flagged) INTERNALDATE "08-Oct-2026 18:20:26 +0000" BODY[HEADER.FIELDS (FROM TO SUBJECT)] "#),
            Piece::Lit(b"From: (a) <a@b.c>\r\nSubject: hi\r\n\r\n".to_vec()),
            text(")"),
        ];
        let v = parse(&pieces);
        assert_eq!(v[2], Val::Atom("FETCH".into()));
        let attrs = v[3].list();
        assert_eq!(attrs.len(), 14);
        assert_eq!(attrs[1].text(), "1278455344230334865");
        assert_eq!(attrs[5].list()[1].text(), "Money Stuff");
        assert_eq!(attrs[9].list().len(), 2);
        assert_eq!(attrs[12].text(), "BODY[HEADER.FIELDS (FROM TO SUBJECT)]");
        assert!(attrs[13].bytes().starts_with(b"From: (a)"));
    }

    #[test]
    fn parses_status_and_list_lines() {
        let v = parse(&[text("* OK [UIDVALIDITY 3] UIDs valid")]);
        assert_eq!(v[2].text(), "[UIDVALIDITY 3]");
        let v = parse(&[text(r#"* LIST (\HasNoChildren \All) "/" "[Gmail]/All Mail""#)]);
        assert_eq!(v[2].list()[1].text(), "\\All");
        assert_eq!(v[4].text(), "[Gmail]/All Mail");
        let v = parse(&[text("* SEARCH 3 5 8")]);
        assert_eq!(v.len(), 5);
    }

    #[test]
    fn helpers() {
        assert_eq!(uid_set(&[9, 1, 2, 3, 7, 10]), "1:3,7,9:10");
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(parse_internaldate("01-Jan-1970 00:00:00 +0000"), Some(0));
        assert_eq!(parse_internaldate("08-Oct-2026 18:20:26 +0000"), Some(1791483626));
        assert_eq!(parse_internaldate("09-Oct-2026 03:20:26 +0900"), Some(1791483626));
    }
}
