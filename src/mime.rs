//! Turning raw RFC 822 messages into things the UI can show safely.

use mail_parser::{Address, MessageParser, MimeHeaders, PartType};
use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

pub struct Headers {
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
}

fn addr_list(a: Option<&Address>) -> String {
    let Some(a) = a else { return String::new() };
    a.iter()
        .map(|x| match (x.name(), x.address()) {
            (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
            (_, Some(e)) => e.to_string(),
            (Some(n), None) => n.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn parse_headers(raw: &[u8]) -> Headers {
    let msg = MessageParser::default().parse(raw);
    let msg = msg.as_ref();
    let from = msg.and_then(|m| m.from()).and_then(|a| a.first());
    Headers {
        from_name: from.and_then(|f| f.name()).unwrap_or_default().to_string(),
        from_addr: from.and_then(|f| f.address()).unwrap_or_default().to_string(),
        to: addr_list(msg.and_then(|m| m.to())),
        cc: addr_list(msg.and_then(|m| m.cc())),
        subject: msg.and_then(|m| m.subject()).unwrap_or("(no subject)").to_string(),
    }
}

/// What a reply to a message needs from it.
pub struct ReplyHeaders {
    /// Who to answer: Reply-To if the message has one, otherwise its sender.
    pub to: String,
    pub message_id: String,
    /// The message's own References, to be extended with its id.
    pub references: String,
}

pub fn reply_headers(raw: &[u8]) -> ReplyHeaders {
    let msg = MessageParser::default().parse(raw);
    let msg = msg.as_ref();
    let reply_to = addr_list(msg.and_then(|m| m.reply_to()));
    ReplyHeaders {
        to: if reply_to.is_empty() { addr_list(msg.and_then(|m| m.from())) } else { reply_to },
        message_id: msg.and_then(|m| m.message_id()).unwrap_or_default().to_string(),
        references: msg
            .and_then(|m| m.header_raw("References"))
            .map(|r| r.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default(),
    }
}

#[derive(Serialize)]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub mime: String,
    pub size: usize,
    /// How the app can show it without saving: "image", "pdf", "text", or "" for no preview.
    pub kind: &'static str,
}

/// What a file can safely be previewed as. The declared type is not trusted on its own: the
/// bytes have to look right too, and only formats that cannot carry script are allowed
/// (so no HTML and no SVG).
fn preview_kind(name: &str, bytes: &[u8]) -> (&'static str, &'static str) {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let image = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    };
    if let Some(mime) = image {
        return ("image", mime);
    }
    if bytes.starts_with(b"%PDF-") {
        return ("pdf", "application/pdf");
    }
    let texty = ["txt", "csv", "tsv", "log", "md", "json", "xml", "yml", "yaml", "ics", "vcf", "eml"];
    if texty.contains(&ext.as_str()) && bytes.len() <= 2_000_000 && !bytes.contains(&0) {
        // always labelled plain text, whatever it contains, so it is shown and never run
        return ("text", "text/plain; charset=utf-8");
    }
    ("", "application/octet-stream")
}

fn mime_of(part: &mail_parser::MessagePart) -> String {
    part.content_type()
        .map(|c| match c.subtype() {
            Some(s) => format!("{}/{}", c.ctype(), s),
            None => c.ctype().to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
        .to_ascii_lowercase()
}

/// Bump when `summarize` changes, so stored messages are summarised again at startup.
pub const SUMMARY_VERSION: &str = "7";

/// What we keep alongside the raw message for the list view and for search.
pub struct Summary {
    pub snippet: String,
    pub text: String,
    /// JSON list of the attachments, as the UI wants it.
    pub attachments: String,
    /// JSON of what else the message says about itself (see `facts`).
    pub facts: String,
    /// Sent by a system rather than written by a person.
    pub automated: bool,
}

/// What a message's headers alone say about it (person or system, how to unsubscribe, the
/// real sender behind a group), for mail whose body has not been downloaded: the facts as
/// JSON, and whether a system sent it. The headers must include those in `sync::INDEX_ITEMS`.
pub fn header_facts(headers: &[u8]) -> Option<(String, bool)> {
    let msg = MessageParser::default().parse(headers)?;
    let f = crate::facts::facts(&msg, headers);
    Some((serde_json::to_string(&f).ok()?, f.automated))
}

pub fn summarize(raw: &[u8]) -> Summary {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Summary { snippet: String::new(), text: String::new(), attachments: "[]".into(), facts: "{}".into(), automated: true };
    };
    let mut text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    // Plenty of HTML mail ships a throwaway plain-text part ("view this in a browser"),
    // and some put raw markup in it.
    static MARKUP: OnceLock<Regex> = OnceLock::new();
    let looks_like_markup = re(&MARKUP, r"(?i)^\s*<(!doctype|!--|html|head|link|meta|style|body|div|table|p|span)\b")
        .is_match(&text)
        || text.matches("</").count() > 10;
    if text.trim().len() < 80 || looks_like_markup {
        if let Some(html) = msg.html_body.first().and_then(|_| msg.body_html(0)) {
            static CODE: OnceLock<Regex> = OnceLock::new();
            let code = re(&CODE, r"(?is)<script\b.*?</script>|<style\b.*?</style>|<head\b.*?</head>");
            static BLOCK: OnceLock<Regex> = OnceLock::new();
            let block = re(&BLOCK, r"(?i)<(/?(?:p|div|br|tr|td|th|li|h[1-6]|table|blockquote|ul|ol)\b)");
            // a space before block tags keeps adjacent paragraphs from running together
            let spaced = code.replace_all(&html, " ");
            let spaced = block.replace_all(&spaced, " <$1");
            let from_html = mail_parser::decoders::html::html_to_text(&spaced);
            if looks_like_markup || from_html.trim().len() > text.trim().len() {
                text = from_html;
            }
        } else if looks_like_markup {
            text = mail_parser::decoders::html::html_to_text(&text);
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(20_000).collect();
    // Link addresses make a poor preview; the full text (kept for search) still has them.
    static LINK: OnceLock<Regex> = OnceLock::new();
    let readable = re(&LINK, r"[<\[(]?https?://\S+").replace_all(&text, " ");
    let snippet: String = readable.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(220).collect();
    let attachments = serde_json::to_string(&list_attachments(&msg)).unwrap_or_else(|_| "[]".into());
    let facts = crate::facts::facts(&msg, raw);
    Summary { snippet, text, attachments, automated: facts.automated, facts: serde_json::to_string(&facts).unwrap_or_else(|_| "{}".into()) }
}

/// Pictures that the message body actually displays are not what people mean by "attachment".
/// Having a Content-ID is not enough to tell: Gmail gives every attached file one.
fn list_attachments(msg: &mail_parser::Message) -> Vec<Attachment> {
    let html = msg.html_body.first().and_then(|_| msg.body_html(0)).unwrap_or_default();
    let shown_inline = |a: &mail_parser::MessagePart| {
        mime_of(a).starts_with("image/")
            && a.content_id()
                .map(|id| id.trim_matches(|c| c == '<' || c == '>'))
                .is_some_and(|id| !id.is_empty() && html.contains(&format!("cid:{id}")))
    };
    msg.attachments()
        .enumerate()
        .filter(|(_, a)| !shown_inline(a))
        .map(|(index, a)| {
            let name = a.attachment_name().unwrap_or("attachment").to_string();
            let kind = preview_kind(&name, a.contents()).0;
            Attachment { index, name, mime: mime_of(a), size: a.contents().len(), kind }
        })
        .collect()
}

/// (content type to serve, bytes) for an attachment that can be previewed.
pub fn attachment_preview(raw: &[u8], index: usize) -> Option<(&'static str, Vec<u8>)> {
    let (name, _, bytes) = attachment(raw, index)?;
    match preview_kind(&name, &bytes) {
        ("", _) => None,
        (_, mime) => Some((mime, bytes)),
    }
}

/// (file name, mime type, bytes) of the attachment at `index`.
pub fn attachment(raw: &[u8], index: usize) -> Option<(String, String, Vec<u8>)> {
    let msg = MessageParser::default().parse(raw)?;
    let a = msg.attachments().nth(index)?;
    Some((a.attachment_name().unwrap_or("attachment").to_string(), mime_of(a), a.contents().to_vec()))
}

/// The inline part whose Content-ID is `cid`.
pub fn inline_part(raw: &[u8], cid: &str) -> Option<(String, Vec<u8>)> {
    let msg = MessageParser::default().parse(raw)?;
    let want = cid.trim_matches(|c| c == '<' || c == '>');
    msg.parts
        .iter()
        .find(|p| p.content_id().map(|c| c.trim_matches(|c| c == '<' || c == '>')) == Some(want))
        .map(|p| (mime_of(p), p.contents().to_vec()))
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// Drop 0- and 1-pixel images before they can phone home.
fn strip_tracking_pixels(html: &str) -> String {
    static IMG: OnceLock<Regex> = OnceLock::new();
    static TINY: OnceLock<Regex> = OnceLock::new();
    let img = re(&IMG, r"(?is)<img\b[^>]*>");
    let tiny = re(
        &TINY,
        r#"(?i)\b(?:width|height)\s*=\s*["']?[01](?:px)?["'\s>/]|(?:^|[;"'\s])(?:max-)?(?:width|height)\s*:\s*[01](?:px)?\s*(?:;|!|["'])"#,
    );
    img.replace_all(html, |c: &regex::Captures| if tiny.is_match(&c[0]) { String::new() } else { c[0].to_string() })
        .into_owned()
}

/// Email HTML is hostile input. The viewer frame is sandboxed with scripts off and
/// served under a CSP that only allows images and inline styles, so this pass is
/// the second line of defence: it removes anything active while keeping layout.
fn sanitize(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.add_tags(["style", "center", "font", "u", "s", "section", "article", "header", "footer", "main", "nav"]);
    b.rm_clean_content_tags(["style"]);
    // A <title> is not for display; without this its text lands at the top of the message.
    b.clean_content_tags(["script", "title", "noscript", "template"].into());
    b.add_generic_attributes([
        "style", "class", "id", "align", "valign", "bgcolor", "background", "border", "cellpadding", "cellspacing",
        "color", "face", "size", "width", "height", "dir", "lang", "title", "role", "nowrap",
    ]);
    b.add_tag_attributes("img", ["src", "alt", "width", "height", "srcset"]);
    b.add_tag_attributes("a", ["href", "name", "target"]);
    b.add_tag_attributes("td", ["colspan", "rowspan"]);
    b.add_tag_attributes("th", ["colspan", "rowspan"]);
    b.url_schemes(["http", "https", "mailto", "tel", "cid", "data"].into());
    b.link_rel(Some("noopener noreferrer"));
    b.clean(html).to_string()
}

/// The frame is as tall as its content, so the page inside must not size itself to the frame.
/// Marketing mail routinely says `html, body { height: 100% !important }`; the `:root`
/// selectors here outrank that wherever it appears.
const FRAME_CSS: &str = "
html:root,html:root>body{height:auto!important;min-height:0!important;max-height:none!important}
html:root{overflow-x:auto!important;overflow-y:hidden!important}
body{margin:0!important;padding:12px 14px!important;box-sizing:border-box!important;background:#fff;color:#111;
     font:15px/1.5 'Segoe UI','Yu Gothic UI','Meiryo',-apple-system,BlinkMacSystemFont,sans-serif;
     -webkit-text-size-adjust:100%;overflow-wrap:break-word}
img:not([width]):not([height]){max-width:100%;height:auto}
pre{white-space:pre-wrap}
blockquote{margin:0 0 0 .8ex;border-left:2px solid #ccc;padding-left:1ex}
html.dk img:not([data-keep]){filter:invert(1) hue-rotate(180deg)}
";

fn frame(body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base target=\"_blank\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <style>{FRAME_CSS}</style></head><body>{body}</body></html>"
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Which alternative versions of the body a message carries.
#[derive(Serialize)]
pub struct Parts {
    pub html: bool,
    pub text: bool,
}

pub fn parts(raw: &[u8]) -> Parts {
    let Some(msg) = MessageParser::default().parse(raw) else { return Parts { html: false, text: false } };
    Parts {
        html: msg.html_body.iter().any(|&i| matches!(msg.parts.get(i).map(|p| &p.body), Some(PartType::Html(_)))),
        text: msg.text_body.iter().any(|&i| matches!(msg.parts.get(i).map(|p| &p.body), Some(PartType::Text(_)))),
    }
}

/// The plain-text version of a message, as a document for the message frame.
/// Web addresses in it become links.
pub fn render_text(raw: &[u8]) -> String {
    static LINK: OnceLock<Regex> = OnceLock::new();
    let msg = MessageParser::default().parse(raw);
    let text = msg.as_ref().and_then(|m| {
        m.text_body.iter().find_map(|&i| match m.parts.get(i).map(|p| &p.body) {
            Some(PartType::Text(t)) => Some(t.to_string()),
            _ => None,
        })
    });
    let Some(text) = text else { return frame("<p><i>This message has no plain-text version.</i></p>") };
    // Everything is escaped; web addresses additionally become links.
    let text = text.trim();
    let mut linked = String::new();
    let mut last = 0;
    for m in re(&LINK, r#"https?://[^\s<>"]+[^\s<>".,;:!?)\]]"#).find_iter(text) {
        let url = escape(m.as_str());
        linked.push_str(&escape(&text[last..m.start()]));
        linked.push_str(&format!("<a href=\"{url}\">{url}</a>"));
        last = m.end();
    }
    linked.push_str(&escape(&text[last..]));
    frame(&format!("<pre style=\"font:inherit;margin:0\">{linked}</pre>"))
}

/// The message exactly as it arrived (headers, then every part still encoded), for
/// looking under the hood. Long messages are cut off so the frame stays usable.
pub fn render_source(raw: &[u8]) -> String {
    const LIMIT: usize = 400_000;
    let shown = String::from_utf8_lossy(&raw[..raw.len().min(LIMIT)]);
    let note = if raw.len() > LIMIT {
        format!("\n\n[… cut off here: the whole message is {} bytes]", raw.len())
    } else {
        String::new()
    };
    frame(&format!(
        "<pre style=\"font:12.5px/1.45 ui-monospace,Consolas,monospace;margin:0\">{}{}</pre>",
        escape(&shown),
        escape(&note)
    ))
}

/// The complete document shown in the message frame.
pub fn render_html(raw: &[u8], message_id: i64) -> String {
    static CID: OnceLock<Regex> = OnceLock::new();
    // A message written as plain text has no formatted version; show the text, with links.
    let p = parts(raw);
    if p.text && !p.html {
        return render_text(raw);
    }
    let body = MessageParser::default()
        .parse(raw)
        .and_then(|m| m.body_html(0).map(|h| h.into_owned()))
        .unwrap_or_else(|| "<p><i>This message has no readable body.</i></p>".into());
    let body = sanitize(&strip_tracking_pixels(&body));
    let body = re(&CID, r#"(?i)(["'(=\s])cid:([^"'()\s>]+)"#)
        .replace_all(&body, |c: &regex::Captures| format!("{}/api/message/{}/cid/{}", &c[1], message_id, &c[2]));
    frame(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSG: &[u8] = b"From: =?utf-8?q?Matt_Levin=C3=A9?= <matt@example.com>\r\n\
To: Dan <dan@example.com>, x@example.com\r\nSubject: Hello\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=b1\r\n\r\n\
--b1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<p onclick=\"evil()\" style=\"color:red\">Hi <b>there</b></p><script>alert(1)</script>\
<img src=\"https://t.example/p.gif\" width=\"1\" height=\"1\"><img src=\"cid:logo@x\" width=\"200\">\
<a href=\"javascript:alert(1)\">x</a>\r\n\
--b1\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
--b1\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--b1--\r\n";

    #[test]
    fn alternative_views() {
        let msg = b"From: a@b.c\r\nSubject: s\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=b1\r\n\r\n\
--b1\r\nContent-Type: text/plain\r\n\r\nSee <b> https://example.com/a?x=1&y=2.\r\n\
--b1\r\nContent-Type: text/html\r\n\r\n<p>See <b>it</b></p>\r\n--b1--\r\n";
        let p = parts(msg);
        assert!(p.html && p.text);
        let p = parts(MSG);
        assert!(p.html && !p.text, "an HTML-only message has no plain-text version");
        let text = render_text(msg);
        assert!(text.contains("See &lt;b&gt; <a href=\"https://example.com/a?x=1&amp;y=2\">"), "{text}");
        assert!(text.contains("</a>."), "the full stop is not part of the link");
        let source = render_source(msg);
        assert!(source.contains("Content-Type: multipart/alternative") && source.contains("&lt;p&gt;See"));
    }

    #[test]
    fn plain_text_mail_gets_links() {
        let msg = b"From: a@b.c\r\nSubject: s\r\n\r\nCancel here:\r\nhttps://booking.example.jp/info/475?site=e01&key=d9dee\r\n\r\nThanks\r\n";
        let html = render_html(msg, 1);
        assert!(html.contains("<a href=\"https://booking.example.jp/info/475?site=e01&amp;key=d9dee\">"), "{html}");
        assert!(html.contains("Cancel here:"));
    }

    #[test]
    fn frame_rules_outrank_a_messages_own_page_sizing() {
        let msg = b"From: a@b.c\r\nSubject: s\r\nContent-Type: text/html\r\n\r\n\
<style>html, body { height: 100% !important; }</style><p>hi</p>\r\n";
        let html = render_html(msg, 1);
        // the message's rule survives sanitising, so ours has to be the more specific one
        assert!(html.contains("height: 100% !important"));
        assert!(html.contains("html:root,html:root>body{height:auto!important"));
    }

    #[test]
    fn long_words_wrap_without_squeezing_table_columns() {
        // "anywhere" lets a table shrink a column to one character wide (a price came out as
        // a vertical strip beside a long description); "break-word" wraps without doing that
        assert!(FRAME_CSS.contains("overflow-wrap:break-word"));
        assert!(!FRAME_CSS.contains("overflow-wrap:anywhere"));
    }

    #[test]
    fn reply_headers_come_from_the_message() {
        let msg = b"From: Aoife <aoife@example.com>\r\nReply-To: list@example.com\r\nSubject: s\r\n\
Message-ID: <abc@mail.example.com>\r\nReferences: <one@x>\r\n <two@x>\r\n\r\nhi\r\n";
        let r = reply_headers(msg);
        assert_eq!(r.to, "list@example.com");
        assert_eq!(r.message_id, "abc@mail.example.com");
        assert_eq!(r.references, "<one@x> <two@x>");
        assert_eq!(reply_headers(MSG).to, "Matt Leviné <matt@example.com>");
    }

    #[test]
    fn headers_decode() {
        let h = parse_headers(MSG);
        assert_eq!(h.from_name, "Matt Leviné");
        assert_eq!(h.from_addr, "matt@example.com");
        assert_eq!(h.to, "Dan <dan@example.com>, x@example.com");
        assert_eq!(h.subject, "Hello");
    }

    #[test]
    fn html_is_defanged() {
        let html = render_html(MSG, 7);
        assert!(html.contains("<b>there</b>"));
        assert!(html.contains("color:red"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("alert(1)"));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("t.example"), "tracking pixel should be gone");
        assert!(html.contains("/api/message/7/cid/logo@x"));
    }

    #[test]
    fn title_text_is_not_shown_and_markup_in_plain_part_is_not_a_preview() {
        let msg = b"From: a@b.c\r\nSubject: s\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=b1\r\n\r\n\
--b1\r\nContent-Type: text/plain\r\n\r\n\r\n\r\n<link rel=stylesheet href=https://cdn.example/x.css>\r\n<div>ignored</div>\r\n\
--b1\r\nContent-Type: text/html\r\n\r\n\
<html><head><title>Hidden Title</title></head><body><p>The world's most innovative countries</p></body></html>\r\n--b1--\r\n";
        assert!(!render_html(msg, 1).contains("Hidden Title"));
        let s = summarize(msg);
        assert!(s.snippet.starts_with("The world"), "got {:?}", s.snippet);
        let plain = b"From: a@b.c\r\nSubject: s\r\n\r\n<http://email.example.com/c/eJxUkEtu> Showtimes for [https://x.example/y] Friday\r\n";
        assert_eq!(summarize(plain).snippet, "Showtimes for Friday");
    }

    #[test]
    fn attachments_exclude_inline_images() {
        let list: serde_json::Value = serde_json::from_str(&summarize(MSG).attachments).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["name"], "report.pdf");
        assert_eq!(list[0]["mime"], "application/pdf");
        let (name, _, bytes) = attachment(MSG, list[0]["index"].as_u64().unwrap() as usize).unwrap();
        // A photo attached in Gmail has a Content-ID too, but the body never refers to it.
        let photo = b"From: a@b.c\r\nSubject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b1\r\n\r\n\
--b1\r\nContent-Type: text/html\r\n\r\n<div>Photo attached.</div>\r\n\
--b1\r\nContent-Type: image/jpeg; name=\"car.jpg\"\r\nContent-Disposition: attachment; filename=\"car.jpg\"\r\n\
Content-ID: <f_abc123>\r\nX-Attachment-Id: f_abc123\r\nContent-Transfer-Encoding: base64\r\n\r\n/9j/4AAQ\r\n--b1--\r\n";
        assert!(summarize(photo).attachments.contains("car.jpg"), "{}", summarize(photo).attachments);
        assert_eq!(name, "report.pdf");
        assert_eq!(bytes, b"%PDF-");
        assert!(inline_part(MSG, "logo@x").is_some());
        let s = summarize(MSG);
        assert!(s.attachments.contains("report.pdf"));
        // previews: decided by what the bytes are, never by the name alone
        assert_eq!(list[0]["kind"], "pdf");
        assert_eq!(attachment_preview(MSG, list[0]["index"].as_u64().unwrap() as usize).unwrap().0, "application/pdf");
        assert_eq!(preview_kind("photo.png", b"<html><script>x</script>").0, "");
        assert_eq!(preview_kind("page.html", b"<html>").0, "");
        assert_eq!(preview_kind("drawing.svg", b"<svg onload=x>").0, "");
        assert_eq!(preview_kind("notes.txt", b"<script>hi</script>"), ("text", "text/plain; charset=utf-8"));
        assert_eq!(preview_kind("renamed.exe", b"\xff\xd8\xff\xe0").0, "image");
        assert!(s.snippet.contains("Hi there"));
    }
}
