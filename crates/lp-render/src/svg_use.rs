use crate::Fetcher;
use blitz_dom::BaseDocument;
use std::{
    collections::HashMap,
    sync::{mpsc, Arc},
    time::Duration,
};

pub(crate) fn resolve(
    doc: &mut BaseDocument,
    base: &str,
    fetcher: &Arc<dyn Fetcher>,
    timeout: Duration,
) -> Result<(), String> {
    doc.set_html_parser_provider(Arc::new(blitz_html::HtmlProvider));
    let mut resources = HashMap::<String, String>::new();
    let roots = doc.query_selector_all("svg").map_err(|e| format!("{e:?}"))?;
    for root in roots.into_iter().rev() {
        let uses = doc.query_selector_all_in(root, "use").map_err(|e| format!("{e:?}"))?;
        let mut definitions = String::new();
        let mut replacements = Vec::new();
        for id in uses {
            let node = doc.get_node(id).ok_or("missing SVG use")?;
            let Some(el) = node.element_data() else { continue };
            let Some(href) = el.attrs().iter().find(|a| &*a.name.local == "href").map(|a| a.value.to_string()) else {
                continue;
            };
            if href.starts_with('#') || replacements.iter().any(|(from, _)| from == &href) {
                continue;
            }
            let mut url = url::Url::parse(base).and_then(|base| base.join(&href)).map_err(|e| e.to_string())?;
            let fragment = url.fragment().ok_or("external SVG use has no fragment")?.to_string();
            url.set_fragment(None);
            let address = url.to_string();
            if !resources.contains_key(&address) {
                let (tx, rx) = mpsc::channel();
                fetcher.fetch(
                    address.clone(),
                    Box::new(move |result| {
                        let _ = tx.send(result);
                    }),
                );
                let bytes = rx.recv_timeout(timeout).map_err(|e| e.to_string())??;
                resources.insert(address.clone(), String::from_utf8(bytes).map_err(|e| e.to_string())?);
            }
            let source = &resources[&address];
            let parsed = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
            let symbol = parsed
                .descendants()
                .find(|n| n.attribute("id") == Some(fragment.as_str()))
                .ok_or_else(|| format!("missing SVG fragment {fragment}"))?;
            let local_id = format!("lp-external-{root}-{}", replacements.len());
            let range = symbol.range();
            let id_range = symbol.attribute_node("id").ok_or("missing SVG id")?.range_value();
            let mut definition = source[range.clone()].to_string();
            definition.replace_range(id_range.start - range.start..id_range.end - range.start, &local_id);
            definitions.push_str(&definition);
            replacements.push((href, format!("#{local_id}")));
        }
        if replacements.is_empty() {
            continue;
        }
        let node = doc.get_node(root).ok_or("missing SVG root")?;
        let mut content = String::new();
        for child in &node.children {
            doc.get_node(*child).ok_or("missing SVG child")?.write_outer_html(&mut content);
        }
        for (from, to) in replacements {
            content = content.replace(
                &format!("href=\"{}\"", from.replace('&', "&amp;").replace('"', "&quot;")),
                &format!("href=\"{to}\""),
            );
        }
        doc.mutate().set_inner_html(root, &format!("<defs>{definitions}</defs>{content}"));
    }
    Ok(())
}
