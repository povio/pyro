use blitz_dom::util::ToColorColor as _;
use blitz_dom::{BaseDocument, Node};
use style::color::AbsoluteColor;
pub(crate) use style::computed_values::filter::single_value::T as StyloFilter;

use anyrender::filters::component_transfer::{
    ComponentTransferFilter, GammaTransferFunction, LinearTransferFunction, TransferFunction,
};
use anyrender::filters::{Filter, FilterEffect, color_transformation::ColorMatrix};

/// pyro: the filter in device pixels: lengths are CSS px, and the layers the filter is applied
/// to are drawn in device pixels (`scale` of them per CSS px) without a scale in their transform.
/// `current_color` is the element's `color`, which a `drop-shadow()` without one has. A `url()`
/// names an SVG `<filter>` of `dom` ([`reference_filter`]).
pub(crate) fn convert_filters(
    filters: &[StyloFilter],
    scale: f64,
    current_color: &AbsoluteColor,
    dom: &BaseDocument,
) -> Option<Filter> {
    if filters.is_empty() {
        return None;
    }

    Some(Filter::linear_list(filters.iter().flat_map(|filter| match filter {
        StyloFilter::Url(url) => reference_filter(dom, url).unwrap_or_default(),
        filter => convert_single_filter(filter, scale as f32, current_color).into_iter().collect(),
    })))
}

/// pyro: one layer of an element's `filter` list when an SVG filter in it is no chain of colour
/// functions ([`filter_steps`]).
#[cfg(feature = "svg")]
pub(crate) enum FilterStep {
    /// Filter functions (and `url()` colour chains) applied as CSS applies them: to the content,
    /// reading it past the layer's clip.
    Chain(std::sync::Arc<Filter>),
    /// An SVG `<filter>`'s graph, in the element's user space (CSS px from its border box's
    /// corner), applied within its filter region.
    Graph(anyrender_svg::FilterGraph),
}

/// pyro: the element's `filter` list as layers, the first applied first, when a `url()` in it names
/// an SVG `<filter>` that is no chain of colour functions which keep transparent pixels transparent
/// (those stay CSS chains, as [`convert_filters`] makes them): its primitives apply within its
/// filter region, which they may fill whatever the element paints. `size` is the border box in
/// CSS px. `None` when there is no such `url()` (the list is [`convert_filters`]'s).
#[cfg(feature = "svg")]
pub(crate) fn filter_steps(
    filters: &[StyloFilter],
    scale: f64,
    current_color: &AbsoluteColor,
    dom: &BaseDocument,
    size: (f32, f32),
    references: &[Option<blitz_dom::node::ImageResourceData>],
    source_tainted: bool,
) -> Option<Vec<FilterStep>> {
    if !filters.iter().any(|f| matches!(f, StyloFilter::Url(_))) {
        return None;
    }
    let mut steps = Vec::new();
    let mut chain: Vec<FilterEffect> = Vec::new();
    let mut any_graph = false;
    // Whether the source of what follows is tainted: a `drop-shadow()` coloured from
    // `currentcolor` (which a `:visited` link's colour can be) taints it.
    // pyro: and a backdrop (`source_tainted`), as Chromium treats one: a displacement map read
    // from it moves nothing (WPT filter-effects `backdrop-filter-svg-displacement`).
    let mut tainted = source_tainted;
    for filter in filters {
        match filter {
            StyloFilter::Url(url) => {
                if let Some(effects) = reference_filter(dom, url).filter(|effects| keeps_transparent(effects)) {
                    chain.extend(effects);
                    continue;
                }
                let Some(graph) = reference_graph(dom, url, size, tainted, references) else { continue };
                if !chain.is_empty() {
                    steps.push(FilterStep::Chain(std::sync::Arc::new(Filter::linear_list(chain.drain(..)))));
                }
                tainted = graph.tainted;
                steps.push(FilterStep::Graph(graph));
                any_graph = true;
            }
            filter => {
                if let StyloFilter::DropShadow(shadow) = filter {
                    // Its colour as computed keeps `currentcolor` (in a function too) unresolved.
                    tainted |= uses_current_color(&shadow.color);
                }
                chain.extend(convert_single_filter(filter, scale as f32, current_color));
            }
        }
    }
    if !any_graph {
        return None;
    }
    if !chain.is_empty() {
        steps.push(FilterStep::Chain(std::sync::Arc::new(Filter::linear_list(chain.into_iter()))));
    }
    Some(steps)
}

/// Whether a computed colour is `currentcolor` or made from it (in `color-mix()`, as a relative
/// colour's origin, in `contrast-color()`).
#[cfg(feature = "svg")]
fn uses_current_color(color: &style::values::computed::Color) -> bool {
    use style::values::generics::color::GenericColor;
    match color {
        GenericColor::Absolute(_) => false,
        GenericColor::CurrentColor => true,
        GenericColor::ColorMix(mix) => mix.items.iter().any(|item| uses_current_color(&item.color)),
        GenericColor::ContrastColor(color) => uses_current_color(color),
        GenericColor::ColorFunction(function) => function.origin_color().is_some_and(uses_current_color),
    }
}

/// pyro: for culling an element whose `filter` list is `steps` (`render_node`): how far the steps
/// move or spread content (a rect around the origin, as `layers::filter_reach`: the chains' and
/// graphs' reaches added up), the union of the graphs' filter regions (a graph's output lies in
/// its region whatever the content: a flood, turbulence), both in the frame's pixels, and whether
/// a graph has an `feTile` (which reads its input anywhere in its region). `transform` is the
/// element's to the frame (a chain is in its device pixels); `scale`, device pixels per CSS px (a
/// graph is in CSS px from the border box's corner).
#[cfg(feature = "svg")]
pub(crate) fn steps_reach(steps: &[FilterStep], transform: kurbo::Affine, scale: f64) -> (kurbo::Rect, Option<kurbo::Rect>, bool) {
    use kurbo::{Affine, Rect};
    let linear = |t: Affine| {
        let [a, b, c, d, _, _] = t.as_coeffs();
        Affine::new([a, b, c, d, 0.0, 0.0])
    };
    let user = transform * Affine::scale(scale);
    let (mut reach, mut regions, mut tiles) = (Rect::ZERO, None::<Rect>, false);
    for step in steps {
        let own = match step {
            FilterStep::Chain(filter) => crate::layers::filter_reach(filter, transform),
            FilterStep::Graph(graph) => {
                let region = user.transform_rect_bbox(graph.region);
                regions = Some(regions.map_or(region, |r| r.union(region)));
                let (own, tile) = graph.reach();
                tiles |= tile;
                linear(user).transform_rect_bbox(own)
            }
        };
        reach = Rect::new(reach.x0 + own.x0, reach.y0 + own.y0, reach.x1 + own.x1, reach.y1 + own.y1);
    }
    (reach, regions, tiles)
}

/// Whether colour functions keep transparent black transparent (a matrix whose alpha row adds
/// nothing, a transfer of the alpha that maps 0 to 0): applied as a chain, they change nothing
/// where the content is not, as they would over their filter region.
#[cfg(feature = "svg")]
fn keeps_transparent(effects: &[FilterEffect]) -> bool {
    effects.iter().all(|effect| match effect {
        FilterEffect::ColorMatrix(m) => m.0[19] <= 0.0,
        FilterEffect::ComponentTransfer(t) => match &t.alpha_function {
            TransferFunction::Identity => true,
            TransferFunction::Table(values) => values.first().is_none_or(|v| *v <= 0.0),
            TransferFunction::Discrete(values) => values.first().is_none_or(|v| *v <= 0.0),
            TransferFunction::Linear(f) => f.intercept <= 0.0,
            TransferFunction::Gamma(f) => f.offset <= 0.0,
        },
        _ => true,
    })
}

/// pyro: the SVG `<filter>` a `url(#id)` names as a graph for an element of `size` (CSS px): the
/// filter parsed by usvg for a stand-in of the element's border box (the object bounding box, and
/// the viewport its percentages resolve against), in the document of the `<svg>` around it.
#[cfg(feature = "svg")]
fn reference_graph(
    dom: &BaseDocument,
    url: &style::url::ComputedUrl,
    (width, height): (f32, f32),
    source_tainted: bool,
    references: &[Option<blitz_dom::node::ImageResourceData>],
) -> Option<anyrender_svg::FilterGraph> {
    // pyro: a `<filter>` of another document the element has loaded (`url(filters.svg#blur)`,
    // its `reference_images`), else of this one.
    let external = crate::render::external_reference(references, url)
        .filter(|(source, id)| {
            blitz_dom::node::svg_source_element(source, id, "id").is_some_and(|(name, _)| name == "filter")
        });
    let fragment;
    let (id, node_id) = match &external {
        Some((_, id)) => (id.as_str(), None),
        None => {
            fragment = url.url()?.fragment()?.to_string();
            let node_id = dom.get_element_by_id(&fragment)?;
            if local_name(&dom.tree()[node_id]) != Some("filter") {
                return None;
            }
            (fragment.as_str(), Some(node_id))
        }
    };
    // The stand-in: a box with no paint (an empty group where the box has no area).
    let escaped: String = id
        .chars()
        .flat_map(|c| match c {
            '"' => "&quot;".chars().collect::<Vec<_>>(),
            '&' => "&amp;".chars().collect(),
            '<' => "&lt;".chars().collect(),
            c => vec![c],
        })
        .collect();
    let content = if width > 0.0 && height > 0.0 {
        format!("<rect width=\"{width}\" height=\"{height}\" fill=\"none\" filter=\"url(#{escaped})\"/>")
    } else {
        format!("<g filter=\"url(#{escaped})\"/>")
    };
    let svg = match (node_id, &external) {
        (Some(node_id), _) => dom.svg_reference_document(node_id, &content, width.max(1e-3), height.max(1e-3))?,
        (None, Some((source, _))) => blitz_dom::node::SvgImageData::reference_document_from_source(
            source,
            &content,
            width.max(1e-3),
            height.max(1e-3),
        )?,
        (None, None) => return None,
    };
    fn first_filter(group: &anyrender_svg::usvg::Group) -> Option<std::sync::Arc<anyrender_svg::usvg::filter::Filter>> {
        group.filters().first().cloned().or_else(|| {
            group.children().iter().find_map(|node| match node {
                anyrender_svg::usvg::Node::Group(g) => first_filter(g),
                _ => None,
            })
        })
    }
    let filter = first_filter(svg.tree.root())?;
    // (What the document knows of its primitives; another document's are taken as usvg has them.)
    let notes = node_id.map(|node_id| primitive_notes(dom, &dom.tree()[node_id])).unwrap_or_default();
    anyrender_svg::FilterGraph::new(&filter, &|index| notes.get(index).copied().unwrap_or_default(), source_tainted)
}

/// pyro: what the document knows of the filters of an inline `<svg>`'s tree that usvg does not
/// keep (see [`anyrender_svg::PrimitiveNotes`]), by filter id; `None` for another element or a tree
/// without filters. A tree filter's id names the `<filter>` in the `<svg>` (the first of that id,
/// as usvg resolves it in the source it reads), else in the document, when its primitives are that
/// element's. usvg names the second and later uses of a filter it cannot share (an
/// `objectBoundingBox` one, the default) `filterN`: such a use takes the notes of the uses it
/// repeats (the same primitives and colours), all of them if several (a tainted one taints).
#[cfg(feature = "svg")]
pub(crate) fn svg_filter_notes(
    dom: &BaseDocument,
    node: &Node,
    tree: &anyrender_svg::usvg::Tree,
) -> Option<anyrender_svg::FilterNotes> {
    use anyrender_svg::usvg::filter::Filter as TreeFilter;
    if tree.filters().is_empty() || local_name(node) != Some("svg") {
        return None;
    }
    // The `<filter>`s in the `<svg>`, by id.
    let mut scoped: std::collections::HashMap<&str, blitz_dom::NodeId> = std::collections::HashMap::new();
    let mut stack: Vec<blitz_dom::NodeId> = node.children.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let child = &dom.tree()[id];
        if local_name(child) == Some("filter")
            && let Some(name) = attr(child, "id")
        {
            scoped.entry(name).or_insert(id);
        }
        stack.extend(child.children.iter().rev().copied());
    }
    let mut notes = anyrender_svg::FilterNotes::new();
    let mut repeats: Vec<&TreeFilter> = Vec::new();
    for filter in tree.filters() {
        let element = scoped
            .get(filter.id())
            .copied()
            .or_else(|| dom.get_element_by_id(filter.id()))
            .map(|id| &dom.tree()[id])
            .filter(|element| local_name(element) == Some("filter") && same_primitives(dom, element, filter));
        match element {
            Some(element) => {
                notes.insert(filter.id().to_owned(), primitive_notes(dom, element));
            }
            None => repeats.push(filter),
        }
    }
    // pyro: the filters usvg made of CSS `drop-shadow()` functions (ids `filterN`, one
    // `feDropShadow` each, in the order usvg met them: the SVG's elements in tree order, each
    // list's functions in order) are the drop shadows of the SVG elements' computed `filter`s, in
    // that order: one coloured from `currentcolor` is tainted, as an HTML element's is
    // (`filter_steps`; WPT filter-effects `tainting-css-dropshadow-001`). Only when they pair up.
    let generated: Vec<&TreeFilter> = tree
        .filters()
        .iter()
        .filter(|filter| {
            filter.id().strip_prefix("filter").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                && matches!(filter.primitives(), [p] if matches!(p.kind(), anyrender_svg::usvg::filter::Kind::DropShadow(_)))
        })
        .map(|filter| &**filter)
        .collect();
    if !generated.is_empty() {
        let mut shadows: Vec<bool> = Vec::new();
        let mut stack: Vec<blitz_dom::NodeId> = node.children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let child = &dom.tree()[id];
            if let Some(style) = child.primary_styles().filter(|_| child.element_data().is_some()) {
                for function in style.get_effects().filter.0.iter() {
                    if let StyloFilter::DropShadow(shadow) = function {
                        shadows.push(uses_current_color(&shadow.color));
                    }
                }
            }
            stack.extend(child.children.iter().rev().copied());
        }
        if shadows.len() == generated.len() {
            for (filter, tainted) in generated.iter().zip(shadows) {
                if tainted {
                    notes.insert(
                        filter.id().to_owned(),
                        vec![anyrender_svg::PrimitiveNotes { tainted: true, ..Default::default() }],
                    );
                }
            }
            repeats.retain(|filter| !generated.iter().any(|g| std::ptr::eq(*g, *filter)));
        }
    }
    for filter in repeats {
        let signature = tree_signature(filter);
        let mut merged: Option<Vec<anyrender_svg::PrimitiveNotes>> = None;
        for known in tree.filters() {
            let Some(known_notes) = notes.get(known.id()) else { continue };
            if tree_signature(known) != signature {
                continue;
            }
            match &mut merged {
                None => merged = Some(known_notes.clone()),
                Some(merged) => {
                    for (m, n) in merged.iter_mut().zip(known_notes) {
                        m.tainted |= n.tainted;
                        m.no_light |= n.no_light;
                        m.flood &= n.flood;
                        m.element_image |= n.element_image;
                    }
                }
            }
        }
        if let Some(merged) = merged {
            notes.insert(filter.id().to_owned(), merged);
        }
    }
    Some(notes)
}

/// What tells a tree filter's uses apart: its primitives' kinds and colours (a use's subregions
/// and lengths depend on the element it filters).
#[cfg(feature = "svg")]
fn tree_signature(filter: &anyrender_svg::usvg::filter::Filter) -> Vec<(std::mem::Discriminant<anyrender_svg::usvg::filter::Kind>, String)> {
    use anyrender_svg::usvg::filter::Kind;
    filter
        .primitives()
        .iter()
        .map(|primitive| {
            let kind = primitive.kind();
            let colours = match kind {
                Kind::Flood(f) => format!("{:?} {}", f.color(), f.opacity().get()),
                Kind::DropShadow(s) => format!("{:?} {}", s.color(), s.opacity().get()),
                Kind::DiffuseLighting(l) => format!("{:?}", l.lighting_color()),
                Kind::SpecularLighting(l) => format!("{:?}", l.lighting_color()),
                _ => String::new(),
            };
            (std::mem::discriminant(kind), colours)
        })
        .collect()
}

/// Whether the `<filter>` `element`'s primitives are those of the tree filter `filter`, one for
/// one and kind for kind (usvg's stand-in for one it could not read, a transparent flood, stands
/// for any).
#[cfg(feature = "svg")]
fn same_primitives(dom: &BaseDocument, element: &Node, filter: &anyrender_svg::usvg::filter::Filter) -> bool {
    use anyrender_svg::usvg::filter::Kind;
    let elements = filter_primitives(dom, element);
    elements.len() == filter.primitives().len()
        && elements.iter().zip(filter.primitives()).all(|(element, primitive)| {
            let name = local_name(element).unwrap_or_default();
            match primitive.kind() {
                Kind::Flood(_) => true,
                Kind::Blend(_) => name == "feBlend",
                Kind::ColorMatrix(_) => name == "feColorMatrix",
                Kind::ComponentTransfer(_) => name == "feComponentTransfer",
                Kind::Composite(_) => name == "feComposite",
                Kind::ConvolveMatrix(_) => name == "feConvolveMatrix",
                Kind::DiffuseLighting(_) => name == "feDiffuseLighting",
                Kind::DisplacementMap(_) => name == "feDisplacementMap",
                Kind::DropShadow(_) => name == "feDropShadow",
                Kind::GaussianBlur(_) => name == "feGaussianBlur",
                Kind::Image(_) => name == "feImage",
                Kind::Merge(_) => name == "feMerge",
                Kind::Morphology(_) => name == "feMorphology",
                Kind::Offset(_) => name == "feOffset",
                Kind::SpecularLighting(_) => name == "feSpecularLighting",
                Kind::Tile(_) => name == "feTile",
                Kind::Turbulence(_) => name == "feTurbulence",
            }
        })
}

/// The primitives of a `<filter>`: its own, or, when it has none, those of the `<filter>` its
/// `href` names (a template), as usvg takes them.
#[cfg(feature = "svg")]
fn filter_primitives<'a>(dom: &'a BaseDocument, filter: &'a Node) -> Vec<&'a Node> {
    let mut at = filter;
    for _ in 0..16 {
        let primitives: Vec<&Node> = at
            .children
            .iter()
            .map(|&id| &dom.tree()[id])
            .filter(|n| local_name(n).is_some_and(|name| PRIMITIVES.contains(&name)))
            .collect();
        if !primitives.is_empty() {
            return primitives;
        }
        let Some(next) = attr(at, "href")
            .and_then(|href| href.trim().strip_prefix('#'))
            .and_then(|id| dom.get_element_by_id(id))
            .map(|id| &dom.tree()[id])
            .filter(|n| local_name(n) == Some("filter"))
        else {
            break;
        };
        at = next;
    }
    Vec::new()
}

/// The filter primitives usvg reads, by element name.
#[cfg(feature = "svg")]
const PRIMITIVES: &[&str] = &[
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDropShadow",
    "feFlood",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMorphology",
    "feOffset",
    "feSpecularLighting",
    "feTile",
    "feTurbulence",
];

/// The notes of a `<filter>`'s primitives, in order: which colour (`flood-color` or
/// `lighting-color`, as an attribute or a declaration of the `style` attribute) is computed from
/// `currentcolor` and which lighting primitive has no light source.
#[cfg(feature = "svg")]
fn primitive_notes(dom: &BaseDocument, filter: &Node) -> Vec<anyrender_svg::PrimitiveNotes> {
    let children = |node: &Node| -> Vec<&Node> {
        node.children.iter().map(|&id| &dom.tree()[id]).filter(|n| n.is_element()).collect::<Vec<_>>()
    };
    filter_primitives(dom, filter)
        .into_iter()
        .filter_map(|primitive| {
            let name = local_name(primitive)?;
            let property = match name {
                "feFlood" | "feDropShadow" => Some("flood-color"),
                "feDiffuseLighting" | "feSpecularLighting" => Some("lighting-color"),
                _ => None,
            };
            let tainted = property.is_some_and(|property| {
                let current = |value: &str| value.to_ascii_lowercase().contains("currentcolor");
                attr(primitive, property).is_some_and(current)
                    || attr(primitive, "style").is_some_and(|style| {
                        style
                            .split(';')
                            .filter_map(|d| d.split_once(':'))
                            .any(|(name, value)| name.trim().eq_ignore_ascii_case(property) && current(value))
                    })
            });
            let no_light = matches!(name, "feDiffuseLighting" | "feSpecularLighting")
                && !children(primitive)
                    .iter()
                    .any(|n| matches!(local_name(n), Some("feDistantLight" | "fePointLight" | "feSpotLight")));
            let element_image = name == "feImage"
                && attr(primitive, "href")
                    .is_some_and(|href| href.trim_start().starts_with('#'));
            Some(anyrender_svg::PrimitiveNotes { tainted, no_light, flood: name == "feFlood", element_image })
        })
        .collect()
}

pub(crate) fn convert_single_filter(
    filter: &StyloFilter,
    scale: f32,
    current_color: &AbsoluteColor,
) -> Option<FilterEffect> {
    Some(match filter {
        StyloFilter::Blur(radius) => FilterEffect::blur(radius.px() * scale),
        StyloFilter::Brightness(amount) => color_matrix(brightness(amount.0)),
        StyloFilter::Contrast(amount) => color_matrix(contrast(amount.0)),
        StyloFilter::Grayscale(amount) => color_matrix(grayscale(1.0 - amount.0)),
        StyloFilter::HueRotate(angle) => color_matrix(hue_rotate(angle.degrees())),
        StyloFilter::Invert(amount) => color_matrix(invert(amount.0)),
        StyloFilter::Opacity(amount) => color_matrix(opacity(amount.0)),
        StyloFilter::Saturate(amount) => color_matrix(saturate(amount.0)),
        StyloFilter::Sepia(amount) => color_matrix(sepia(1.0 - amount.0)),
        // pyro: Chromium takes the third length as the blur's standard deviation (measured: the
        // edge of `drop-shadow(0 0 20px)` falls off as a Gaussian of 20px), where `text-shadow`
        // and `box-shadow` take half of it.
        StyloFilter::DropShadow(shadow) => FilterEffect::drop_shadow(
            shadow.horizontal.px() * scale,
            shadow.vertical.px() * scale,
            shadow.blur.px() * scale,
            shadow
                .color
                .resolve_to_absolute(current_color)
                .as_color_color(),
        ),
        StyloFilter::Url(_) => return None,
    })
}

/// pyro: a `url(#id)` filter: the SVG `<filter>` of `dom` it names, when that is a chain of colour
/// primitives (`feColorMatrix`, `feComponentTransfer`; each one's input the one before it). Each
/// primitive works in its `color-interpolation-filters` space: linearRGB unless an ancestor's
/// attribute says sRGB, and sRGB for a filter in a `display: none` subtree, as in Chromium (a
/// `backdrop-filter: url()` matches `backdrop-filter: invert(1)` there). `None` for anything else.
fn reference_filter(dom: &BaseDocument, url: &style::url::ComputedUrl) -> Option<Vec<FilterEffect>> {
    let id = url.url()?.fragment()?;
    let filter = &dom.tree()[dom.get_element_by_id(id)?];
    if local_name(filter) != Some("filter") {
        return None;
    }
    // Stylo styles nothing in a `display: none` subtree.
    let unstyled = filter.primary_styles().is_none();
    let mut effects = Vec::new();
    let mut previous: Option<&str> = None;
    let primitives = filter.children.iter().map(|&id| &dom.tree()[id]).filter(|node| node.is_element());
    for (index, primitive) in primitives.enumerate() {
        match attr(primitive, "in") {
            None => {}
            Some("SourceGraphic") if index == 0 => {}
            Some(input) if Some(input) == previous => {}
            Some(_) => return None,
        }
        let effect = match local_name(primitive)? {
            "feColorMatrix" => color_matrix_primitive(primitive)?,
            "feComponentTransfer" => component_transfer_primitive(dom, primitive)?,
            _ => return None,
        };
        let linear = match interpolation(dom, primitive) {
            Some(space) => space.eq_ignore_ascii_case("linearRGB"),
            None => !unstyled,
        };
        if linear {
            effects.extend([to_linear(), effect, to_srgb()]);
        } else {
            effects.push(effect);
        }
        previous = attr(primitive, "result");
    }
    Some(effects)
}

fn local_name(node: &Node) -> Option<&str> {
    node.element_data().map(|element| &*element.name.local)
}

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attrs()?.iter().find(|attr| &*attr.name.local == name).map(|attr| attr.value.as_str())
}

/// The `color-interpolation-filters` of a primitive: its attribute, or the nearest ancestor's
/// (`auto` is sRGB).
fn interpolation<'a>(dom: &'a BaseDocument, node: &'a Node) -> Option<&'a str> {
    let mut at = Some(node);
    while let Some(node) = at {
        match attr(node, "color-interpolation-filters").map(str::trim) {
            Some("inherit") | None => {}
            Some("auto") => return Some("sRGB"),
            Some(space) => return Some(space),
        }
        at = node.parent.map(|parent| &dom.tree()[parent]);
    }
    None
}

fn numbers(value: &str) -> Vec<f32> {
    value
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|n| !n.is_empty())
        .map_while(|n| n.parse().ok())
        .collect()
}

fn number(node: &Node, name: &str, default: f32) -> f32 {
    attr(node, name).and_then(|value| value.trim().parse().ok()).unwrap_or(default)
}

/// `feColorMatrix`, as the filter effects spec defines each `type` (a missing or malformed
/// `values` is the identity).
fn color_matrix_primitive(node: &Node) -> Option<FilterEffect> {
    let values = attr(node, "values").map(numbers);
    Some(color_matrix(match attr(node, "type").unwrap_or("matrix").trim() {
        "matrix" => match values {
            Some(values) if values.len() == 20 => values.try_into().ok()?,
            _ => opacity(1.0),
        },
        "saturate" => saturate_spec(values.and_then(|v| v.first().copied()).unwrap_or(1.0)),
        "hueRotate" => hue_rotate(values.and_then(|v| v.first().copied()).unwrap_or(0.0)),
        "luminanceToAlpha" => {
            let mut m = [0.0; 20];
            (m[15], m[16], m[17]) = (0.2125, 0.7154, 0.0721);
            m
        }
        _ => return None,
    }))
}

/// `feColorMatrix type="saturate"`: the spec's matrix (the blue column as written, not what the
/// row leaves of 1 as Chromium builds CSS `saturate()`).
fn saturate_spec(s: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.213 + 0.787 * s;
    m[1] = 0.715 - 0.715 * s;
    m[2] = 0.072 - 0.072 * s;
    m[5] = 0.213 - 0.213 * s;
    m[6] = 0.715 + 0.285 * s;
    m[7] = 0.072 - 0.072 * s;
    m[10] = 0.213 - 0.213 * s;
    m[11] = 0.715 - 0.715 * s;
    m[12] = 0.072 + 0.928 * s;
    m[18] = 1.0;
    m
}

/// `feComponentTransfer`: its `feFuncR`, `feFuncG`, `feFuncB` and `feFuncA` (the last of each).
fn component_transfer_primitive(dom: &BaseDocument, node: &Node) -> Option<FilterEffect> {
    let mut functions = [const { TransferFunction::Identity }; 4];
    for child in node.children.iter().map(|&id| &dom.tree()[id]) {
        let channel = match local_name(child) {
            Some("feFuncR") => 0,
            Some("feFuncG") => 1,
            Some("feFuncB") => 2,
            Some("feFuncA") => 3,
            _ => continue,
        };
        let values = attr(child, "tableValues").map(numbers).unwrap_or_default();
        functions[channel] = match attr(child, "type").unwrap_or("identity").trim() {
            "table" if !values.is_empty() => {
                TransferFunction::Table(if values.len() == 1 { vec![values[0]; 2] } else { values }.into())
            }
            "discrete" if !values.is_empty() => TransferFunction::Discrete(values),
            "linear" => TransferFunction::Linear(LinearTransferFunction {
                slope: number(child, "slope", 1.0),
                intercept: number(child, "intercept", 0.0),
            }),
            "gamma" => TransferFunction::Gamma(GammaTransferFunction {
                amplitude: number(child, "amplitude", 1.0),
                exponent: number(child, "exponent", 1.0),
                offset: number(child, "offset", 0.0),
            }),
            _ => TransferFunction::Identity,
        };
    }
    let [red_function, green_function, blue_function, alpha_function] = functions;
    Some(FilterEffect::ComponentTransfer(ComponentTransferFilter {
        red_function,
        green_function,
        blue_function,
        alpha_function,
    }))
}

/// sRGB values to linearRGB (the colour channels, as a table of 256 for the values of bytes).
pub(crate) fn to_linear() -> FilterEffect {
    srgb_table(256, |c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) })
}

/// linearRGB values back to sRGB (a table of 1024: the curve is steep near black).
pub(crate) fn to_srgb() -> FilterEffect {
    srgb_table(1024, |c| if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 })
}

fn srgb_table(len: usize, curve: impl Fn(f32) -> f32) -> FilterEffect {
    let table: Vec<f32> = (0..len).map(|i| curve(i as f32 / (len - 1) as f32)).collect();
    let function = TransferFunction::Table(table.into());
    FilterEffect::ComponentTransfer(ComponentTransferFilter {
        red_function: function.clone(),
        green_function: function.clone(),
        blue_function: function,
        alpha_function: TransferFunction::Identity,
    })
}

// pyro: the colour functions' matrices as Chromium builds them (`cc/paint/render_surface_filters.cc`),
// f32 for f32: the filter effects spec's `feColorMatrix` equivalents, with grayscale's and
// saturate's last column of each row what the row leaves of 1, and grayscale and sepia given
// 1 - amount. Rows R, G, B, A; the fifth column is an offset in 0..1. They apply to sRGB values,
// as Chromium applies them (`color-interpolation-filters` is for SVG filter primitives).

fn color_matrix(matrix: [f32; 20]) -> FilterEffect {
    FilterEffect::ColorMatrix(ColorMatrix(matrix))
}

fn brightness(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    (m[0], m[6], m[12], m[18]) = (amount, amount, amount, 1.0);
    m
}

fn contrast(amount: f32) -> [f32; 20] {
    let mut m = brightness(amount);
    let intercept = -0.5 * amount + 0.5;
    (m[4], m[9], m[14]) = (intercept, intercept, intercept);
    m
}

/// `amount` is 1 - the function's.
fn grayscale(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.2126 + 0.7874 * amount;
    m[1] = 0.7152 - 0.7152 * amount;
    m[2] = 1.0 - (m[0] + m[1]);
    m[5] = 0.2126 - 0.2126 * amount;
    m[6] = 0.7152 + 0.2848 * amount;
    m[7] = 1.0 - (m[5] + m[6]);
    m[10] = 0.2126 - 0.2126 * amount;
    m[11] = 0.7152 - 0.7152 * amount;
    m[12] = 1.0 - (m[10] + m[11]);
    m[18] = 1.0;
    m
}

/// `amount` is 1 - the function's.
fn sepia(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.393 + 0.607 * amount;
    m[1] = 0.769 - 0.769 * amount;
    m[2] = 0.189 - 0.189 * amount;
    m[5] = 0.349 - 0.349 * amount;
    m[6] = 0.686 + 0.314 * amount;
    m[7] = 0.168 - 0.168 * amount;
    m[10] = 0.272 - 0.272 * amount;
    m[11] = 0.534 - 0.534 * amount;
    m[12] = 0.131 + 0.869 * amount;
    m[18] = 1.0;
    m
}

fn saturate(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.213 + 0.787 * amount;
    m[1] = 0.715 - 0.715 * amount;
    m[2] = 1.0 - (m[0] + m[1]);
    m[5] = 0.213 - 0.213 * amount;
    m[6] = 0.715 + 0.285 * amount;
    m[7] = 1.0 - (m[5] + m[6]);
    m[10] = 0.213 - 0.213 * amount;
    m[11] = 0.715 - 0.715 * amount;
    m[12] = 1.0 - (m[10] + m[11]);
    m[18] = 1.0;
    m
}

fn hue_rotate(degrees: f32) -> [f32; 20] {
    let radians = degrees * std::f32::consts::PI / 180.0;
    let (sin, cos) = (radians.sin(), radians.cos());
    let mut m = [0.0; 20];
    m[0] = 0.213 + cos * 0.787 - sin * 0.213;
    m[1] = 0.715 - cos * 0.715 - sin * 0.715;
    m[2] = 0.072 - cos * 0.072 + sin * 0.928;
    m[5] = 0.213 - cos * 0.213 + sin * 0.143;
    m[6] = 0.715 + cos * 0.285 + sin * 0.140;
    m[7] = 0.072 - cos * 0.072 - sin * 0.283;
    m[10] = 0.213 - cos * 0.213 - sin * 0.787;
    m[11] = 0.715 - cos * 0.715 + sin * 0.715;
    m[12] = 0.072 + cos * 0.928 + sin * 0.072;
    m[18] = 1.0;
    m
}

fn invert(amount: f32) -> [f32; 20] {
    let mut m = brightness(1.0 - 2.0 * amount);
    (m[4], m[9], m[14]) = (amount, amount, amount);
    m
}

fn opacity(amount: f32) -> [f32; 20] {
    let mut m = brightness(1.0);
    m[18] = amount;
    m
}

/// pyro: a `filter()` image (CSS `<image>`): the image it filters, through nested `filter()`s,
/// and their filters, innermost first, in device pixels as [`convert_filters`] makes them.
/// `None` for an image that is not one.
pub(crate) fn image_filter<'a>(
    image: &'a style::values::computed::Image,
    scale: f64,
    current_color: &AbsoluteColor,
    dom: &BaseDocument,
) -> (
    &'a style::values::computed::Image,
    Option<std::sync::Arc<Filter>>,
) {
    use style::values::generics::image::GenericImage;
    let mut inner = image;
    let mut levels = Vec::new();
    while let GenericImage::Filter(filtered) = inner {
        levels.push(&filtered.filters);
        inner = &filtered.image;
    }
    if levels.is_empty() {
        return (image, None);
    }
    let filters: Vec<StyloFilter> = levels
        .iter()
        .rev()
        .flat_map(|level| level.iter().cloned())
        .collect();
    (
        inner,
        convert_filters(&filters, scale, current_color, dom).map(std::sync::Arc::new),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour (bytes) through a matrix, as Chromium rounds it.
    fn through(m: [f32; 20], [r, g, b]: [u8; 3]) -> [u8; 3] {
        let c = [r, g, b].map(|v| f32::from(v) / 255.0);
        std::array::from_fn(|row| {
            let m = &m[row * 5..row * 5 + 5];
            let v = (m[0] * c[0] + m[1] * c[1] + m[2] * c[2] + m[3] + m[4]).clamp(0.0, 1.0);
            (v * 255.0 + 0.5) as u8
        })
    }

    /// The colours WPT's references give (their values are Chromium's).
    #[test]
    fn the_colour_functions_give_the_colours_wpt_expects() {
        // filter-grayscale-001: blue at grayscale(100%) is #121212.
        assert_eq!(through(grayscale(0.0), [0, 0, 255]), [0x12; 3]);
        // filter-contrast-002: red at contrast(0%) is #808080.
        assert_eq!(through(contrast(0.0), [255, 0, 0]), [0x80; 3]);
        // filter-hue_rotate-002: red at hue-rotate(120deg) is #007100.
        assert_eq!(through(hue_rotate(120.0), [255, 0, 0]), [0, 0x71, 0]);
        // filter-saturate-001: yellow at saturate(0%) is #EDEDED.
        assert_eq!(through(saturate(0.0), [255, 255, 0]), [0xED; 3]);
        // composited-filters-under-opacity: yellow at invert(100%) is blue.
        assert_eq!(through(invert(1.0), [255, 255, 0]), [0, 0, 255]);
    }

    /// At the amounts that change nothing, the matrices are the identity to f32 rounding (which the
    /// renderer skips: no byte changes).
    #[test]
    fn neutral_amounts_are_the_identity() {
        let identity = opacity(1.0);
        for m in [
            brightness(1.0),
            contrast(1.0),
            grayscale(1.0),
            sepia(1.0),
            saturate(1.0),
            invert(0.0),
            hue_rotate(0.0),
        ] {
            assert!(
                m.iter().zip(identity).all(|(a, b)| (a - b).abs() <= 1e-6),
                "{m:?}"
            );
        }
    }
}
