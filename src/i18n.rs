//! The interface language: `tr("English text")` looks the text up in the
//! active catalog (`defaults/locales/<code>.json`, keyed by the English
//! source text, gettext style) and falls back to the English when the catalog
//! has no entry. Placeholders are named: `"{name} needs you"`.
//!
//! settings.json's `language` is `"auto"` (the system's UI language), `"en"`
//! or a catalog code (`"zh-CN"`). Anything else shows English.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

const ZH_CN: &str = include_str!("../defaults/locales/zh-CN.json");

const EN: u8 = 0;
const ZH: u8 = 1;
static ACTIVE: AtomicU8 = AtomicU8::new(EN);

fn catalog(code: u8) -> Option<&'static HashMap<String, String>> {
    static ZH_CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    match code {
        ZH => Some(ZH_CATALOG.get_or_init(|| serde_json::from_str(ZH_CN).unwrap_or_default())),
        _ => None,
    }
}

/// The catalog a settings.json `language` value selects; `auto` asks the system.
fn code_of(setting: &str) -> u8 {
    match setting.trim() {
        "auto" | "" => from_locale(&system_locale()),
        other => from_locale(other),
    }
}

/// Simplified Chinese for any `zh` locale but the traditional ones, else English.
/// Takes catalog codes (`zh-CN`) and system locale names (`zh_CN.UTF-8`, `zh-Hans-CN`).
fn from_locale(locale: &str) -> u8 {
    let l = locale.split(['.', '@']).next().unwrap_or("").to_ascii_lowercase().replace('_', "-");
    if l == "zh" || l.starts_with("zh-") {
        let traditional = l.split('-').skip(1).any(|p| matches!(p, "tw" | "hk" | "mo" | "hant"));
        if !traditional {
            return ZH;
        }
    }
    EN
}

/// Selects the language for `tr`; called whenever settings change.
pub fn set_language(setting: &str) {
    ACTIVE.store(code_of(setting), Ordering::Relaxed);
}

/// The language showing now, as a settings.json `language` value.
#[allow(dead_code)]
pub fn current() -> &'static str {
    if ACTIVE.load(Ordering::Relaxed) == ZH { "zh-CN" } else { "en" }
}

/// The system's UI language as a locale name (`zh-CN`, `en_US.UTF-8`, ...), or "".
fn system_locale() -> String {
    // Tests run in English whatever the machine speaks.
    if cfg!(test) {
        return String::new();
    }
    static LOCALE: OnceLock<String> = OnceLock::new();
    LOCALE.get_or_init(detect_locale).clone()
}

#[cfg(windows)]
fn detect_locale() -> String {
    // Windows' own UI language (not the region). Its LANGID is 0x0804 for
    // Simplified Chinese (PRC), 0x1004 for Singapore; 0x0404, 0x0c04, 0x1404 are traditional.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    let id = unsafe { GetUserDefaultUILanguage() };
    match id {
        0x0804 | 0x1004 => "zh-CN".into(),
        id if id & 0x3ff == 0x04 => "zh-TW".into(),
        _ => "en".into(),
    }
}

#[cfg(not(windows))]
fn detect_locale() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return v;
        }
    }
    // A Finder-launched app has no LANG: ask the system's language list.
    #[cfg(target_os = "macos")]
    if let Some(out) = crate::spawn::command("defaults").args(["read", "-g", "AppleLanguages"]).output().ok().filter(|o| o.status.success()) {
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if let Some(first) = text.split('"').nth(1) {
            return first.to_owned();
        }
    }
    String::new()
}

/// `text` in the active language (the English itself when there's no entry).
pub fn tr(text: &str) -> String {
    match catalog(ACTIVE.load(Ordering::Relaxed)).and_then(|c| c.get(text)) {
        Some(t) => t.clone(),
        None => text.to_owned(),
    }
}

/// Marks text that is translated where it is shown, not where it is written (the key itself).
pub const fn tr_key(text: &'static str) -> &'static str {
    text
}

/// `tr`, with each `{name}` replaced by its value from `args`.
pub fn tr_fmt(text: &str, args: &[(&str, &str)]) -> String {
    fill(&tr(text), args.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())))
}

/// Replaces `{name}` with its value; other braces stay as they are.
pub fn fill(text: &str, args: impl IntoIterator<Item = (String, String)>) -> String {
    let args: HashMap<String, String> = args.into_iter().collect();
    if args.is_empty() || !text.contains('{') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}').and_then(|end| args.get(&after[..end]).map(|v| (end, v))) {
            Some((end, v)) => {
                out.push_str(v);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    #[test]
    fn locales_name_the_catalogs() {
        assert_eq!(from_locale("zh_CN.UTF-8"), ZH);
        assert_eq!(from_locale("zh-Hans-CN"), ZH);
        assert_eq!(from_locale("zh"), ZH);
        assert_eq!(from_locale("zh_TW.UTF-8"), EN);
        assert_eq!(from_locale("zh-Hant"), EN);
        assert_eq!(from_locale("en_US.UTF-8"), EN);
        assert_eq!(from_locale("C"), EN);
        assert_eq!(from_locale("fr"), EN);
        assert_eq!(code_of("zh-CN"), ZH);
        assert_eq!(code_of("en"), EN);
        assert_eq!(code_of("klingon"), EN);
    }

    #[test]
    fn placeholders_are_named() {
        let args = [("name".to_owned(), "api".to_owned())];
        assert_eq!(fill("{name} needs you", args.clone()), "api needs you");
        assert_eq!(fill("{other} {name}", args.clone()), "{other} api");
        assert_eq!(fill("a { b", args), "a { b");
        assert_eq!(fill("{x}", []), "{x}");
    }

    #[test]
    fn missing_entries_fall_back_to_english() {
        assert_eq!(tr("No such sentence in any catalog"), "No such sentence in any catalog");
    }

    // ---- the catalog covers every string the code translates ----------------

    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                // The easter-egg mod is English only.
                if p.file_name().is_some_and(|n| n == "win98") {
                    continue;
                }
                walk(&p, ext, out);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }

    fn unescape(raw: &str) -> String {
        let mut out = String::new();
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                }
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    /// Every literal handed to `tr(...)` / `tr_fmt(...)` in the Rust and JS sources.
    fn used() -> BTreeSet<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let call = Regex::new(r#"(?s)\btr(?:_fmt|_key)?\(\s*(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|`((?:[^`\\$]|\\.)*)`)"#).unwrap();
        let mut found = BTreeSet::new();
        let mut files = Vec::new();
        walk(&root.join("src"), "rs", &mut files);
        walk(&root.join("defaults"), "js", &mut files);
        for f in files {
            // This file's own examples are not strings to translate.
            if f.ends_with("i18n.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for c in call.captures_iter(&text) {
                let raw = c.get(1).or(c.get(2)).or(c.get(3)).unwrap().as_str();
                found.insert(unescape(raw));
            }
        }
        // Texts the code reaches through tables rather than a literal call.
        found.extend(crate::commands::COMMANDS.iter().map(|c| c.about.to_owned()));
        // The key listing words its actions as the help mod does: `pane.go:left` is "pane go" and "left".
        for keys in [include_str!("../defaults/keys.json"), include_str!("../defaults/keys.macos.json")] {
            let sections: std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>> = serde_json::from_str(keys).unwrap();
            for action in sections.values().flat_map(|s| s.values()).filter_map(serde_json::Value::as_str) {
                if action.starts_with("send:") || action == "terminal" {
                    continue;
                }
                let (name, arg) = action.split_once(':').unwrap_or((action, ""));
                found.insert(name.replace(['.', '_'], " "));
                if !arg.is_empty() && arg.parse::<u32>().is_err() {
                    found.insert(arg.to_owned());
                }
            }
        }
        found
    }

    fn catalog_of(file: &str) -> HashMap<String, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/locales").join(file);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn zh_cn_translates_every_string_and_only_those() {
        let used = used();
        let catalog = catalog_of("zh-CN.json");
        let missing: Vec<&String> = used.iter().filter(|s| !catalog.contains_key(*s)).collect();
        let unused: Vec<&String> = catalog.keys().filter(|k| !used.contains(*k)).collect();
        assert!(missing.is_empty(), "defaults/locales/zh-CN.json has no entry for: {missing:#?}");
        assert!(unused.is_empty(), "defaults/locales/zh-CN.json has entries nothing uses: {unused:#?}");
    }

    #[test]
    fn zh_cn_keeps_the_placeholders() {
        let names = |s: &str| -> BTreeSet<String> { Regex::new(r"\{(\w+)\}").unwrap().captures_iter(s).map(|c| c[1].to_owned()).collect() };
        for (en, zh) in catalog_of("zh-CN.json") {
            assert_eq!(names(&en), names(&zh), "placeholders differ: {en:?} -> {zh:?}");
            assert!(!zh.trim().is_empty(), "empty translation for {en:?}");
        }
    }
}
