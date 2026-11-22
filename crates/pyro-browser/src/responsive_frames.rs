//! pyro: css-sizing-4 responsive iframes (https://drafts.csswg.org/css-sizing-4/#responsive-iframes):
//! an iframe of `frame-sizing: content-height` (`content-width`, or a logical value) showing a
//! document that opted in with `<meta name="responsive-embedded-sizing" content="allow-origins=…">`,
//! whose list admits the embedder's origin, takes that document's content size as its natural size.
//!
//! Each document's [`source`] sets those sizes on its iframes in the page's DOM store
//! (`Dom::set_frame_content_size`, which the render tree carries to the engine) whenever a script is
//! about to read its layout or it renders ([`pyro_webapi::PageDom::sync_responsive_frames`]), so an
//! embedder's `offsetHeight` and ResizeObservers see the size the renderer paints.
//!
//! Not modelled: Chrome resizes only at the embedded document's `DOMContentLoaded`, `load` and
//! `window.requestResize()`; here the size follows the content at every read.

use std::cell::RefCell;
use std::rc::Rc;

use crate::frames::{FrameHost, FrameKey};
use pyro_dom::NodeId;

/// The size an iframe was last given and the origin of the document that gave it.
#[derive(Clone)]
struct SizedFrame {
    owner: NodeId,
    size: (f32, f32),
    origin: Option<String>,
}

/// The update of the frame `key`'s document's responsively embedded child frames
/// ([`pyro_webapi::PageDom::set_responsive_frames_source`]).
pub(crate) fn source(host: &Rc<FrameHost>, key: FrameKey) -> pyro_webapi::ResponsiveFramesSource {
    let host = Rc::downgrade(host);
    let sized: RefCell<Vec<SizedFrame>> = Default::default();
    Rc::new(move || {
        let Some(host) = host.upgrade() else { return };
        sync(&host, key, &sized);
    })
}

/// Sets the content size of each child frame of `parent` showing a responsively embedded document
/// its container may take the size of, and clears the others'. While a child frame's new document
/// is still parsing its head (no body yet, so it may opt in yet), a document of the same origin as
/// the last that sized the iframe keeps that size (Chrome: a same-origin navigation keeps the
/// iframe's size; a cross-origin one resets it; WPT css-sizing
/// `responsive-iframe/responsive-iframe-navigation`).
fn sync(host: &FrameHost, parent: FrameKey, sized: &RefCell<Vec<SizedFrame>>) {
    let children = host.child_keys(parent);
    if children.is_empty() && sized.borrow().is_empty() {
        return;
    }
    let Some(parent_dom) = host.page_dom(parent) else { return };
    let mut sizes: Vec<(NodeId, Option<(f32, f32)>)> = Vec::new();
    let mut next: Vec<SizedFrame> = Vec::new();
    let mut parent_origin: Option<Option<String>> = None;
    for key in children {
        let Some((owner, page_dom, document, url, opaque)) = host
            .with_frame(key, |f| (f.owner, f.page_dom.clone(), f.document, f.url.clone(), f.sandbox.opaque))
        else {
            continue;
        };
        let Some(owner) = owner else { continue };
        let previous = sized.borrow().iter().find(|s| s.owner == owner).cloned();
        let (Some(page_dom), Some(document)) = (page_dom, document) else {
            keep(owner, previous, &mut sizes, &mut next);
            continue;
        };
        let state = page_dom.dom.try_borrow().ok().map(|dom| {
            (
                dom.is_responsively_embedded(document),
                dom.responsive_embedding_policy(document).map(str::to_owned),
                dom.has_body(document),
            )
        });
        // (A store borrowed elsewhere: as it was.)
        let Some((responsive, policy, has_body)) = state else {
            keep(owner, previous, &mut sizes, &mut next);
            continue;
        };
        // (The origin is asked for only here: this runs after every task of a page with frames.)
        let child_origin = || crate::bindings::frame_origin(host, key, &url, opaque);
        if responsive {
            let child_origin = child_origin();
            let embedder = parent_origin.get_or_insert_with(|| embedder_origin(host, parent, &parent_dom)).clone();
            if policy.as_deref().is_some_and(|policy| allows_embedder(policy, embedder.as_deref(), child_origin.as_deref())) {
                let size = page_dom.try_layout().and_then(|layout| {
                    let dom = page_dom.dom.try_borrow().ok()?;
                    layout.document_content_size(&dom, document)
                });
                match size {
                    Some((width, height)) => {
                        let size = (width as f32, height as f32);
                        sizes.push((owner, Some(size)));
                        next.push(SizedFrame { owner, size, origin: child_origin });
                    }
                    None => keep(owner, previous, &mut sizes, &mut next),
                }
                continue;
            }
        } else if !has_body && previous.is_some() {
            let origin = child_origin();
            keep(owner, previous.filter(|p| origin.is_some() && p.origin == origin), &mut sizes, &mut next);
            continue;
        }
        sizes.push((owner, None));
    }
    // Iframes that no longer show a frame of this document.
    let gone: Vec<NodeId> =
        sized.borrow().iter().map(|s| s.owner).filter(|owner| !sizes.iter().any(|(o, _)| o == owner)).collect();
    sizes.extend(gone.into_iter().map(|owner| (owner, None)));
    *sized.borrow_mut() = next;
    let Ok(mut dom) = parent_dom.dom.try_borrow_mut() else { return };
    for (owner, size) in sizes {
        dom.set_frame_content_size(owner, size);
    }
}

/// The iframe `owner` keeps the size `previous` gave it, if any (none otherwise).
fn keep(
    owner: NodeId,
    previous: Option<SizedFrame>,
    sizes: &mut Vec<(NodeId, Option<(f32, f32)>)>,
    next: &mut Vec<SizedFrame>,
) {
    match previous {
        Some(previous) => {
            sizes.push((owner, Some(previous.size)));
            next.push(previous);
        }
        None => sizes.push((owner, None)),
    }
}

/// The origin of the frame `parent`'s document (`None`: opaque).
fn embedder_origin(host: &FrameHost, parent: FrameKey, page_dom: &pyro_webapi::PageDom) -> Option<String> {
    let url = page_dom
        .dom
        .try_borrow()
        .ok()?
        .document_url_parsed(page_dom.document())
        .ok()
        .flatten()
        .map(|u| u.to_string())
        .unwrap_or_default();
    crate::bindings::frame_origin(host, parent, &url, page_dom.has_opaque_origin())
}

/// Whether a responsive embedded sizing `<meta>`'s `content` admits the embedder of origin
/// `embedder`: `allow-origins=` (exactly so, case-sensitive) followed by a source list in CSP's
/// syntax (`*`, `'self'`: the embedded document's own origin `own`, `'none'`, scheme sources and
/// host sources with `*.` subdomain wildcards and `:*` ports), one of which matches. An opaque
/// embedder is never admitted. (WPT css-sizing `responsive-iframe/responsive-iframe-allow-origins`.)
pub(crate) fn allows_embedder(content: &str, embedder: Option<&str>, own: Option<&str>) -> bool {
    let Some(list) = content.trim_start_matches(|c: char| c.is_ascii_whitespace()).strip_prefix("allow-origins=")
    else {
        return false;
    };
    let Some(embedder) = embedder.and_then(|origin| url::Url::parse(origin).ok()) else { return false };
    let own = own.and_then(|origin| url::Url::parse(origin).ok());
    list.split_ascii_whitespace().any(|source| source_matches(source, &embedder, own.as_ref()))
}

fn scheme_matches(source: &str, scheme: &str) -> bool {
    // (CSP: an `http` source admits its secure upgrade too.)
    source == scheme || (source == "http" && scheme == "https")
}

fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn source_matches(source: &str, embedder: &url::Url, own: Option<&url::Url>) -> bool {
    let source = source.to_ascii_lowercase();
    match source.as_str() {
        "*" => matches!(embedder.scheme(), "http" | "https") || own.is_some_and(|own| own.scheme() == embedder.scheme()),
        "'self'" => own.is_some_and(|own| own.origin() == embedder.origin()),
        _ if source.starts_with('\'') => false,
        _ => {
            // A scheme source (`https:`).
            if let Some(scheme) = source.strip_suffix(':').filter(|s| is_scheme(s)) {
                return scheme_matches(scheme, embedder.scheme());
            }
            host_source_matches(&source, embedder, own)
        }
    }
}

/// A host source: `[scheme "://"] host [":" port] [path]` (a path is not compared: an origin has none).
fn host_source_matches(source: &str, embedder: &url::Url, own: Option<&url::Url>) -> bool {
    let (scheme, rest) = match source.split_once("://") {
        Some((scheme, rest)) if is_scheme(scheme) => (Some(scheme), rest),
        Some(_) => return false,
        None => (None, source),
    };
    match scheme {
        Some(scheme) if !scheme_matches(scheme, embedder.scheme()) => return false,
        // Without a scheme: the embedded document's own (or a secure upgrade of it).
        None if !own.map_or(matches!(embedder.scheme(), "http" | "https"), |own| scheme_matches(own.scheme(), embedder.scheme())) => {
            return false
        }
        _ => {}
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let Some(embedder_host) = embedder.host_str().map(str::to_ascii_lowercase) else { return false };
    let host_ok = if host == "*" {
        true
    } else if let Some(suffix) = host.strip_prefix("*.") {
        embedder_host.len() > suffix.len() + 1 && embedder_host.ends_with(&format!(".{suffix}"))
    } else {
        host == embedder_host
    };
    if !host_ok {
        return false;
    }
    let embedder_port = embedder.port_or_known_default();
    match port {
        Some("*") => true,
        Some(port) => port.parse::<u16>().ok().is_some_and(|port| Some(port) == embedder_port),
        None => {
            let default = match scheme.unwrap_or(embedder.scheme()) {
                "http" | "ws" => Some(80),
                "https" | "wss" => Some(443),
                _ => None,
            };
            default.is_some_and(|default| Some(default) == embedder_port)
                || (scheme == Some("http") && embedder.scheme() == "https" && embedder_port == Some(443))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::allows_embedder;

    const PARENT: &str = "http://web-platform.test:8000";
    const SUB: &str = "http://www1.web-platform.test:8000";
    const OTHER: &str = "http://not-web-platform.test:8000";

    #[test]
    fn allow_origins_lists_admit_their_embedders() {
        let same = |content: &str| allows_embedder(content, Some(PARENT), Some(PARENT));
        assert!(same("allow-origins=*"));
        assert!(same(&format!("allow-origins={PARENT}")));
        assert!(!same("allow-origins=https://invalid-origin.example"));
        assert!(!same("allow-origins=http://*.web-platform.test:8000"));
        assert!(same("allow-origins=http:"));
        assert!(!same("allow-origins=https:"));
        assert!(same("allow-origins='self'"));
        assert!(!same("allow-origins='none'"));
        assert!(same(&format!("allow-origins=https://invalid-origin.example {PARENT}")));
        assert!(!same("*"));
        assert!(!same("alloworigins=*"));
        assert!(!same("ALLOW-ORIGINS=*"));
        assert!(!same("allow-origins= "));
        assert!(!same("allow-origins="));
        // A subdomain embedder and a wildcard host.
        assert!(allows_embedder("allow-origins=http://*.web-platform.test:8000", Some(SUB), Some(SUB)));
        assert!(allows_embedder("allow-origins=http://*.web-platform.test:8000", Some(SUB), Some(OTHER)));
        // A cross-origin document: `'self'` and its own origin name it, not the embedder.
        let cross = |content: &str| allows_embedder(content, Some(PARENT), Some(OTHER));
        assert!(cross("allow-origins=*"));
        assert!(cross(&format!("allow-origins={PARENT}")));
        assert!(!cross(&format!("allow-origins={OTHER}")));
        assert!(!cross("allow-origins='self'"));
        // An opaque embedder.
        assert!(!allows_embedder("allow-origins=*", None, Some(PARENT)));
    }
}
