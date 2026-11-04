//! Auto table width hints and nested editable flex controls.
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;

const SOURCE: &str = r#"<!doctype html><style>
        *{box-sizing:border-box}body{margin:0;font:13px Arial}
        table{table-layout:auto;width:800px;border-spacing:0}
        th{width:25px;min-width:25px}td{padding:2px 4px;border:1px solid;min-width:0;overflow:hidden}
        #buy{width:224px}.rate{display:flex;gap:8px;min-width:0}
        .amount{display:flex;flex:1;min-width:0;padding:3px;border:1px solid}
        input{width:100%;min-width:0;padding:0;border:0}.currency{width:96px;min-width:96px;flex-shrink:0}
        </style><table><thead><tr><th>Charge/Fee</th><th>Additional Text</th><th>Quantity</th><th>Buy Rate</th><th>Exchange Rate</th><th>VAT Rule</th><th></th></tr></thead>
        <tbody><tr><td>Freight</td><td>Text</td><td>1</td><td id=buy><div class=rate><div class=amount><input id=amount value=80.00></div><div class=currency>EUR</div></div></td><td>1</td><td>Standard</td><td></td></tr></tbody></table>"#;

fn measured_widths(source: &str) -> (f32, f32, f32, f32) {
    let mut doc = HtmlDocument::from_html(source, DocumentConfig::default());
    doc.viewport_mut().window_size = (1920, 1080);
    doc.resolve(0.0);
    let width = |selector: &str| {
        let id = doc.query_selector(selector).unwrap().unwrap();
        doc.get_node(id).unwrap().unrounded_layout().size.width
    };
    let sum = doc
        .query_selector_all("tbody td")
        .unwrap()
        .iter()
        .map(|&id| doc.get_node(id).unwrap().unrounded_layout().size.width)
        .sum();
    (width("#buy"), width("#amount"), width("table"), sum)
}

#[test]
fn auto_table_body_width_hint_keeps_nested_amount_input_usable() {
    let (cell, input, _, _) = measured_widths(SOURCE);
    eprintln!("auto table body224: cell={cell}, amount input={input}");
    assert!(cell >= 224.0, "specified body width floor: {cell}");
    assert!(input > 0.0, "fixed currency must leave the editable amount usable");
}

#[test]
fn auto_table_inline_small_width_hints_preserve_flexible_amount_intrinsic_space() {
    // Table sets width/min-width inline on body and header cells; this wins
    // over the Buy Rate's class width. Other text has large max-content but
    // can wrap, so a narrow minimum alone is not its preferred width.
    let html=SOURCE.replace("width:800px","width:1808px")
        .replace("<td id=buy>","<td id=buy style='width:25px;min-width:25px'>")
        .replace("<td>","<td style='width:25px;min-width:25px'>")
        .replace("<td style='width:25px;min-width:25px'>Text</td>","<td style='width:25px;min-width:25px'>Additional shipping information with many words that can wrap across the available column width and continue onto another line</td>")
        .replace("<td style='width:25px;min-width:25px'>Freight</td>","<td style='width:25px;min-width:25px'>Ocean freight and associated transport service charges</td>");
    let (cell, input, table, sum) = measured_widths(&html);
    eprintln!(
        "auto table inline25 competing text: table={table}, cell sum={sum}, rate cell={cell}, amount input={input}"
    );
    assert!(
        (sum - table).abs() < 0.1,
        "auto columns must allocate the table's available width: columns={sum}, table={table}"
    );
    assert!(
        input > 0.0,
        "auto allocation must retain editable amount space beside the fixed96px currency: cell={cell}"
    );
}

#[test]
fn fixed_table_still_uses_first_row_hints_instead_of_body_widths() {
    let html = SOURCE.replace("table-layout:auto", "table-layout:fixed").replace("width:800px", "width:700px");
    let (cell, _, table, sum) = measured_widths(&html);
    assert!((table - 700.0).abs() < 0.1);
    assert!((sum - table).abs() < 0.1);
    assert!((cell - 100.0).abs() < 0.1, "fixed seven equal header hints must keep equal allocation: {cell}");
}

#[test]
fn all_auto_headers_keep_existing_body_hint_intrinsic_allocation() {
    // Fixed dimensions distinguish min-content100 from max-content200
    // without glyph metrics. This previously supported path keeps its
    // original capped body-hint preference, outside all-hinted allocation.
    let html = r#"<!doctype html><style>
      *{box-sizing:border-box}body{margin:0}table{table-layout:auto;width:600px;border-spacing:0}
      th{width:auto;min-width:0}td{padding:0;border:0;min-width:0}
      #buy{width:100px}.wrap{display:flex;flex-wrap:wrap}.wrap>div{flex:0 0 100px;width:100px;height:10px}
    </style><table><thead><tr><th></th><th></th></tr></thead><tbody><tr><td id=buy><div class=wrap><div></div><div></div></div></td><td id=amount><div style='width:200px;height:10px'></div></td></tr></tbody></table>"#;
    let (first, second, table, sum) = measured_widths(html);
    assert!((table - 600.0).abs() < 0.1);
    assert!((sum - table).abs() < 0.1);
    assert!((first - 200.0).abs() < 0.1, "existing body-hint allocation must stay unchanged: {first}");
    assert!((second - 400.0).abs() < 0.1, "existing companion allocation must stay unchanged: {second}");
}

#[test]
fn vertical_hinted_table_keeps_existing_logical_axis_grid_fallback() {
    let mut doc = HtmlDocument::from_html(
        r#"<!doctype html><style>*{box-sizing:border-box}body{margin:0}
        table{writing-mode:vertical-rl;table-layout:auto;width:200px;height:600px;border-spacing:0}
        th,td{width:25px;height:25px;min-width:25px;min-height:25px;padding:0;border:0}
        </style><table><thead><tr><th></th><th></th></tr></thead><tbody><tr><td id=a></td><td id=b></td></tr></tbody></table>"#,
        DocumentConfig::default(),
    );
    doc.viewport_mut().window_size = (1920, 1080);
    doc.resolve(0.0);
    let height = |selector: &str| {
        let id = doc.query_selector(selector).unwrap().unwrap();
        doc.get_node(id).unwrap().final_layout().size.height
    };
    assert!((height("table") - 600.0).abs() < 0.1);
    assert!((height("#a") - 25.0).abs() < 0.1, "vertical hints retain the original grid allocation");
    assert!((height("#b") - 25.0).abs() < 0.1, "horizontal surplus allocation must not be applied to vertical tracks");
}

/// Column widths of a 576 px wide auto table, from where Chrome 156 prints each cell's text
/// (`bench/pdf/fixtures/chrome-156/tcell-x.json`): a percentage is a share of the table, even of an empty cell.
#[test]
fn percentage_cells_of_auto_tables_take_their_share() {
    let td = |width: &str, text: &str| format!(r#"<td style="padding:0;width:{width}">{text}</td>"#);
    for (cells, chrome) in [
        (td("50%", "") + &td("auto", "a1"), &[288.0, 288.0][..]),
        (td("50%", " ") + &td("auto", "a1"), &[288.0, 288.0][..]),
        (td("50%", "a1") + &td("auto", "b1"), &[288.0, 288.0][..]),
        (td("30%", "a1") + &td("20%", "b1") + &td("auto", "c1"), &[172.8, 115.2, 288.0][..]),
        (td("50%", "a1") + &td("100px", "b1") + &td("auto", "c1"), &[288.0, 100.0, 188.0][..]),
    ] {
        let html = format!(
            r#"<!doctype html><body style="margin:0;font:12px/16px Arial"><table style="width:576px;border-spacing:0"><tr>{cells}</tr></table>"#
        );
        let mut doc = HtmlDocument::from_html(&html, DocumentConfig::default());
        doc.viewport_mut().window_size = (800, 600);
        doc.resolve(0.0);
        let widths: Vec<f32> = doc
            .query_selector_all("td")
            .unwrap()
            .iter()
            .map(|&id| doc.get_node(id).unwrap().unrounded_layout().size.width)
            .collect();
        assert_eq!(widths.len(), chrome.len(), "{cells}");
        for (got, want) in widths.iter().zip(chrome) {
            assert!((got - want).abs() < 0.2, "{cells}: {widths:?}, Chrome {chrome:?}");
        }
    }
}

/// A flex item's nowrap label beside a `width: 100%` value, as in the ToS invoice's header: the table keeps its flex share.
#[test]
fn a_full_width_cell_does_not_widen_a_table_that_is_a_flex_item() {
    let html = r#"<!doctype html><body style="margin:0;font:12px/16px Arial"><div style="display:flex;width:600px"><div style="flex:1">left</div><table id=t style="flex:1;align-self:flex-start;border-collapse:collapse"><tr><td id=l style="padding:0 22px 0 0;white-space:nowrap">Datum</td><td id=v style="padding:0;width:100%">value</td></tr></table></div>"#;
    let mut doc = HtmlDocument::from_html(html, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    let width = |s: &str| doc.get_node(doc.query_selector(s).unwrap().unwrap()).unwrap().unrounded_layout().size.width;
    assert!((width("#t") - 300.0).abs() < 0.5, "table {}", width("#t"));
    assert!(width("#l") < 80.0, "label {}", width("#l"));
}
