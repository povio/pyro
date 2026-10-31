//! Blitz geometry for the engine's pages (`lp_layout`): sub-resources resolve
//! against the document's URL and load synchronously when they are local:
//! `data:` URLs, `file:` URLs and `serve --static` roots. Network ones are
//! fetched in the background through the browser's network client and kept
//! per process by URL: a layout never waits for the network, and later
//! documents lay out with them at once.

use std::sync::Arc;

use lp_render::Fetcher;

pub(crate) use lp_layout::{set_fork_template, take_stats};

/// Network sub-resources loaded for layouts, by URL (process-wide).
static NETWORK_BODIES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Arc<Vec<u8>>>>> =
    std::sync::LazyLock::new(Default::default);

/// Makes this thread's new documents take their geometry from Blitz.
pub(crate) fn install(statics: lp_net::StaticFiles, net: Option<lp_net::NetClient>) {
    lp_layout::install(Arc::new(LocalFiles { statics, net }));
}

/// Loads sub-resources synchronously: `data:` and `file:` URLs and
/// `serve --static` roots from disk, the rest through `net` (cached).
struct LocalFiles {
    statics: lp_net::StaticFiles,
    net: Option<lp_net::NetClient>,
}

impl LocalFiles {
    /// A network body from the process cache. A miss starts the fetch in
    /// the background (into the cache) and answers `None` now: a layout
    /// never waits for the network (a request held by interception would
    /// need this thread), and the next documents get the body at once.
    fn network(&self, url: &lp_net::Url) -> Option<Vec<u8>> {
        let key = url.as_str().to_owned();
        {
            let mut cache = NETWORK_BODIES.lock().ok()?;
            match cache.get(&key) {
                Some(b) if !b.is_empty() => return Some(b.to_vec()),
                Some(_) => return None,
                // An empty entry marks the fetch in flight (or failed).
                None => cache.insert(key.clone(), Arc::new(Vec::new())),
            };
        }
        let net = self.net.as_ref()?;
        let path = url.path().to_ascii_lowercase();
        let destination = if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
            lp_net::Destination::Font
        } else if path.ends_with(".css") || url.host_str() == Some("fonts.googleapis.com") {
            lp_net::Destination::Style
        } else {
            lp_net::Destination::Image
        };
        let mut body = Vec::new();
        let mut ok = false;
        net.fetch(
            lp_net::Request::get(url.clone(), destination),
            Box::new(move |ev| match ev {
                lp_net::ResponseEvent::Response(h) => ok = h.status.is_success(),
                lp_net::ResponseEvent::Data(d) => body.extend_from_slice(&d),
                lp_net::ResponseEvent::Finished { .. } if ok => {
                    if let Ok(mut cache) = NETWORK_BODIES.lock() {
                        cache.insert(key.clone(), Arc::new(std::mem::take(&mut body)));
                    }
                }
                _ => {}
            }),
        );
        None
    }
}

impl Fetcher for LocalFiles {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = (|| {
            let u = lp_net::Url::parse(&url).ok()?;
            match u.scheme() {
                "data" => lp_net::data_url::parse(&u).map(|(_, b)| b.to_vec()),
                "file" => std::fs::read(u.to_file_path().ok()?).ok(),
                _ => match self.statics.lookup(&u, false) {
                    Some(hit) => Some(hit.file.body.to_vec()),
                    None => match self.statics.resolve(&u, false) {
                        Some(hit) => std::fs::read(hit.path).ok(),
                        None if matches!(u.scheme(), "http" | "https") => self.network(&u),
                        None => None,
                    },
                },
            }
        })();
        done(body.ok_or_else(|| format!("{url}: not a local resource")));
    }
}

/// `Page.printToPDF` from the page document's own Blitz document (the one
/// its layout queries were answered from), on this thread. `None`: the
/// document has no Blitz geometry (print through the render thread).
pub(crate) fn print_pdf(
    page_dom: &lp_webapi::PageDom,
    opts: &lp_render::PdfOptions,
) -> Option<Result<lp_render::PdfOutput, String>> {
    lp_layout::print_pdf(&page_dom.dom.borrow(), &page_dom.layout.borrow(), opts)
}
