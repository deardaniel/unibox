//! Turning raw RFC 822 messages into things the UI can show safely.

use mail_parser::{Address, MessageParser, MimeHeaders};
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

#[derive(Serialize)]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub mime: String,
    pub size: usize,
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

/// What we keep alongside the raw message for the list view and for search.
pub struct Summary {
    pub snippet: String,
    pub text: String,
    pub has_attach: bool,
}

pub fn summarize(raw: &[u8]) -> Summary {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Summary { snippet: String::new(), text: String::new(), has_attach: false };
    };
    let mut text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    // Plenty of HTML mail ships a throwaway plain-text part ("view this in a browser").
    if text.trim().len() < 80 {
        if let Some(html) = msg.html_body.first().and_then(|_| msg.body_html(0)) {
            static CODE: OnceLock<Regex> = OnceLock::new();
            let code = re(&CODE, r"(?is)<script\b.*?</script>|<style\b.*?</style>|<head\b.*?</head>");
            static BLOCK: OnceLock<Regex> = OnceLock::new();
            let block = re(&BLOCK, r"(?i)<(/?(?:p|div|br|tr|td|th|li|h[1-6]|table|blockquote|ul|ol)\b)");
            // a space before block tags keeps adjacent paragraphs from running together
            let spaced = code.replace_all(&html, " ");
            let spaced = block.replace_all(&spaced, " <$1");
            let from_html = mail_parser::decoders::html::html_to_text(&spaced);
            if from_html.trim().len() > text.trim().len() {
                text = from_html;
            }
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(20_000).collect();
    let snippet: String = text.chars().take(220).collect();
    // inline images carry a Content-ID and are not what people mean by "attachment"
    let has_attach = msg.attachments().any(|a| a.content_id().is_none() || !mime_of(a).starts_with("image/"));
    Summary { snippet, text, has_attach }
}

pub fn attachments(raw: &[u8]) -> Vec<Attachment> {
    let Some(msg) = MessageParser::default().parse(raw) else { return Vec::new() };
    msg.attachments()
        .enumerate()
        .filter(|(_, a)| a.content_id().is_none() || !mime_of(a).starts_with("image/"))
        .map(|(index, a)| Attachment {
            index,
            name: a.attachment_name().unwrap_or("attachment").to_string(),
            mime: mime_of(a),
            size: a.contents().len(),
        })
        .collect()
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

const FRAME_CSS: &str = "
html,body{height:auto!important;min-height:0!important;overflow:hidden!important}
body{margin:0!important;padding:12px 16px!important;background:#fff;color:#111;
     font:15px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;
     -webkit-text-size-adjust:100%;overflow-wrap:anywhere}
img{max-width:100%!important;height:auto!important}
table{max-width:100%!important}
pre{white-space:pre-wrap}
blockquote{margin:0 0 0 .8ex;border-left:2px solid #ccc;padding-left:1ex}
html.dk img:not([data-keep]){filter:invert(1) hue-rotate(180deg)}
";

/// The complete document shown in the message frame.
pub fn render_html(raw: &[u8], message_id: i64) -> String {
    static CID: OnceLock<Regex> = OnceLock::new();
    let body = MessageParser::default()
        .parse(raw)
        .and_then(|m| m.body_html(0).map(|h| h.into_owned()))
        .unwrap_or_else(|| "<p><i>This message has no readable body.</i></p>".into());
    let body = sanitize(&strip_tracking_pixels(&body));
    let body = re(&CID, r#"(?i)(["'(=\s])cid:([^"'()\s>]+)"#)
        .replace_all(&body, |c: &regex::Captures| format!("{}/api/message/{}/cid/{}", &c[1], message_id, &c[2]));
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base target=\"_blank\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <style>{FRAME_CSS}</style></head><body>{body}</body></html>"
    )
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
    fn attachments_exclude_inline_images() {
        let list = attachments(MSG);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "report.pdf");
        assert_eq!(list[0].mime, "application/pdf");
        let (name, _, bytes) = attachment(MSG, list[0].index).unwrap();
        assert_eq!(name, "report.pdf");
        assert_eq!(bytes, b"%PDF-");
        assert!(inline_part(MSG, "logo@x").is_some());
        let s = summarize(MSG);
        assert!(s.has_attach);
        assert!(s.snippet.contains("Hi there"));
    }
}
