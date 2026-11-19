//! Terminal text highlighting, ported from Orca: recolors plain text that
//! matches rules (errors, paths, numbers, ...) and marks Claude Code
//! transcripts (tool calls, the prompt echo, inline code, quote bars, diffs).
//! The program's output is untouched; only the cells of the frame that is
//! drawn change.
//!
//! `Vt::frame` hands every frame's rows through `Highlighter::decorate`. A
//! row whose cells and marks are the same as last frame comes back as the
//! same `Rc`, so the painter keeps its drawing; regex work is cached by the
//! text of a logical line.

mod claude;
mod rules;

use crate::vt::{Cell, Rgb, Theme};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher;
use std::rc::Rc;

pub use rules::preset_ids;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hue {
    Red,
    #[default]
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
}

/// How Claude Code tool calls are marked: the name tinted, whole rows tinted, or not at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeBlocks {
    #[default]
    Tags,
    Rows,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Tags,
    Rows,
}

/// A text rule of a person's own, which runs before the presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomRule {
    pub id: String,
    /// A regex (fancy-regex syntax) matched against logical lines.
    pub pattern: String,
    pub hue: Hue,
    pub case_sensitive: bool,
    /// Half-way between the hue and the text color; dim text is skipped.
    pub subtle: bool,
    pub enabled: bool,
}

impl Default for CustomRule {
    fn default() -> Self {
        CustomRule { id: String::new(), pattern: String::new(), hue: Hue::default(), case_sensitive: false, subtle: false, enabled: true }
    }
}

/// settings.json's `terminal_highlights`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalHighlights {
    pub enabled: bool,
    /// Preset id -> on or off; presets not named are on.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub presets: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<CustomRule>,
    pub claude_blocks: ClaudeBlocks,
}

impl Default for TerminalHighlights {
    fn default() -> Self {
        TerminalHighlights { enabled: true, presets: BTreeMap::new(), rules: Vec::new(), claude_blocks: ClaudeBlocks::Tags }
    }
}

impl TerminalHighlights {
    /// Problems an agent should hear about before the file is applied.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for id in self.presets.keys().filter(|id| !preset_ids().any(|p| p == id.as_str())) {
            out.push(format!("terminal_highlights.presets.{id}: not a preset ({})", preset_ids().collect::<Vec<_>>().join(", ")));
        }
        for rule in &self.rules {
            if let Err(e) = rules::valid(&rule.pattern) {
                out.push(format!("terminal_highlights.rules {:?}: {e}", rule.id));
            }
        }
        out
    }
}

/// The six hues and the surfaces they are mixed with, from the terminal theme.
#[derive(Clone, Debug, PartialEq)]
struct Palette {
    /// In `Hue` order.
    hues: [Rgb; 6],
    bg: Rgb,
    fg: Rgb,
    dark: bool,
}

fn mix(c: Rgb, bg: Rgb, a: f32) -> Rgb {
    let ch = |c: u8, b: u8| (a * f32::from(c) + (1.0 - a) * f32::from(b)).round() as u8;
    Rgb(ch(c.0, bg.0), ch(c.1, bg.1), ch(c.2, bg.2))
}

impl Palette {
    fn new(theme: &Theme) -> Palette {
        let l = |c: Rgb| 0.2126 * f32::from(c.0) / 255.0 + 0.7152 * f32::from(c.1) / 255.0 + 0.0722 * f32::from(c.2) / 255.0;
        let dark = l(theme.bg) < 0.5;
        // The theme's bright hues on dark backgrounds, the normal ones on light.
        let base = if dark { 8 } else { 0 };
        let p = &theme.palette16;
        // red, yellow, green, cyan, blue, magenta
        let hues = [p[base + 1], p[base + 3], p[base + 2], p[base + 6], p[base + 4], p[base + 5]];
        Palette { hues, bg: theme.bg, fg: theme.fg, dark }
    }

    fn hue(&self, hue: Hue) -> Rgb {
        self.hues[hue as usize]
    }

    /// Backgrounds stay a step below the selection: a hue tint behind text of the same hue, or a
    /// neutral surface, so marked text never reads as selected.
    fn colors(&self, paint: claude::Paint, hue: Hue) -> (Option<Rgb>, Option<Rgb>) {
        use claude::Paint::*;
        let (h, d) = (self.hue(hue), self.dark);
        let by = |dark: f32, light: f32| Some(mix(h, self.bg, if d { dark } else { light }));
        match paint {
            Header => (None, by(0.22, 0.16)),
            Body => (None, by(0.08, 0.06)),
            Tag => (Some(h), by(0.16, 0.12)),
            Band | Code => (None, Some(mix(self.fg, self.bg, if d { 0.08 } else { 0.06 }))),
            QuoteBar => (Some(h), by(0.4, 0.3)),
            Diff => (None, by(0.16, 0.14)),
            Name | Connector | Prompt => (Some(h), None),
        }
    }
}

/// The fg and bg a run of cells is drawn in instead of the program's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mark {
    x: u16,
    width: u16,
    fg: Option<Rgb>,
    bg: Option<Rgb>,
}

/// A row above the viewport that the marks of the rows below depend on.
pub struct Above {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
    /// Every cell is read: the row can be part of a logical line the rules match.
    /// Otherwise only its start is, which is all the Claude blocks need.
    pub full: bool,
}

struct Drawn {
    raw: Rc<Vec<Cell>>,
    marks: Vec<Mark>,
    out: Rc<Vec<Cell>>,
}

const MAX_WRAPPED_ROWS: usize = 8;
const MAX_CACHED_LINES: usize = 4096;

/// A quick hash for the text of a line: it runs over every cell of every changed row.
struct Fx(u64);

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub struct Highlighter {
    rules: Vec<rules::Rule>,
    claude: Option<Style>,
    palette: Palette,
    /// Hits by the hash of a logical line's text and which of it is plain.
    cache: HashMap<u64, Rc<Vec<rules::Hit>>>,
    /// The logical lines of last frame's viewport, by their first row, with the rows they were
    /// matched from: rows kept by the terminal need no hashing to be found again.
    lines: HashMap<*const Vec<Cell>, (Vec<Rc<Vec<Cell>>>, Rc<Vec<rules::Hit>>)>,
    /// Each viewport row as drawn last frame.
    drawn: Vec<Option<Drawn>>,
}

impl Highlighter {
    /// `None` when there is nothing to do.
    pub fn new(cfg: &TerminalHighlights, theme: &Theme) -> Option<Highlighter> {
        if !cfg.enabled {
            return None;
        }
        let rules = rules::compile(cfg);
        let claude = match cfg.claude_blocks {
            ClaudeBlocks::Tags => Some(Style::Tags),
            ClaudeBlocks::Rows => Some(Style::Rows),
            ClaudeBlocks::Off => None,
        };
        if rules.is_empty() && claude.is_none() {
            return None;
        }
        Some(Highlighter { rules, claude, palette: Palette::new(theme), cache: HashMap::new(), lines: HashMap::new(), drawn: Vec::new() })
    }

    /// Whether the Claude blocks look at rows above the viewport.
    pub fn wants_blocks(&self) -> bool {
        self.claude.is_some()
    }

    fn line_hits(&mut self, lines: &[&[Cell]]) -> Rc<Vec<rules::Hit>> {
        let fg = self.palette.fg;
        let plain = move |c: &Cell| claude::is_plain(c, fg);
        let mut h = Fx(0);
        for line in lines {
            h.write_usize(line.len());
            for c in line.iter() {
                h.write(c.text.as_bytes());
                h.write_u8(u8::from(plain(c)) | u8::from(c.faint) << 1 | u8::from(c.wide) << 2 | 0x80);
            }
        }
        let key = h.finish();
        if let Some(hits) = self.cache.get(&key) {
            return hits.clone();
        }
        let hits = Rc::new(rules::find_hits(lines.iter().copied(), &plain, &self.rules));
        if self.cache.len() >= MAX_CACHED_LINES {
            self.cache.clear();
        }
        self.cache.insert(key, hits.clone());
        hits
    }

    /// The viewport `rows` (`wraps`: whether each is the wrapped continuation of the row above) with
    /// the marks applied. Rows that need none, and rows marked as last frame, are the same `Rc`.
    pub fn decorate(&mut self, above: &[Above], rows: &[Rc<Vec<Cell>>], wraps: &[bool]) -> Vec<Rc<Vec<Cell>>> {
        let n = above.len();
        let views: Vec<claude::Row> = above
            .iter()
            .map(|a| claude::Row { cells: &a.cells, wrapped: a.wrapped })
            .chain(rows.iter().zip(wraps).map(|(r, w)| claude::Row { cells: r, wrapped: *w }))
            .collect();
        let mut marks: Vec<Vec<Mark>> = vec![Vec::new(); rows.len()];

        if !self.rules.is_empty() {
            let mut last_frame = std::mem::take(&mut self.lines);
            let mut i = 0;
            while i < views.len() {
                let mut j = i + 1;
                while j < views.len() && views[j].wrapped && j - i < MAX_WRAPPED_ROWS {
                    j += 1;
                }
                let readable = (i..j).all(|k| k >= n || above[k].full);
                if j > n && readable {
                    let hits = if i >= n {
                        let kept = &rows[i - n..j - n];
                        let key = Rc::as_ptr(&kept[0]);
                        let same = last_frame.remove(&key).filter(|(prev, _)| prev.len() == kept.len() && prev.iter().zip(kept).all(|(a, b)| Rc::ptr_eq(a, b)));
                        let (_, hits) = same.unwrap_or_else(|| {
                            let lines: Vec<&[Cell]> = kept.iter().map(|r| r.as_slice()).collect();
                            (Vec::new(), self.line_hits(&lines))
                        });
                        self.lines.insert(key, (kept.to_vec(), hits.clone()));
                        hits
                    } else {
                        let lines: Vec<&[Cell]> = views[i..j].iter().map(|r| r.cells).collect();
                        self.line_hits(&lines)
                    };
                    for hit in hits.iter().filter(|h| i + h.row >= n) {
                        let hue = self.palette.hue(hit.hue);
                        let fg = if hit.subtle { mix(hue, self.palette.fg, 0.5) } else { hue };
                        marks[i + hit.row - n].push(Mark { x: hit.x as u16, width: hit.width as u16, fg: Some(fg), bg: None });
                    }
                }
                i = j;
            }
        }
        if let Some(style) = self.claude {
            for r in claude::find_blocks(&views, style, n, self.palette.fg) {
                let (fg, bg) = self.palette.colors(r.paint, r.hue);
                marks[r.row - n].push(Mark { x: r.x as u16, width: r.width as u16, fg, bg });
            }
        }

        self.drawn.resize_with(rows.len(), || None);
        self.drawn.truncate(rows.len());
        let mut out = Vec::with_capacity(rows.len());
        for (y, (raw, marks)) in rows.iter().zip(marks).enumerate() {
            let reuse = self.drawn[y].as_ref().filter(|d| Rc::ptr_eq(&d.raw, raw) && d.marks == marks).map(|d| d.out.clone());
            let row = if marks.is_empty() {
                self.drawn[y] = None;
                raw.clone()
            } else if let Some(same) = reuse {
                same
            } else {
                let out = Rc::new(apply(raw, &marks));
                self.drawn[y] = Some(Drawn { raw: raw.clone(), marks, out: out.clone() });
                out
            };
            out.push(row);
        }
        out
    }
}

/// `raw` drawn with `marks`; the terminal's own selection stays as it is.
fn apply(raw: &[Cell], marks: &[Mark]) -> Vec<Cell> {
    let mut cells = raw.to_vec();
    for m in marks {
        let end = (m.x as usize + m.width as usize).min(cells.len());
        for cell in cells.iter_mut().take(end).skip(m.x as usize).filter(|c| !c.selected) {
            if let Some(fg) = m.fg {
                cell.fg = fg;
            }
            if let Some(bg) = m.bg {
                cell.bg = Some(bg);
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests;
