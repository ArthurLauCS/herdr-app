//! LOCAL PATCH (branch local/patches, never sent upstream): the custom build
//! made by C:\WorkFile\CodeBase\herdr-custom. Its build folder holds
//! `herdr-custom-build.json` beside herdr-app.exe; without that file this does nothing.
//!
//! - Runs the pipeline's `sync.mjs` hidden, 2 minutes after start and then
//!   hourly (the script does real work once a day), and shows the notice it
//!   leaves as a toast.
//! - Keeps checking for herdr-core updates, which a build from source
//!   otherwise skips (update.rs `checks_core`).

use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

const FIRST: Duration = Duration::from_secs(2 * 60);
const EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    node: PathBuf,
    sync_script: PathBuf,
    notice_file: PathBuf,
}

fn marker() -> Option<&'static Marker> {
    static MARKER: OnceLock<Option<Marker>> = OnceLock::new();
    MARKER
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let text = std::fs::read_to_string(exe.parent()?.join("herdr-custom-build.json")).ok()?;
            serde_json::from_str(&text).map_err(|e| tracing::warn!("herdr-custom-build.json: {e}")).ok()
        })
        .as_ref()
}

/// Whether this is the custom build.
pub fn active() -> bool {
    marker().is_some()
}

/// Starts the sync clock; call once the window can show toasts.
pub fn start() {
    let Some(m) = marker() else { return };
    let spawned = std::thread::Builder::new().name("custom-sync".into()).spawn(move || {
        std::thread::sleep(FIRST);
        loop {
            run_sync(m);
            show_notice(m);
            std::thread::sleep(EVERY);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the custom sync clock: {e}");
    }
}

fn run_sync(m: &Marker) {
    let mut cmd = std::process::Command::new(&m.node);
    cmd.arg(&m.sync_script)
        .current_dir(m.sync_script.parent().unwrap_or(std::path::Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.status() {
        Ok(status) if !status.success() => tracing::warn!("custom sync exited with {status}"),
        Ok(_) => {}
        Err(e) => tracing::warn!("cannot run the custom sync ({}): {e}", m.node.display()),
    }
}

/// The pipeline's last notice (`{ title, body, level }`), shown once.
fn show_notice(m: &Marker) {
    let Ok(text) = std::fs::read_to_string(&m.notice_file) else { return };
    let _ = std::fs::remove_file(&m.notice_file);
    let Ok(notice) = serde_json::from_str::<Value>(&text) else { return };
    let field = |k: &str| notice.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let level = if field("level") == "error" { "error" } else { "info" };
    crate::mods::request(crate::mods::Request::OpenOverlay {
        id: crate::mods::new_overlay_id(),
        spec: serde_json::json!({ "kind": "toast", "notification": { "title": field("title"), "body": field("body"), "level": level } }),
    });
}
