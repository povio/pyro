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
            let local_id = format!("pyro-external-{root}-{}", replacements.len());
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
        let local_id = format!("pyro-sprite-{}", doc.svg_sprites.len());
        if let Ok(definition) = definition(&source, &fragment, &local_id) {
            doc.svg_sprites.insert(href, (local_id, definition));
        }
    }
}

/// Whether an SVG `<image>`/`<feImage>` href names a resource usvg cannot load itself (not an
/// element of the document, not a `data:` URL).
pub(crate) fn is_external_image(href: &str) -> bool {
    let href = href.trim();
    !href.is_empty() && !href.starts_with('#') && !href.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:"))
}

/// The bytes at `address`, through `fetcher` (waits up to `timeout`).
fn fetch_image(fetcher: &Arc<dyn Fetcher>, address: &str, timeout: Duration) -> Result<crate::Fetched, String> {
    let (tx, rx) = mpsc::channel();
    fetcher.fetch_final(
        address.to_owned(),
        &crate::FetchOptions::default().for_destination(blitz_traits::net::Destination::Image),
        Box::new(move |result| {
            let _ = tx.send(result);
        }),
    );
    rx.recv_timeout(timeout).map_err(|e| e.to_string())?
}

use blitz_dom::util::SvgResource;
fn retain_source<Key: Eq + std::hash::Hash>(map: &mut HashMap<Key, Arc<SvgResource>>, key: Key, bytes: Arc<SvgResource>, key_bytes: impl Fn(&Key) -> usize) -> bool {
    const BUDGET: usize = 32 << 20;
    let charge = |key: &Key, resource: &SvgResource| key_bytes(key).saturating_add(resource.bytes.capacity()).saturating_add(128);
    let needed = charge(&key, &bytes);
    if needed > BUDGET { return false; }
    if map.len() >= 256 || map.iter().map(|(key, bytes)| charge(key, bytes)).sum::<usize>() > BUDGET - needed { map.clear(); }
    map.insert(key, bytes);
    true
}

/// Resolves the external SVG `<image>`/`<feImage>` hrefs `hrefs` of `doc` into
/// [`blitz_dom::BaseDocument::svg_images`] by reference, which its `<svg>` elements use
/// when they are rendered. Resources come from `sources` (by resource scope and URL) or are
/// fetched into it; one that cannot be loaded is left out (usvg draws nothing for it).
pub(crate) fn resolve_images(
    doc: &mut BaseDocument,
    hrefs: Vec<String>,
    base: &str,
    scope: &str,
    sources: &mut HashMap<(String, String), Arc<SvgResource>>,
    fetcher: &Arc<dyn Fetcher>,
    timeout: Duration,
) {
    const MAX_BYTES: usize = 8 << 20;
    let Ok(base) = url::Url::parse(base) else { return };
    let mut changed = false;
    for href in hrefs {
        if doc.svg_images.read().unwrap().contains_key(&href) {
            continue;
        }
        let Ok(url) = base.join(href.trim()) else { continue };
        let key = (scope.to_owned(), url.to_string());
        let data = match sources.get(&key) {
            Some(data) => data.clone(),
            None => {
                let Ok(response) = fetch_image(fetcher, &key.1, timeout) else { continue };
                if response.body.is_empty() || response.body.len() > MAX_BYTES {
                    continue;
                }
                let data = Arc::new(SvgResource { bytes: Arc::new(response.body), opaque: response.opaque });
                if !retain_source(sources, key, data.clone(), |(scope, url)| scope.capacity() + url.capacity()) { continue; }
                data
            }
        };
        changed |= retain_source(&mut doc.svg_images.write().unwrap(), href, data, String::capacity);
    }
    if changed {
        doc.damage_inline_svgs();
    }
}

/// The external SVG `<image>`/`<feImage>` hrefs of a fresh document. (Walked, not selected: a
/// type selector does not match `feImage` in an HTML document.)
pub(crate) fn image_hrefs(doc: &BaseDocument) -> Vec<String> {
    let mut hrefs: Vec<String> = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else { continue };
        stack.extend(node.children.iter().copied());
        let Some(el) = node.element_data() else { continue };
        if el.name.ns != blitz_dom::ns!(svg) || !matches!(&*el.name.local, "image" | "feImage") {
            continue;
        }
        for attr in el.attrs() {
            if &*attr.name.local == "href" && is_external_image(&attr.value) && !hrefs.contains(&attr.value) {
                hrefs.push(attr.value.clone());
            }
        }
    }
    hrefs
}

#[cfg(test)]
mod image_tests {
    #[test]
    fn absolute_image_with_foreign_base_shares_the_cached_resource() {
        use super::*;
        struct Image;
        impl Fetcher for Image {
            fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
                assert_eq!(url, "https://document.test/image.gif");
                done(Ok(b"GIF89a".to_vec()));
            }
        }
        let fetcher: Arc<dyn Fetcher> = Arc::new(Image);
        let mut sources = HashMap::new();
        let mut document = BaseDocument::new(blitz_dom::DocumentConfig::default());
        let href = "https://document.test/image.gif";
        resolve_images(&mut document, vec![href.into()], "https://base.test/", "document", &mut sources, &fetcher, Duration::from_secs(1));
        assert!(Arc::ptr_eq(&document.svg_images.read().unwrap()[href], sources.values().next().unwrap()));
    }
    #[test]
    fn resource_retention_is_byte_bounded() {
        let mut resources = super::HashMap::new();
        for index in 0..12 {
            let data = super::Arc::new(super::SvgResource { bytes: super::Arc::new(vec![index; 7 << 20]), opaque: false });
            assert!(super::retain_source(&mut resources, index.to_string(), data, String::capacity));
            assert!(resources.values().map(|data| data.bytes.capacity()).sum::<usize>() <= 32 << 20);
        }
    }
}
