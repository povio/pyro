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
                resources.insert(address.clone(), fetch_text(fetcher, &address, timeout)?);
            }
            let local_id = format!("lp-external-{root}-{}", replacements.len());
            definitions.push_str(&definition(&resources[&address], &fragment, &local_id)?);
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

/// The text at `address`, through `fetcher` (waits up to `timeout`).
pub(crate) fn fetch_text(fetcher: &Arc<dyn Fetcher>, address: &str, timeout: Duration) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    fetcher.fetch(
        address.to_owned(),
        Box::new(move |result| {
            let _ = tx.send(result);
        }),
    );
    let bytes = rx.recv_timeout(timeout).map_err(|e| e.to_string())??;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// The element of `source` (an SVG document) with id `fragment`, as source,
/// its id replaced by `local_id`.
pub(crate) fn definition(source: &str, fragment: &str, local_id: &str) -> Result<String, String> {
    let parsed = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
    let symbol = parsed
        .descendants()
        .find(|n| n.attribute("id") == Some(fragment))
        .ok_or_else(|| format!("missing SVG fragment {fragment}"))?;
    let range = symbol.range();
    let id_range = symbol.attribute_node("id").ok_or("missing SVG id")?.range_value();
    let mut definition = source[range.clone()].to_string();
    definition.replace_range(id_range.start - range.start..id_range.end - range.start, local_id);
    Ok(definition)
}

/// Resolves the external `<use href>` values `hrefs` of `doc` (a persistent
/// document) into [`blitz_dom::BaseDocument::svg_sprites`], which the
/// document's `<svg>` elements use when they are rendered; the document's
/// nodes are not changed. Sprite files come from `sources` (by resource
/// scope and URL) or are fetched into it. One that cannot be loaded is
/// left out (its `<use>` draws nothing) and tried again when it changes.
pub(crate) fn resolve_sprites(
    doc: &mut BaseDocument,
    hrefs: Vec<String>,
    base: &str,
    scope: &str,
    sources: &mut HashMap<(String, String), Arc<String>>,
    fetcher: &Arc<dyn Fetcher>,
    timeout: Duration,
) {
    let Ok(base) = url::Url::parse(base) else { return };
    for href in hrefs {
        if doc.svg_sprites.contains_key(&href) {
            continue;
        }
        let Ok(mut url) = base.join(&href) else { continue };
        let Some(fragment) = url.fragment().map(str::to_owned) else { continue };
        url.set_fragment(None);
        let key = (scope.to_owned(), url.to_string());
        let source = match sources.get(&key) {
            Some(source) => source.clone(),
            None => {
                let Ok(text) = fetch_text(fetcher, &key.1, timeout) else { continue };
                if sources.len() >= 256 {
                    sources.clear();
                }
                let text = Arc::new(text);
                sources.insert(key, text.clone());
                text
            }
        };
        let local_id = format!("lp-sprite-{}", doc.svg_sprites.len());
        if let Ok(definition) = definition(&source, &fragment, &local_id) {
            doc.svg_sprites.insert(href, (local_id, definition));
        }
    }
}
