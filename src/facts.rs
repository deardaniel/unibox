//! What a message says about itself beyond its text: whether a person wrote it, how to get
//! off its mailing list, which Google Group passed it on and who really wrote it, the
//! calendar event or booking it carries, and whether showing it would fetch pictures from
//! the sender. All of it is worked out once, when the body is stored, and kept as JSON.

use crate::trust::{self, Trust};
use mail_parser::{Message, MessageParser, MimeHeaders};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::sync::OnceLock;

#[derive(Serialize, Default, Debug)]
pub struct Facts {
    /// Sent by a system (newsletter, notification, receipt) rather than written by a person.
    pub automated: bool,
    /// What decided that, as the end of the sentence "...because it ___".
    pub why: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsubscribe: Option<Unsubscribe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    /// Calendar events, from every invitation or .ics file in the message.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Card>,
    /// The formatted version refers to pictures on someone's server.
    pub remote_images: bool,
}

/// How the sender says to leave its list (RFC 2369 and RFC 8058).
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Unsubscribe {
    /// A web address: posted to when `one_click`, otherwise a page to open.
    pub url: String,
    /// The sender accepts a bare POST to `url` as the whole request.
    pub one_click: bool,
    /// An address to write to, as the `mailto:` link given.
    pub mailto: String,
}

/// A Google Group sent this on. `sender_*` is the person who wrote to the group.
#[derive(Serialize, Default, Debug)]
pub struct Group {
    pub address: String,
    pub sender_name: String,
    pub sender_addr: String,
    /// The group put its own address in the From line, so Gmail's verdict is about the group.
    pub rewritten: bool,
    /// What the group found when the message reached it, about the person who wrote it.
    pub trust: Option<Trust>,
}

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct When {
    /// As written in the invitation: "20261010T130000", or "20261010" for a whole day.
    pub value: String,
    /// "UTC", a time zone name, or empty for "wherever the reader is".
    pub tz: String,
    pub all_day: bool,
}

#[derive(Serialize, Default, Debug)]
pub struct Attendee {
    pub name: String,
    pub email: String,
    /// "accepted", "declined", "tentative", or "" for no answer yet.
    pub status: String,
}

#[derive(Serialize, Default, Debug)]
pub struct Event {
    /// "request" (an invitation), "cancel", "reply", "publish", or "".
    pub method: String,
    pub summary: String,
    pub location: String,
    pub start: When,
    pub end: Option<When>,
    pub organizer: Attendee,
    pub attendees: Vec<Attendee>,
    pub recurs: bool,
    /// The event's UID, to notice the same event sent twice (an invitation and its .ics).
    #[serde(skip)]
    pub uid: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
    /// `value` is a date or date-time as the sender wrote it (ISO 8601), for the UI to format.
    pub time: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Link {
    pub label: String,
    pub url: String,
}

/// A booking, order or delivery the sender described in machine-readable form.
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Card {
    pub title: String,
    pub rows: Vec<Row>,
    pub actions: Vec<Link>,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

fn header(msg: &Message, name: &str) -> String {
    msg.header_raw(name).map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

/// (name, address) out of a header value like `Aoife <aoife@example.com>`.
fn mailbox(value: &str) -> (String, String) {
    let raw = format!("From: {value}\r\n\r\n");
    let msg = MessageParser::default().parse(raw.as_bytes());
    let a = msg.as_ref().and_then(|m| m.from()).and_then(|a| a.first());
    (
        a.and_then(|a| a.name()).unwrap_or_default().to_string(),
        a.and_then(|a| a.address()).unwrap_or_default().to_lowercase(),
    )
}

pub fn facts(msg: &Message, raw: &[u8]) -> Facts {
    let html = msg.html_body.first().and_then(|_| msg.body_html(0)).unwrap_or_default();
    let group = group(msg, raw);
    let events = events(msg);
    let mut cards = cards(&html);
    if !events.is_empty() {
        // the invitation itself says it better
        cards.retain(|c| !c.rows.iter().any(|r| r.label == "When" && c.actions.is_empty()));
    }
    let (automated, why) = automated(msg, group.as_ref());
    Facts { automated, why, unsubscribe: unsubscribe(msg), group, events, cards, remote_images: remote_images(&html) }
}

// ---- unsubscribe ---------------------------------------------------------------
/// Whether an unsubscribe address is somewhere on the public web. The request is made from
/// this PC on a stranger's say-so, so it must not be aimed at the PC itself or the home network.
fn public_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("HTTPS://")) else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || authority.starts_with('[') {
        return false;
    }
    let host = authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority).to_ascii_lowercase();
    host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && !host.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !["localhost", ".local", ".lan", ".internal", ".home.arpa", ".test"].iter().any(|s| host.ends_with(s))
}

/// Tell the sender of a stored message to stop, the one-click way (RFC 8058): a POST with a
/// fixed body to the address the message gave. Returns the sender's address.
pub fn unsubscribe_now(raw: &[u8]) -> anyhow::Result<String> {
    let msg = MessageParser::default().parse(raw).ok_or_else(|| anyhow::anyhow!("unreadable message"))?;
    let u = unsubscribe(&msg).filter(|u| u.one_click).ok_or_else(|| anyhow::anyhow!("This sender offers no one-click unsubscribe."))?;
    unsubscribe_post(&u.url)?;
    Ok(msg.from().and_then(|a| a.first()).and_then(|a| a.address()).unwrap_or_default().to_lowercase())
}

/// The one-click request itself, to the address a message gave for it.
pub fn unsubscribe_post(url: &str) -> anyhow::Result<()> {
    if !public_https(url) {
        anyhow::bail!("The unsubscribe address in this message doesn't look like a public website.");
    }
    // no redirects: the sender named this address, and nothing else gets the request
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(std::time::Duration::from_secs(20)).build();
    match agent.post(url).set("Content-Type", "application/x-www-form-urlencoded").send_string("List-Unsubscribe=One-Click") {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => anyhow::bail!("The sender's site answered {code}."),
        Err(e) => {
            eprintln!("unsubscribe: {e}");
            anyhow::bail!("Couldn't reach the sender's site.")
        }
    }
}

fn unsubscribe(msg: &Message) -> Option<Unsubscribe> {
    static TARGET: OnceLock<Regex> = OnceLock::new();
    let value = header(msg, "List-Unsubscribe");
    let mut u = Unsubscribe::default();
    for m in re(&TARGET, r"<\s*([^<>\s]+)\s*>").captures_iter(&value) {
        let target = &m[1];
        let lower = target.to_ascii_lowercase();
        if lower.starts_with("mailto:") && u.mailto.is_empty() {
            u.mailto = target.to_string();
        } else if lower.starts_with("https://") && !u.url.to_ascii_lowercase().starts_with("https://") {
            u.url = target.to_string();
        } else if lower.starts_with("http://") && u.url.is_empty() {
            u.url = target.to_string();
        }
    }
    u.one_click = u.url.to_ascii_lowercase().starts_with("https://")
        && header(msg, "List-Unsubscribe-Post").to_ascii_lowercase().contains("list-unsubscribe=one-click");
    (!u.url.is_empty() || !u.mailto.is_empty()).then_some(u)
}

// ---- Google Groups -------------------------------------------------------------
fn group(msg: &Message, raw: &[u8]) -> Option<Group> {
    static ADDR: OnceLock<Regex> = OnceLock::new();
    let addr = re(&ADDR, r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}");
    let original = header(msg, "X-Original-Sender");
    if msg.header_raw("X-Google-Group-Id").is_none() || original.is_empty() {
        return None;
    }
    // "list kids@example.com; contact kids+owners@example.com"
    let address = [header(msg, "Mailing-list"), header(msg, "List-Post")]
        .iter()
        .find_map(|h| addr.find(h).map(|m| m.as_str().to_lowercase()))
        .unwrap_or_default();
    let from = msg.from().and_then(|a| a.first());
    let from_addr = from.and_then(|f| f.address()).unwrap_or_default().to_lowercase();
    let from_name = from.and_then(|f| f.name()).unwrap_or_default();
    let (mut sender_name, mut sender_addr) = mailbox(&header(msg, "X-Original-From"));
    if sender_addr.is_empty() {
        sender_addr = addr.find(&original).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
    }
    let rewritten = !sender_addr.is_empty() && sender_addr != from_addr;
    if sender_name.is_empty() {
        // "'Aoife Byrne' via Kids" -> "Aoife Byrne"
        let name = from_name.rsplit_once(" via ").map(|(n, _)| n).unwrap_or(from_name);
        sender_name = name.trim_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace()).to_string();
    }
    if sender_name.eq_ignore_ascii_case(&sender_addr) {
        sender_name.clear();
    }
    let head = &raw[..raw.len().min(65536)];
    Some(Group { trust: trust::assess_original(head, &sender_addr), address, sender_name, sender_addr, rewritten })
}

// ---- a person, or a system? ----------------------------------------------------
fn automated(msg: &Message, group: Option<&Group>) -> (bool, &'static str) {
    static ROBOT: OnceLock<Regex> = OnceLock::new();
    let has = |name: &str| msg.header_raw(name).is_some();
    let from = msg.from().and_then(|a| a.first()).and_then(|a| a.address()).unwrap_or_default().to_lowercase();
    let sender = group.map(|g| g.sender_addr.as_str()).filter(|a| !a.is_empty()).unwrap_or(&from);
    let local = sender.split('@').next().unwrap_or_default();
    let robot = re(
        &ROBOT,
        r"^(?:.*no[-_.]?reply.*|.*do[-_.]?not[-_.]?reply.*|mailer-daemon|postmaster|bounces?|notifications?|notify|newsletters?|news|alerts?|updates?|digest|receipts?|billing|invoices?|marketing|promo(?:tions)?|offers?|deals?|mailer|auto(?:mated)?|system|robot|bot)(?:[+-].*)?$",
    );
    let auto_submitted = header(msg, "Auto-Submitted").to_ascii_lowercase();
    let precedence = header(msg, "Precedence").to_ascii_lowercase();
    // A list people post to carries their own writing: its posting address belongs to someone
    // other than the author. A newsletter's "posting address" is just its own reply address.
    let post = header(msg, "List-Post").to_ascii_lowercase();
    let post_domain = post.split("mailto:").nth(1).and_then(|m| m.split(['>', '?']).next()).and_then(|a| a.rsplit('@').next()).unwrap_or_default().to_string();
    let from_domain = from.rsplit('@').next().unwrap_or_default();
    let discussion = group.is_some() || (!post_domain.is_empty() && trust::org(&post_domain) != trust::org(from_domain));
    let marks = [
        (!auto_submitted.is_empty() && auto_submitted != "no", "says it was generated automatically"),
        (matches!(precedence.as_str(), "bulk" | "junk"), "is marked as bulk mail"),
        ((has("List-Unsubscribe") || has("List-ID")) && !discussion, "is from a mailing list that only sends"),
        (has("Feedback-ID"), "carries a bulk sender's tracking id"),
        (robot.is_match(local), "comes from an address that isn't a person's"),
    ];
    if let Some((_, why)) = marks.iter().find(|(hit, _)| *hit) {
        return (true, why);
    }
    // Plenty of receipts and notices carry none of those marks. What they also lack is any
    // sign of a mail program a person types into, or of being part of a conversation.
    static CLIENT: OnceLock<Regex> = OnceLock::new();
    let id = msg.message_id().unwrap_or_default().to_ascii_lowercase();
    let mailer = format!("{} {}", header(msg, "X-Mailer"), header(msg, "User-Agent")).to_ascii_lowercase();
    let signs = [
        (group.is_some(), "was written to a Google Group"),
        (discussion, "was posted to a discussion list"),
        // mail this account sent was never checked on the way in
        (!has("Authentication-Results"), "was sent from this account"),
        (
            ["mail.gmail.com", "outlook.com", "icloud.com", "yahoo.com", "yahoo.co.jp"].iter().any(|d| id.ends_with(d))
                || re(&CLIENT, "iphone|ipad|apple mail|outlook|thunderbird|mutt|becky|ymail|yahoomail|airmail|spark|mailmate|evolution|k-9|android|shuriken|al-mail|icloud").is_match(&mailer)
                || has("Thread-Index")
                || has("X-MS-Has-Attach"),
            "was written in a mail program",
        ),
        (has("In-Reply-To"), "is a reply in a conversation"),
    ];
    match signs.iter().find(|(hit, _)| *hit) {
        Some((_, why)) => (false, why),
        None => (true, "shows no sign of a mail program or of being a reply"),
    }
}

// ---- pictures ------------------------------------------------------------------
fn remote_images(html: &str) -> bool {
    static REMOTE: OnceLock<Regex> = OnceLock::new();
    re(
        &REMOTE,
        r#"(?is)<img\b[^>]*\bsrc\s*=\s*["']?\s*(?:https?:)?//|url\(\s*(?:&quot;|["'])?\s*(?:https?:)?//|\bbackground\s*=\s*["']?\s*(?:https?:)?//"#,
    )
    .is_match(html)
}

// ---- calendar events -----------------------------------------------------------
/// The events in every calendar part of the message. An invitation often comes both as the
/// message's calendar part and as an .ics attachment; the same event is kept once.
fn events(msg: &Message) -> Vec<Event> {
    let mut events: Vec<Event> = Vec::new();
    let calendars = msg.parts.iter().filter(|p| {
        let kind = p.content_type().map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()).to_ascii_lowercase());
        matches!(kind.as_deref(), Some("text/calendar" | "application/ics" | "text/x-vcalendar"))
            || p.attachment_name().is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"))
    });
    for part in calendars {
        for e in parse_events(&String::from_utf8_lossy(part.contents())) {
            let same = |o: &Event| if e.uid.is_empty() { o.summary == e.summary && o.start == e.start } else { o.uid == e.uid };
            if events.len() < 20 && !events.iter().any(same) {
                events.push(e);
            }
        }
    }
    events
}

/// One line of an iCalendar file: (NAME, parameters, value).
fn ical_line(line: &str) -> Option<(String, Vec<(String, String)>, &str)> {
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?;
    let (left, value) = (&line[..colon.0], &line[colon.0 + 1..]);
    let mut pieces = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in left.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == ';' && !quoted {
            pieces.push(&left[start..i]);
            start = i + 1;
        }
    }
    pieces.push(&left[start..]);
    let name = pieces[0].to_ascii_uppercase();
    let params = pieces[1..]
        .iter()
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some((name, params, value))
}

fn ical_text(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out.trim().to_string()
}

/// Every event in an iCalendar file. Changes to single dates of a repeating event (those
/// with a RECURRENCE-ID) are left out: the event they belong to is already there.
fn parse_events(text: &str) -> Vec<Event> {
    // long lines are folded: a line starting with a space continues the one before
    let unfolded = text.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "");
    let mut events = Vec::new();
    let mut method = String::new();
    // the event being read, whether it was cancelled, and whether it changes one date of another
    let mut current: Option<(Event, bool, bool)> = None;
    let mut nested = 0;
    let param = |params: &[(String, String)], key: &str| {
        params.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default()
    };
    let when = |params: &[(String, String)], value: &str| {
        let value = value.trim();
        When {
            all_day: !value.contains('T'),
            tz: if value.ends_with('Z') { "UTC".into() } else { param(params, "TZID") },
            value: value.trim_end_matches('Z').to_string(),
        }
    };
    let person = |params: &[(String, String)], value: &str| Attendee {
        name: param(params, "CN"),
        email: value.trim().strip_prefix("mailto:").or_else(|| value.trim().strip_prefix("MAILTO:")).unwrap_or_default().to_lowercase(),
        status: match param(params, "PARTSTAT").to_ascii_lowercase().as_str() {
            s @ ("accepted" | "declined" | "tentative") => s.to_string(),
            _ => String::new(),
        },
    };
    for line in unfolded.lines() {
        let Some((name, params, value)) = ical_line(line) else { continue };
        match (name.as_str(), value.trim().to_ascii_uppercase().as_str(), current.is_some()) {
            ("BEGIN", "VEVENT", false) => {
                current = Some(Default::default());
                nested = 0;
                continue;
            }
            ("END", "VEVENT", true) if nested == 0 => {
                if let Some((mut e, cancelled, one_date)) = current.take() {
                    let dated = e.start.value.len() >= 8 && e.start.value.bytes().take(8).all(|b| b.is_ascii_digit());
                    if dated && !one_date {
                        if cancelled {
                            e.method = "cancel".into();
                        }
                        events.push(e);
                    }
                }
                continue;
            }
            // alarms inside the event have a DESCRIPTION and more of their own
            ("BEGIN", _, true) => nested += 1,
            ("END", _, true) if nested > 0 => nested -= 1,
            ("METHOD", m, false) => method = m.to_ascii_lowercase(),
            _ => {}
        }
        let Some((e, cancelled, one_date)) = current.as_mut().filter(|_| nested == 0) else { continue };
        match name.as_str() {
            "SUMMARY" => e.summary = ical_text(value),
            "LOCATION" => e.location = ical_text(value),
            "DTSTART" => e.start = when(&params, value),
            "DTEND" => e.end = Some(when(&params, value)),
            "ORGANIZER" => e.organizer = person(&params, value),
            "ATTENDEE" if e.attendees.len() < 40 => e.attendees.push(person(&params, value)),
            "RRULE" => e.recurs = true,
            "UID" => e.uid = value.trim().to_string(),
            "RECURRENCE-ID" => *one_date = true,
            "STATUS" if value.trim().eq_ignore_ascii_case("CANCELLED") => *cancelled = true,
            _ => {}
        }
    }
    for e in &mut events {
        if e.method.is_empty() {
            e.method = method.clone();
        }
    }
    events
}

// ---- bookings, orders, deliveries ------------------------------------------------
// Senders describe these for Gmail in schema.org terms, either as JSON-LD in a <script> or
// as "microdata" attributes on the HTML itself. Both are read into the same JSON shape.

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#34;", "\"").replace("&#39;", "'").replace("&apos;", "'")
        .replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn json_ld(html: &str) -> Vec<Value> {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    re(&SCRIPT, r#"(?is)<script\b[^>]*application/ld\+json[^>]*>(.*?)</script>"#)
        .captures_iter(html)
        .filter_map(|c| serde_json::from_str::<Value>(c[1].trim()).ok())
        .flat_map(|v| match v {
            Value::Array(items) => items,
            Value::Object(mut o) if o.contains_key("@graph") => match o.remove("@graph") {
                Some(Value::Array(items)) => items,
                _ => vec![],
            },
            other => vec![other],
        })
        .collect()
}

fn microdata(html: &str) -> Vec<Value> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    if !html.to_ascii_lowercase().contains("itemscope") {
        return vec![];
    }
    enum Prop {
        Text(String),
        Item(usize),
    }
    struct Item {
        kind: String,
        props: Vec<(String, Prop)>,
    }
    let tag = re(&TAG, r#"(?s)<(/?)([a-zA-Z][a-zA-Z0-9]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#);
    let attr = re(&ATTR, r#"(?s)([a-zA-Z][\w-]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?"#);
    const VOID: [&str; 13] = ["meta", "link", "img", "br", "hr", "input", "area", "base", "col", "embed", "source", "track", "wbr"];
    let mut items: Vec<Item> = Vec::new();
    let mut roots = Vec::new();
    let mut open: Vec<(String, Option<usize>)> = Vec::new();
    for t in tag.captures_iter(html) {
        let name = t[2].to_ascii_lowercase();
        if &t[1] == "/" {
            if let Some(at) = open.iter().rposition(|(n, _)| *n == name) {
                open.truncate(at);
            }
            continue;
        }
        let mut attrs = std::collections::HashMap::new();
        for a in attr.captures_iter(&t[3]) {
            let value = a.get(2).or(a.get(3)).or(a.get(4)).map(|m| unescape(m.as_str())).unwrap_or_default();
            attrs.entry(a[1].to_ascii_lowercase()).or_insert(value);
        }
        let current = open.iter().rev().find_map(|(_, item)| *item);
        let prop = attrs.get("itemprop").cloned().filter(|p| !p.is_empty());
        let mut scope = None;
        if attrs.contains_key("itemscope") {
            let kind = attrs.get("itemtype").map(|t| t.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string());
            items.push(Item { kind: kind.unwrap_or_default(), props: Vec::new() });
            let id = items.len() - 1;
            match (current, prop) {
                (Some(parent), Some(p)) => items[parent].props.push((p, Prop::Item(id))),
                _ => roots.push(id),
            }
            scope = Some(id);
        } else if let (Some(parent), Some(p)) = (current, prop) {
            if let Some(v) = ["content", "href", "src", "datetime"].iter().find_map(|k| attrs.get(*k)) {
                items[parent].props.push((p, Prop::Text(v.trim().to_string())));
            }
        }
        if !VOID.contains(&name.as_str()) && !t[3].trim_end().ends_with('/') {
            open.push((name, scope));
        }
        if items.len() > 400 {
            break;
        }
    }
    fn build(items: &[Item], id: usize, depth: usize) -> Value {
        let mut o = Map::new();
        o.insert("@type".into(), items[id].kind.clone().into());
        for (name, p) in &items[id].props {
            let v = match p {
                Prop::Text(s) => Value::String(s.clone()),
                Prop::Item(child) if depth < 12 => build(items, *child, depth + 1),
                Prop::Item(_) => continue,
            };
            match o.get_mut(name) {
                Some(Value::Array(list)) => list.push(v),
                Some(first) => *first = Value::Array(vec![first.take(), v]),
                None => {
                    o.insert(name.clone(), v);
                }
            }
        }
        Value::Object(o)
    }
    roots.into_iter().map(|id| build(&items, id, 0)).collect()
}

/// The text at a dotted path. An object stands for its name, a list for its first entry.
fn at(v: &Value, path: &str) -> String {
    let mut v = v;
    for key in path.split('.') {
        if let Value::Array(list) = v {
            v = list.first().unwrap_or(&Value::Null);
        }
        v = v.get(key).unwrap_or(&Value::Null);
    }
    text(v)
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Number(n) => n.to_string(),
        Value::Array(list) => list.first().map(text).unwrap_or_default(),
        Value::Object(o) => o.get("name").map(text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn kind(v: &Value) -> String {
    let t = text(v.get("@type").unwrap_or(&Value::Null));
    t.rsplit('/').next().unwrap_or_default().to_string()
}

fn address(v: &Value, path: &str) -> String {
    let direct = at(v, path);
    if !direct.is_empty() {
        return direct;
    }
    ["streetAddress", "addressLocality", "addressRegion", "postalCode"]
        .iter()
        .map(|k| at(v, &format!("{path}.{k}")))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn web(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.starts_with("https://") || u.starts_with("http://")
}

/// "Go to" buttons the sender offers: `potentialAction` (or Gmail's older `action`).
fn actions(v: &Value, out: &mut Vec<Link>) {
    for key in ["potentialAction", "action"] {
        let list = match v.get(key) {
            Some(Value::Array(list)) => list.iter().collect(),
            Some(one) => vec![one],
            None => vec![],
        };
        for a in list {
            // other kinds (confirm, save, RSVP) act on a click without showing a page; leave those out
            if !matches!(kind(a).as_str(), "ViewAction" | "TrackAction") {
                continue;
            }
            let url = [at(a, "url"), text(a.get("target").unwrap_or(&Value::Null)), at(a, "target.urlTemplate"), at(a, "target.url"), at(a, "handler.url")]
                .into_iter()
                .find(|u| web(u));
            let label = at(a, "name");
            if let Some(url) = url {
                let label = if label.is_empty() || label.chars().count() > 40 { "Open".to_string() } else { label };
                out.push(Link { label, url });
            }
        }
    }
}

fn card(v: &Value) -> Option<Card> {
    let mut c = Card::default();
    let mut row = |label: &'static str, value: String, time: bool| {
        if !value.is_empty() {
            c.rows.push(Row { label, value, time });
        }
    };
    let arrow = |a: String, b: String| if a.is_empty() || b.is_empty() { String::new() } else { format!("{a} → {b}") };
    let or = |a: String, b: String| if a.is_empty() { b } else { a };
    let k = kind(v);
    let title = match k.as_str() {
        "FlightReservation" => {
            let f = "reservationFor";
            row("Departs", at(v, &format!("{f}.departureTime")), true);
            row("From", at(v, &format!("{f}.departureAirport")), false);
            row("Arrives", at(v, &format!("{f}.arrivalTime")), true);
            row("To", at(v, &format!("{f}.arrivalAirport")), false);
            row("Passenger", at(v, "underName"), false);
            let number = format!("{}{}", at(v, &format!("{f}.airline.iataCode")), at(v, &format!("{f}.flightNumber")));
            let route = arrow(at(v, &format!("{f}.departureAirport.iataCode")), at(v, &format!("{f}.arrivalAirport.iataCode")));
            format!("Flight {number} {route}").trim().to_string()
        }
        "TrainReservation" | "BusReservation" => {
            let f = "reservationFor";
            row("Departs", at(v, &format!("{f}.departureTime")), true);
            row("Arrives", at(v, &format!("{f}.arrivalTime")), true);
            let (from, to) = if k == "TrainReservation" { ("departureStation", "arrivalStation") } else { ("departureBusStop", "arrivalBusStop") };
            or(arrow(at(v, &format!("{f}.{from}")), at(v, &format!("{f}.{to}"))), "Trip".into())
        }
        "LodgingReservation" => {
            row("Check-in", at(v, "checkinDate"), true);
            row("Check-out", at(v, "checkoutDate"), true);
            row("Address", address(v, "reservationFor.address"), false);
            or(at(v, "reservationFor"), "Stay".into())
        }
        "FoodEstablishmentReservation" => {
            row("When", at(v, "startTime"), true);
            row("Party", at(v, "partySize"), false);
            row("Address", address(v, "reservationFor.address"), false);
            or(at(v, "reservationFor"), "Table booking".into())
        }
        "RentalCarReservation" => {
            row("Pick-up", at(v, "pickupTime"), true);
            row("At", at(v, "pickupLocation"), false);
            row("Drop-off", at(v, "dropoffTime"), true);
            or(at(v, "reservationFor"), "Car hire".into())
        }
        "EventReservation" => {
            row("When", at(v, "reservationFor.startDate"), true);
            row("Where", at(v, "reservationFor.location"), false);
            row("Address", address(v, "reservationFor.location.address"), false);
            or(at(v, "reservationFor"), "Event booking".into())
        }
        "Event" => {
            row("When", at(v, "startDate"), true);
            row("Until", at(v, "endDate"), true);
            row("Where", at(v, "location"), false);
            or(at(v, "name"), "Event".into())
        }
        "Order" => {
            row("From", or(at(v, "merchant"), at(v, "seller")), false);
            row("Status", at(v, "orderStatus").rsplit('/').next().unwrap_or_default().trim_start_matches("Order").to_string(), false);
            let price = or(at(v, "price"), at(v, "acceptedOffer.price"));
            let currency = or(at(v, "priceCurrency"), at(v, "acceptedOffer.priceCurrency"));
            row("Total", format!("{price} {currency}").trim().to_string(), false);
            format!("Order {}", at(v, "orderNumber")).trim().to_string()
        }
        "ParcelDelivery" => {
            row("Carrier", at(v, "carrier"), false);
            row("Tracking number", at(v, "trackingNumber"), false);
            row("Expected", or(at(v, "expectedArrivalUntil"), at(v, "expectedArrivalFrom")), true);
            row("To", address(v, "deliveryAddress"), false);
            if web(&at(v, "trackingUrl")) {
                c.actions.push(Link { label: "Track".into(), url: at(v, "trackingUrl") });
            }
            or(at(v, "itemShipped"), "Delivery".into())
        }
        // an email that only offers a button
        _ => String::new(),
    };
    if k.ends_with("Reservation") {
        let mut row = |label: &'static str, value: String| {
            if !value.is_empty() {
                c.rows.push(Row { label, value, time: false });
            }
        };
        row("Booking reference", at(v, "reservationNumber"));
        if at(v, "reservationStatus").to_ascii_lowercase().contains("cancel") {
            row("Status", "Cancelled".into());
        }
        for (label, key) in [("Manage booking", "modifyReservationUrl"), ("Cancel booking", "cancelReservationUrl")] {
            if web(&at(v, key)) {
                c.actions.push(Link { label: label.into(), url: at(v, key) });
            }
        }
    }
    c.title = title;
    actions(v, &mut c.actions);
    if matches!(k.as_str(), "Order" | "ParcelDelivery") && c.actions.is_empty() && web(&at(v, "url")) {
        c.actions.push(Link { label: "View".into(), url: at(v, "url") });
    }
    c.actions.truncate(3);
    (!c.rows.is_empty() || !c.actions.is_empty()).then_some(c)
}

fn cards(html: &str) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    for v in json_ld(html).iter().chain(microdata(html).iter()) {
        if let Some(c) = card(v) {
            // the same thing is often given in both forms
            if !out.contains(&c) && out.len() < 4 {
                out.push(c);
            }
        }
    }
    // a bare button is only worth showing when nothing fuller offers the same link
    let full: Vec<String> = out.iter().filter(|c| !c.rows.is_empty()).flat_map(|c| c.actions.iter().map(|a| a.url.clone())).collect();
    out.retain(|c| !c.rows.is_empty() || !c.actions.iter().all(|a| full.contains(&a.url)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(raw: &str) -> Facts {
        let raw = raw.replace('\n', "\r\n");
        let msg = MessageParser::default().parse(raw.as_bytes()).unwrap();
        facts(&msg, raw.as_bytes())
    }

    #[test]
    fn unsubscribe_prefers_one_click() {
        let f = of("From: News <news@shop.example>\nList-Unsubscribe: <mailto:u@shop.example?subject=off>,\n <https://shop.example/u/abc>\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\nSubject: s\n\nhi\n");
        let u = f.unsubscribe.unwrap();
        assert_eq!(u.url, "https://shop.example/u/abc");
        assert!(u.one_click);
        assert_eq!(u.mailto, "mailto:u@shop.example?subject=off");
        assert!(f.automated);
        // a plain web page is not one-click, and an insecure one never is
        let f = of("From: a@shop.example\nList-Unsubscribe: <http://shop.example/u>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\nSubject: s\n\nhi\n");
        assert!(!f.unsubscribe.unwrap().one_click);
        assert!(of("From: a@b.example\nSubject: s\n\nhi\n").unsubscribe.is_none());
    }

    #[test]
    fn unsubscribe_requests_only_go_to_public_sites() {
        assert!(public_https("https://shop.example.com/u/abc?x=1"));
        assert!(public_https("https://shop.example.com:8443/u"));
        for bad in ["http://shop.example.com/u", "https://127.0.0.1/u", "https://localhost/u", "https://192.168.0.1:8443/x",
            "https://[::1]/u", "https://router/u", "https://nas.local/u", "https://shop.example.com@127.0.0.1/u", "https://2130706433/"] {
            assert!(!public_https(bad), "{bad}");
        }
    }

    /// Makes a real request to a public echo service: `cargo test live_unsubscribe -- --ignored`.
    #[test]
    #[ignore]
    fn live_unsubscribe() {
        let ok = b"From: News <news@shop.example>\r\nList-Unsubscribe: <https://postman-echo.com/post>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\nhi\r\n";
        assert_eq!(unsubscribe_now(ok).unwrap(), "news@shop.example");
        let gone = b"From: a@shop.example\r\nList-Unsubscribe: <https://postman-echo.com/status/404>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\nhi\r\n";
        assert!(unsubscribe_now(gone).unwrap_err().to_string().contains("404"));
    }

    #[test]
    fn people_and_systems() {
        let checked = "Authentication-Results: mx.google.com; spf=pass smtp.mailfrom=x@example.com\n";
        let gmail = "Message-ID: <CAF123@mail.gmail.com>\n";
        assert!(!of(&format!("{checked}{gmail}From: Aoife <aoife@example.com>\nSubject: lunch\n\nhi\n")).automated);
        assert!(!of(&format!("{checked}From: Aoife <aoife@example.com>\nX-Mailer: iPhone Mail (22A3354)\nSubject: lunch\n\nhi\n")).automated);
        assert!(!of(&format!("{checked}From: Desk <desk@example.com>\nIn-Reply-To: <a@b>\nSubject: Re: lunch\n\nhi\n")).automated);
        assert!(!of("From: Me <me@example.com>\nSubject: sent from here\n\nhi\n").automated);
        // a receipt with no list headers at all: nothing about it says a person typed it
        let bank = of(&format!("{checked}From: Bank <info@bank.example>\nMessage-ID: <1.2@bank.example>\nSubject: Statement\n\nhi\n"));
        assert!(bank.automated && bank.why.starts_with("shows no sign"));
        assert_eq!(of(&format!("{checked}{gmail}From: Aoife <aoife@example.com>\nSubject: lunch\n\nhi\n")).why, "was written in a mail program");
        assert!(of(&format!("{checked}{gmail}From: Shop <no-reply@shop.example>\nSubject: s\n\nhi\n")).automated);
        assert!(of(&format!("{checked}{gmail}From: Shop <hello@shop.example>\nPrecedence: bulk\nSubject: s\n\nhi\n")).automated);
        assert!(of(&format!("{checked}{gmail}From: Aoife <aoife@example.com>\nAuto-Submitted: auto-replied\nSubject: away\n\nhi\n")).automated);
        assert!(!of(&format!("{checked}{gmail}From: Aoife <aoife@example.com>\nAuto-Submitted: no\nSubject: s\n\nhi\n")).automated);
        // a discussion list is people talking; a newsletter's posting address is only its own
        let list = "List-ID: <dev.lists.example>\nList-Unsubscribe: <mailto:leave@lists.example>\nPrecedence: list\n";
        assert!(!of(&format!("{checked}From: Aoife <aoife@example.com>\n{list}List-Post: <mailto:dev@lists.example>\nSubject: s\n\nhi\n")).automated);
        assert!(of(&format!("{checked}From: News <news@lists.example>\n{list}List-Post: <mailto:reply@mg.lists.example>\nSubject: s\n\nhi\n")).automated);
        assert!(of(&format!("{checked}{gmail}From: Aoife <aoife@example.com>\n{list}Subject: s\n\nhi\n")).automated);
        assert!(!of(&format!("{checked}{gmail}From: Newsome <newsome@example.com>\nSubject: s\n\nhi\n")).automated, "a name that merely starts like a robot's");
    }

    const GROUP: &str = "Authentication-Results: mx.google.com;\n       dkim=pass header.i=@example-ie.20230601.gappssmtp.com header.s=20230601;\n       spf=pass smtp.mailfrom=\"kids+bncABC@example.ie\"\n\
X-Original-Authentication-Results: mx.google.com;       dkim=pass header.i=@gmail.com header.s=20230601;\n       spf=pass (google.com: domain of aoife@gmail.com designates 1.2.3.4 as permitted sender) smtp.mailfrom=aoife@gmail.com;\n       dmarc=pass (p=NONE sp=QUARANTINE dis=NONE) header.from=gmail.com\n\
From: \"'Aoife Byrne' via Kids\" <kids@example.ie>\nReply-To: Aoife Byrne <aoife@gmail.com>\nX-Original-Sender: aoife@gmail.com\n\
Precedence: list\nMailing-list: list kids@example.ie; contact kids+owners@example.ie\nList-ID: <kids.example.ie>\nX-Google-Group-Id: 123\n\
List-Post: <https://groups.google.com/a/example.ie/group/kids/post>, <mailto:kids@example.ie>\nList-Unsubscribe: <mailto:googlegroups-manage+123+unsubscribe@googlegroups.com>\nSubject: s\n\nhi\n";

    #[test]
    fn a_group_message_names_who_wrote_it() {
        let f = of(GROUP);
        let g = f.group.unwrap();
        assert_eq!(g.address, "kids@example.ie");
        assert_eq!((g.sender_name.as_str(), g.sender_addr.as_str()), ("Aoife Byrne", "aoife@gmail.com"));
        assert!(g.rewritten);
        let t = g.trust.unwrap();
        assert_eq!((t.level, t.domain.as_str()), ("verified", "gmail.com"));
        assert!(!f.automated, "a person wrote to the group");
        // a group that leaves the From line alone
        let plain = GROUP.replace("\"'Aoife Byrne' via Kids\" <kids@example.ie>", "Aoife Byrne <aoife@gmail.com>");
        let g = of(&plain).group.unwrap();
        assert!(!g.rewritten);
        assert_eq!(g.sender_name, "Aoife Byrne");
    }

    fn first_event(text: &str) -> Option<Event> {
        parse_events(text).into_iter().next()
    }

    #[test]
    fn every_event_is_read_once() {
        let ics = |uid: &str, summary: &str, day: &str| {
            format!("BEGIN:VCALENDAR\nMETHOD:PUBLISH\nBEGIN:VEVENT\nUID:{uid}\nDTSTART:{day}T040000Z\nSUMMARY:{summary}\nEND:VEVENT\nEND:VCALENDAR\n")
        };
        // two files, one of them also sent as the message's own calendar part
        let f = of(&format!("From: a@example.com\nSubject: s\nMIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=b1\n\n\
--b1\nContent-Type: text/plain\n\nTwo events\n--b1\nContent-Type: text/calendar; charset=utf-8\n\n{}\
--b1\nContent-Type: application/octet-stream; name=lunch.ics\nContent-Disposition: attachment; filename=lunch.ics\n\n{}\
--b1\nContent-Type: application/octet-stream; name=live.ics\nContent-Disposition: attachment; filename=live.ics\n\n{}--b1--\n",
            ics("1@x", "Lunch", "20261024"), ics("1@x", "Lunch", "20261024"), ics("2@x", "Live", "20261230")));
        let names: Vec<_> = f.events.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(names, ["Lunch", "Live"]);
        assert_eq!(f.events[1].method, "publish");
        // two events in one file; a changed date of a repeating one is not a third
        let both = parse_events("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20261024T040000Z\nRRULE:FREQ=WEEKLY\nSUMMARY:Class\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20261031T040000Z\nDTSTART:20261101T040000Z\nSUMMARY:Class\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:b\nDTSTART:20261230T090000Z\nSUMMARY:Live\nSTATUS:CANCELLED\nEND:VEVENT\nEND:VCALENDAR\n");
        assert_eq!(both.iter().map(|e| (e.summary.as_str(), e.method.as_str())).collect::<Vec<_>>(), [("Class", ""), ("Live", "cancel")]);
    }

    #[test]
    fn invitations_are_read() {
        let ics = "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VTIMEZONE\nTZID:Asia/Tokyo\nBEGIN:STANDARD\nDTSTART:19700101T000000\nEND:STANDARD\nEND:VTIMEZONE\n\
BEGIN:VEVENT\nDTSTART;TZID=Asia/Tokyo:20261010T130000\nDTEND;TZID=Asia/Tokyo:20261010T140000\nRRULE:FREQ=WEEKLY\n\
ORGANIZER;CN=Aoife Byrne:mailto:aoife@example.com\nATTENDEE;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;CN=\"Byrne, Dan\";X-NUM-GUESTS=0:mailto:D\n an@example.com\n\
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:x@example.com\nSUMMARY:Lunch\\, then a walk\nLOCATION:Cafe\\; upstairs\n\
BEGIN:VALARM\nDESCRIPTION:Reminder\nEND:VALARM\nEND:VEVENT\nEND:VCALENDAR\n";
        let e = first_event(&ics.replace('\n', "\r\n")).unwrap();
        assert_eq!(e.method, "request");
        assert_eq!(e.summary, "Lunch, then a walk");
        assert_eq!(e.location, "Cafe; upstairs");
        assert_eq!(e.start, When { value: "20261010T130000".into(), tz: "Asia/Tokyo".into(), all_day: false });
        assert_eq!(e.organizer.name, "Aoife Byrne");
        assert_eq!(e.attendees.len(), 2);
        assert_eq!((e.attendees[0].name.as_str(), e.attendees[0].email.as_str(), e.attendees[0].status.as_str()), ("Byrne, Dan", "dan@example.com", "accepted"));
        assert_eq!(e.attendees[1].status, "");
        assert!(e.recurs);
        let day = first_event("BEGIN:VEVENT\nDTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261226\nSUMMARY:Away\nEND:VEVENT\n").unwrap();
        assert!(day.start.all_day && day.start.tz.is_empty());
        let utc = first_event("BEGIN:VEVENT\nDTSTART:20261010T040000Z\nSUMMARY:x\nSTATUS:CANCELLED\nEND:VEVENT\n").unwrap();
        assert_eq!((utc.start.value.as_str(), utc.start.tz.as_str(), utc.method.as_str()), ("20261010T040000", "UTC", "cancel"));
        assert!(first_event("BEGIN:VCALENDAR\nEND:VCALENDAR\n").is_none());
        // as a part of a message
        let f = of(&format!("From: a@example.com\nSubject: s\nMIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=b1\n\n\
--b1\nContent-Type: text/plain\n\nInvitation\n--b1\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\n\n{ics}--b1--\n"));
        assert_eq!(f.events[0].summary, "Lunch, then a walk");
    }

    #[test]
    fn bookings_become_cards() {
        let ld = r#"<html><body><script type="application/ld+json">{"@context":"http://schema.org","@type":"FoodEstablishmentReservation",
"reservationNumber":"R-77","partySize":3,"startTime":"2026-10-17T13:15:00+09:00",
"reservationFor":{"@type":"FoodEstablishment","name":"Diner Ariake","address":{"@type":"PostalAddress","streetAddress":"1-2-3 Ariake","addressLocality":"Koto"}},
"modifyReservationUrl":"https://book.example/r/77"}</script><p>See you then</p></body></html>"#;
        let c = &cards(ld)[0];
        assert_eq!(c.title, "Diner Ariake");
        assert!(c.rows.contains(&Row { label: "When", value: "2026-10-17T13:15:00+09:00".into(), time: true }));
        assert!(c.rows.contains(&Row { label: "Party", value: "3".into(), time: false }));
        assert!(c.rows.contains(&Row { label: "Address", value: "1-2-3 Ariake, Koto".into(), time: false }));
        assert!(c.rows.contains(&Row { label: "Booking reference", value: "R-77".into(), time: false }));
        assert_eq!(c.actions, vec![Link { label: "Manage booking".into(), url: "https://book.example/r/77".into() }]);

        let micro = r#"<div itemscope itemtype="http://schema.org/FlightReservation"><meta itemprop="reservationNumber" content="ABC123"/>
<div itemprop="underName" itemscope itemtype="http://schema.org/Person"><meta itemprop="name" content="Dan Byrne"></div>
<div itemprop="reservationFor" itemscope itemtype="http://schema.org/Flight"><meta itemprop="flightNumber" content="7">
<div itemprop="airline" itemscope itemtype="http://schema.org/Airline"><meta itemprop="name" content="Example Air"><meta itemprop="iataCode" content="EX"></div>
<div itemprop="departureAirport" itemscope itemtype="http://schema.org/Airport"><meta itemprop="name" content="Haneda"><meta itemprop="iataCode" content="HND"></div>
<meta itemprop="departureTime" content="2026-12-20T10:40:00+09:00">
<div itemprop="arrivalAirport" itemscope itemtype="http://schema.org/Airport"><meta itemprop="iataCode" content="DUB"></div></div></div>
<div itemscope itemtype="http://schema.org/EmailMessage"><div itemprop="potentialAction" itemscope itemtype="http://schema.org/ViewAction">
<link itemprop="target" href="https://air.example/trip?a=1&amp;b=2"/><meta itemprop="name" content="View trip"/></div>
<div itemprop="potentialAction" itemscope itemtype="http://schema.org/ConfirmAction"><link itemprop="target" href="https://air.example/confirm"/></div></div>"#;
        let found = cards(micro);
        assert_eq!(found[0].title, "Flight EX7 HND → DUB");
        assert!(found[0].rows.contains(&Row { label: "From", value: "Haneda".into(), time: false }));
        assert!(found[0].rows.contains(&Row { label: "Passenger", value: "Dan Byrne".into(), time: false }));
        assert_eq!(found[1].actions, vec![Link { label: "View trip".into(), url: "https://air.example/trip?a=1&b=2".into() }]);
        assert_eq!(found.len(), 2, "an action that acts without showing a page is not offered");
        // marketing markup makes no card; broken JSON is ignored
        assert!(cards(r#"<script type="application/ld+json">{"@type":"PromotionCard","image":"https://x.example/a.png"}</script>"#).is_empty());
        assert!(cards(r#"<script type="application/ld+json">{nope</script>"#).is_empty());
    }

    /// Counts over a copy of a real mail cache: `UNIBOX_TALLY=/path/mail.db cargo test tally -- --ignored --nocapture`.
    /// Prints numbers, field names and sender domains only, never what a message says.
    #[test]
    #[ignore]
    fn tally() {
        let Ok(path) = std::env::var("UNIBOX_TALLY") else { return };
        let c = rusqlite::Connection::open(path).unwrap();
        let mut st = c.prepare("SELECT raw FROM bodies").unwrap();
        let mut n: std::collections::BTreeMap<String, usize> = Default::default();
        let mut people: std::collections::HashMap<String, usize> = Default::default();
        let mut count = |k: String| *n.entry(k).or_default() += 1;
        for raw in st.query_map([], |r| r.get::<_, Vec<u8>>(0)).unwrap() {
            let raw = raw.unwrap();
            let Some(msg) = MessageParser::default().parse(&raw[..]) else { continue };
            let f = facts(&msg, &raw);
            count(format!("automated={}", f.automated));
            if !f.automated {
                let from = msg.from().and_then(|a| a.first()).and_then(|a| a.address()).unwrap_or_default().to_lowercase();
                *people.entry(from.rsplit('@').next().unwrap_or_default().to_string()).or_default() += 1;
            }
            if let Some(u) = &f.unsubscribe {
                count(format!("unsubscribe one_click={} page={} mailto={}", u.one_click, !u.one_click && !u.url.is_empty(), !u.mailto.is_empty()));
            }
            if let Some(g) = &f.group {
                count(format!("group rewritten={} named={} sender={}", g.rewritten, !g.sender_name.is_empty(), g.trust.as_ref().map(|t| t.level).unwrap_or("no record")));
            }
            for e in &f.events {
                count(format!("event method={} tz={} attendees={}", e.method, if e.start.all_day { "all-day" } else if e.start.tz == "UTC" { "utc" } else if e.start.tz.is_empty() { "floating" } else { "named" }, e.attendees.len().min(3)));
            }
            for card in &f.cards {
                count(format!("card rows=[{}] actions={}", card.rows.iter().map(|r| r.label).collect::<Vec<_>>().join(","), card.actions.len()));
            }
            count(format!("remote_images={}", f.remote_images));
        }
        for (k, v) in &n {
            println!("{v:6}  {k}");
        }
        let mut people: Vec<_> = people.into_iter().collect();
        people.sort_by(|a, b| b.1.cmp(&a.1));
        println!("\nsender domains counted as people:");
        for (d, v) in people.iter().take(45) {
            println!("{v:6}  {d}");
        }
    }

    #[test]
    fn remote_pictures_are_noticed() {
        assert!(remote_images(r#"<p><img alt="" src="https://cdn.example/a.png"></p>"#));
        assert!(remote_images(r#"<td style="background:url('http://cdn.example/a.png')">"#));
        assert!(remote_images(r#"<td background="//cdn.example/a.png">"#));
        assert!(!remote_images(r#"<img src="cid:logo@x"><img src="data:image/png;base64,AAAA"><a href="https://example.com">x</a>"#));
    }
}
