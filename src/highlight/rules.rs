//! Text rules: the presets and a person's own, matched against one logical
//! line (a row plus the rows the terminal wrapped onto it).
//!
//! Orca's rules are JavaScript regexes, where `\b`, `\w` and `\d` are ASCII.
//! Rust's are Unicode and fancy-regex can't switch that off or look behind a
//! variable length, so the presets spell the ASCII classes out and avoid
//! variable lookbehind: `lists` captures its marker after `^\s*`, and
//! `NOT_ZERO` (no "0 " or "no " right before) is a check on the text before
//! the match.

use super::{Hue, TerminalHighlights};
use crate::vt::Cell;

/// One preset: `group` is the capture that holds the colored text; `guard`
/// a capture that `NOT_ZERO` applies to.
struct Preset {
    id: &'static str,
    hue: Hue,
    subtle: bool,
    ignore_case: bool,
    pattern: &'static str,
    group: usize,
    guard: Option<usize>,
}

/// An ASCII word character, for `\b` and `\w`.
macro_rules! w {
    () => {
        "A-Za-z0-9_"
    };
}

const PRESETS: [Preset; 7] = [
    Preset {
        id: "errors",
        hue: Hue::Red,
        subtle: false,
        ignore_case: true,
        pattern: concat!(
            "((?<![", w!(), "])(?:error|errors|failed|failing|failure|fatal|exception|panic|denied|refused)(?![", w!(), "]))|[\u{2717}\u{2718}]"
        ),
        group: 0,
        guard: Some(1),
    },
    Preset {
        id: "warnings",
        hue: Hue::Yellow,
        subtle: false,
        ignore_case: true,
        pattern: concat!("((?<![", w!(), "])(?:warn|warning|warnings|deprecated)(?![", w!(), "]))|\u{26a0}"),
        group: 0,
        guard: Some(1),
    },
    Preset {
        id: "success",
        hue: Hue::Green,
        subtle: false,
        ignore_case: true,
        pattern: concat!("(?<![", w!(), "])(?:passed|passing|success|successful|succeeded)(?![", w!(), "])|[\u{2713}\u{2714}]"),
        group: 0,
        guard: None,
    },
    Preset {
        id: "paths",
        hue: Hue::Cyan,
        subtle: false,
        ignore_case: false,
        pattern: concat!(
            // A URL.
            "(?<![", w!(), "])https?://[", w!(), "\\-.~:/?#@!$&*+,;=%]*[", w!(), "\\-~/#=&%]",
            // A path with an extension, optionally :line:col.
            "|(?:[A-Za-z]:[\\\\/]|~[\\\\/]|/)?(?:[", w!(), ".@-]+[\\\\/])+[", w!(), ".@-]+\\.[A-Za-z0-9]{1,8}(?::[0-9]+(?::[0-9]+)?)?(?![", w!(), "]|\\.[", w!(), "])",
            // file.ext:line after a word boundary (a leading . or - needs a word character before it).
            "|(?:(?<=[", w!(), "])(?=[.\\-])|(?<![", w!(), "])(?=[", w!(), "]))[", w!(), ".\\-]+\\.[A-Za-z][A-Za-z0-9]{0,7}:[0-9]+(?::[0-9]+)?",
        ),
        group: 0,
        guard: None,
    },
    Preset {
        id: "references",
        hue: Hue::Magenta,
        subtle: false,
        ignore_case: false,
        pattern: concat!(
            "(?<![", w!(), "#&])#[0-9]+(?![", w!(), "])",
            "|(?<![", w!(), "])(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}(?![", w!(), "])",
        ),
        group: 0,
        guard: None,
    },
    Preset {
        id: "lists",
        hue: Hue::Blue,
        subtle: false,
        ignore_case: true,
        pattern: "^\\s*(-|\\*|\u{2022}|[0-9]+[.)])\\s",
        group: 1,
        guard: None,
    },
    Preset {
        id: "metrics",
        hue: Hue::Yellow,
        subtle: true,
        ignore_case: true,
        pattern: concat!(
            "(?<![", w!(), ".])[0-9]+(?:,[0-9]{3})*(?:\\.[0-9]+)*",
            "(?:\\s?(?:ms|s|sec|min|m|h|KB|MB|GB|KiB|MiB|GiB)(?![", w!(), "])|[kKM](?![", w!(), "])|%)?",
            "(?![", w!(), "]|\\.[", w!(), "])",
        ),
        group: 0,
        guard: None,
    },
];

/// The preset ids, in priority order.
pub fn preset_ids() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|p| p.id)
}

pub struct Rule {
    regex: fancy_regex::Regex,
    pub hue: Hue,
    pub subtle: bool,
    group: usize,
    guard: Option<usize>,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `NOT_ZERO`: the text before ends in the whole word "0 " or "no ".
fn follows_zero(before: &str) -> bool {
    let b = before.as_bytes();
    let token = if b.ends_with(b"0 ") {
        2
    } else if b.len() >= 3 && b[b.len() - 3..].eq_ignore_ascii_case(b"no ") {
        3
    } else {
        return false;
    };
    !before[..before.len() - token].chars().next_back().is_some_and(is_word)
}

impl Rule {
    fn new(pattern: &str, ignore_case: bool, hue: Hue, subtle: bool, group: usize, guard: Option<usize>) -> Option<Rule> {
        let pattern = if ignore_case { format!("(?i){pattern}") } else { pattern.to_owned() };
        let regex = fancy_regex::Regex::new(&pattern).ok()?;
        Some(Rule { regex, hue, subtle, group, guard })
    }

    /// The byte ranges of `text` this rule colors, in order. A pattern that
    /// backtracks too long errors out: it finds nothing more.
    pub fn find(&self, text: &str, out: &mut Vec<(usize, usize)>) {
        if self.group == 0 && self.guard.is_none() {
            for m in self.regex.find_iter(text).map_while(Result::ok) {
                if !m.as_str().is_empty() {
                    out.push((m.start(), m.end()));
                }
            }
            return;
        }
        for caps in self.regex.captures_iter(text).map_while(Result::ok) {
            let Some(m) = caps.get(self.group) else { continue };
            if m.as_str().is_empty() {
                continue;
            }
            if self.guard.is_some_and(|g| caps.get(g).is_some_and(|w| follows_zero(&text[..w.start()]))) {
                continue;
            }
            out.push((m.start(), m.end()));
        }
    }
}

/// Custom rules first, then the presets that are on; a pattern that doesn't compile is skipped.
pub fn compile(cfg: &TerminalHighlights) -> Vec<Rule> {
    let mut rules = Vec::new();
    for rule in cfg.rules.iter().filter(|r| r.enabled) {
        rules.extend(Rule::new(&rule.pattern, !rule.case_sensitive, rule.hue, rule.subtle, 0, None));
    }
    for p in &PRESETS {
        if cfg.presets.get(p.id).copied().unwrap_or(true) {
            rules.extend(Rule::new(p.pattern, p.ignore_case, p.hue, p.subtle, p.group, p.guard));
        }
    }
    rules
}

/// Whether `pattern` compiles (for settings problems).
pub fn valid(pattern: &str) -> Result<(), String> {
    fancy_regex::Regex::new(pattern).map(|_| ()).map_err(|e| e.to_string())
}

struct LineCell {
    row: usize,
    x: usize,
    width: usize,
    plain: bool,
    dim: bool,
}

/// What a rule colors: `width` cells at `x` of row `row` (an index in the logical line).
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub row: usize,
    pub x: usize,
    pub width: usize,
    pub hue: Hue,
    pub subtle: bool,
}

/// Bounds regex work on very long logical lines (minified output, base64).
const MAX_LINE_CHARS: usize = 4096;

/// Matches `rules` against the cells of one logical line; only cells for
/// which `plain` holds are colored, one rule per cell.
pub fn find_hits<'a>(rows: impl Iterator<Item = &'a [Cell]>, plain: &dyn Fn(&Cell) -> bool, rules: &[Rule]) -> Vec<Hit> {
    let mut text = String::new();
    let mut cells: Vec<LineCell> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut chars = 0usize;
    for (row, line) in rows.enumerate() {
        let mut tail = false;
        for (x, cell) in line.iter().enumerate() {
            // The tail half of a wide character draws nothing of its own.
            if std::mem::replace(&mut tail, cell.wide) {
                continue;
            }
            if chars >= MAX_LINE_CHARS {
                break;
            }
            starts.push(text.len());
            let s = if cell.text.is_empty() { " " } else { cell.text.as_str() };
            text.push_str(s);
            chars += s.chars().count();
            cells.push(LineCell { row, x, width: if cell.wide { 2 } else { 1 }, plain: plain(cell), dim: cell.faint });
        }
    }
    let mut taken = vec![false; cells.len()];
    let mut hits: Vec<Hit> = Vec::new();
    let mut found = Vec::new();
    for rule in rules {
        found.clear();
        rule.find(&text, &mut found);
        for &(start, end) in &found {
            // The cells whose text the match touches.
            let first = starts.partition_point(|&s| s <= start).saturating_sub(1);
            let last = starts.partition_point(|&s| s < end);
            if first >= last || (first..last).any(|i| !cells[i].plain || (rule.subtle && cells[i].dim) || taken[i]) {
                continue;
            }
            for i in first..last {
                taken[i] = true;
                let c = &cells[i];
                match hits.last_mut() {
                    Some(h) if h.row == c.row && h.hue == rule.hue && h.subtle == rule.subtle && h.x + h.width == c.x => h.width += c.width,
                    _ => hits.push(Hit { row: c.row, x: c.x, width: c.width, hue: rule.hue, subtle: rule.subtle }),
                }
            }
        }
    }
    hits
}
