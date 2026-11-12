//! Resumed line breaking (parley `build_into_keeping_lines` + `BreakLines::resume`): after random
//! appends, truncations and insertions, a layout built again and broken from its first changed
//! line must be identical to one built and broken from scratch. Blitz breaks every inline root
//! this way, so the document-level test does the same through text mutations.
use std::borrow::Cow;

use parley::{
    Alignment, AlignmentOptions, BaseDirection, FontContext, FontFamily, IndentOptions, Layout, LayoutContext,
    OverflowWrap, TextStyle, TextWrapMode, TreeBuilder, WhiteSpaceCollapse, YieldData,
};

type Brush = [u8; 4];

/// A deterministic xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone)]
struct Case {
    name: &'static str,
    words: &'static [&'static str],
    collapse: WhiteSpaceCollapse,
    overflow_wrap: OverflowWrap,
    /// Spans besides the root style: (family, size).
    spans: &'static [(&'static str, f32)],
    /// Whether the spans are `text-wrap-mode: nowrap`.
    spans_nowrap: bool,
    width: f32,
    indent: f32,
    indent_options: IndentOptions,
    alignment: Alignment,
    direction: BaseDirection,
}

const LATIN: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet,",
    "consectetur",
    "adipiscing",
    "elit.",
    "a",
    "Pellentesque",
    "habitant",
    "morbi",
    "tristique",
    "senectus",
    "et",
    "netus.",
    "supercalifragilisticexpialidocious",
    "x",
    "fi",
    "ffl",
    "office",
    "Wavy",
];
const PRE: &[&str] = &[
    "lorem",
    "ipsum  ",
    "dolor\n",
    "  sit",
    "amet,",
    "\n",
    "consectetur",
    "adipiscing   elit.",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "b",
    "\t",
    "c d",
];
const EMOJI: &[&str] = &["hello", "😀", "world", "👍🏽", "ok", "🇸🇮", "x", "naïve", "ﬁne", "日本語"];
const ARABIC: &[&str] = &["مرحبا", "بالعالم", "hello", "سلام", "world", "كتاب"];

fn root_style(case: &Case) -> TextStyle<'static, 'static, Brush> {
    TextStyle {
        font_family: FontFamily::Source(Cow::Borrowed("sans-serif")),
        white_space_collapse: case.collapse,
        overflow_wrap: case.overflow_wrap,
        ..TextStyle::default()
    }
}

/// Text segments, each in the root style (`0`) or in span `i - 1` of the case.
type Content = Vec<(usize, String)>;

fn push_content(tb: &mut TreeBuilder<'_, Brush>, case: &Case, content: &Content) {
    for (variant, text) in content {
        if *variant == 0 {
            tb.push_text(text);
        } else {
            let (family, size) = case.spans[*variant - 1];
            tb.push_style_span(TextStyle {
                font_family: FontFamily::Source(Cow::Borrowed(family)),
                font_size: size,
                text_wrap_mode: if case.spans_nowrap { TextWrapMode::NoWrap } else { TextWrapMode::Wrap },
                ..root_style(case)
            });
            tb.push_text(text);
            tb.pop_style_span();
        }
    }
}

/// Breaks `layout` as Blitz does without floats, resuming if `resume`; returns the lines kept.
fn break_and_align(layout: &mut Layout<Brush>, case: &Case, width: f32, resume: bool) -> usize {
    layout.set_text_indent(case.indent, case.indent_options);
    let mut breaker = layout.break_lines();
    let state = breaker.state_mut();
    state.set_layout_max_advance(width);
    // The first line's geometry differs from the others', as Blitz's can (it comes from the
    // float context): the kept lines must have had both.
    state.set_line_max_advance(width - 3.0);
    state.set_line_x(0.0);
    state.set_line_y(0.0);
    let kept = if resume { breaker.resume(0.0, width) } else { 0 };
    while let Some(yielded) = breaker.break_next() {
        match yielded {
            YieldData::LineBreak(_) => {
                let state = breaker.state_mut();
                state.set_line_x(0.0);
                state.set_line_max_advance(width);
            }
            _ => unreachable!("no boxes and no max height"),
        }
    }
    breaker.finish();
    layout.align(case.alignment, AlignmentOptions::default());
    kept
}

fn build(
    lcx: &mut LayoutContext<Brush>,
    fcx: &mut FontContext,
    case: &Case,
    content: &Content,
    layout: &mut Layout<Brush>,
    keeping_lines: bool,
) {
    let root = root_style(case);
    let mut tb = lcx.tree_builder(fcx, 1.0, true, &root);
    tb.set_base_direction(case.direction);
    push_content(&mut tb, case, content);
    if keeping_lines {
        tb.build_into_keeping_lines(layout);
    } else {
        tb.build_into(layout);
    }
}

fn random_text(rng: &mut Rng, case: &Case, words: usize) -> String {
    let mut text = String::new();
    for _ in 0..words {
        text.push_str(case.words[rng.below(case.words.len())]);
        if case.collapse != WhiteSpaceCollapse::Preserve || rng.below(3) == 0 {
            text.push(' ');
        }
    }
    text
}

/// The largest char boundary of `text` at or below `at`.
fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Applies random edits to the content of one layout and checks each resumed break against a
/// from-scratch one (the whole layout data). Returns the lines kept over all steps.
fn run_case(fcx: &mut FontContext, case: &Case, seed: u64, steps: usize) -> usize {
    let mut lcx = LayoutContext::new();
    // The reference shapes from scratch: its own context, so no build of the other is reused.
    let mut ref_lcx = LayoutContext::new();
    let mut rng = Rng(seed);
    let mut content: Content = vec![(0, random_text(&mut rng, case, 30))];
    let mut layout = Layout::new();
    let mut total_kept = 0;
    for step in 0..steps {
        let last = content.len() - 1;
        // Mostly appends, as when pagination moves content in word by word.
        match rng.below(10) {
            0..=4 => {
                let words = 1 + rng.below(3);
                let more = random_text(&mut rng, case, words);
                content[last].1.push_str(&more);
            }
            5 | 6 => {
                let text = &mut content[last].1;
                let at = floor_char(text, text.len().saturating_sub(1 + rng.below(12)));
                text.truncate(at);
            }
            7 => {
                let segment = rng.below(content.len());
                let text = &mut content[segment].1;
                let at = floor_char(text, rng.below(text.len() + 1));
                let more = random_text(&mut rng, case, 1);
                text.insert_str(at, &more);
            }
            8 if !case.spans.is_empty() => {
                let variant = 1 + rng.below(case.spans.len());
                let more = random_text(&mut rng, case, 2);
                content.push((variant, more));
            }
            _ => {
                let more = random_text(&mut rng, case, 1);
                content[last].1.push_str(&more);
            }
        }
        // Sometimes the width changes: no line can be kept then.
        let width = if rng.below(12) == 0 { case.width * 0.75 } else { case.width };

        build(&mut lcx, fcx, case, &content, &mut layout, true);
        let kept = break_and_align(&mut layout, case, width, true);
        total_kept += kept;

        let mut reference = Layout::new();
        build(&mut ref_lcx, fcx, case, &content, &mut reference, false);
        break_and_align(&mut reference, case, width, false);

        let (got, want) = (format!("{layout:#?}"), format!("{reference:#?}"));
        if got != want {
            let at = got.bytes().zip(want.bytes()).take_while(|(a, b)| a == b).count();
            let from = got[..at].rfind("lines").unwrap_or(at.saturating_sub(400));
            panic!(
                "{} seed {seed} step {step} (kept {kept}): resumed layout differs from scratch\n\
                 resumed: …{}\nscratch: …{}",
                case.name,
                &got[from..(at + 300).min(got.len())],
                &want[from..(at + 300).min(want.len())],
            );
        }
        if case.direction == BaseDirection::Rtl {
            assert_eq!(kept, 0, "{}: RTL text is broken from the start", case.name);
        }
    }
    total_kept
}

fn base_case() -> Case {
    Case {
        name: "normal",
        words: LATIN,
        collapse: WhiteSpaceCollapse::Collapse,
        overflow_wrap: OverflowWrap::Normal,
        spans: &[],
        spans_nowrap: false,
        width: 160.0,
        indent: 0.0,
        indent_options: IndentOptions::default(),
        alignment: Alignment::Start,
        direction: BaseDirection::Ltr,
    }
}

/// Runs `case` from a few seeds; returns the lines kept.
fn check(case: Case) -> usize {
    let mut fcx = FontContext::new();
    [0x9E37_79B9_7F4A_7C15, 0xD1B5_4A32_D192_ED03, 7, 12345]
        .into_iter()
        .map(|seed| run_case(&mut fcx, &case, seed, 60))
        .sum()
}

#[track_caller]
fn check_resumes(case: Case) {
    let name = case.name;
    assert!(check(case) > 0, "{name}: no break resumed, so nothing was tested");
}

#[test]
fn resume_normal_white_space() {
    check_resumes(base_case());
}

#[test]
fn resume_pre_wrap_overflow_wrap_anywhere() {
    check_resumes(Case {
        name: "pre-wrap anywhere",
        words: PRE,
        collapse: WhiteSpaceCollapse::Preserve,
        overflow_wrap: OverflowWrap::Anywhere,
        width: 120.0,
        ..base_case()
    });
}

#[test]
fn resume_justify() {
    check_resumes(Case { name: "justify", alignment: Alignment::Justify, ..base_case() });
}

#[test]
fn resume_text_indent() {
    for (name, indent_options) in [
        ("indent", IndentOptions::default()),
        ("indent each-line", IndentOptions { each_line: true, hanging: false }),
        ("indent hanging", IndentOptions { each_line: true, hanging: true }),
    ] {
        check_resumes(Case {
            name,
            words: PRE,
            collapse: WhiteSpaceCollapse::Preserve,
            indent: 24.0,
            indent_options,
            ..base_case()
        });
    }
}

#[test]
fn resume_mixed_fonts() {
    check_resumes(Case { name: "mixed fonts", spans: &[("serif", 24.0), ("monospace", 11.0)], ..base_case() });
}

#[test]
fn resume_nowrap_spans() {
    check_resumes(Case {
        name: "nowrap spans",
        spans: &[("sans-serif", 16.0), ("serif", 14.0)],
        spans_nowrap: true,
        ..base_case()
    });
}

#[test]
fn resume_emoji_fallback() {
    check_resumes(Case { name: "emoji", words: EMOJI, ..base_case() });
}

#[test]
fn resume_rtl_takes_the_fallback() {
    check(Case { name: "rtl", words: ARABIC, direction: BaseDirection::Rtl, ..base_case() });
}

#[test]
fn resume_bidi_auto() {
    check(Case { name: "bidi auto", words: ARABIC, direction: BaseDirection::Auto, ..base_case() });
}

/// A style change that keeps every run boundary (letter spacing here) keeps no lines.
#[test]
fn resume_after_a_style_change_keeps_no_lines() {
    let case = base_case();
    let mut fcx = FontContext::new();
    let mut lcx = LayoutContext::new();
    let content: Content = vec![(0, random_text(&mut Rng(99), &case, 60))];
    let mut layout = Layout::new();
    build(&mut lcx, &mut fcx, &case, &content, &mut layout, true);
    break_and_align(&mut layout, &case, case.width, true);
    assert!(layout.len() > 4);

    // The same text again: all lines are kept up to the one after which the breaker read the
    // end of the text (the last, or the one before when its overflowing atom ended the text).
    build(&mut lcx, &mut fcx, &case, &content, &mut layout, true);
    let kept = break_and_align(&mut layout, &case, case.width, true);
    assert!((layout.len() - 2..layout.len()).contains(&kept), "kept {kept} of {}", layout.len());

    let root = TextStyle { letter_spacing: 1.0, ..root_style(&case) };
    let mut tb = lcx.tree_builder(&mut fcx, 1.0, true, &root);
    push_content(&mut tb, &case, &content);
    tb.build_into_keeping_lines(&mut layout);
    assert_eq!(break_and_align(&mut layout, &case, case.width, true), 0);
}

mod document {
    //! The same through Blitz: a paragraph's text changes and the persistent document is laid
    //! out again; boxes and pixels must match a document laid out from scratch.
    use super::Rng;
    use crate::incremental_construction::{paint, Page};
    use pyro_render::*;

    const WORDS: &[&str] = &[
        "lorem",
        "ipsum",
        "dolor",
        "sit",
        "amet,",
        "consectetur",
        "adipiscing",
        "elit.",
        "a",
        "Pellentesque",
        "habitant",
        "morbi",
        "supercalifragilisticexpialidocious",
        "fi",
        "office",
    ];

    /// Edits the text of `#p` in `html` at random; returns the lines kept by resumed breaks.
    #[track_caller]
    fn check(what: &str, html: &str, seed: u64) -> u64 {
        let page = Page::new(html);
        let mut live = Renderer::new();
        paint(&mut live, &page.snapshot());
        let node = page.text("#p");
        let mut text = page.dom.borrow().text_content(node).unwrap().unwrap();
        let mut rng = Rng(seed);
        let mut kept = 0;
        for step in 0..40 {
            match rng.below(6) {
                0..=2 => {
                    for _ in 0..1 + rng.below(3) {
                        text.push(' ');
                        text.push_str(WORDS[rng.below(WORDS.len())]);
                    }
                }
                3 | 4 => text.truncate(text.len().saturating_sub(1 + rng.below(15)).max(1)),
                _ => {
                    let at = rng.below(text.len() + 1);
                    text.insert_str(at, &format!("{} ", WORDS[rng.below(WORDS.len())]));
                }
            }
            page.dom.borrow_mut().set_text_content(node, &text).unwrap();
            let snap = page.snapshot();
            let before = blitz_line_breaks();
            let incremental = paint(&mut live, &snap);
            kept += blitz_line_breaks().1 - before.1;
            let mut fresh = Renderer::new();
            let scratch = paint(&mut fresh, &snap);
            let (a, b) = (live.mirrored_layout(1), fresh.mirrored_layout(1));
            assert_eq!(a, b, "{what} step {step}: boxes differ");
            assert_eq!(incremental.hash, scratch.hash, "{what} step {step}: pixels differ");
        }
        kept
    }

    fn doc(p_style: &str, before: &str) -> String {
        format!(
            "<!doctype html><style>body{{margin:0;font:14px sans-serif}} #p{{width:180px;{p_style}}}</style>\
             {before}<p id=p>{}</p><p>after</p>",
            WORDS.iter().cycle().take(40).cloned().collect::<Vec<_>>().join(" ")
        )
    }

    #[test]
    fn a_paragraph_edited_at_random_resumes_and_matches_a_fresh_layout() {
        for (what, style, expect_resumes) in [
            ("normal", "", true),
            ("pre-wrap anywhere", "white-space:pre-wrap;overflow-wrap:anywhere", true),
            ("text-indent", "text-indent:30px", true),
            ("centered", "text-align:center", true),
            ("justify", "text-align:justify", false),
            ("rtl", "direction:rtl", false),
            // The first line is wider than the others: a word broken over several lines may
            // fit on the first line again once shortened.
            ("negative indent anywhere", "text-indent:-150px;padding-left:150px;overflow-wrap:anywhere", true),
            ("hanging indent anywhere", "text-indent:120px hanging;overflow-wrap:anywhere", true),
        ] {
            let kept: u64 = [3, 99].into_iter().map(|seed| check(what, &doc(style, ""), seed)).sum();
            assert_eq!(kept > 0, expect_resumes, "{what}: lines kept {kept}");
        }
    }

    /// A long word that did not fit beside the first line's text is broken over the narrower
    /// lines after it; shortened, it fits on the first line again.
    #[test]
    fn a_shortened_word_moves_back_to_a_wider_first_line() {
        for style in [
            "text-indent:-300px;padding-left:300px;overflow-wrap:anywhere",
            "text-indent:150px hanging;overflow-wrap:anywhere",
        ] {
            let html = format!(
                "<!doctype html><style>body{{margin:0;font:14px sans-serif}} #p{{width:180px;{style}}}</style>\
                 <p id=p>a {}</p><p>after</p>",
                "x".repeat(120)
            );
            let page = Page::new(&html);
            let mut live = Renderer::new();
            paint(&mut live, &page.snapshot());
            let node = page.text("#p");
            let mut kept = 0;
            for n in (1..120).rev() {
                let text = format!("a {}", "x".repeat(n));
                page.dom.borrow_mut().set_text_content(node, &text).unwrap();
                let snap = page.snapshot();
                let before = blitz_line_breaks().1;
                let incremental = paint(&mut live, &snap);
                kept += blitz_line_breaks().1 - before;
                let mut fresh = Renderer::new();
                let scratch = paint(&mut fresh, &snap);
                assert_eq!(live.mirrored_layout(1), fresh.mirrored_layout(1), "{style}, {n} x: boxes differ");
                assert_eq!(incremental.hash, scratch.hash, "{style}, {n} x: pixels differ");
            }
            assert!(kept > 0, "{style}: nothing resumed");
        }
    }

    #[test]
    fn a_paragraph_beside_a_float_is_broken_from_the_start() {
        let html = doc("", "<div style='float:left;width:60px;height:300px'></div>");
        assert_eq!(check("float", &html, 5), 0);
    }
}

/// Builds `first`, breaks it, builds each of `then` keeping lines, and checks each resumed break
/// against a from-scratch one; returns the lines kept.
fn check_sequence(case: &Case, first: &str, then: &[String]) -> usize {
    let mut fcx = FontContext::new();
    let mut lcx = LayoutContext::new();
    let mut ref_lcx = LayoutContext::new();
    let mut layout = Layout::new();
    build(&mut lcx, &mut fcx, case, &vec![(0, first.to_string())], &mut layout, true);
    break_and_align(&mut layout, case, case.width, true);
    let mut total = 0;
    for text in then {
        let content: Content = vec![(0, text.clone())];
        build(&mut lcx, &mut fcx, case, &content, &mut layout, true);
        total += break_and_align(&mut layout, case, case.width, true);
        let mut reference = Layout::new();
        build(&mut ref_lcx, &mut fcx, case, &content, &mut reference, false);
        break_and_align(&mut reference, case, case.width, false);
        let lines = |l: &Layout<Brush>| {
            l.lines().map(|line| line.text_range()).collect::<Vec<_>>()
        };
        assert_eq!(lines(&layout), lines(&reference), "{}: {text:?}", case.name);
        assert_eq!(format!("{layout:#?}"), format!("{reference:#?}"), "{}: {text:?}", case.name);
    }
    total
}

/// When a later line is narrower than an earlier one (a hanging or negative `text-indent`, or
/// `each-line` after a forced break), the atom that overflowed a line can lie many lines on: a
/// long word that did not fit beside the first line's text is broken over several narrow lines.
/// Shortening that word can let it fit on the first line again, so those lines cannot be kept.
#[test]
fn resume_when_later_lines_are_narrower_than_the_line_they_follow() {
    let word = |n: usize| format!("a {}", "x".repeat(n));
    for (name, indent, indent_options, collapse) in [
        ("hanging", 70.0, IndentOptions { each_line: false, hanging: true }, WhiteSpaceCollapse::Collapse),
        ("negative", -140.0, IndentOptions::default(), WhiteSpaceCollapse::Collapse),
    ] {
        let case = Case {
            name,
            overflow_wrap: OverflowWrap::Anywhere,
            collapse,
            width: 100.0,
            indent,
            indent_options,
            ..base_case()
        };
        let then: Vec<String> = (1..60).rev().map(word).collect();
        assert!(check_sequence(&case, &word(80), &then) > 0, "{name}: nothing resumed");
    }
    // `each-line`: the line after a forced break is the narrow one.
    let case = Case {
        name: "each-line",
        overflow_wrap: OverflowWrap::Anywhere,
        collapse: WhiteSpaceCollapse::Preserve,
        width: 100.0,
        indent: 70.0,
        indent_options: IndentOptions { each_line: true, hanging: false },
        ..base_case()
    };
    let text = |n: usize| format!("b\na {}", "x".repeat(n));
    let then: Vec<String> = (1..60).rev().map(text).collect();
    check_sequence(&case, &text(80), &then);
}
