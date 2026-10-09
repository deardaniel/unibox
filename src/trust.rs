//! How far a message's sender can be believed.
//!
//! Gmail checks every incoming message (SPF, DKIM, DMARC) and records the verdict in an
//! `Authentication-Results` header that it puts above everything the sender supplied. This
//! reads that verdict back; it does no checking of its own. "Verified" only ever means the
//! message really came from the domain in its From line, not that the domain is a good one.

use mail_parser::MessageParser;
use serde::Serialize;

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Dkim {
    pub result: String,
    pub domain: String,
}

#[derive(Serialize, Default, Debug)]
pub struct Trust {
    /// "verified", "failed", "unverified", or "none" when Gmail recorded no verdict
    /// (mail you sent yourself, drafts).
    pub level: &'static str,
    /// The domain in the From line.
    pub domain: String,
    /// One sentence saying what the verdict means.
    pub summary: String,
    pub spf: String,
    /// The domain SPF was checked for (the envelope sender), which need not be the From domain.
    pub spf_domain: String,
    pub dkim: Vec<Dkim>,
    pub dmarc: String,
    /// What the sender's domain asks receivers to do with mail that fails: "none",
    /// "quarantine" or "reject" (empty when it publishes no policy).
    pub dmarc_policy: String,
    /// The message reached this account by forwarding, and was verified at the first stop.
    pub forwarded: bool,
    /// Where replies go, when that is a different organisation from the sender.
    pub reply_to: String,
    /// An address shown inside the sender's display name that is not the real sender.
    pub name_address: String,
    /// Gmail's header as it stands.
    pub raw: String,
}

/// The first `Authentication-Results` header in a message head, unfolded. Being first is
/// what makes it Gmail's: a sender can put a forged one in their message, but only below.
fn top_auth_results(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let mut found: Option<String> = None;
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break; // end of the headers
        }
        let continues = line.starts_with(' ') || line.starts_with('\t');
        match &mut found {
            Some(value) if continues => {
                value.push(' ');
                value.push_str(line.trim());
            }
            Some(_) => break,
            None if !continues => {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("Authentication-Results") {
                        found = Some(value.trim().to_string());
                    }
                }
            }
            None => {}
        }
    }
    found
}

fn without_comments(s: &str) -> String {
    let mut depth = 0;
    s.chars()
        .filter(|c| {
            match c {
                '(' => depth += 1,
                ')' if depth > 0 => {
                    depth -= 1;
                    return false;
                }
                _ => {}
            }
            depth == 0
        })
        .collect()
}

fn domain_of(addr: &str) -> String {
    addr.rsplit('@').next().unwrap_or("").trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_lowercase()
}

/// The registrable part of a domain, near enough: `mail.example.co.jp` -> `example.co.jp`.
/// (No public-suffix list; two-letter country domains with a generic second level keep three labels.)
pub fn org(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').filter(|l| !l.is_empty()).collect();
    let n = labels.len();
    let generic = ["co", "com", "ne", "or", "ac", "go", "net", "org", "gov", "edu", "ad", "ed", "gr", "lg"];
    let keep = if n >= 3 && labels[n - 1].len() == 2 && generic.contains(&labels[n - 2]) { 3 } else { 2 };
    labels[n.saturating_sub(keep)..].join(".").to_lowercase()
}

/// Google Workspace signs for a customer who has not set up their own key with a domain of
/// its own that spells the customer's with dashes: `daniel.ie` -> `daniel-ie.20230601.gappssmtp.com`.
/// (Mail from a Google Group on that domain is signed this way.) Google only does so for a
/// domain whose ownership it verified, so it vouches for the sender like the domain's own
/// signature would. Only a two-label name with no dash of its own is accepted: with more
/// dashes the spelling is ambiguous (`my-bank-com` is both `my.bank.com` and `my-bank.com`).
fn workspace_signs_for(signer: &str, domain: &str) -> bool {
    let labels: Vec<&str> = signer.split('.').collect();
    let [name, date, "gappssmtp", "com"] = labels[..] else { return false };
    date.chars().all(|c| c.is_ascii_digit())
        && domain.matches('.').count() == 1
        && !domain.contains('-')
        && name == domain.replace('.', "-")
}

fn same_org(a: &str, b: &str) -> bool {
    !a.is_empty() && org(a) == org(b)
}

/// Read the verdict out of the start of a raw message (its headers are enough).
pub fn assess(head: &[u8]) -> Trust {
    let msg = MessageParser::default().parse(head);
    let msg = msg.as_ref();
    let from = msg.and_then(|m| m.from()).and_then(|a| a.first());
    let from_addr = from.and_then(|f| f.address()).unwrap_or_default().to_lowercase();
    let from_name = from.and_then(|f| f.name()).unwrap_or_default();
    let domain = domain_of(&from_addr);
    let mut t = Trust { level: "none", domain: domain.clone(), ..Default::default() };

    if let Some(reply) = msg.and_then(|m| m.reply_to()).and_then(|a| a.first()).and_then(|a| a.address()) {
        if !same_org(&domain_of(reply), &domain) {
            t.reply_to = reply.to_lowercase();
        }
    }
    // "PayPal <service@paypal.com>" as the name of some other sender
    t.name_address = from_name
        .split(|c: char| c.is_whitespace() || "<>()[]\"',;".contains(c))
        .find(|w| w.contains('@') && domain_of(w).contains('.') && !same_org(&domain_of(w), &domain))
        .unwrap_or_default()
        .to_lowercase();

    let Some(raw) = top_auth_results(head) else { return t };
    let clean = without_comments(&raw);
    let mut parts = clean.split(';');
    // only Gmail's own verdict counts
    if !parts.next().is_some_and(|id| id.trim().to_lowercase().ends_with("google.com")) {
        return t;
    }
    t.raw = raw.clone();
    for part in parts {
        let mut words = part.split_whitespace();
        let Some((method, result)) = words.next().and_then(|w| w.split_once('=')) else { continue };
        let prop = |name: &str| {
            part.split_whitespace().find_map(|w| w.strip_prefix(name)).map(|v| v.to_string()).unwrap_or_default()
        };
        let result = result.to_lowercase();
        match method.to_lowercase().as_str() {
            "spf" => {
                t.spf = result;
                t.spf_domain = domain_of(&prop("smtp.mailfrom="));
            }
            "dkim" => {
                let d = prop("header.d=");
                let d = if d.is_empty() { prop("header.i=") } else { d };
                t.dkim.push(Dkim { result, domain: domain_of(&d) });
            }
            "dmarc" => t.dmarc = result,
            _ => {}
        }
    }
    // Details Gmail keeps in comments. The DMARC policy: "dmarc=fail (p=NONE sp=NONE ...)".
    let lower = raw.to_lowercase();
    let after = |text: &str, key: &str| -> Option<String> {
        let rest = &text[text.find(key)? + key.len()..];
        Some(rest.chars().take_while(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c)).collect())
    };
    if let Some(at) = lower.find("dmarc=") {
        let comment = lower[at..].split(')').next().unwrap_or("");
        t.dmarc_policy = after(comment, "(p=").unwrap_or_default();
    }
    // Forwarded mail fails the checks at the last hop by its nature. Gmail then goes by the
    // sealed record (ARC) of what the first server found:
    //   "arc=pass (i=2 spf=pass ... dmarc=pass fromdomain=example.com)"
    if let Some(at) = lower.find("arc=pass (") {
        let record = lower[at..].split(')').next().unwrap_or("");
        t.forwarded = record.contains("dmarc=pass") && after(record, "fromdomain=").is_some_and(|d| same_org(&d, &domain));
    }

    let dkim_ok = t
        .dkim
        .iter()
        .any(|d| d.result == "pass" && (same_org(&d.domain, &domain) || workspace_signs_for(&d.domain, &domain)));
    let spf_ok = t.spf == "pass" && same_org(&t.spf_domain, &domain);
    let other_signer = t.dkim.iter().find(|d| d.result == "pass").map(|d| d.domain.clone());
    let shown = if domain.is_empty() { "the sender".to_string() } else { domain.clone() };
    // A domain that publishes "p=none" asks for failing mail to be delivered as usual, and
    // plenty of genuine senders with that setting fail every time (their mailing service
    // signs instead). That is worth a question mark, not an alarm.
    let lenient = t.dmarc == "fail" && t.dmarc_policy == "none";
    (t.level, t.summary) = if t.dmarc == "pass" || (t.dmarc != "fail" && (dkim_ok || spf_ok)) {
        ("verified", format!("Gmail confirmed this really came from {shown}."))
    } else if t.forwarded {
        ("verified", format!("This really came from {shown}. It was checked before being forwarded to this account."))
    } else if (t.dmarc == "fail" && !lenient) || (matches!(t.spf.as_str(), "fail" | "softfail") && other_signer.is_none()) {
        ("failed", format!("Gmail could not confirm this came from {shown}. It may be forged."))
    } else {
        let via = match other_signer.or_else(|| (t.spf == "pass").then(|| t.spf_domain.clone())) {
            Some(d) if !d.is_empty() => format!(" It was sent through {d}."),
            _ => String::new(),
        };
        ("unverified", format!("Nothing proves this came from {shown}.{via}"))
    };
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(auth: &str, rest: &str) -> Vec<u8> {
        format!("Delivered-To: me@example.com\r\n{auth}{rest}\r\nSubject: s\r\n\r\nbody\r\n").into_bytes()
    }

    #[test]
    fn reads_gmails_verdict() {
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n       dkim=pass header.i=@substack.com header.s=mkt header.b=AbC;\r\n       \
             spf=pass (google.com: domain of bounce@mg1.substack.com designates 1.2.3.4 as permitted sender) \
             smtp.mailfrom=\"bounce+x=y@mg1.substack.com\";\r\n       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=substack.com\r\n",
            "From: Works in Progress <wip@substack.com>\r\nReply-To: reply+1@mg1.substack.com",
        ));
        assert_eq!(t.level, "verified");
        assert_eq!((t.spf.as_str(), t.spf_domain.as_str(), t.dmarc.as_str()), ("pass", "mg1.substack.com", "pass"));
        assert_eq!(t.dkim, vec![Dkim { result: "pass".into(), domain: "substack.com".into() }]);
        assert_eq!(t.reply_to, "", "same organisation, so not worth mentioning");
        assert!(t.summary.contains("substack.com"));
    }

    #[test]
    fn failures_and_mismatches() {
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n spf=softfail (google.com: transitioning) smtp.mailfrom=x@evil.example;\r\n \
             dmarc=fail (p=REJECT sp=REJECT dis=NONE) header.from=paypal.com\r\n",
            "From: \"service@paypal.com\" <service@paypal.com>\r\nReply-To: Help <collect@evil.example>",
        ));
        assert_eq!(t.level, "failed");
        assert_eq!(t.reply_to, "collect@evil.example");

        // signed, but by the mailing service rather than the domain in From, and no DMARC
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@sendgrid.net header.s=s1;\r\n \
             spf=pass smtp.mailfrom=bounces@sendgrid.net;\r\n dmarc=none header.from=smallshop.example\r\n",
            "From: Small Shop <hello@smallshop.example>",
        ));
        assert_eq!(t.level, "unverified");
        assert!(t.summary.contains("sendgrid.net"), "{}", t.summary);

        // fails DMARC, but the domain's policy is "none" and its mailing service signed: common
        // for genuine mail, so a question mark rather than an alarm
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@sendgrid.net header.s=smtpapi header.b=x;\r\n \
             spf=pass (google.com: domain of b@sendgrid.net designates 1.2.3.4 as permitted sender) smtp.mailfrom=\"b@sendgrid.net\";\r\n \
             dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=shop.example\r\n",
            "From: Shop <info@shop.example>",
        ));
        assert_eq!((t.level, t.dmarc_policy.as_str()), ("unverified", "none"));
        // ...unless nothing vouches for it at all
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n spf=softfail smtp.mailfrom=x@evil.example;\r\n dmarc=fail (p=NONE) header.from=shop.example\r\n",
            "From: Shop <info@shop.example>",
        ));
        assert_eq!(t.level, "failed");

        // forwarded from another account: fails here, but was verified where it first arrived
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@me.example header.s=google header.b=x;\r\n \
             arc=pass (i=2 spf=pass spfdomain=bma.mpse.jp dkim=pass dkdomain=mail.school.co.jp dmarc=pass fromdomain=mail.school.co.jp);\r\n \
             spf=pass (google.com: domain of me@me.example designates 1.2.3.4 as permitted sender) smtp.mailfrom=me@me.example;\r\n \
             dmarc=fail (p=NONE sp=NONE dis=NONE arc=pass) header.from=mail.school.co.jp;\r\n dara=neutral header.i=@me.example\r\n",
            "From: School <news@mail.school.co.jp>",
        ));
        assert_eq!((t.level, t.forwarded), ("verified", true));
        assert!(t.summary.contains("forwarded"));

        // no DMARC record, but the signature is the sender's own
        let t = assess(&msg(
            "Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@mail.smallshop.co.jp;\r\n spf=none smtp.mailfrom=x@other.example\r\n",
            "From: hello@smallshop.co.jp",
        ));
        assert_eq!(t.level, "verified");

        // a real address in the name, a different one doing the sending
        let t = assess(&msg("", "From: \"PayPal <service@paypal.com>\" <x@evil.example>"));
        assert_eq!(t.name_address, "service@paypal.com");
        assert_eq!(t.level, "none");
    }

    #[test]
    fn only_the_top_header_from_gmail_counts() {
        // the sender's own claim sits below Gmail's and is ignored
        let t = assess(&msg(
            "Authentication-Results: mx.google.com; dmarc=fail header.from=bank.example\r\n\
             Authentication-Results: mx.google.com; dmarc=pass header.from=bank.example\r\n",
            "From: Bank <a@bank.example>",
        ));
        assert_eq!(t.level, "failed");
        // a header from some other server says nothing we trust
        let t = assess(&msg("Authentication-Results: mail.evil.example; dmarc=pass\r\n", "From: a@bank.example"));
        assert_eq!(t.level, "none");
        // none at all: mail you sent
        assert_eq!(assess(&msg("", "From: me@example.com")).level, "none");
    }

    #[test]
    fn google_workspace_default_signature() {
        // a Google Group on the sender's own Workspace domain, which publishes no DMARC record
        let group = |from: &str| {
            assess(&msg(
                "Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@daniel-ie.20230601.gappssmtp.com header.s=20230601 header.b=x;\r\n \
                 arc=pass (i=2 spf=pass spfdomain=example.com dkim=pass dkdomain=example.com dmarc=pass fromdomain=example.com);\r\n \
                 spf=neutral (google.com: 1.2.3.4 is neither permitted nor denied) smtp.mailfrom=kids+bnc@daniel.ie\r\n",
                from,
            ))
        };
        assert_eq!(group("From: \"'Teacher' via Kids\" <kids@daniel.ie>").level, "verified");
        // the same signature says nothing about any other domain
        assert_eq!(group("From: x@daniel.com").level, "unverified");
        assert_eq!(group("From: x@daniel-ie.com").level, "unverified");
        assert!(workspace_signs_for("daniel-ie.20230601.gappssmtp.com", "daniel.ie"));
        // ambiguous spellings are not accepted
        assert!(!workspace_signs_for("my-bank-com.20230601.gappssmtp.com", "my.bank.com"));
        assert!(!workspace_signs_for("my-bank-com.20230601.gappssmtp.com", "my-bank.com"));
        assert!(!workspace_signs_for("daniel-ie.evil.gappssmtp.com", "daniel.ie"));
        assert!(!workspace_signs_for("daniel-ie.20230601.gappssmtp.com.evil.example", "daniel.ie"));
    }

    #[test]
    fn organisations() {
        assert_eq!(org("mg1.substack.com"), "substack.com");
        assert_eq!(org("mail.example.co.jp"), "example.co.jp");
        assert_eq!(org("genelife.jp"), "genelife.jp");
        assert_eq!(org("a.b.daniel.ie"), "daniel.ie");
    }
}
