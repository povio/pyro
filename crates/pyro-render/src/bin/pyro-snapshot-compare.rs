//! JSON-lines worker: compare captured visual states outside browser/fork processes.
use pyro_render::{
    worker::{Baseline, Job, MatchOptions, RenderWorker},
    Area, Fetcher, Snapshot,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capture {
    html: String,
    base_url: String,
    sheets: HashMap<String, String>,
    width: u32,
    height: u32,
    device_scale_factor: f64,
    dark: bool,
    scroll_x: f64,
    scroll_y: f64,
    /// The document facts ([`Snapshot::xml_document`], [`Snapshot::content_language`]); absent in
    /// captures written before they were recorded.
    #[serde(default)]
    xml_document: bool,
    #[serde(default)]
    quirks_mode: bool,
    #[serde(default)]
    limited_quirks_mode: bool,
    #[serde(default)]
    content_language: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: u64,
    snapshot: Capture,
    baseline: PathBuf,
    asset_root: PathBuf,
    asset_origin: String,
    #[serde(default)]
    update: bool,
    #[serde(default)]
    element: bool,
    #[serde(default)]
    full_page: bool,
}
struct Assets {
    root: PathBuf,
    origin: String,
    errors: Arc<Mutex<Vec<String>>>,
}
impl Fetcher for Assets {
    fn fetch(&self, address: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let result = (|| {
            if address.starts_with("data:") {
                let data = data_url::DataUrl::process(&address).map_err(|e| format!("invalid data URL: {e:?}"))?;
                return data
                    .decode_to_vec()
                    .map(|(bytes, _)| bytes)
                    .map_err(|e| format!("invalid data URL payload: {e:?}"));
            }
            let url = url::Url::parse(&address).map_err(|e| e.to_string())?;
            if url.origin().ascii_serialization() != self.origin {
                return Err(format!("uncaptured resource origin: {}", url.origin().ascii_serialization()));
            }
            let relative = url.path().trim_start_matches('/');
            let path = self.root.join(relative).canonicalize().map_err(|e| format!("asset {relative}: {e}"))?;
            let root = self.root.canonicalize().map_err(|e| e.to_string())?;
            if !path.starts_with(root) {
                return Err("asset escaped snapshot root".into());
            }
            std::fs::read(path).map_err(|e| e.to_string())
        })();
        if let Err(error) = &result {
            self.errors.lock().unwrap().push(error.clone());
        }
        done(result);
    }
}
fn compare(worker: &RenderWorker, request: Request) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let existed = request.baseline.exists();
    let baseline = if !request.update && existed {
        let png = std::fs::read(&request.baseline).map_err(|e| e.to_string())?;
        Some(Arc::new(Baseline::from_png(png, None)))
    } else {
        None
    };
    let s = request.snapshot;
    let snapshot = Snapshot {
        html: s.html,
        base_url: s.base_url,
        sheets: s.sheets,
        width: s.width,
        height: s.height,
        device_scale_factor: s.device_scale_factor,
        dark: s.dark,
        print: false,
        scroll_x: s.scroll_x,
        scroll_y: s.scroll_y,
        resource_scope: request.id.to_string(),
        live: None,
        element_scroll: None,
        sticky_offsets: None,
        scroll_targets: None,
        background: None,
        canvases: Vec::new(),
        animations: None,
        xml_document: s.xml_document,
        quirks_mode: s.quirks_mode,
        limited_quirks_mode: s.limited_quirks_mode,
        content_language: s.content_language,
        selection: None,
        highlights: Vec::new(),
        control_selection: None,
    };
    let errors = Arc::new(Mutex::new(Vec::new()));
    let fetcher = Arc::new(Assets { root: request.asset_root, origin: request.asset_origin, errors: errors.clone() });
    let area = if request.element {
        Area::Element
    } else if request.full_page {
        Area::FullPage
    } else {
        Area::Viewport
    };
    let (tx, rx) = mpsc::channel();
    worker
        .submit(Job::Match {
            snapshot,
            area,
            baseline,
            opts: MatchOptions::default(),
            fetcher,
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        })
        .map_err(|_| "render worker exited")?;
    let outcome = rx.recv_timeout(Duration::from_secs(15)).map_err(|e| e.to_string())??;
    let errors = errors.lock().unwrap();
    if !errors.is_empty() {
        return Err(format!("snapshot resources failed: {}", errors.join("; ")));
    }
    let actual = request.baseline.with_extension("actual.png");
    let diff = request.baseline.with_extension("diff.png");
    if let Some(parent) = request.baseline.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if outcome.recorded && request.update {
        std::fs::write(&request.baseline, outcome.actual.as_ref().ok_or("recording has no pixels")?)
            .map_err(|e| e.to_string())?;
    } else if !outcome.pass || outcome.recorded {
        if let Some(bytes) = &outcome.actual {
            std::fs::write(&actual, bytes).map_err(|e| e.to_string())?;
        }
        if let Some(bytes) = &outcome.diff {
            std::fs::write(&diff, bytes).map_err(|e| e.to_string())?;
        }
    }
    let pass = outcome.pass && (!outcome.recorded || request.update);
    if pass {
        let _ = std::fs::remove_file(&actual);
        let _ = std::fs::remove_file(&diff);
    }
    Ok(json!({"pass":pass,"recorded":outcome.recorded,"hashHit":outcome.hash_hit,"diffPixels":outcome.diff_pixels,
        "width":outcome.width,"height":outcome.height,"ms":started.elapsed().as_secs_f64()*1000.0,
        "baseline":request.baseline,"actual":actual,"diff":diff,"missingBaseline":!existed&&!request.update}))
}
fn main() {
    let worker = RenderWorker::spawn().expect("render worker");
    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let parsed = serde_json::from_str::<Request>(&line);
        let (id, result) = match parsed {
            Ok(request) => (request.id, compare(&worker, request)),
            Err(e) => (0, Err(e.to_string())),
        };
        let reply = match result {
            Ok(value) => json!({"id":id,"value":value}),
            Err(error) => json!({"id":id,"error":error}),
        };
        println!("{reply}");
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fetch(address: &str) -> Result<Vec<u8>, String> {
        let assets =
            Assets { root: PathBuf::new(), origin: "http://test".into(), errors: Arc::new(Mutex::new(Vec::new())) };
        let (send, receive) = mpsc::channel();
        assets.fetch(address.into(), Box::new(move |result| send.send(result).unwrap()));
        receive.recv().unwrap()
    }
    #[test]
    fn embedded_resources_decode_without_network_or_relaxing_origin_checks() {
        assert_eq!(fetch("data:image/png;base64,AQIDBA==").unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(fetch("data:image/svg+xml,%3Csvg%3E%3C/svg%3E").unwrap(), b"<svg></svg>");
        assert!(fetch("https://external.test/image.png").unwrap_err().contains("uncaptured resource origin"));
        assert!(fetch("data:image/png;base64,?").is_err());
    }
}
