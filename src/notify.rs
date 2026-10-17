//! Windows notifications (toasts). Windows shows a toast only for an app it knows: a program
//! without an installer package is known by an AppUserModelID registered under
//! HKCU\Software\Classes\AppUserModelId, which gives the toast its name and icon.

/// The name Windows knows Unibox by. Only toasts use it: the process itself keeps the ID
/// Windows makes up for it, so the window still groups with the pinned Start Menu shortcut.
#[cfg(windows)]
const AUMID: &str = "Daniel.Unibox";

/// Text for the toast's XML, with the five characters XML gives a meaning escaped.
#[cfg_attr(not(windows), allow(dead_code))]
fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `open` comes back as the toast's arguments when it is clicked.
#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(title: &str, body: &str, open: &str) -> String {
    format!(
        "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_text(open),
        xml_text(title),
        xml_text(body)
    )
}

/// Tell Windows who `AUMID` is: "Unibox", with the icon the installer put beside the app.
#[cfg(windows)]
fn register() -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
    let set = |name: &str, value: &str| {
        let wide: Vec<u16> = value.encode_utf16().chain([0]).collect();
        unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, &key, &HSTRING::from(name), REG_SZ.0, Some(wide.as_ptr().cast()), (wide.len() * 2) as u32)
        }
        .ok()
    };
    set("DisplayName", "Unibox")?;
    if let Some(ico) = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("unibox.ico"))).filter(|p| p.exists()) {
        set("IconUri", &ico.to_string_lossy())?;
    }
    Ok(())
}

/// Above this many new messages at once, one toast says how many instead of one each.
const MAX_EACH: usize = 3;

/// A message that just arrived in the inbox, for its toast.
pub struct Arrival {
    pub account_id: i64,
    pub thrid: String,
    pub from: String,
    pub subject: String,
}

/// One toast to show: title, text, the conversation ("account:thread") clicking it opens (empty
/// for none), and its tag. Windows shows one toast at a time and queues the rest; a toast with
/// the tag of one already there replaces it instead of waiting behind it.
#[derive(Debug, PartialEq)]
pub struct Toast {
    pub title: String,
    pub body: String,
    pub open: String,
    pub tag: String,
}

/// The toasts for mail that just arrived in `account`'s inbox, in the page's language `lang`:
/// one per message (sender, subject), or one counting them when there are more than `MAX_EACH`.
pub fn toasts(new: &[Arrival], account: &str, lang: &str) -> Vec<Toast> {
    let ja = lang.starts_with("ja");
    if new.len() > MAX_EACH {
        let title = if ja { format!("{} 件の新着メッセージ", new.len()) } else { format!("{} new messages", new.len()) };
        return vec![Toast { title, body: account.to_string(), open: String::new(), tag: format!("count:{account}") }];
    }
    new.iter()
        .map(|a| Toast {
            title: a.from.clone(),
            body: if ja && a.subject == "(no subject)" { "(件名なし)".into() } else { a.subject.clone() },
            open: format!("{}:{}", a.account_id, a.thrid),
            // one toast per conversation: a second message in it takes the first one's place
            tag: format!("{}:{}", a.account_id, a.thrid),
        })
        .collect()
}

/// Whether one of Unibox's windows is the one in front, so new mail is in view already.
pub fn app_in_front() -> bool {
    #[cfg(windows)]
    return crate::window::in_front();
    #[cfg(not(windows))]
    false
}

/// Show a toast. Clicking it brings the main window to the front and calls `clicked` with the
/// toast's `open`.
#[cfg(windows)]
pub fn show(toast: &Toast, clicked: impl Fn(&str) + Send + 'static) -> anyhow::Result<()> {
    use std::sync::Mutex;
    use windows::core::{Interface, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
    // The last few toasts are held on to, so a click still reaches us while they are on screen.
    static RECENT: Mutex<Vec<ToastNotification>> = Mutex::new(Vec::new());
    register()?;
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(&toast.title, &toast.body, &toast.open)))?;
    let shown = ToastNotification::CreateToastNotification(&doc)?;
    if !toast.tag.is_empty() {
        // at most 64 characters
        shown.SetTag(&HSTRING::from(toast.tag.chars().take(64).collect::<String>()))?;
    }
    shown.Activated(&TypedEventHandler::new(move |_, args: windows::core::Ref<windows::core::IInspectable>| {
        crate::window::focus();
        let open = args.ok()?.cast::<ToastActivatedEventArgs>()?.Arguments()?;
        clicked(&open.to_string());
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&shown)?;
    let mut recent = RECENT.lock().unwrap();
    recent.push(shown);
    let extra = recent.len().saturating_sub(10);
    recent.drain(..extra);
    Ok(())
}

#[cfg(not(windows))]
pub fn show(_toast: &Toast, _clicked: impl Fn(&str) + Send + 'static) -> anyhow::Result<()> {
    anyhow::bail!("notifications need Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_text_is_escaped() {
        assert_eq!(
            toast_xml("A & B", "<b>\"hi\"</b>", "1:2"),
            "<toast launch=\"1:2\"><visual><binding template=\"ToastGeneric\"><text>A &amp; B</text>\
             <text>&lt;b&gt;&quot;hi&quot;&lt;/b&gt;</text></binding></visual></toast>"
        );
    }

    fn arrival(n: usize) -> Arrival {
        Arrival { account_id: 2, thrid: format!("{n}"), from: format!("Sender {n}"), subject: "(no subject)".into() }
    }

    #[test]
    fn a_few_messages_get_a_toast_each_and_more_get_one_counting_them() {
        let three: Vec<_> = (1..=3).map(arrival).collect();
        let each = toasts(&three, "me@example.com", "en-US");
        assert_eq!(each.len(), 3);
        assert_eq!(each[0], Toast { title: "Sender 1".into(), body: "(no subject)".into(), open: "2:1".into(), tag: "2:1".into() });
        assert_eq!(toasts(&three, "me@example.com", "ja-JP")[0].body, "(件名なし)");
        let four: Vec<_> = (1..=4).map(arrival).collect();
        assert_eq!(
            toasts(&four, "me@example.com", "en-GB"),
            vec![Toast {
                title: "4 new messages".into(),
                body: "me@example.com".into(),
                open: String::new(),
                tag: "count:me@example.com".into()
            }]
        );
        assert!(toasts(&[], "me@example.com", "en-US").is_empty());
    }
}
