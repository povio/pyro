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
fn fetch_bytes(fetcher: &Arc<dyn Fetcher>, address: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let (tx, rx) = mpsc::channel();
    fetcher.fetch(
        address.to_owned(),
        Box::new(move |result| {
            let _ = tx.send(result);
        }),
    );
    rx.recv_timeout(timeout).map_err(|e| e.to_string())?
}

/// A JPEG as PNGs of its pixels turned as its EXIF orientation says and as stored (usvg ignores
/// the orientation, and decodes JPEGs unlike `<img>`; browsers turn the image unless
/// `image-orientation: none`: WPT `feImage-orientation.tentative`, `svg-image-orientation-*`).
fn jpeg_as_png(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    use image::ImageDecoder as _;
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let stored = image::DynamicImage::from_decoder(decoder).ok()?;
    let png = |pixels: &image::DynamicImage| {
        let mut png = Vec::new();
        pixels.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        Some(png)
    };
    let raw = png(&stored)?;
    if orientation == image::metadata::Orientation::NoTransforms {
        return Some((raw.clone(), raw));
    }
    let mut turned = stored;
    turned.apply_orientation(orientation);
    Some((png(&turned)?, raw))
}

/// `bytes` as `data:` URLs, turned by its EXIF orientation and as stored (the same but for a JPEG).
fn data_urls(bytes: &[u8]) -> (String, String) {
    match jpeg_as_png(bytes) {
        Some((turned, raw)) => (data_url(&turned), data_url(&raw)),
        None => {
            let url = data_url(bytes);
            (url.clone(), url)
        }
    }
}

/// `bytes` as a `data:` URL, its type sniffed (PNG, JPEG, GIF, WebP, else SVG).
fn data_url(bytes: &[u8]) -> String {

    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/svg+xml"
    };
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 32);
    out.push_str("data:");
    out.push_str(mime);
    out.push_str(";base64,");
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Resolves the external SVG `<image>`/`<feImage>` hrefs `hrefs` of `doc` into
/// [`blitz_dom::BaseDocument::svg_images`] (as `data:` URLs), which its `<svg>` elements use
/// when they are rendered. Resources come from `sources` (by resource scope and URL) or are
/// fetched into it; one that cannot be loaded is left out (usvg draws nothing for it).
pub(crate) fn resolve_images(
    doc: &mut BaseDocument,
    hrefs: Vec<String>,
    base: &str,
    scope: &str,
    sources: &mut HashMap<(String, String), Arc<(String, String)>>,
    fetcher: &Arc<dyn Fetcher>,
    timeout: Duration,
) {
    const MAX_BYTES: usize = 8 << 20;
    let Ok(base) = url::Url::parse(base) else { return };
    let mut changed = false;
    for href in hrefs {
        if doc.svg_images.contains_key(&href) {
            continue;
        }
        let Ok(url) = base.join(href.trim()) else { continue };
        let key = (scope.to_owned(), url.to_string());
        let data = match sources.get(&key) {
            Some(data) => data.clone(),
            None => {
                let Ok(bytes) = fetch_bytes(fetcher, &key.1, timeout) else { continue };
                if bytes.is_empty() || bytes.len() > MAX_BYTES {
                    continue;
                }
                if sources.len() >= 256 {
                    sources.clear();
                }
                let data = Arc::new(data_urls(&bytes));
                sources.insert(key, data.clone());
                data
            }
        };
        doc.svg_images.insert(href, (*data).clone());
        changed = true;
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
    fn data_urls_are_base64_with_a_sniffed_type() {
        assert_eq!(super::data_url(b"GIF8a"), "data:image/gif;base64,R0lGOGE=");
        assert_eq!(super::data_url(b"<svg/>"), "data:image/svg+xml;base64,PHN2Zy8+");
    }
}
