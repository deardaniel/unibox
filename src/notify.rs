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

#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
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

/// Show a toast; clicking it brings the main window to the front.
#[cfg(windows)]
pub fn show(title: &str, body: &str) -> anyhow::Result<()> {
    use std::sync::Mutex;
    use windows::core::{IInspectable, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    // The last toast is held on to, so its click still reaches us while it is on screen.
    static LAST: Mutex<Option<ToastNotification>> = Mutex::new(None);
    register()?;
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(title, body)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(|_, _| {
        crate::window::focus();
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)?;
    *LAST.lock().unwrap() = Some(toast);
    Ok(())
}

#[cfg(not(windows))]
pub fn show(_title: &str, _body: &str) -> anyhow::Result<()> {
    anyhow::bail!("notifications need Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_text_is_escaped() {
        assert_eq!(
            toast_xml("A & B", "<b>\"hi\"</b>"),
            "<toast><visual><binding template=\"ToastGeneric\"><text>A &amp; B</text>\
             <text>&lt;b&gt;&quot;hi&quot;&lt;/b&gt;</text></binding></visual></toast>"
        );
    }
}
