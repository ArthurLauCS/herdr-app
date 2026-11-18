use super::claude::{self, Bullet};
use super::rules::{self, Hit};
use super::*;
use crate::settings;
use crate::vt::{RawColor, Src, Vt};

fn hx(s: &str) -> Rgb {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

const FG: Rgb = Rgb(255, 255, 255);
const BG: Rgb = Rgb(16, 16, 16);
const RED: Rgb = Rgb(255, 0, 0);
const GREEN: Rgb = Rgb(0, 255, 0);
const YELLOW: Rgb = Rgb(255, 255, 0);
const BLUE: Rgb = Rgb(0, 0, 255);
const MAGENTA: Rgb = Rgb(255, 0, 255);
const CYAN: Rgb = Rgb(0, 255, 255);

/// A dark theme whose bright hues are pure, so expected colors read at a glance.
fn dark() -> Theme {
    let mut p = [Rgb::default(); 16];
    for (i, c) in [(0, 0x000000), (1, 0xcd0000), (2, 0x00cd00), (3, 0xcdcd00), (4, 0x0000ee), (5, 0xcd00cd), (6, 0x00cdcd), (7, 0xc0c0c0), (8, 0x808080)] {
        p[i] = hx(&format!("{c:06x}"));
    }
    p[9] = RED;
    p[10] = GREEN;
    p[11] = YELLOW;
    p[12] = BLUE;
    p[13] = MAGENTA;
    p[14] = CYAN;
    p[15] = FG;
    Theme { fg: FG, bg: BG, cursor: FG, selection_bg: hx("336699"), palette16: p, minimum_contrast: 1.0 }
}

fn light() -> Theme {
    let mut t = dark();
    t.bg = Rgb(250, 250, 250);
    t.fg = Rgb(20, 20, 20);
    t
}

fn blocks_only(style: ClaudeBlocks) -> TerminalHighlights {
    TerminalHighlights { presets: preset_ids().map(|id| (id.to_owned(), false)).collect(), claude_blocks: style, ..Default::default() }
}

fn text_only() -> TerminalHighlights {
    TerminalHighlights { claude_blocks: ClaudeBlocks::Off, ..Default::default() }
}

/// A terminal of `cols` x `rows` with the settings and theme given and `input` written.
fn terminal(cols: u16, rows: u16, hl: TerminalHighlights, bold_bright: bool, theme: Theme, input: &str) -> Vt {
    let mut s = (*settings::get()).clone();
    s.terminal_highlights = hl;
    s.terminal_bold_bright = bold_bright;
    settings::set(s);
    let mut vt = Vt::new(cols, rows);
    vt.set_theme(theme);
    vt.write(input.as_bytes());
    vt
}

fn lines(rows: &[&str]) -> String {
    rows.join("\r\n")
}

/// What highlighting changed on screen: runs of cells `(row, x, width, fg, bg)` whose fg or bg
/// differ from the same text drawn without highlighting.
fn marks(cols: u16, rows: u16, hl: TerminalHighlights, input: &str) -> Vec<(usize, usize, usize, Option<Rgb>, Option<Rgb>)> {
    let off = TerminalHighlights { enabled: false, ..hl.clone() };
    let plain = terminal(cols, rows, off, true, dark(), input).frame();
    let shown = terminal(cols, rows, hl, true, dark(), input).frame();
    let mut out = Vec::new();
    for (y, (a, b)) in plain.rows.iter().zip(&shown.rows).enumerate() {
        let mut run: Option<(usize, usize, Option<Rgb>, Option<Rgb>)> = None;
        for x in 0..=a.len() {
            let delta = (x < a.len() && (a[x].fg != b[x].fg || a[x].bg != b[x].bg)).then(|| ((a[x].fg != b[x].fg).then_some(b[x].fg), (a[x].bg != b[x].bg).then_some(b[x].bg).flatten()));
            match (run, delta) {
                (Some((s, w, f, g)), Some((f2, g2))) if (f, g) == (f2, g2) => run = Some((s, w + 1, f, g)),
                (Some((s, w, f, g)), _) => {
                    out.push((y, s, w, f, g));
                    run = delta.map(|(f, g)| (x, 1, f, g));
                }
                (None, Some((f, g))) => run = Some((x, 1, f, g)),
                (None, None) => {}
            }
        }
    }
    out
}

fn row_text(row: &[Cell]) -> String {
    row.iter().map(|c| if c.text.is_empty() { " " } else { c.text.as_str() }).collect()
}

/// The cells of `needle` in `row`.
fn word<'a>(row: &'a [Cell], needle: &str) -> &'a [Cell] {
    let at = row_text(row).find(needle).unwrap_or_else(|| panic!("{needle:?} not in {:?}", row_text(row)));
    let x = row_text(row)[..at].chars().count();
    &row[x..x + needle.chars().count()]
}

fn fg_of(frame: &crate::vt::Frame, y: usize, needle: &str) -> Vec<Rgb> {
    word(&frame.rows[y], needle).iter().map(|c| c.fg).collect()
}

fn same(needle: &str, color: Rgb) -> Vec<Rgb> {
    vec![color; needle.chars().count()]
}

// ---------------------------------------------------------------------------------------------
// The matcher, as Orca's tests drive it.

/// Cells of `text`; those at `colored` are in a program color, those at `dimmed` dim.
fn cells(text: &str, colored: &[usize], dimmed: &[usize]) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    for ch in text.chars() {
        let wide = ('\u{4e00}'..='\u{9fff}').contains(&ch);
        let i = out.len();
        let src = Src { fg: if colored.contains(&i) { RawColor::Palette(1) } else { RawColor::Default }, ..Default::default() };
        out.push(Cell { text: ch.to_string(), wide, faint: dimmed.contains(&i), src, ..Default::default() });
        if wide {
            out.push(Cell { src, faint: dimmed.contains(&i), ..Default::default() });
        }
    }
    out
}

fn hits(rows: &[Vec<Cell>], rules: &[rules::Rule]) -> Vec<Hit> {
    rules::find_hits(rows.iter().map(|r| r.as_slice()), &|c| claude::is_plain(c, FG), rules)
}

fn hit(row: usize, x: usize, width: usize, hue: Hue, subtle: bool) -> Hit {
    Hit { row, x, width, hue, subtle }
}

fn presets() -> Vec<rules::Rule> {
    rules::compile(&TerminalHighlights::default())
}

#[test]
fn colors_failure_words_but_not_a_zero_count() {
    let row = cells("Tests: 2 failed, 0 errors", &[], &[]);
    assert_eq!(hits(&[row], &presets()), [hit(0, 9, 6, Hue::Red, false), hit(0, 7, 1, Hue::Yellow, true), hit(0, 17, 1, Hue::Yellow, true)]);
}

#[test]
fn not_zero_skips_only_a_whole_zero_or_no() {
    let red = |text: &str| hits(&[cells(text, &[], &[])], &presets()).iter().any(|h| h.hue == Hue::Red);
    assert!(!red("0 errors"));
    assert!(!red("no errors"));
    assert!(!red("No failures found, NO ERROR"));
    assert!(red("10 errors"));
    assert!(red("piano errors"));
    assert!(red("zero errors"));
    // Only the guarded word is skipped: a symbol after a zero still counts.
    assert!(red("0 \u{2717}"));
    assert!(red("0 errors, 3 errors"));
    let warn = |text: &str| hits(&[cells(text, &[], &[])], &presets()).iter().any(|h| h.hue == Hue::Yellow && !h.subtle);
    assert!(!warn("0 warnings"));
    assert!(!warn("no deprecated"));
    assert!(warn("3 warnings"));
    assert!(warn("\u{26a0} careful"));
}

#[test]
fn ascii_word_boundaries_like_javascript() {
    // JavaScript's \b is ASCII: a CJK character before a word is no word character.
    let first = |text: &str| hits(&[cells(text, &[], &[])], &presets());
    assert_eq!(first("\u{4e2d}error")[0], hit(0, 2, 5, Hue::Red, false));
    assert_eq!(first("xerror"), []);
    assert_eq!(first("passed"), [hit(0, 0, 6, Hue::Green, false)]);
}

#[test]
fn colors_list_markers_and_standalone_numbers_not_digits_inside_words() {
    let rows = [cells("1) took 56.9 s, 3,590,582 rows, 86.1% v2 x64 11.8x", &[], &[])];
    assert_eq!(
        hits(&rows, &presets()),
        [hit(0, 0, 2, Hue::Blue, false), hit(0, 8, 6, Hue::Yellow, true), hit(0, 16, 9, Hue::Yellow, true), hit(0, 32, 5, Hue::Yellow, true)]
    );
    assert_eq!(hits(&[cells("  - 2 a-b", &[], &[])], &presets()), [hit(0, 2, 1, Hue::Blue, false), hit(0, 4, 1, Hue::Yellow, true)]);
}

#[test]
fn list_markers_are_only_at_the_start_of_a_line_and_before_whitespace() {
    let lists = |text: &str| hits(&[cells(text, &[], &[])], &presets()).into_iter().filter(|h| h.hue == Hue::Blue).collect::<Vec<_>>();
    assert_eq!(lists("- item"), [hit(0, 0, 1, Hue::Blue, false)]);
    assert_eq!(lists("   * item"), [hit(0, 3, 1, Hue::Blue, false)]);
    assert_eq!(lists("\u{2022} item"), [hit(0, 0, 1, Hue::Blue, false)]);
    assert_eq!(lists("12. item"), [hit(0, 0, 3, Hue::Blue, false)]);
    assert_eq!(lists("3) item"), [hit(0, 0, 2, Hue::Blue, false)]);
    assert_eq!(lists("-item"), []);
    assert_eq!(lists("a - item"), []);
    assert_eq!(lists("item - more"), []);
    // A list marker on a wrapped continuation row is not at the line's start.
    assert_eq!(lists("   - x"), [hit(0, 3, 1, Hue::Blue, false)]);
    let wrapped = hits(&[cells("some text that wrapped ", &[], &[]), cells("- not a marker", &[], &[])], &presets());
    assert!(wrapped.iter().all(|h| h.hue != Hue::Blue));
}

#[test]
fn colors_durations_and_counts_whole_but_not_numbers_in_dimmed_text() {
    let row = cells("8m 6s 870k 3.2M", &[], &[11, 12, 13, 14]);
    assert_eq!(hits(&[row], &presets()), [hit(0, 0, 2, Hue::Yellow, true), hit(0, 3, 2, Hue::Yellow, true), hit(0, 6, 4, Hue::Yellow, true)]);
}

#[test]
fn colors_status_symbols() {
    let row = cells("\u{2717} build \u{2713} lint \u{26a0} deps", &[], &[]);
    assert_eq!(hits(&[row], &presets()), [hit(0, 0, 1, Hue::Red, false), hit(0, 15, 1, Hue::Yellow, false), hit(0, 8, 1, Hue::Green, false)]);
}

#[test]
fn leaves_text_the_program_already_colored() {
    assert_eq!(hits(&[cells("error: boom", &[0, 1, 2, 3, 4], &[])], &presets()), []);
}

#[test]
fn splits_a_match_across_wrapped_rows() {
    let rows = [cells("see src/ma", &[], &[]), cells("in.ts:42 ok", &[], &[])];
    assert_eq!(hits(&rows, &presets()), [hit(0, 4, 6, Hue::Cyan, false), hit(1, 0, 8, Hue::Cyan, false)]);
}

#[test]
fn maps_matches_after_wide_characters_to_cell_columns() {
    assert_eq!(hits(&[cells("\u{4e2d}\u{6587} error", &[], &[])], &presets()), [hit(0, 5, 5, Hue::Red, false)]);
}

#[test]
fn colors_a_url_as_one_range_without_the_punctuation_after_it() {
    assert_eq!(hits(&[cells("\u{89c1} https://github.com/a/b.js?x=1\u{3002}", &[], &[])], &presets()), [hit(0, 3, 29, Hue::Cyan, false)]);
}

#[test]
fn colors_a_path_whole_or_not_at_all() {
    assert_eq!(hits(&[cells("Ui/Log.rbxmx.stash_backup", &[], &[])], &presets()), []);
}

#[test]
fn paths_references_and_file_lines() {
    let one = |text: &str| hits(&[cells(text, &[], &[])], &presets()).into_iter().filter(|h| h.hue == Hue::Cyan).collect::<Vec<_>>();
    assert_eq!(one("edit src/main.rs now"), [hit(0, 5, 11, Hue::Cyan, false)]);
    assert_eq!(one("see ~/a/b.txt:12:3"), [hit(0, 4, 14, Hue::Cyan, false)]);
    assert_eq!(one("C:\\x\\y.rs here"), [hit(0, 0, 9, Hue::Cyan, false)]);
    assert_eq!(one("main.rs:42"), [hit(0, 0, 10, Hue::Cyan, false)]);
    assert_eq!(one("at (lib.rs:7)"), [hit(0, 4, 8, Hue::Cyan, false)]);
    // A leading dot needs a word character before it for \b; a space does not give one.
    assert_eq!(one(" .b.c:12"), [hit(0, 2, 6, Hue::Cyan, false)]);
    let refs = |text: &str| hits(&[cells(text, &[], &[])], &presets()).into_iter().filter(|h| h.hue == Hue::Magenta).collect::<Vec<_>>();
    assert_eq!(refs("fixes #123 and a1b2c3d"), [hit(0, 6, 4, Hue::Magenta, false), hit(0, 15, 7, Hue::Magenta, false)]);
    assert_eq!(refs("abcdefg 1234567 &#12 a#1"), [], "hex needs a digit and a letter; # after a word or & is no reference");
    assert_eq!(refs("deadbeef0cafe"), [hit(0, 0, 13, Hue::Magenta, false)]);
}

#[test]
fn custom_rules_have_priority_and_invalid_patterns_are_skipped() {
    let cfg = TerminalHighlights {
        presets: [("metrics".to_owned(), false)].into(),
        rules: vec![
            CustomRule { id: "bad".into(), pattern: "(".into(), hue: Hue::Green, ..Default::default() },
            CustomRule { id: "deploy".into(), pattern: "deploy failed".into(), hue: Hue::Yellow, ..Default::default() },
            CustomRule { id: "off".into(), pattern: "in".into(), hue: Hue::Green, enabled: false, ..Default::default() },
        ],
        ..Default::default()
    };
    assert_eq!(hits(&[cells("deploy failed in 3s", &[], &[])], &rules::compile(&cfg)), [hit(0, 0, 13, Hue::Yellow, false)]);
    assert_eq!(cfg.problems().len(), 1);
    // Case: insensitive unless asked.
    let cs = TerminalHighlights { presets: preset_ids().map(|id| (id.to_owned(), false)).collect(), rules: vec![CustomRule { pattern: "Foo".into(), case_sensitive: true, ..Default::default() }], ..Default::default() };
    assert_eq!(hits(&[cells("foo Foo", &[], &[])], &rules::compile(&cs)), [hit(0, 4, 3, Hue::Yellow, false)]);
}

#[test]
fn presets_can_be_turned_off_one_by_one() {
    let cfg = TerminalHighlights { presets: [("errors".to_owned(), false)].into(), ..Default::default() };
    assert_eq!(hits(&[cells("error", &[], &[])], &rules::compile(&cfg)), []);
    assert!(TerminalHighlights { presets: [("nope".to_owned(), true)].into(), ..Default::default() }.problems()[0].contains("nope"));
}

#[test]
fn a_runaway_pattern_finds_nothing_instead_of_hanging() {
    let cfg = TerminalHighlights { presets: preset_ids().map(|id| (id.to_owned(), false)).collect(), rules: vec![CustomRule { pattern: "(?=(a+)+b)(a+)+$".into(), ..Default::default() }], ..Default::default() };
    let line = "a".repeat(60) + "c";
    assert_eq!(hits(&[cells(&line, &[], &[])], &rules::compile(&cfg)), []);
}

// ---------------------------------------------------------------------------------------------
// Through the terminal: what a frame's cells come out as.

#[test]
fn plain_text_takes_the_theme_hue_and_program_colored_text_keeps_its_own() {
    let mut vt = terminal(30, 4, text_only(), true, dark(), &lines(&["build \x1b[31merror\x1b[0m error", "Tests: 1 failed", "ok passed \x1b[7merror\x1b[0m", "\x1b[44merror\x1b[0m \x1b[38;2;128;128;128merror\x1b[0m"]));
    let f = vt.frame();
    assert_eq!(fg_of(&f, 0, "build"), same("build", FG));
    assert_eq!(f.rows[0][6].fg, hx("cd0000"), "the program's own red stays palette red");
    assert_eq!(fg_of(&f, 0, "error error")[6..], same("error", RED));
    assert_eq!(fg_of(&f, 1, "failed"), same("failed", RED));
    assert_eq!(fg_of(&f, 2, "passed"), same("passed", GREEN));
    let inverse = word(&f.rows[2], "error");
    assert!(inverse.iter().all(|c| c.fg == BG && c.bg == Some(FG)), "inverse text is left alone");
    assert_eq!(word(&f.rows[3], "error")[0].bg, Some(hx("0000ee")));
    assert_eq!(word(&f.rows[3], "error")[0].fg, FG, "text on a program background is left alone");
    assert_eq!(f.rows[3][6].fg, RED, "gray truecolor text is plain");
}

#[test]
fn neutral_palette_colors_and_the_theme_foreground_are_plain() {
    let input = lines(&["\x1b[37merror\x1b[0m \x1b[90merror\x1b[0m \x1b[97merror\x1b[0m \x1b[30merror", "\x1b[38;2;255;255;255merror\x1b[0m \x1b[38;2;250;100;100merror\x1b[0m \x1b[32merror"]);
    let f = terminal(60, 2, text_only(), true, dark(), &input).frame();
    for at in [0, 6, 12, 18] {
        assert_eq!(f.rows[0][at].fg, RED, "column {at}");
    }
    assert_eq!(f.rows[1][0].fg, RED);
    assert_eq!(f.rows[1][6].fg, Rgb(250, 100, 100));
    assert_eq!(f.rows[1][12].fg, hx("00cd00"));
}

#[test]
fn light_themes_use_the_normal_hues() {
    let f = terminal(20, 1, text_only(), true, light(), "an error").frame();
    assert_eq!(fg_of(&f, 0, "error"), same("error", hx("cd0000")));
}

#[test]
fn subtle_numbers_blend_the_hue_into_the_text_color() {
    let f = terminal(20, 1, text_only(), true, dark(), "took 5 ms").frame();
    let mixed = mix(YELLOW, FG, 0.5);
    assert_eq!(mixed, Rgb(255, 255, 128));
    assert_eq!(fg_of(&f, 0, "5 ms"), same("5 ms", mixed));
}

#[test]
fn wrapped_matches_color_across_the_wrap() {
    // 12 columns: "see src/main.rs:4" wraps inside the path.
    let f = terminal(12, 3, text_only(), true, dark(), "see src/main.rs:4 ok").frame();
    assert_eq!(fg_of(&f, 0, "src/main"), same("src/main", CYAN));
    assert_eq!(f.rows[1][0].fg, CYAN);
    assert_eq!(row_text(&f.rows[1]).trim_end(), ".rs:4 ok");
    assert_eq!(fg_of(&f, 1, ".rs:4"), same(".rs:4", CYAN));
    assert_eq!(fg_of(&f, 1, "ok"), same("ok", FG));
}

#[test]
fn off_changes_nothing() {
    let off = TerminalHighlights { enabled: false, ..Default::default() };
    let f = terminal(20, 2, off, false, dark(), "error 12\r\n\x1b[1;31mx").frame();
    assert_eq!(fg_of(&f, 0, "error"), same("error", FG));
    assert_eq!(f.rows[1][0].fg, hx("cd0000"));
    let none = TerminalHighlights { presets: preset_ids().map(|id| (id.to_owned(), false)).collect(), claude_blocks: ClaudeBlocks::Off, ..Default::default() };
    assert!(Highlighter::new(&none, &dark()).is_none());
}

#[test]
fn bold_text_in_ansi_colors_draws_bright() {
    let input = "\x1b[1;31mA\x1b[0m\x1b[31mB\x1b[0m\x1b[1mC\x1b[0m\x1b[1;91mD\x1b[0m\x1b[1;38;5;1mE\x1b[0m\x1b[1;38;2;205;0;0mF\x1b[0m\x1b[1;30mG\x1b[0m\x1b[1;37mH";
    let off = TerminalHighlights { enabled: false, ..Default::default() };
    let on = terminal(20, 1, off.clone(), true, dark(), input).frame();
    let fg: Vec<Rgb> = on.rows[0][..8].iter().map(|c| c.fg).collect();
    assert_eq!(fg, [RED, hx("cd0000"), FG, RED, RED, hx("cd0000"), hx("808080"), FG]);
    let still = terminal(20, 1, off, false, dark(), input).frame();
    assert_eq!(still.rows[0][0].fg, hx("cd0000"));
    assert_eq!(still.rows[0][6].fg, Rgb(0, 0, 0));
}

#[test]
fn bold_bright_follows_the_setting_live() {
    let off = TerminalHighlights { enabled: false, ..Default::default() };
    let mut vt = terminal(10, 1, off.clone(), true, dark(), "\x1b[1;31mA");
    assert_eq!(vt.frame().rows[0][0].fg, RED);
    let mut s = (*settings::get()).clone();
    s.terminal_bold_bright = false;
    settings::set(s);
    assert_eq!(vt.frame().rows[0][0].fg, hx("cd0000"));
}

#[test]
fn highlights_follow_the_settings_live_and_a_new_theme() {
    let mut vt = terminal(20, 1, text_only(), true, dark(), "an error");
    assert_eq!(vt.frame().rows[0][3].fg, RED);
    let mut s = (*settings::get()).clone();
    s.terminal_highlights.presets.insert("errors".into(), false);
    settings::set(s);
    assert_eq!(vt.frame().rows[0][3].fg, FG);
    let mut s = (*settings::get()).clone();
    s.terminal_highlights.presets.clear();
    settings::set(s);
    assert_eq!(vt.frame().rows[0][3].fg, RED);
    vt.set_theme(light());
    assert_eq!(vt.frame().rows[0][3].fg, hx("cd0000"));
}

#[test]
fn the_terminal_selection_stays_as_it_is() {
    let mut vt = terminal(20, 1, text_only(), true, dark(), "an error");
    vt.select_at(Some(crate::vt::CellPosition { row: 0, col: 3 }), "word").unwrap();
    let f = vt.frame();
    let error = word(&f.rows[0], "error");
    assert!(error.iter().all(|c| c.bg == Some(hx("336699")) && c.fg == FG));
}

#[test]
fn unchanged_rows_come_out_as_the_same_rc() {
    let mut vt = terminal(30, 6, TerminalHighlights::default(), true, dark(), &lines(&["error in a.rs:3", "plain", "\x1b[92m●\x1b[39m \x1b[1mBash\x1b[22m(x)", "  ⎿  3 failed", "", "spinner 1"]));
    let first = vt.frame();
    let second = vt.frame();
    for y in 0..6 {
        assert!(Rc::ptr_eq(&first.rows[y], &second.rows[y]), "row {y}");
    }
    // The spinner redraws one row; the decorated ones are kept.
    vt.write(b"\x1b[6;1H\x1b[2Kspinner 2");
    let third = vt.frame();
    for y in 0..5 {
        assert!(Rc::ptr_eq(&second.rows[y], &third.rows[y]), "row {y}");
    }
    assert!(!Rc::ptr_eq(&second.rows[5], &third.rows[5]));
    // A row with nothing to mark is the terminal's own row.
    let off = TerminalHighlights { enabled: false, ..Default::default() };
    let mut vt = terminal(30, 2, off, true, dark(), "error\r\nok");
    let a = vt.frame();
    let b = vt.frame();
    assert!(Rc::ptr_eq(&a.rows[0], &b.rows[0]));
    // Marking changes a row only when what is marked does.
    let mut vt = terminal(30, 3, TerminalHighlights::default(), true, dark(), "a\r\nb\r\nc");
    let a = vt.frame();
    vt.write(b"\x1b[3;1H\x1b[2Kc 5");
    let b = vt.frame();
    assert!(Rc::ptr_eq(&a.rows[0], &b.rows[0]) && Rc::ptr_eq(&a.rows[1], &b.rows[1]));
    assert_eq!(b.rows[2][2].fg, mix(YELLOW, FG, 0.5));
}

// ---------------------------------------------------------------------------------------------
// Claude Code blocks.

// SGR as Claude Code emits it in its ANSI themes.
fn transcript() -> String {
    lines(&[
        "\x1b[100m❯ \x1b[97mfix it\x1b[39m\x1b[49m",
        "",
        "\x1b[92m●\x1b[39m \x1b[1mBash\x1b[22m(npm test)",
        "\x1b[2m  ⎿  FAIL a.test.ts\x1b[22m",
        "     Tests: 1 failed",
        "",
        "\x1b[91m●\x1b[39m \x1b[1mUpdate\x1b[22m(a.ts)",
        "  ⎿  Error: modified",
        "",
        "\x1b[2C\x1b[2mRan \x1b[22;1m1\x1b[22;2m shell command\x1b[22m",
        "",
        "\x1b[97m●\x1b[39m done",
    ])
}

fn some(c: Rgb) -> Option<Rgb> {
    Some(c)
}

#[test]
fn tints_tool_names_colors_summaries_and_connectors_and_marks_a_failed_call_red() {
    let band = Some(hx("232323"));
    assert_eq!(
        marks(40, 12, blocks_only(ClaudeBlocks::Tags), &transcript()),
        [
            (0, 0, 1, some(BLUE), band),
            (0, 1, 7, None, band),
            (2, 2, 4, some(MAGENTA), some(hx("360d36"))),
            (3, 2, 1, some(MAGENTA), None),
            (6, 2, 6, some(RED), some(hx("360d0d"))),
            (7, 2, 1, some(RED), None),
            (9, 2, 3, some(MAGENTA), None),
        ]
    );
}

#[test]
fn tints_whole_rows_of_a_tool_block_stronger_on_the_call_line() {
    let band = Some(hx("232323"));
    assert_eq!(
        marks(40, 12, blocks_only(ClaudeBlocks::Rows), &transcript()),
        [
            (0, 0, 1, some(BLUE), band),
            (0, 1, 7, None, band),
            (2, 0, 40, None, some(hx("450c45"))),
            (3, 0, 40, None, some(hx("230f23"))),
            (4, 0, 40, None, some(hx("230f23"))),
            (6, 0, 40, None, some(hx("450c0c"))),
            (7, 0, 40, None, some(hx("230f0f"))),
            (9, 0, 40, None, some(hx("450c45"))),
        ]
    );
}

#[test]
fn claude_blocks_off_marks_nothing() {
    assert_eq!(marks(40, 12, blocks_only(ClaudeBlocks::Off), &transcript()), []);
}

#[test]
fn lays_a_neutral_band_under_every_prompt_echo_row_around_claude_codes_own_selection() {
    let input = lines(&["\x1b[100m❯ fix the config  \x1b[49m", "\x1b[100m  loader \x1b[44mplease\x1b[100m   \x1b[49m", "", "next"]);
    let band = Some(hx("232323"));
    assert_eq!(marks(20, 4, blocks_only(ClaudeBlocks::Tags), &input), [(0, 0, 1, some(BLUE), band), (0, 1, 17, None, band), (1, 0, 9, None, band), (1, 15, 3, None, band)]);
}

#[test]
fn leaves_text_under_claude_codes_own_mouse_selection_in_the_programs_colors() {
    let input = lines(&["\x1b[92m●\x1b[39m \x1b[1mSk\x1b[44mill\x1b[22m(x) \x1b[49m", "  build \x1b[44merror\x1b[49m error"]);
    let cfg = TerminalHighlights { claude_blocks: ClaudeBlocks::Tags, ..Default::default() };
    assert_eq!(marks(30, 3, cfg, &input), [(0, 2, 2, some(BLUE), some(hx("0d0d36"))), (1, 14, 5, some(RED), None)]);
}

#[test]
fn gives_inline_code_in_a_reply_a_neutral_background_across_blank_lines_but_not_in_tool_calls() {
    let input = lines(&["\x1b[97m●\x1b[39m run \x1b[94mnpm test\x1b[39m", "", "  then \x1b[94mlint\x1b[39m", "\x1b[92m●\x1b[39m \x1b[1mRead\x1b[22m(\x1b[94ma.ts\x1b[39m)"]);
    let band = Some(hx("232323"));
    assert_eq!(marks(30, 5, blocks_only(ClaudeBlocks::Tags), &input), [(0, 6, 8, None, band), (2, 7, 4, None, band), (3, 2, 4, some(CYAN), some(hx("0d3636")))]);
}

#[test]
fn reads_a_bullet_in_the_theme_foreground_as_a_reply_and_tags_a_whole_cjk_command_word() {
    let mut theme = dark();
    theme.fg = hx("cdd6f4");
    let input = lines(&["\x1b[38;2;205;214;244m●\x1b[39m 我先跑几次", "\x1b[38;2;166;227;161m●\x1b[39m 列出 文件"]);
    let mut vt = terminal(30, 3, blocks_only(ClaudeBlocks::Tags), true, theme, &input);
    let f = vt.frame();
    assert_eq!(f.rows[0].iter().filter(|c| c.bg.is_some()).count(), 0);
    let tagged: Vec<usize> = (0..30).filter(|x| f.rows[1][*x].bg.is_some()).collect();
    assert_eq!(tagged, [2, 3, 4, 5]);
    assert_eq!(f.rows[1][2].fg, MAGENTA);
}

#[test]
fn turns_quote_bars_solid_and_tints_a_run_of_diff_lines_as_one_block() {
    let input = lines(&[
        "  \x1b[2m▎\x1b[22m \x1b[3mquoted\x1b[23m",
        "  \x1b[2m▎ ▎\x1b[22m nested",
        "",
        "  \x1b[31m- old line\x1b[39m",
        "  \x1b[32m+ new\x1b[39m",
        "    unchanged",
        "  \x1b[31m-\x1b[39m not a diff line",
        "  \x1b[32m-- lua comment\x1b[39m",
    ]);
    let bar = some(mix(BLUE, BG, 0.4));
    assert_eq!(bar, some(hx("0a0a70")));
    let red_tint = some(mix(RED, BG, 0.16));
    let green_tint = some(mix(GREEN, BG, 0.16));
    assert_eq!(red_tint, some(hx("360d0d")));
    assert_eq!(
        marks(30, 8, blocks_only(ClaudeBlocks::Tags), &input),
        [(0, 2, 1, some(BLUE), bar), (1, 2, 1, some(BLUE), bar), (1, 4, 1, some(BLUE), bar), (3, 2, 12, None, red_tint), (4, 2, 12, None, green_tint)]
    );
}

#[test]
fn a_wrapped_diff_line_continues_the_one_above_and_the_tint_follows_the_text_not_the_selection() {
    // 12 columns: the added line wraps onto a second row.
    let input = "\x1b[32m+ aaaaaaaaaaaaaaaaaaaa\x1b[39m";
    let m = marks(12, 3, blocks_only(ClaudeBlocks::Tags), input);
    let tint = some(mix(GREEN, BG, 0.16));
    assert_eq!(m, [(0, 0, 12, None, tint), (1, 0, 12, None, tint)]);
}

#[test]
fn light_themes_use_weaker_tints() {
    let input = lines(&["\x1b[92m●\x1b[39m \x1b[1mBash\x1b[22m(x)"]);
    let f = terminal(20, 1, blocks_only(ClaudeBlocks::Tags), true, light(), &input).frame();
    assert_eq!(f.rows[0][2].fg, hx("cd00cd"));
    assert_eq!(f.rows[0][2].bg, Some(mix(hx("cd00cd"), Rgb(250, 250, 250), 0.12)));
}

#[test]
fn tool_hues() {
    let tag = |name_sgr: &str, bullet: &str| {
        let f = terminal(30, 1, blocks_only(ClaudeBlocks::Tags), true, dark(), &format!("{bullet}●\x1b[39m {name_sgr}(x)")).frame();
        f.rows[0][2].fg
    };
    let bold = |n: &str| format!("\x1b[1m{n}\x1b[22m");
    for (name, hue) in [("Read", CYAN), ("Grep", CYAN), ("Edit", YELLOW), ("Write", YELLOW), ("Bash", MAGENTA), ("Agent", BLUE), ("WebFetch", GREEN), ("Web Search", GREEN)] {
        assert_eq!(tag(&bold(name), "\x1b[92m"), hue, "{name}");
    }
    assert_eq!(tag(&bold("mcp__foo"), "\x1b[92m"), BLUE, "an unknown bold name is a plugin tool");
    assert_eq!(tag("Counting files", "\x1b[92m"), MAGENTA, "an unknown plain word describes a command");
    assert_eq!(tag(&bold("Read"), "\x1b[91m"), RED, "a failed call is red whatever the tool");
    assert_eq!(tag(&bold("Read"), "\x1b[31m"), RED);
    assert_eq!(tag(&bold("Read"), "\x1b[2;92m"), CYAN, "a dim bullet is a tool call");
}

#[test]
fn bullet_styles() {
    let cell = |src: RawColor, faint: bool| Cell { text: "●".into(), faint, src: Src { fg: src, ..Default::default() }, ..Default::default() };
    let b = |c: &Cell| claude::bullet_of(c, FG);
    assert_eq!(b(&cell(RawColor::Default, false)), Bullet::Reply);
    assert_eq!(b(&cell(RawColor::Palette(15), false)), Bullet::Reply);
    assert_eq!(b(&cell(RawColor::Rgb(FG), false)), Bullet::Reply);
    assert_eq!(b(&cell(RawColor::Default, true)), Bullet::Tool);
    assert_eq!(b(&cell(RawColor::Palette(2), false)), Bullet::Tool);
    assert_eq!(b(&cell(RawColor::Palette(1), false)), Bullet::Failed);
    assert_eq!(b(&cell(RawColor::Palette(9), false)), Bullet::Failed);
    assert_eq!(b(&cell(RawColor::Rgb(Rgb(255, 107, 128)), false)), Bullet::Failed);
    assert_eq!(b(&cell(RawColor::Rgb(Rgb(78, 186, 101)), false)), Bullet::Tool);
}

#[test]
fn a_bulletless_summary_needs_a_blank_row_before_it_an_indent_and_a_dim_first_word() {
    let tagged = |input: &str| marks(40, 6, blocks_only(ClaudeBlocks::Tags), input);
    assert_eq!(tagged(&lines(&["", "\x1b[2C\x1b[2mReading 3 files\x1b[22m"])), [(1, 2, 7, some(CYAN), None)]);
    assert_eq!(tagged(&lines(&["text", "\x1b[2C\x1b[2mReading 3 files\x1b[22m"])), [], "no blank row before it");
    assert_eq!(tagged(&lines(&["", "\x1b[2C\x1b[2mDreaming 3 files\x1b[22m"])), [], "not a known verb");
    assert_eq!(tagged(&lines(&["", "\x1b[2CReading 3 files"])), [], "not dim");
    assert_eq!(tagged(&lines(&["", "Reading 3 files"])), [], "not indented");
}

#[test]
fn output_rows_run_to_a_blank_or_unindented_row() {
    let input = lines(&["\x1b[92m●\x1b[39m \x1b[1mBash\x1b[22m(x)", "  ⎿  one", "     two", "three", "  ⎿  not ours", "", "  ⎿  neither"]);
    let m = marks(30, 8, blocks_only(ClaudeBlocks::Tags), &input);
    let rows: Vec<usize> = m.iter().map(|m| m.0).collect();
    assert_eq!(rows, [0, 1]);
}

#[test]
fn a_tool_block_that_starts_above_the_viewport_still_colors_its_output() {
    // 6 rows; the transcript is longer, so the output rows at the top come from scrollback.
    let mut input = vec!["\x1b[91m●\x1b[39m \x1b[1mBash\x1b[22m(long)".to_string()];
    input.extend((0..9).map(|i| format!("  ⎿  line {i}")));
    let mut vt = terminal(30, 6, blocks_only(ClaudeBlocks::Tags), true, dark(), &input.join("\r\n"));
    // The viewport shows the last six rows: the call line is above it, in scrollback.
    let f = vt.frame();
    assert!(row_text(&f.rows[0]).contains("line"), "{}", row_text(&f.rows[0]));
    assert_eq!(f.rows[0][2].fg, RED, "the connector takes the failed call's hue");
    // An output row without a call above it stays plain.
    let mut vt = terminal(30, 6, blocks_only(ClaudeBlocks::Tags), true, dark(), &(0..9).map(|i| format!("  ⎿  line {i}")).collect::<Vec<_>>().join("\r\n"));
    assert_eq!(vt.frame().rows[0][2].fg, FG);
}

#[test]
fn rows_style_tints_output_rows_scrolled_above_the_viewport() {
    let mut input = vec!["\x1b[92m●\x1b[39m \x1b[1mRead\x1b[22m(f)".to_string()];
    input.extend((0..9).map(|i| format!("  line {i}")));
    let mut vt = terminal(30, 6, blocks_only(ClaudeBlocks::Rows), true, dark(), &input.join("\r\n"));
    let f = vt.frame();
    assert_eq!(f.rows[0][0].bg, Some(mix(CYAN, BG, 0.08)));
    vt.scroll_to_top();
    let f = vt.frame();
    assert_eq!(f.rows[0][0].bg, Some(mix(CYAN, BG, 0.22)));
    assert_eq!(f.rows[1][0].bg, Some(mix(CYAN, BG, 0.08)));
}

#[test]
fn a_logical_line_that_starts_above_the_viewport_is_matched_whole() {
    let mut vt = terminal(12, 2, text_only(), true, dark(), "see src/main.rs:4 ok\r\nx\r\ny");
    // Rows: "see src/main", ".rs:4 ok", "x", "y"; the viewport shows the last two after a scroll.
    vt.scroll_viewport(-1);
    let f = vt.frame();
    assert_eq!(row_text(&f.rows[0]).trim_end(), ".rs:4 ok");
    assert_eq!(fg_of(&f, 0, ".rs:4"), same(".rs:4", CYAN));
}

// ---------------------------------------------------------------------------------------------
// Settings.

#[test]
fn settings_defaults_are_the_users_values_and_round_trip() {
    let person: settings::Person = serde_json::from_str("{}").unwrap();
    assert!(person.terminal_highlights.enabled && person.terminal_bold_bright);
    assert_eq!(person.terminal_highlights.claude_blocks, ClaudeBlocks::Tags);
    assert!(person.terminal_highlights.presets.is_empty() && person.terminal_highlights.rules.is_empty());
    let text = serde_json::to_string(&person).unwrap();
    assert!(!text.contains("terminal_highlights") && !text.contains("terminal_bold_bright"), "defaults are not written: {text}");
    let person: settings::Person = serde_json::from_str(
        r#"{"terminal_bold_bright": false, "terminal_highlights": {"claude_blocks": "rows", "presets": {"lists": false}, "rules": [{"id": "x", "pattern": "TODO", "hue": "magenta", "subtle": true}]}}"#,
    )
    .unwrap();
    let h = &person.terminal_highlights;
    assert!(h.enabled && !person.terminal_bold_bright);
    assert_eq!((h.claude_blocks, h.presets["lists"]), (ClaudeBlocks::Rows, false));
    assert_eq!((h.rules[0].hue, h.rules[0].subtle, h.rules[0].case_sensitive, h.rules[0].enabled), (Hue::Magenta, true, false, true));
    assert_eq!(serde_json::from_str::<settings::Person>(&serde_json::to_string(&person).unwrap()).unwrap(), person);
    assert!(serde_json::from_str::<settings::Person>(r#"{"terminal_highlights": {"claude_blocks": "loud"}}"#).is_err());
}
