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
    let mut sources = HashMap::new();
    let base = url::Url::parse(base).ok();
    for id in doc.query_selector_all("use").map_err(|error| format!("{error:?}"))? {
        let Some(el) = doc.get_node(id).and_then(|node| node.element_data()) else { continue };
        if el.name.ns != blitz_dom::ns!(svg) { continue; }
        let Some(href) = el.attrs().iter().find(|attr| &*attr.name.local == "href").map(|attr| attr.value.clone()) else { continue };
        if !href.starts_with('#') && !href.is_empty() {
            resolve_sprite(doc, href, base.as_ref().ok_or("invalid SVG base URL")?, "", &mut sources, fetcher, timeout)?;
        }
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
fn definition(source: &str, fragment: &str) -> Result<(std::ops::Range<usize>, std::ops::Range<usize>), String> {
    let parsed = roxmltree::Document::parse(source).map_err(|error| error.to_string())?;
    let symbol = parsed.descendants().find(|node| node.attribute("id") == Some(fragment))
        .ok_or_else(|| format!("missing SVG fragment {fragment}"))?;
    let id = symbol.attribute_node("id").ok_or("missing SVG id")?.range_value();
    Ok((symbol.range().start..id.start, id.end..symbol.range().end))
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
    sources: &mut HashMap<(String, String), Arc<SvgResource>>,
    fetcher: &Arc<dyn Fetcher>,
    timeout: Duration,
) {
    let Ok(base) = url::Url::parse(base) else { return };
    for href in hrefs {
        let _ = resolve_sprite(doc, href, &base, scope, sources, fetcher, timeout);
    }
}

fn resolve_sprite(doc: &mut BaseDocument, href: String, base: &url::Url, scope: &str,
    sources: &mut HashMap<(String, String), Arc<SvgResource>>, fetcher: &Arc<dyn Fetcher>, timeout: Duration) -> Result<(), String> {
    if doc.svg_sprites.contains_key(&href) { return Ok(()); }
    let mut url = base.join(&href).map_err(|error| error.to_string())?;
    let fragment = url.fragment().ok_or("external SVG use has no fragment")?.to_owned();
    url.set_fragment(None);
    let key = (scope.to_owned(), url.to_string());
    let source = match sources.get(&key).filter(|resource| resource.belongs_to(&doc.raster_budget)) {
        Some(source) => source.clone(),
        None => {
            let text = fetch_text(fetcher, &key.1, timeout)?;
            if text.len() > 8 << 20 { return Ok(()); }
            let Some(source) = SvgResource::new(text.into_bytes(), false, &doc.raster_budget) else { return Ok(()) };
            let Some(source) = blitz_dom::util::prepare_sprite(source, &doc.raster_budget) else { return Ok(()) };
            let source = Arc::new(source);
            if !retain_source(sources, key, source.clone(), |(scope, url)| scope.capacity() + url.capacity()) { return Ok(()); }
            source
        }
    };
    let (prefix, suffix) = definition(std::str::from_utf8(&source.bytes).map_err(|error| error.to_string())?, &fragment)?;
    if !source.bindings.is_empty() {
        let mut images = doc.svg_images.write().unwrap();
        for (name, image) in &source.bindings {
            if !images.contains_key(name) && !retain_source(&mut images, name.clone(), image.clone(), String::capacity) { return Ok(()); }
        }
    }
    let local = format!("pyro-sprite-{}", doc.svg_sprites.len());
    let Ok(charge) = doc.raster_budget.reserve(href.capacity() + local.capacity() + std::mem::size_of::<(String, blitz_dom::util::SvgSprite)>()) else { return Ok(()) };
    doc.svg_sprites.insert(href, blitz_dom::util::SvgSprite { local, source, prefix, suffix, charge });
    Ok(())
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
    let charge = |key: &Key, resource: &SvgResource| key_bytes(key).saturating_add(resource.retained_bytes()).saturating_add(128);
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
        let data = match sources.get(&key).filter(|source| source.belongs_to(&doc.raster_budget)) {
            Some(data) => data.clone(),
            None => {
                let Ok(response) = fetch_image(fetcher, &key.1, timeout) else { continue };
                if response.body.is_empty() || response.body.len() > MAX_BYTES {
                    continue;
                }
                let Some(data) = SvgResource::new(response.body, response.opaque, &doc.raster_budget) else { continue };
                let data = Arc::new(data);
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
        let page = pyro_raster::PageRasters::default();
        for index in 0..12 {
            let data = super::Arc::new(super::SvgResource::new(vec![index; 7 << 20], false, &page).unwrap());
            assert!(super::retain_source(&mut resources, index.to_string(), data, String::capacity));
            assert!(resources.values().map(|data| data.bytes.capacity()).sum::<usize>() <= 32 << 20);
            assert_eq!(resources.values().map(|data| data.bytes.capacity() + std::mem::size_of::<pyro_raster::SourceLease>()).sum::<usize>(), page.used());
        }
    }
}
