use style::values::generics::calc::{CalcNodeLeaf, RoundingStrategy};
use style::values::specified::calc::{CalcNode, Leaf};

#[test]
fn exceptional_math_values_survive_folding_and_resolution() {
    let number = |v| Box::new(CalcNode::Leaf(Leaf::new_number(v)));
    let pow = |a, b| CalcNode::Pow(number(a), number(b));
    let log = |a, b| CalcNode::Log(number(a), Some(number(b)).into());
    let round = |a, b| CalcNode::Round {
        strategy: RoundingStrategy::Nearest,
        value: number(a),
        step: number(b),
    };
    for (mut node, expected) in [
        (pow(f32::NAN, 0.0), f32::NAN),
        (pow(1.0, f32::NAN), f32::NAN),
        (pow(-1.0, f32::NEG_INFINITY), f32::NAN),
        (pow(-1.0, 3.0), -1.0),
        (log(2.0, 1.0), f32::NAN),
        (log(1.0, 0.5), 0.0),
        (log(-0.0, 0.5), f32::NEG_INFINITY),
        (log(f32::INFINITY, f32::INFINITY), f32::INFINITY),
        (log(2.0, 0.5), -1.0),
        (round(f32::INFINITY, f32::NAN), f32::NAN),
        (round(f32::NAN, f32::INFINITY), f32::NAN),
    ] {
        let check = |node: &CalcNode| {
            let actual = node.resolve().unwrap().as_number().unwrap();
            assert!(actual.to_bits() == expected.to_bits() || (actual.is_nan() && expected.is_nan()), "{node:?}: {actual}");
        };
        check(&node);
        node.simplify_and_sort();
        check(&node);
    }
}

#[test]
fn attr_unit_names_parse_before_element_specific_validation() {
    use pyro_dom::stylo::declarations::DeclarationBlock;

    for ty in ["furlong", "kHz", "grad", "fr", "%", "number", "type(<image>)"] {
        let value = format!("attr(data-value {ty}, 3px)");
        let declarations = DeclarationBlock::parse(&format!("width: {value}"));
        assert!(!declarations.value("width").is_empty(), "{value}");
    }
    for ty in ["default", "type(<url>)", "type(<url> | <length>)", "type(<url>+)"] {
        let value = format!("attr(data-value {ty}, 3px)");
        let declarations = DeclarationBlock::parse(&format!("width: {value}"));
        assert!(declarations.value("width").is_empty(), "{value}");
    }
}
