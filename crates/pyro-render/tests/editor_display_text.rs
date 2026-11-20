//! pyro: a text control's editor showing its value in a `text-transform`
//! (`parley::PlainEditor::set_display_text`) hit-tests and moves on the text it shows, and maps
//! positions back to its value (`ß` shown as `SS`).

use std::borrow::Cow;

use parley::editing::Cursor;
use parley::layout::Affinity;
use parley::{FontContext, FontFamily, LayoutContext, PlainEditor, StyleProperty};

type Brush = [u8; 4];

/// `text` and its upper-case display text, character by character, with the offset map.
fn uppercase(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut shown = String::new();
    let mut map = Vec::new();
    for (index, c) in text.char_indices() {
        map.push((index, shown.len()));
        shown.extend(c.to_uppercase());
    }
    map.push((text.len(), shown.len()));
    (shown, map)
}

fn editor(text: &str, width: Option<f32>) -> (PlainEditor<Brush>, FontContext, LayoutContext<Brush>) {
    let mut font_cx = FontContext::new();
    let mut layout_cx = LayoutContext::new();
    let mut editor = PlainEditor::new(20.0);
    editor.set_text(text);
    editor.set_width(width);
    editor.edit_styles().insert(StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed("sans-serif"))));
    editor.refresh_layout(&mut font_cx, &mut layout_cx);
    editor.set_display_text(Some(uppercase(text)), &mut font_cx, &mut layout_cx);
    (editor, font_cx, layout_cx)
}

/// Where the displayed text's byte offset `index` is drawn (the middle of its line).
fn point(editor: &PlainEditor<Brush>, index: usize) -> (f32, f32) {
    let layout = editor.try_layout().unwrap();
    let rect = Cursor::from_byte_index(layout, index, Affinity::Downstream).geometry(layout, 0.0);
    (rect.x0 as f32, ((rect.y0 + rect.y1) / 2.0) as f32)
}

#[test]
fn a_click_on_the_displayed_text_goes_to_the_value_position_it_shows() {
    let (mut editor, mut font_cx, mut layout_cx) = editor("ßx", None);
    assert_eq!(editor.try_layout().unwrap().lines().next().unwrap().text_range(), 0..3, "the layout shown is `SSX`");
    // Just after the displayed `SS`: after `ß` (2 bytes), not after `x` as on the value's layout.
    let (x, y) = point(&editor, 2);
    editor.driver(&mut font_cx, &mut layout_cx).move_to_point(x + 0.5, y);
    assert_eq!(editor.raw_selection().focus().index(), 2);
    // A quarter of the way into `SS`: the nearer edge of `ß`.
    let (start, _) = point(&editor, 0);
    editor.driver(&mut font_cx, &mut layout_cx).move_to_point(start + (x - start) * 0.25, y);
    assert_eq!(editor.raw_selection().focus().index(), 0);
}

#[test]
fn arrows_cross_what_one_character_is_shown_as() {
    let (mut editor, mut font_cx, mut layout_cx) = editor("ßx", None);
    let mut driver = editor.driver(&mut font_cx, &mut layout_cx);
    driver.move_to_text_start();
    driver.move_right();
    assert_eq!(editor.raw_selection().focus().index(), 2, "past `SS` at once");
    editor.driver(&mut font_cx, &mut layout_cx).move_right();
    assert_eq!(editor.raw_selection().focus().index(), 3);
    editor.driver(&mut font_cx, &mut layout_cx).select_left();
    let selection = editor.raw_selection();
    assert_eq!((selection.anchor().index(), selection.focus().index()), (3, 2));
}

#[test]
fn vertical_movement_follows_the_displayed_lines() {
    // Upper case is wider: at about half the width of the text shown on one line, it wraps where
    // the value's lines do not.
    let text = "ßß ßß ßß ßß ßß ßß ßß ßß";
    let one_line = editor(text, None).0.try_layout().unwrap().width();
    let (mut editor, mut font_cx, mut layout_cx) = editor(text, Some(one_line * 0.55));
    let shown = editor.try_layout().unwrap();
    assert!(shown.len() >= 2, "the text shown wraps");
    let second_line = shown.lines().nth(1).unwrap().text_range().start;
    let mut driver = editor.driver(&mut font_cx, &mut layout_cx);
    driver.move_to_text_start();
    driver.move_down();
    // On the second line shown, at its start: the value offset of what it starts with.
    let (_, map) = uppercase(text);
    let expected = map.iter().find(|&&(_, at)| at == second_line).map(|&(value, _)| value).unwrap();
    assert_eq!(editor.raw_selection().focus().index(), expected);
}
