//! Getting the person's attention from outside the window. When an agent needs
//! them or finishes while Herdr isn't the active window, the in-window toast
//! can't be seen: the taskbar button flashes until the window comes forward,
//! and a system notification says who and where; clicking it brings the
//! window forward on that agent. `settings.json` `notifications` turns either off.
//!
//! Windows only for now (other platforms accept the calls and do nothing; the
//! Dock badge covers macOS).

/// One alert, as the window words it.
pub struct Alert<'a> {
    pub title: &'a str,
    pub body: Option<&'a str>,
    /// The terminal to focus when the notification is clicked.
    pub terminal: &'a str,
}

/// Called for every alert Herdr raises; acts only when the window isn't active.
pub fn alert(window: &gpui::Window, window_active: bool, alert: Alert) {
    if window_active {
        return;
    }
    let n = crate::settings::get().notifications.clone();
    if n.taskbar {
        platform::flash(window);
    }
    if n.system {
        platform::notify(alert);
    }
}

/// The window came forward: its notifications are old news.
pub fn window_activated() {
    platform::clear();
}

/// Toast XML text: notifications show these characters literally.
#[cfg_attr(not(windows), allow(dead_code))]
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no place for other control characters.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// The notification's XML: title, place, no sound (Herdr plays its own).
#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(alert: &Alert) -> String {
    let body = alert.body.filter(|b| !b.is_empty()).map(|b| format!("<text>{}</text>", escape(b))).unwrap_or_default();
    format!(
        "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text>{body}</binding></visual><audio silent=\"true\"/></toast>",
        escape(alert.terminal),
        escape(alert.title),
    )
}

/// Windows limits a notification tag to 64 characters; one per terminal, so a
/// newer alert replaces that terminal's older one.
#[cfg_attr(not(windows), allow(dead_code))]
fn tag(terminal: &str) -> String {
    if terminal.chars().count() <= 64 {
        return terminal.to_owned();
    }
    use sha2::Digest;
    let digest = sha2::Sha256::digest(terminal.as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
mod platform {
    use super::Alert;
    use std::sync::OnceLock;
    use std::sync::mpsc::{Sender, channel};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
    use windows::core::{HSTRING, IInspectable, Interface};

    /// The identity notifications show under (name and icon), registered for
    /// the current user: an unpackaged app has none of its own.
    const APP_ID: &str = "Herdr.App";
    const GROUP: &str = "herdr";
    const ICON: &[u8] = include_bytes!("../assets/icons/herdr-ui-icon-clean.png");

    enum Job {
        Show { xml: String, tag: String },
        Clear,
    }

    pub fn flash(window: &gpui::Window) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
        let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: h.hwnd.get() as HWND,
            // The taskbar button only, until the window comes to the foreground.
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        unsafe {
            FlashWindowEx(&info);
        }
    }

    pub fn notify(alert: Alert) {
        send(Job::Show { xml: super::toast_xml(&alert), tag: super::tag(alert.terminal) });
    }

    pub fn clear() {
        // Nothing shown yet: don't start the thread just to clear.
        if let Some(tx) = WORKER.get() {
            let _ = tx.lock().unwrap().send(Job::Clear);
        }
    }

    static WORKER: OnceLock<std::sync::Mutex<Sender<Job>>> = OnceLock::new();

    /// WinRT calls run on one thread of their own, off the UI thread.
    fn send(job: Job) {
        let tx = WORKER.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let spawned = std::thread::Builder::new().name("notifications".into()).spawn(move || {
                unsafe {
                    use windows_sys::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
                    CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
                }
                register();
                // Shown notifications stay referenced so their click handlers live.
                let mut shown: Vec<(String, ToastNotification)> = Vec::new();
                for job in rx {
                    let result = match job {
                        Job::Show { xml, tag } => show(&xml, &tag).map(|toast| {
                            shown.retain(|(t, _)| *t != tag);
                            shown.push((tag, toast));
                        }),
                        Job::Clear => {
                            shown.clear();
                            ToastNotificationManager::History().and_then(|h| h.RemoveGroupWithId(&HSTRING::from(GROUP), &HSTRING::from(APP_ID)))
                        }
                    };
                    if let Err(e) = result {
                        tracing::warn!("system notification: {e}");
                    }
                }
            });
            if let Err(e) = spawned {
                tracing::warn!("cannot start the notification thread: {e}");
            }
            std::sync::Mutex::new(tx)
        });
        let _ = tx.lock().unwrap().send(job);
    }

    fn show(xml: &str, tag: &str) -> windows::core::Result<ToastNotification> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from(GROUP))?;
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(|_, args| {
            let terminal = args
                .as_ref()
                .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
                .and_then(|a| a.Arguments().ok())
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            go_to(terminal);
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
        Ok(toast)
    }

    /// The click: the window comes forward, on that agent.
    fn go_to(terminal: String) {
        let (reply, _) = channel();
        crate::mods::request(crate::mods::Request::Control { cmd: serde_json::json!({ "raise": true }), reply });
        if !terminal.is_empty() {
            crate::mods::request(crate::mods::Request::Run { name: "agent.focus".into(), args: serde_json::json!({ "terminal": terminal }) });
        }
    }

    /// `HKCU\Software\Classes\AppUserModelId\Herdr.App`: the name and icon
    /// Windows shows on our notifications. Written on each start (the icon
    /// file is in the temp folder, which may have been cleaned).
    fn register() {
        let icon = std::env::temp_dir().join("herdr-app-notification.png");
        if std::fs::read(&icon).ok().as_deref() != Some(ICON)
            && let Err(e) = std::fs::write(&icon, ICON)
        {
            tracing::warn!("cannot write {}: {e}", icon.display());
        }
        let key = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
        for (name, value) in [("DisplayName", "Herdr".to_owned()), ("IconUri", icon.display().to_string())] {
            if let Err(e) = set_string(&key, name, &value) {
                tracing::warn!("cannot register notifications ({name}): error {e}");
            }
        }
    }

    fn set_string(key: &str, name: &str, value: &str) -> Result<(), u32> {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (key, name, value) = (wide(key), wide(name), wide(value));
        let code = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
        };
        if code == 0 { Ok(()) } else { Err(code) }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn flash(_window: &gpui::Window) {}
    pub fn notify(_alert: super::Alert) {}
    pub fn clear() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_text_is_escaped_and_keeps_its_place_line() {
        let xml = toast_xml(&Alert { title: "Claude <code> & co needs you", body: Some("app / \"main\""), terminal: "vps/t7" });
        assert_eq!(
            xml,
            "<toast launch=\"vps/t7\"><visual><binding template=\"ToastGeneric\"><text>Claude &lt;code&gt; &amp; co needs you</text><text>app / &quot;main&quot;</text></binding></visual><audio silent=\"true\"/></toast>"
        );
    }

    #[test]
    fn a_toast_without_a_place_has_one_line() {
        let xml = toast_xml(&Alert { title: "Codex finished", body: None, terminal: "t1" });
        assert!(xml.contains("<text>Codex finished</text></binding>"), "{xml}");
        assert_eq!(toast_xml(&Alert { title: "x", body: Some(""), terminal: "t1" }).matches("<text>").count(), 1);
    }

    #[test]
    fn control_characters_never_reach_the_xml() {
        assert_eq!(escape("a\u{1b}[31mb\tc"), "a[31mb\tc");
    }

    #[test]
    fn tags_fit_windows_limit_and_stay_one_per_terminal() {
        assert_eq!(tag("t12"), "t12");
        let long = "machine-with-a-very-long-name.example.internal/terminal-0123456789";
        assert!(long.len() > 64);
        assert_eq!(tag(long).len(), 32);
        assert_eq!(tag(long), tag(long));
        assert_ne!(tag(long), tag(&format!("{long}x")));
    }
}
