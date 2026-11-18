//! Claude Code transcript marks: tool-call blocks, the prompt echo, inline
//! code in replies, quote bars and diff lines, found from the cells alone.

use super::{Hue, Style};
use crate::vt::{Cell, RawColor, Rgb};

/// What Claude Code's bullet in column 0 marks: its prose, a tool call, or a failed tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bullet {
    Reply,
    Tool,
    Failed,
}

/// `Tag`: tinted tool name; `Name`: colored name; `Connector`: colored connector; `Prompt`: colored prompt
/// arrow; `Band` and `Code`: neutral surface under the prompt echo and inline code; `Header`/`Body`: row
/// tints; `QuoteBar`: solid bar under a quote's bar; `Diff`: tint under a diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Header,
    Body,
    Tag,
    Name,
    Connector,
    Prompt,
    Band,
    Code,
    QuoteBar,
    Diff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub row: usize,
    pub x: usize,
    pub width: usize,
    pub hue: Hue,
    pub paint: Paint,
}

pub struct Row<'a> {
    pub cells: &'a [Cell],
    /// The terminal wrapped the row above onto this one.
    pub wrapped: bool,
}

// Tool names, plus the verbs of collapsed summaries ("Reading 3 files", "Ran 2 shell commands").
const TOOLS: [(Hue, &[&str]); 5] = [
    (Hue::Cyan, &["Read", "Reading", "Search", "Searched", "Searching", "Grep", "Glob", "LS", "Listed"]),
    (Hue::Yellow, &["Update", "Updated", "Write", "Wrote", "Edit", "Edited", "MultiEdit", "NotebookEdit"]),
    (Hue::Magenta, &["Bash", "PowerShell", "BashOutput", "Monitor", "Ran", "Running"]),
    (Hue::Blue, &["Agent", "Task", "Skill"]),
    (Hue::Green, &["Web Search", "WebSearch", "Fetch", "Fetched", "WebFetch"]),
];

fn tool_hue(name: &str) -> Option<Hue> {
    TOOLS.iter().find(|(_, names)| names.contains(&name)).map(|(hue, _)| *hue)
}

// Unknown bold names are MCP and plugin tools (delegated work); unknown plain words describe a running shell command.
const OTHER_TOOL_HUE: Hue = Hue::Blue;
const DESCRIBED_COMMAND_HUE: Hue = Hue::Magenta;
// "● " precedes the tool name; "  ⎿" puts the result connector in the same column.
const NAME_COLUMN: usize = 2;
const MAX_NAME_CELLS: usize = 40;

/// Claude Code's ANSI themes draw inline code in bright blue and the prompt echo on bright black.
const CODE_PALETTE: u8 = 12;
const ECHO_BG_PALETTE: u8 = 8;

/// Text the program left in a neutral color (default, black, white, bright black, bright white, the
/// theme's foreground or a gray): the only text the rules recolor.
pub fn is_neutral_fg(cell: &Cell, theme_fg: Rgb) -> bool {
    if cell.src.inverse {
        return false;
    }
    match cell.src.fg {
        RawColor::Default => true,
        RawColor::Palette(i) => matches!(i, 0 | 7 | 8 | 15),
        // Truecolor themes draw body text in the theme's foreground or a gray RGB; other RGB is emphasis.
        RawColor::Rgb(c) => c == theme_fg || c.0.max(c.1).max(c.2) - c.0.min(c.1).min(c.2) <= 24,
    }
}

pub fn own_bg(cell: &Cell) -> bool {
    cell.src.bg != RawColor::Default
}

pub fn echo_bg(cell: &Cell) -> bool {
    cell.src.bg == RawColor::Palette(ECHO_BG_PALETTE)
}

/// Text on a background the program painted (Claude Code's mouse selection, diff lines) keeps its colors.
pub fn is_plain(cell: &Cell, theme_fg: Rgb) -> bool {
    is_neutral_fg(cell, theme_fg) && (!own_bg(cell) || echo_bg(cell))
}

pub fn bullet_of(cell: &Cell, theme_fg: Rgb) -> Bullet {
    // Claude Code dims the bullet of a running call and of collapsed summaries.
    if cell.faint {
        return Bullet::Tool;
    }
    if is_neutral_fg(cell, theme_fg) {
        return Bullet::Reply;
    }
    match cell.src.fg {
        RawColor::Palette(i) => if matches!(i, 1 | 9) { Bullet::Failed } else { Bullet::Tool },
        // Truecolor themes: Claude Code's error red is rgb(255,107,128).
        RawColor::Rgb(c) => {
            if f32::from(c.0) > 1.5 * f32::from(c.1.max(c.2)) { Bullet::Failed } else { Bullet::Tool }
        }
        RawColor::Default => Bullet::Reply,
    }
}

fn chars(cell: &Cell) -> &str {
    if cell.text.is_empty() { " " } else { &cell.text }
}

/// The trailing half of a wide character has no text but is not a space.
fn is_space(cells: &[Cell], x: usize) -> bool {
    let tail = x > 0 && cells.get(x - 1).is_some_and(|c| c.wide);
    cells.get(x).is_none_or(|c| !tail && chars(c) == " ")
}

fn is_quote_bar(cell: &Cell) -> bool {
    cell.text == "\u{258e}" && cell.faint && !own_bg(cell)
}

/// Drawn in palette red or green, as diff lines are.
fn diff_hue(cell: &Cell) -> Option<Hue> {
    if own_bg(cell) {
        return None;
    }
    match cell.src.fg {
        RawColor::Palette(1 | 9) => Some(Hue::Red),
        RawColor::Palette(2 | 10) => Some(Hue::Green),
        _ => None,
    }
}

/// The bold tool name ("Web Search"), else the first word of a summary line.
fn tool_name(cells: &[Cell]) -> (String, usize) {
    let end = cells.len().min(NAME_COLUMN + MAX_NAME_CELLS);
    let name = cells.get(NAME_COLUMN..end).unwrap_or_default();
    let mut width = name.iter().position(|c| !c.bold || c.text == "(");
    if width.is_none_or(|w| w == 0) {
        width = (0..name.len()).find(|&i| is_space(cells, NAME_COLUMN + i));
    }
    let width = width.unwrap_or(name.len());
    let text: String = name[..width].iter().map(|c| c.text.as_str()).collect();
    (text.trim().to_owned(), width)
}

/// A finished collapsed group ("  Ran 1 shell command"): Claude Code drops its bullet and leaves it dim.
fn is_summary(cells: &[Cell], bullet: Option<Bullet>) -> bool {
    bullet.is_none()
        && is_space(cells, 0)
        && is_space(cells, 1)
        && cells.get(NAME_COLUMN).is_some_and(|c| c.faint)
        && tool_hue(&tool_name(cells).0).is_some()
}

struct DiffLine {
    row: usize,
    hue: Hue,
    first: usize,
    end: usize,
}

/// A diff line (code blocks, `git diff`): a red "-" or a green "+" and the rest of its text in the
/// same hue; a wrapped row continues the diff line right above it. `first`/`end` bound its text.
/// The sign must match its hue: a Lua or SQL comment ("-- note") is all green but no diff line.
fn diff_line(i: usize, row: &Row, above: Option<&DiffLine>) -> Option<DiffLine> {
    let cells = row.cells;
    let first = (0..cells.len()).find(|&x| !is_space(cells, x))?;
    let hue = diff_hue(&cells[first])?;
    let sign = (cells[first].text == "-" && hue == Hue::Red) || (cells[first].text == "+" && hue == Hue::Green);
    let continues = row.wrapped && above.is_some_and(|a| a.hue == hue && a.row + 1 == i);
    if !(sign || continues) {
        return None;
    }
    let mut end = cells.len();
    while end > first && is_space(cells, end - 1) {
        end -= 1;
    }
    (first..end).all(|x| is_space(cells, x) || diff_hue(&cells[x]) == Some(hue)).then_some(DiffLine { row: i, hue, first, end })
}

/// Marks Claude Code transcript blocks: a tool call line plus its indented output, up to the next
/// blank or unindented line; inline code in a reply, which runs from its bullet through blank and
/// indented lines; the prompt echo, the rows after the prompt arrow that keep its background; and,
/// outside tool output and the echo, quote bars and diff lines (found by their cells, since Claude
/// Code's full-screen view can scroll a reply's bullet away). `rows` start at or above the block
/// holding row `from`; ranges above `from` are dropped.
pub fn find_blocks(rows: &[Row], style: Style, from: usize, theme_fg: Rgb) -> Vec<Range> {
    let mut ranges = Vec::new();
    let mut push = |row: usize, x: usize, width: usize, hue: Hue, paint: Paint| {
        if row >= from && width > 0 {
            ranges.push(Range { row, x, width, hue, paint });
        }
    };
    // Paints each run of cells in [from, to) that pass `test`.
    let mut paint_runs = |i: usize, cells: &[Cell], hue: Hue, paint: Paint, test: &dyn Fn(&Cell) -> bool, lo: usize, hi: usize| {
        let end = hi.min(cells.len());
        let mut start: Option<usize> = None;
        for x in lo..=end {
            let on = x < end && test(&cells[x]);
            match (on, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    push(i, s, x - s, hue, paint);
                    start = None;
                }
                _ => {}
            }
        }
    };
    let unpainted = |c: &Cell| !own_bg(c);

    // Consecutive diff lines share their left and right edges, so the tints read as one block.
    let mut diff_run: Vec<DiffLine> = Vec::new();
    let mut hue: Option<Hue> = None;
    let mut reply = false;
    let mut echo = false;
    let mut after_blank = true;
    for (i, row) in rows.iter().enumerate() {
        let cells = row.cells;
        let blank = (0..cells.len()).all(|x| is_space(cells, x));
        let bullet = cells.first().filter(|c| c.text == "\u{25cf}").map(|c| bullet_of(c, theme_fg));
        // The prompt echo: Claude Code's ANSI themes give it the bright-black background, which many
        // palettes also use for the selection, so a selection inside it would not show. Claude Code's
        // own mouse selection repaints cells in another color; the band leaves those.
        echo = cells.first().is_some_and(|c| own_bg(c) && (echo || c.text == "\u{276f}"));
        let call = matches!(bullet, Some(Bullet::Tool | Bullet::Failed)) || (after_blank && is_summary(cells, bullet));
        if call {
            reply = false;
            let (name, width) = tool_name(cells);
            let h = if bullet == Some(Bullet::Failed) {
                Hue::Red
            } else {
                tool_hue(&name).unwrap_or(if cells.get(NAME_COLUMN).is_some_and(|c| c.bold) { OTHER_TOOL_HUE } else { DESCRIBED_COMMAND_HUE })
            };
            hue = Some(h);
            if style == Style::Rows {
                paint_runs(i, cells, h, Paint::Header, &unpainted, 0, cells.len());
            } else {
                // Collapsed summaries are dimmed by Claude Code; dim text over a tint is hard to read.
                let paint = if bullet.is_some() { Paint::Tag } else { Paint::Name };
                paint_runs(i, cells, h, paint, &unpainted, NAME_COLUMN, NAME_COLUMN + width);
            }
        } else if let Some(h) = hue.filter(|_| bullet.is_none() && !blank && (row.wrapped || is_space(cells, 0))) {
            if style == Style::Rows {
                paint_runs(i, cells, h, Paint::Body, &unpainted, 0, cells.len());
            } else if cells.get(NAME_COLUMN).is_some_and(|c| c.text == "\u{23bf}") {
                paint_runs(i, cells, h, Paint::Connector, &unpainted, NAME_COLUMN, NAME_COLUMN + 1);
            }
        } else {
            hue = None;
            reply = !echo && (bullet == Some(Bullet::Reply) || (reply && (blank || row.wrapped || is_space(cells, 0))));
            if reply {
                paint_runs(i, cells, Hue::Blue, Paint::Code, &|c| is_code(c) && !own_bg(c), 0, cells.len());
            } else if echo {
                paint_runs(i, cells, Hue::Blue, Paint::Band, &echo_bg, 0, cells.len());
                paint_runs(i, cells, Hue::Blue, Paint::Prompt, &|c| echo_bg(c) && c.text == "\u{276f}", 0, 1);
            }
            if !echo {
                let text = (0..cells.len()).find(|&x| !is_space(cells, x) && !is_quote_bar(&cells[x]));
                paint_runs(i, cells, Hue::Blue, Paint::QuoteBar, &is_quote_bar, 0, text.unwrap_or(usize::MAX));
            }
            let diff = if echo { None } else { diff_line(i, row, diff_run.last()) };
            if diff.is_none() || diff_run.last().is_some_and(|d| d.row + 1 != i) {
                flush_diff(&mut diff_run, rows, &mut paint_runs, &unpainted);
            }
            diff_run.extend(diff);
        }
        after_blank = blank;
    }
    flush_diff(&mut diff_run, rows, &mut paint_runs, &unpainted);
    ranges
}

fn is_code(cell: &Cell) -> bool {
    cell.src.fg == RawColor::Palette(CODE_PALETTE) && !cell.faint
}

type PaintRuns<'a> = dyn FnMut(usize, &[Cell], Hue, Paint, &dyn Fn(&Cell) -> bool, usize, usize) + 'a;

fn flush_diff(run: &mut Vec<DiffLine>, rows: &[Row], paint_runs: &mut PaintRuns, unpainted: &dyn Fn(&Cell) -> bool) {
    if let (Some(left), Some(right)) = (run.iter().map(|l| l.first).min(), run.iter().map(|l| l.end).max()) {
        for line in run.iter() {
            paint_runs(line.row, rows[line.row].cells, line.hue, Paint::Diff, unpainted, left, right + 2);
        }
    }
    run.clear();
}
