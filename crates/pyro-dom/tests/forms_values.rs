//! Input value sanitization, maxlength on insertion, numeric values and
//! stepping, and the constraint-validation flags that depend on them
//! (html/semantics/forms/the-input-element/{range,date,time-2,week,
//! datetime-local,maxlength,input-stepup,input-valueasdate}.html,
//! constraints/form-validation-validity-*.html).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

fn el(dom: &Dom, d: NodeId, id: &str) -> NodeId {
    dom.get_element_by_id(d, id).unwrap().unwrap()
}

#[test]
fn maxlength_truncates_insertion() {
    let (dom, d) =
        doc(r#"<input id=t maxlength=4><textarea id=ta maxlength=3></textarea><input id=n type=number maxlength=2>
        <input id=e maxlength=3>"#);
    let mut dom = dom.borrow_mut();
    let t = el(&dom, d, "t");
    assert!(dom.insert_text(t, "12345678").unwrap());
    assert_eq!(dom.value(t).unwrap(), "1234");
    assert_eq!(dom.selection(t).unwrap(), (4, 4));
    assert!(!dom.insert_text(t, "x").unwrap(), "no room left");
    // Replacing a selection frees its room.
    dom.set_selection_range(t, 1, 3).unwrap();
    assert!(dom.insert_text(t, "abc").unwrap());
    assert_eq!(dom.value(t).unwrap(), "1ab4");
    let ta = el(&dom, d, "ta");
    assert!(dom.insert_text(ta, "ab😀").unwrap());
    assert_eq!(dom.value(ta).unwrap(), "ab", "a surrogate pair is not split");
    // maxlength does not apply to a number input.
    let n = el(&dom, d, "n");
    assert!(dom.insert_text(n, "12345").unwrap());
    assert_eq!(dom.value(n).unwrap(), "12345");
    // A script-set value over the limit is kept; typing adds nothing.
    let e = el(&dom, d, "e");
    dom.set_value(e, "abcdef").unwrap();
    assert!(!dom.insert_text(e, "g").unwrap());
    assert_eq!(dom.value(e).unwrap(), "abcdef");
}

#[test]
fn range_value_sanitization() {
    let (dom, d) = doc(
        r#"<input type=range id=empty><input type=range id=mid min=2 max=6><input type=range id=step min=0 max=7 step=2>
        <input type=range id=rev min=2 max=-3><input type=range id=over min=0 max=5 value=7>
        <input type=range id=bad min=0 max=5 value=ppp step=xyz><input type=range id=f min=5.3 max=12 step=0.5 value=6.7>
        <input type=range id=s2 min=5.3 max=12 value=6.7><input type=range id=ws value=" 123">"#,
    );
    let mut dom = dom.borrow_mut();
    let v = |dom: &Dom, id: &str| dom.value(el(dom, d, id)).unwrap();
    assert_eq!(v(&dom, "empty"), "50");
    assert_eq!(v(&dom, "mid"), "4");
    assert_eq!(v(&dom, "step"), "4", "a tie snaps up");
    assert_eq!(v(&dom, "rev"), "2");
    assert_eq!(v(&dom, "over"), "5");
    assert_eq!(v(&dom, "bad"), "3");
    assert_eq!(v(&dom, "f"), "6.8");
    assert_eq!(v(&dom, "s2"), "6.3");
    assert_eq!(v(&dom, "ws"), "50");
    // Re-sanitized when the attributes change.
    let over = el(&dom, d, "over");
    dom.set_attribute(over, "max", "3").unwrap();
    assert_eq!(dom.value(over).unwrap(), "3");
    dom.set_value(over, "-4").unwrap();
    assert_eq!(dom.value(over).unwrap(), "0");
    dom.step_value(over, 2.0, true).unwrap();
    assert_eq!(dom.value(over).unwrap(), "2");
}

#[test]
fn date_and_time_sanitization() {
    let (dom, d) =
        doc(r#"<input id=d type=date value=garbage><input id=t type=time><input id=w type=week><input id=m type=month>
        <input id=l type=datetime-local value="2022-04-19T12:34:56.010">"#);
    let mut dom = dom.borrow_mut();
    let d_ = el(&dom, d, "d");
    assert_eq!(dom.value(d_).unwrap(), "");
    for (value, expected) in [("2016-02-29", "2016-02-29"), ("2014-02-29", ""), ("2014-1-01", "")] {
        dom.set_value(d_, value).unwrap();
        assert_eq!(dom.value(d_).unwrap(), expected, "{value}");
    }
    let t = el(&dom, d, "t");
    for (value, expected) in [("00:00:00.0", "00:00:00.0"), ("00:00:00.0000", ""), ("24:00", "")] {
        dom.set_value(t, value).unwrap();
        assert_eq!(dom.value(t).unwrap(), expected, "{value}");
    }
    let w = el(&dom, d, "w");
    for (value, expected) in [("2015-W53", "2015-W53"), ("2014-W53", ""), ("2017-w52", "")] {
        dom.set_value(w, value).unwrap();
        assert_eq!(dom.value(w).unwrap(), expected, "{value}");
    }
    let m = el(&dom, d, "m");
    dom.set_value(m, "2019-13").unwrap();
    assert_eq!(dom.value(m).unwrap(), "");
    let l = el(&dom, d, "l");
    assert_eq!(dom.value(l).unwrap(), "2022-04-19T12:34:56.01");
    dom.set_value(l, "2014-01-01 00:00:00.000").unwrap();
    assert_eq!(dom.value(l).unwrap(), "2014-01-01T00:00");
}

#[test]
fn numeric_values_and_stepping() {
    let (dom, d) = doc(
        r#"<input id=d type=date value=2019-12-10><input id=m type=month value=1970-03><input id=t type=time value=12:00>
        <input id=w type=week><input id=n type=number value=0.1 step=0.1><input id=x type=text>
        <input id=c type=number min=10 max=5 value=0><input id=k type=number step=3 max=7 value=0>
        <input id=a type=number step=any>"#,
    );
    let mut dom = dom.borrow_mut();
    let g = |dom: &Dom, id: &str| el(dom, d, id);
    let date = g(&dom, "d");
    assert_eq!(dom.value_as_number(date).unwrap(), 1_575_936_000_000.0);
    assert_eq!(dom.value_as_date(date).unwrap(), Some(1_575_936_000_000.0));
    dom.step_value(date, 1.0, true).unwrap();
    assert_eq!(dom.value(date).unwrap(), "2019-12-11");
    let month = g(&dom, "m");
    assert_eq!(dom.value_as_number(month).unwrap(), 2.0);
    dom.set_value_as_date(month, Some(1_575_158_400_000.0)).unwrap();
    assert_eq!(dom.value(month).unwrap(), "2019-12");
    let time = g(&dom, "t");
    dom.set_attribute(time, "step", "0.001").unwrap();
    dom.step_value(time, 1.0, false).unwrap();
    assert_eq!(dom.value(time).unwrap(), "11:59:59.999");
    let week = g(&dom, "w");
    dom.step_value(week, 1.0, true).unwrap();
    let first = dom.value_as_number(week).unwrap();
    dom.step_value(week, 1.0, true).unwrap();
    assert_eq!(dom.value_as_number(week).unwrap(), first + 604_800_000.0);
    assert_eq!(dom.value(week).unwrap(), "1970-W03");
    let n = g(&dom, "n");
    dom.step_value(n, 2.0, true).unwrap();
    assert_eq!(dom.value(n).unwrap(), "0.3", "decimal stepping");
    let c = g(&dom, "c");
    dom.step_value(c, 1.0, true).unwrap();
    assert_eq!(dom.value(c).unwrap(), "0", "min > max: no change");
    let k = g(&dom, "k");
    dom.step_value(k, 3.0, true).unwrap();
    assert_eq!(dom.value(k).unwrap(), "6", "clamped to the largest step under max");
    let x = g(&dom, "x");
    assert_eq!(dom.step_value(x, 1.0, true).unwrap_err().kind(), Some(DomErrorKind::InvalidState));
    assert!(dom.value_as_number(x).unwrap().is_nan());
    assert_eq!(dom.value_as_date(x).unwrap(), None);
    let a = g(&dom, "a");
    assert_eq!(dom.step_value(a, 1.0, true).unwrap_err().kind(), Some(DomErrorKind::InvalidState));
}

#[test]
fn pattern_step_bad_input_and_ranges() {
    let (dom, d) = doc(r#"<input id=p pattern="[0-9]+" value=abc><input id=pok pattern="[0-9]+" value=123>
        <input id=pv pattern="[\p{L}--[a-z]]+" value=ABC><input id=pbad pattern="(" value=x>
        <input id=pe type=email multiple pattern="[a-z]+@x\.com" value="a@x.com, b@y.com">
        <input id=s type=number><input id=sv type=number value=1.5><input id=s2 type=number step=0.1 value=0.3>
        <input id=b type=number><input id=dt type=date min=2020-01-02 value=2020-01-01>
        <input id=tr type=time min=22:00 max=02:00 value=12:00><input id=tm type=time min=22:00 max=02:00 value=23:00>
        <input id=e type=email value="a@b"><input id=e2 type=email value="a@-b.com"><input id=u type=url value="nope">
        <input id=u2 type=url value="https://x.test/">"#);
    let mut dom = dom.borrow_mut();
    let v = |dom: &Dom, id: &str| dom.validity(el(dom, d, id)).unwrap();
    assert!(v(&dom, "p").pattern_mismatch);
    assert!(v(&dom, "pok").valid());
    assert!(v(&dom, "pv").valid(), "the v flag: set subtraction");
    assert!(v(&dom, "pbad").valid(), "an invalid pattern is ignored");
    assert!(v(&dom, "pe").pattern_mismatch, "each address of a multiple email");
    let s = el(&dom, d, "s");
    dom.set_value(s, "1.5").unwrap();
    assert!(v(&dom, "s").step_mismatch);
    assert!(v(&dom, "sv").valid(), "the value attribute is the step base");
    assert!(v(&dom, "s2").valid());
    assert!(v(&dom, "dt").range_underflow);
    let tr = v(&dom, "tr");
    assert!(tr.range_underflow && tr.range_overflow, "outside a reversed time range");
    assert!(v(&dom, "tm").valid());
    assert!(v(&dom, "e").valid());
    assert!(v(&dom, "e2").type_mismatch);
    assert!(v(&dom, "u").type_mismatch);
    assert!(v(&dom, "u2").valid());
    // Typed text that is not a number: empty value, bad input.
    let b = el(&dom, d, "b");
    assert!(dom.insert_text(b, "1e").unwrap());
    assert_eq!(dom.value(b).unwrap(), "");
    assert!(v(&dom, "b").bad_input);
    assert!(dom.insert_text(b, "5").unwrap());
    assert_eq!(dom.value(b).unwrap(), "1e5");
    assert!(v(&dom, "b").valid());
}

#[test]
fn length_constraints_need_a_user_edit_and_a_text_state() {
    let (dom, d) = doc(r#"<input id=t minlength=3 value=ab><input id=n type=number minlength=3>"#);
    let mut dom = dom.borrow_mut();
    let t = el(&dom, d, "t");
    assert!(dom.validity(t).unwrap().valid(), "no user edit yet");
    dom.insert_text(t, "c").unwrap();
    dom.set_selection_range(t, 2, 3).unwrap();
    dom.insert_text(t, "").unwrap();
    assert!(dom.validity(t).unwrap().too_short);
    let n = el(&dom, d, "n");
    dom.insert_text(n, "1").unwrap();
    assert!(dom.validity(n).unwrap().valid(), "minlength does not apply to numbers");
}
