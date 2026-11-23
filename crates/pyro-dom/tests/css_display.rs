use pyro_dom::stylo::declarations::DeclarationBlock;
use style::values::specified::box_::Display;
use style_traits::ToCss;

fn specified(value: &str) -> String {
    DeclarationBlock::parse(&format!("display: {value}")).value("display")
}

#[test]
fn display_ruby_and_ruby_text_parse_and_serialize_as_blink() {
    // WPT css-display parsing/display-valid.html, as Chrome 158 (ruby-base and the ruby
    // containers stay invalid there).
    for (value, expected) in [
        ("ruby", "ruby"),
        ("inline ruby", "ruby"),
        ("ruby inline", "ruby"),
        ("block ruby", "block ruby"),
        ("ruby block", "block ruby"),
        ("ruby-text", "ruby-text"),
        ("RUBY", "ruby"),
        ("Ruby-Text", "ruby-text"),
        // Unchanged neighbours.
        ("run-in", "run-in"),
        ("inline", "inline"),
        ("block", "block"),
        ("inline list-item", "inline list-item"),
    ] {
        assert_eq!(specified(value), expected, "{value}");
    }
    // WPT display-invalid.html and Chrome's rejections.
    for value in [
        "ruby-base",
        "ruby-base-container",
        "ruby-text-container",
        "run-in ruby",
        "ruby run-in",
        "ruby list-item",
        "inline ruby list-item",
        "ruby ruby",
        "ruby flow",
        "flow ruby",
        "ruby flow-root",
        "flex ruby",
        "grid ruby",
        "table ruby",
        "none ruby",
        "ruby-text block",
        "math ruby",
    ] {
        assert_eq!(specified(value), "", "{value}");
    }
}

#[test]
fn display_ruby_is_an_inline_or_block_flow_box_to_layout() {
    // Layout sees the box the outside keyword says; only serialization differs.
    for (display, outside, inline_flow) in [
        (Display::PyroRuby, Display::Inline, true),
        (Display::PyroBlockRuby, Display::Block, false),
        (Display::PyroRubyText, Display::Inline, true),
    ] {
        assert_eq!(display.without_ruby(), outside);
        assert_eq!(display.outside(), outside.outside());
        assert_eq!(display.inside(), outside.inside());
        assert_eq!(display.is_inline_flow(), inline_flow);
        assert!(!display.is_list_item() && !display.is_run_in());
    }
    // Blockification, as Chrome: `ruby` → `block ruby`, `ruby-text` → `block`.
    let css = |d: Display| d.to_css_string();
    assert_eq!(css(Display::PyroRuby.equivalent_block_display(false)), "block ruby");
    assert_eq!(css(Display::PyroRuby.equivalent_block_display(true)), "block ruby");
    assert_eq!(css(Display::PyroBlockRuby.equivalent_block_display(false)), "block ruby");
    assert_eq!(css(Display::PyroRubyText.equivalent_block_display(false)), "block");
    assert_eq!(css(Display::RunIn), "run-in");
}
