//! XPath engine tests. The conformance battery is ported from upstream
//! src/browser/tests/xpath/xpath_conformance.html (lightpanda-io/browser
//! @ 2ad38773); the other cases port document_evaluate.html,
//! xpath_evaluator.html (#deep_nesting) and the functions.zig unit tests.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::xpath::{evaluate, XPathError, XPathExpression, XPathNode, XPathValue};
use lp_dom::*;

fn page(html: &str) -> (Dom, NodeId) {
    let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(&dom_ref, "http://test/", html);
    let dom = Rc::try_unwrap(dom_ref).ok().expect("sole owner").into_inner();
    (dom, doc)
}

fn nodes(dom: &Dom, ctx: NodeId, expr: &str) -> Vec<XPathNode> {
    match evaluate(dom, expr, XPathNode::Node(ctx)) {
        Ok(XPathValue::NodeSet(v)) => v,
        other => panic!("{expr}: {other:?}"),
    }
}

/// `id` attribute (or `@name` for attributes, `#text` for text) of each node.
fn ids(dom: &Dom, ctx: NodeId, expr: &str) -> Vec<String> {
    nodes(dom, ctx, expr)
        .into_iter()
        .map(|n| match n {
            XPathNode::Node(id) => match dom.node(id).unwrap().element() {
                Some(e) => e.id().map_or_else(|| e.local_name().to_string(), str::to_owned),
                None => format!("#{}", dom.xpath_string_value(n)),
            },
            XPathNode::Attr { element, index } => {
                format!("@{}", dom.attributes(element).unwrap()[index].qualified_name())
            }
        })
        .collect()
}

fn scalar(dom: &Dom, ctx: NodeId, expr: &str) -> XPathValue {
    evaluate(dom, expr, XPathNode::Node(ctx)).unwrap_or_else(|e| panic!("{expr}: {e:?}"))
}

fn num(dom: &Dom, ctx: NodeId, expr: &str) -> f64 {
    match scalar(dom, ctx, expr) {
        XPathValue::Number(n) => n,
        v => panic!("{expr}: {v:?}"),
    }
}

fn string(dom: &Dom, ctx: NodeId, expr: &str) -> String {
    match scalar(dom, ctx, expr) {
        XPathValue::String(s) => s,
        v => panic!("{expr}: {v:?}"),
    }
}

fn boolean(dom: &Dom, ctx: NodeId, expr: &str) -> bool {
    match scalar(dom, ctx, expr) {
        XPathValue::Boolean(b) => b,
        v => panic!("{expr}: {v:?}"),
    }
}

const CONFORMANCE: &str = r#"<!DOCTYPE html>
<html>
<head>
  <title>XPath conformance</title>
  <script src="../testing.js"></script>
</head>
<body>
  <h1 id="heading" class="primary">Hello World</h1>
  <p id="p1" lang="en" data-x="1">First paragraph with <em>emphasis</em>.</p>
  <p id="p2" class="note">Second paragraph.</p>
  <ul id="list">
    <li class="item odd">Item 1</li>
    <li class="item even">Item 2</li>
    <li class="item odd">Item 3</li>
    <li class="item even">Item 4</li>
    <li class="item odd">Item 5</li>
  </ul>
  <table id="t">
    <thead><tr><th>Name</th><th>Age</th></tr></thead>
    <tbody>
      <tr class="r"><td>Alice</td><td>30</td></tr>
      <tr class="r"><td>Bob</td><td>25</td></tr>
      <tr class="r"><td>Carol</td><td>40</td></tr>
    </tbody>
  </table>
  <div id="container">
    <section id="s1"><span>A</span><span>B</span></section>
    <section id="s2"><a href="/foo" id="link1">Click me</a></section>
    <section id="s3"><a href="/bar" id="link2">Other link</a></section>
  </div>
  <form id="form">
    <label for="name">Name</label>
    <input id="name" type="text" name="name" value="">
    <input id="email" type="email" name="email" value="">
    <input id="hidden" type="hidden" name="csrf" value="x">
    <input id="checkbox" type="checkbox" name="agree">
    <button id="btn" type="submit">Submit</button>
  </form>
  <!-- a comment node -->
  <div id="multi-class" class="alpha beta gamma"></div>
  <article id="art">
    <p>One</p>
    <p>Two</p>
    <p>Three</p>
  </article>
</body>
</html>"#;

#[test]
fn conformance_battery() {
    let (dom, doc) = page(CONFORMANCE);
    let cases: &[(&str, usize)] = &[
        ("/html", 1),
        ("/html/body", 1),
        ("/", 1),
        ("//h1", 1),
        ("//ul/li", 5),
        ("//ul//li", 5),
        (".", 1),
        (".//li", 5),
        ("//section/*", 4),
        ("//*[@id='heading']", 1),
        ("//li[1]/following-sibling::li", 4),
        ("//li[5]/preceding-sibling::li", 4),
        ("//li/parent::ul", 1),
        ("//li/ancestor::body", 1),
        ("//li/ancestor-or-self::body", 1),
        ("//li[3]/preceding::li", 2),
        ("//li[1]/following::li", 4),
        ("//ul/descendant::li", 5),
        ("//ul/descendant-or-self::li", 5),
        ("//section[1]/child::span", 2),
        ("//*[@id='heading']/self::h1", 1),
        ("//a[1]/attribute::href", 2),
        ("//a[1]/@*", 4),
        ("//li[1]", 1),
        ("//li[last()]", 1),
        ("//li[last() - 1]", 1),
        ("//li[position() = 1]", 1),
        ("//li[position() > 2]", 3),
        ("//li[position() mod 2 = 1]", 3),
        ("(//li)[1]", 1),
        ("(//section)[2]", 1),
        ("//li[3]/preceding-sibling::li[1]", 1),
        ("//li[5]/ancestor::*[1]", 1),
        ("//li[contains(concat(' ', @class, ' '), ' even ')][2]", 1),
        ("//*[@id='heading' and @class='primary']", 1),
        ("//*[@id='heading' or @id='p1']", 2),
        ("//section[a]", 2),
        ("//section[count(span) = 2]", 1),
        ("//ul[count(li) = 5]", 1),
        ("//tr[td[1]]", 3),
        ("//tr[td/text() = 'Bob']", 1),
        ("//*[starts-with(@id, 'link')]", 2),
        ("//*[normalize-space() = 'Hello World']", 1),
        ("//*[normalize-space(.) = 'Item 1']", 1),
        ("//*[concat(@id, '-x') = 'heading-x']", 1),
        ("//*[substring(@id, 1, 1) = 'p']", 2),
        ("//*[substring(@id, 2, 1) = '1' and starts-with(@id, 'p')]", 1),
        ("//p[translate(@id, 'p', 'q') = 'q1']", 1),
        ("//*[substring-before(@id, '1') = 'p']", 1),
        ("//*[substring-after(@id, 'lin') = 'k1']", 1),
        ("//tr[number(td[2]) > 28]", 2),
        ("//tr[floor(number(td[2]) div 10) = 3]", 1),
        ("//tr[ceiling(number(td[2]) div 10) = 3]", 2),
        ("//tr[round(number(td[2]) div 10) = 3]", 2),
        ("//ul[sum(li/@data-len) = 0]", 1),
        ("//p[boolean(@lang)]", 1),
        ("//*[false()]", 0),
        ("//*[name() = 'h1']", 1),
        ("//*[local-name() = 'h1']", 1),
        ("id('heading')", 1),
        ("id('heading p1')", 2),
        ("id(//em/parent::p/@id)", 1),
        ("//h1 | //title", 2),
        ("//h1 | //*[@id='p1']", 2),
        ("//*[@id='heading'] | //*[@id='heading']", 1),
        ("//li[position() + 1 = 3]", 1),
        ("//li[position() - 1 = 0]", 1),
        ("//li[position() * 2 = 4]", 1),
        ("//li[position() div 2 = 1]", 1),
        ("//li[(position() mod 2) = 0]", 2),
        ("//tr[number(td[2]) = 30]", 1),
        ("//tr[number(td[2]) != 30]", 3),
        ("//tr[number(td[2]) < 30]", 1),
        ("//tr[number(td[2]) <= 30]", 2),
        ("//tr[number(td[2]) > 30]", 1),
        ("//tr[number(td[2]) >= 30]", 2),
        ("//tr[td[2] = 30]", 1),
        ("//tr[td[2] = '30']", 1),
        ("//comment()", 1),
        (".//a[contains(normalize-space(string(.)), 'Click me')]", 1),
        (".//input[(./@type = 'text')]", 1),
        (".//*[@id='heading']", 1),
        (".//li[contains(concat(' ', @class, ' '), ' even ')]", 2),
        ("//*[@id='heading']/text()", 1),
        ("//em/parent::p", 1),
        ("//p[em]", 1),
        ("//p[not(em)]", 4),
        ("//section[a/@href = '/foo']", 1),
        ("//ul/li[last()][position() = last()]", 1),
        ("//ul[string(count(li)) = '5']", 1),
        ("//body[count(//*[contains(@class, 'item')]) = 5]", 1),
    ];
    assert_eq!(cases.len(), 91);
    let mut failures = Vec::new();
    for (expr, want) in cases {
        let got = nodes(&dom, doc, expr).len();
        if got != *want {
            failures.push(format!("{expr}: got {got}, want {want}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn document_order_axes_and_positions() {
    let (dom, doc) = page(CONFORMANCE);
    // Reverse axes number by proximity; results come back in document order.
    assert_eq!(ids(&dom, doc, "//li[3]/preceding-sibling::li[1]"), ["li"]);
    assert_eq!(string(&dom, doc, "string(//li[3]/preceding-sibling::li[1])"), "Item 2");
    assert_eq!(ids(&dom, doc, "//em/ancestor::*"), ["html", "body", "p1"]);
    assert_eq!(ids(&dom, doc, "//em/ancestor::*[2]"), ["body"]);
    assert_eq!(ids(&dom, doc, "//em/ancestor-or-self::*[1]"), ["em"]);
    assert_eq!(string(&dom, doc, "string(//li[4]/preceding::li[last()])"), "Item 1");
    assert_eq!(ids(&dom, doc, "//section/following::section"), ["s2", "s3"]);
    // Union is document order regardless of operand order.
    assert_eq!(ids(&dom, doc, "//*[@id='s3'] | //h1 | //*[@id='p2']"), ["heading", "p2", "s3"]);
    // Attributes sort after their element and before its children.
    assert_eq!(ids(&dom, doc, "//a/@id | //a | //a/@href"), ["link1", "@href", "@id", "link2", "@href", "@id"]);
    assert_eq!(ids(&dom, doc, "//*[@id='s2']/descendant-or-self::node()/@*"), ["@id", "@href", "@id"]);
    // Multi-context steps merge, sort and dedup.
    assert_eq!(nodes(&dom, doc, "//li/parent::*").len(), 1);
    assert_eq!(ids(&dom, doc, "(//section | //ul)/*[1]"), ["li", "span", "link1", "link2"]);
    // Filter predicates use document order even over reverse-axis paths.
    assert_eq!(ids(&dom, doc, "(//em/ancestor::*)[1]"), ["html"]);
    // Attribute context: parent, ancestors, following/preceding.
    assert_eq!(ids(&dom, doc, "//a[@id='link1']/@href/.."), ["link1"]);
    assert_eq!(ids(&dom, doc, "//a[@id='link1']/@href/ancestor::section"), ["s2"]);
    assert_eq!(ids(&dom, doc, "//section[@id='s2']/@id/following::a"), ["link1", "link2"]);
    assert_eq!(ids(&dom, doc, "//section[@id='s2']/@id/preceding::section"), ["s1"]);
    assert_eq!(nodes(&dom, doc, "//a/@href/self::node()").len(), 2);
    assert_eq!(nodes(&dom, doc, "//a/@href/self::href").len(), 0);
}

#[test]
fn playwright_ancestor_locator() {
    let (dom, doc) = page(
        r#"<div class="outer h-[2.625rem] flex"><div class="row h-[2.625rem]">
             <div class="cell"><span id=label>Customer</span></div></div></div>"#,
    );
    let label = dom.get_element_by_id(doc, "label").unwrap().unwrap();
    let found = nodes(&dom, label, "ancestor::div[contains(@class, 'h-[2.625rem]')][1]");
    assert_eq!(found.len(), 1);
    let XPathNode::Node(row) = found[0] else { panic!() };
    assert_eq!(dom.get_attribute(row, "class").unwrap(), Some("row h-[2.625rem]"));
    // Without [1] both matching ancestors come back, outermost first.
    assert_eq!(nodes(&dom, label, "ancestor::div[contains(@class, 'h-[2.625rem]')]").len(), 2);
}

#[test]
fn html_name_tests_and_namespaces() {
    let (mut dom, doc) =
        page(r#"<div id=d><svg id=s><rect id=r></rect></svg><Span id=sp></Span><p ID=x DATA-Y=2></p></div>"#);
    // HTML elements match case-insensitively; foreign elements need a prefix.
    assert_eq!(ids(&dom, doc, "//SPAN"), ["sp"]);
    assert_eq!(ids(&dom, doc, "//p[@DATA-y = 2]"), ["x"]);
    assert!(nodes(&dom, doc, "//svg").is_empty());
    assert_eq!(ids(&dom, doc, "//*[local-name() = 'svg']"), ["s"]);
    assert_eq!(string(&dom, doc, "namespace-uri(//*[@id='r'])"), "http://www.w3.org/2000/svg");
    assert_eq!(string(&dom, doc, "namespace-uri(//p)"), "http://www.w3.org/1999/xhtml");

    let mut e = XPathExpression::parse("//svg:svg/svg:* | //h:SPAN").unwrap();
    assert_eq!(e.prefixes(), ["svg", "h"]);
    assert!(matches!(e.evaluate(&dom, XPathNode::Node(doc)), Err(XPathError::Namespace(_))));
    let mut asked = Vec::new();
    e.resolve_namespaces(|p| {
        asked.push(p.to_owned());
        match p {
            "svg" => Some("http://www.w3.org/2000/svg".into()),
            "h" => Some("http://www.w3.org/1999/xhtml".into()),
            _ => None,
        }
    })
    .unwrap();
    assert_eq!(asked, ["svg", "h"]);
    let XPathValue::NodeSet(v) = e.evaluate(&dom, XPathNode::Node(doc)).unwrap() else { panic!() };
    assert_eq!(v.len(), 2);
    let mut bad = XPathExpression::parse("//x:a").unwrap();
    assert!(matches!(bad.resolve_namespaces(|_| None), Err(XPathError::Namespace(_))));

    // XML document: unprefixed names match null-namespace elements, case-sensitively.
    let xml = dom.create_document(DocumentKind::Xml, "about:blank");
    let mu = dom.create_element_ns(xml, None, "mu").unwrap();
    dom.append_child(xml, mu).unwrap();
    dom.set_attribute_ns(mu, Some("http://www.w3.org/XML/1998/namespace"), "xml:id", "id1").unwrap();
    for lang in ["en-GB", "no"] {
        let rho = dom.create_element_ns(xml, None, "rho").unwrap();
        dom.set_attribute_ns(rho, Some("http://www.w3.org/XML/1998/namespace"), "xml:lang", lang).unwrap();
        dom.append_child(mu, rho).unwrap();
    }
    let mut e = XPathExpression::parse("//mu[@xml:id=\"id1\"]/rho[@xml:lang=\"en-GB\"]").unwrap();
    e.resolve_namespaces(|p| (p == "xml").then(|| "http://www.w3.org/XML/1998/namespace".into())).unwrap();
    let XPathValue::NodeSet(v) = e.evaluate(&dom, XPathNode::Node(mu)).unwrap() else { panic!() };
    assert_eq!(v, [XPathNode::Node(dom.first_child(mu).unwrap().unwrap())]);
    assert!(nodes(&dom, xml, "//MU").is_empty());
    assert_eq!(nodes(&dom, xml, "//rho[lang('en')]").len(), 1);
    assert_eq!(nodes(&dom, xml, "//rho[lang('EN-gb')]").len(), 1);
    assert_eq!(nodes(&dom, xml, "//rho[lang('n')]").len(), 0);
    assert_eq!(string(&dom, xml, "name(//rho/@*)"), "xml:lang");
    assert_eq!(string(&dom, xml, "local-name(//rho/@*)"), "lang");
}

#[test]
fn scalar_results_and_functions() {
    let (dom, doc) = page(CONFORMANCE);
    let n = |e| num(&dom, doc, e);
    let s = |e| string(&dom, doc, e);
    let b = |e| boolean(&dom, doc, e);
    assert_eq!(n("count(//li)"), 5.0);
    assert_eq!(n("sum(//td[2])"), 95.0);
    assert_eq!(n("1 + 2 * 3"), 7.0);
    assert_eq!(n("10 div 4"), 2.5);
    assert_eq!(n("-7 mod 3"), -1.0);
    assert!(n("0 div 0").is_nan());
    assert_eq!(n("1 div 0"), f64::INFINITY);
    assert_eq!(n("-(3 + 2)"), -5.0);
    assert_eq!(n("string-length('héllo')"), 5.0);
    assert_eq!(n("floor(-1.5) + ceiling(-1.5) + round(2.5)"), -2.0 - 1.0 + 3.0);
    assert_eq!(n("number('  12 ')"), 12.0);
    assert!(n("number('1e3')").is_nan());
    assert_eq!(n("number(true())"), 1.0);
    assert_eq!(n("last()"), 1.0);
    assert_eq!(n("position()"), 1.0);
    assert_eq!(s("string(//h1)"), "Hello World");
    assert_eq!(s("string(42)"), "42");
    assert_eq!(s("string(0.5)"), "0.5");
    assert_eq!(s("string(1 div 0)"), "Infinity");
    assert_eq!(s("concat('a', 1, true())"), "a1true");
    assert_eq!(s("substring('12345', 1.5, 2.6)"), "234");
    assert_eq!(s("substring('12345', 0, 3)"), "12");
    assert_eq!(s("substring('12345', 0 div 0, 3)"), "");
    assert_eq!(s("substring('12345', -42, 1 div 0)"), "12345");
    assert_eq!(s("substring('12345', -1 div 0, 1 div 0)"), "");
    assert_eq!(s("substring('héllo', 2, 2)"), "él");
    assert_eq!(s("substring-before('1999/04/01', '/')"), "1999");
    assert_eq!(s("substring-after('1999/04/01', '/')"), "04/01");
    assert_eq!(s("translate('bar', 'abc', 'ABC')"), "BAr");
    assert_eq!(s("translate('--aaa--', 'abc-', 'ABC')"), "AAA");
    assert_eq!(s("normalize-space('  a \t b\n c ')"), "a b c");
    assert_eq!(s("name(//li[1])"), "li");
    assert_eq!(s("name(//comment())"), "");
    assert_eq!(s("local-name(//a/@href)"), "href");
    assert_eq!(s("string(//a/@href)"), "/foo");
    assert_eq!(s("namespace-uri(//a/@href)"), "");
    assert_eq!(s("string(//nothing)"), "");
    assert_eq!(s("$var"), "");
    assert!(b("true() and not(false())"));
    assert!(b("boolean(//li) and not(boolean(//nothing))"));
    assert!(b("starts-with('hello', 'he') and contains('hello', 'll') and contains('x', '')"));
    assert!(b("//li = 'Item 3'"));
    assert!(b("//li != 'Item 3'"));
    assert!(!b("//nothing = //nothing"));
    assert!(b("//td > 35"));
    assert!(b("35 < //td"));
    assert!(!b("//td > 40"));
    assert!(b("//li = true()"));
    assert!(b("//nothing = false()"));
    assert!(b("'1' = 1.0"));
    assert!(b("true() = 'x'"));
    assert!(!b("(0 div 0) = (0 div 0)"));
    assert!(b("(0 div 0) != (0 div 0)"));
    assert!(b("'abc' < 'abd' = false()"));
    assert!(!b("lang('en')"));
}

#[test]
fn errors() {
    let (dom, doc) = page(CONFORMANCE);
    for bad in
        ["//", "//li[", "foo(", "bogus()", "child::", "ancestor-of::x", "count(1, 2)", "1 +", "'unterminated", "a ? b"]
    {
        assert!(matches!(evaluate(&dom, bad, XPathNode::Node(doc)), Err(XPathError::Syntax(_))), "{bad}");
    }
    for bad in ["count('x')", "1 | //li", "'a'/b", "(1)[1]", "sum(3)", "name(1)"] {
        assert!(matches!(evaluate(&dom, bad, XPathNode::Node(doc)), Err(XPathError::Type(_))), "{bad}");
    }
}

#[test]
fn deep_trees_do_not_recurse() {
    // Port of xpath_evaluator.html #deep_nesting: axes and document-order
    // sorting must not recurse or walk ancestors per comparison.
    let (mut dom, doc) = page("<section id=root></section>");
    let root = dom.get_element_by_id(doc, "root").unwrap().unwrap();
    const DEPTH: usize = 20_000;
    let chain = |dom: &mut Dom, prefix: &str| {
        let mut top = dom.create_element(doc, "div").unwrap();
        dom.set_attribute(top, "id", &format!("{prefix}_deepest")).unwrap();
        for _ in 1..DEPTH {
            let d = dom.create_element(doc, "div").unwrap();
            dom.append_child(d, top).unwrap();
            top = d;
        }
        top
    };
    let before = chain(&mut dom, "before");
    dom.append_child(root, before).unwrap();
    let mid = dom.create_element(doc, "span").unwrap();
    dom.append_child(root, mid).unwrap();
    let after = chain(&mut dom, "after");
    dom.append_child(root, after).unwrap();

    let d = DEPTH as f64;
    assert_eq!(num(&dom, root, "count(.//div)"), d * 2.0);
    assert_eq!(num(&dom, root, "count(descendant::div)"), d * 2.0);
    assert_eq!(num(&dom, mid, "count(preceding::div)"), d);
    assert_eq!(num(&dom, mid, "count(following::div)"), d);
    assert_eq!(ids(&dom, mid, "preceding::div[1]"), ["before_deepest"]);
    assert_eq!(ids(&dom, mid, "following::div[last()]"), ["after_deepest"]);
    // Multi-context steps over the whole chain: merge + sort by index.
    assert_eq!(num(&dom, root, "count(.//div/parent::*)"), d * 2.0 - 1.0);
    assert_eq!(num(&dom, root, "count(//div[1])"), d * 2.0 - 1.0);
}
