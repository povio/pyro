//! Files served in place of an origin (`serve --static ORIGIN=DIR`).
//!
//! A front-end build (`dist/`) is answered from disk for GET/HEAD requests
//! whose origin matches, so no development or static server has to listen
//! there. Resolution follows a static file server: the URL path under `DIR`,
//! a directory's `index.html`, and for document (navigation) requests any
//! path without a file falls back to `DIR/index.html` (single-page-app
//! routing). A request the overlay cannot answer (no file, non-GET, other
//! origin) goes to the network as usual, so API calls on the same origin
//! still reach their server. With SPA fallback disabled, all missing paths,
//! including navigation URLs, reach the network; this is the files-only overlay
//! mode for an origin whose server owns both app routes and dynamic documents.
//!
//! The overlay runs after CDP `Fetch` interception (routes still see and can
//! override these requests) and before the HTTP cache; responses are not
//! cached (the file system is the cache). Only `..` traversal is refused;
//! symlinks inside `DIR` are the operator's choice. Files are read into
//! memory at startup (up to a budget) and answered from there without a file
//! system call; a path that was not read then (a file added later, one past
//! the budget, or none at all) is looked up on disk.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use url::Url;

/// Total bytes of files held in memory per process; larger builds fall back
/// to disk for the files that did not fit.
pub const PRELOAD_BUDGET: usize = 512 * 1024 * 1024;

/// A file of a root held in memory (read once at startup).
#[derive(Debug)]
pub struct StaticFile {
    pub path: PathBuf,
    pub body: Bytes,
    pub content_type: &'static str,
}

type FileMap = HashMap<String, Arc<StaticFile>>;

#[derive(Debug)]
struct StaticRoot {
    /// Storage-key form of the origin: `scheme://host[:port]`.
    origin: String,
    dir: PathBuf,
    /// Relative path (`/`-separated, no leading slash) → file contents.
    /// Read on a background thread so `serve` binds its port without
    /// waiting; the first lookup waits for it instead.
    files: Preloaded,
}

#[derive(Debug, Default)]
struct Preloaded {
    files: std::sync::Mutex<Option<Arc<FileMap>>>,
    ready: std::sync::Condvar,
}

impl Preloaded {
    fn set(&self, files: FileMap) {
        *self.files.lock().unwrap() = Some(Arc::new(files));
        self.ready.notify_all();
    }

    fn get(&self) -> Arc<FileMap> {
        let mut guard = self.files.lock().unwrap();
        while guard.is_none() {
            guard = self.ready.wait(guard).unwrap();
        }
        guard.as_ref().expect("set above").clone()
    }

    /// The files once read, without waiting.
    fn ready(&self) -> Option<Arc<FileMap>> {
        self.files.lock().unwrap().clone()
    }
}

/// The preloaded file at `relative`, or its directory's `index.html`.
fn memory_file<'a>(files: &'a FileMap, relative: &Path) -> Option<&'a Arc<StaticFile>> {
    let key = relative.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
    if key.is_empty() {
        return files.get("index.html");
    }
    files.get(&key).or_else(|| files.get(&format!("{key}/index.html")))
}

/// The configured static roots of a process; `Clone` shares them.
#[derive(Clone, Debug)]
pub struct StaticFiles {
    roots: Arc<Vec<StaticRoot>>,
    spa_fallback: bool,
}

impl Default for StaticFiles {
    fn default() -> Self {
        Self { roots: Arc::new(Vec::new()), spa_fallback: true }
    }
}

/// A request answered from memory (`StaticFiles::lookup`).
#[derive(Clone, Debug)]
pub struct StaticMemoryHit {
    pub file: Arc<StaticFile>,
    /// `index.html` answered a document request for a path with no file.
    pub spa_fallback: bool,
}

/// A file the overlay answers a request with.
#[derive(Debug, PartialEq, Eq)]
pub struct StaticHit {
    pub path: PathBuf,
    /// `index.html` answered a document request for a path with no file.
    pub spa_fallback: bool,
}

impl StaticFiles {
    /// `roots`: (origin or URL, directory). Origins are normalized to
    /// `scheme://host[:port]`; a later root for the same origin replaces an
    /// earlier one. The directory must exist.
    pub fn new(roots: Vec<(String, PathBuf)>) -> Result<Self, String> {
        let mut out: Vec<StaticRoot> = Vec::new();
        for (origin, dir) in roots {
            let url = Url::parse(origin.trim()).map_err(|e| format!("'{origin}': not an origin or URL ({e})"))?;
            let key = match url.origin() {
                url::Origin::Tuple(..) => url.origin().ascii_serialization(),
                url::Origin::Opaque(_) => return Err(format!("'{origin}': not a tuple origin")),
            };
            if !dir.is_dir() {
                return Err(format!("'{}': not a directory", dir.display()));
            }
            match out.iter_mut().find(|r| r.origin == key) {
                Some(root) => root.dir = dir,
                None => out.push(StaticRoot { origin: key, dir, files: Preloaded::default() }),
            }
        }
        let roots = Arc::new(out);
        let reader = roots.clone();
        // Reads happen off the caller's thread; a thread spawn failure means
        // reading inline (still correct, just not overlapped).
        let read = move || {
            let mut budget = PRELOAD_BUDGET;
            for root in reader.iter() {
                let files = preload(&root.dir, budget);
                budget = budget.saturating_sub(files.values().map(|f| f.body.len()).sum());
                root.files.set(files);
            }
        };
        if !roots.is_empty()
            && std::thread::Builder::new().name("pyro-static-preload".into()).spawn(read.clone()).is_err()
        {
            read();
        }
        Ok(Self { roots, spa_fallback: true })
    }

    /// When disabled, missing navigation paths fall through to normal HTTP,
    /// just like missing subresources. Existing files still come from the mount.
    pub fn with_spa_fallback(mut self, enabled: bool) -> Self {
        self.spa_fallback = enabled;
        self
    }

    /// (files, bytes) held in memory across all roots; waits for the preload.
    pub fn preloaded(&self) -> (usize, usize) {
        let maps: Vec<Arc<FileMap>> = self.roots.iter().map(|r| r.files.get()).collect();
        let files = maps.iter().map(|m| m.len()).sum();
        let bytes = maps.iter().flat_map(|m| m.values()).map(|f| f.body.len()).sum();
        (files, bytes)
    }

    /// The in-memory file answering `url`, if any (same resolution rules as
    /// `resolve`, without touching the file system). A root's file that did
    /// not fit the preload budget is only found by `resolve`.
    pub fn lookup(&self, url: &Url, document: bool) -> Option<StaticMemoryHit> {
        let origin = url.origin();
        if !matches!(origin, url::Origin::Tuple(..)) {
            return None;
        }
        let origin = origin.ascii_serialization();
        let root = self.roots.iter().find(|r| r.origin == origin)?;
        let relative = relative_path(url.path())?;
        let files = root.files.get();
        if let Some(file) = memory_file(&files, &relative) {
            return Some(StaticMemoryHit { file: file.clone(), spa_fallback: false });
        }
        if document && self.spa_fallback {
            if let Some(file) = files.get("index.html") {
                return Some(StaticMemoryHit { file: file.clone(), spa_fallback: true });
            }
        }
        None
    }

    /// Every file held in memory with the URL that requests it, in the order
    /// of the roots and of the paths in each; waits for the preload.
    pub fn memory_files(&self) -> Vec<(Url, Arc<StaticFile>)> {
        let mut out = Vec::new();
        for root in self.roots.iter() {
            let Ok(origin) = Url::parse(&root.origin) else { continue };
            let files = root.files.get();
            let mut keys: Vec<&String> = files.keys().collect();
            keys.sort();
            for key in keys {
                let mut url = origin.clone();
                // Each segment percent-encoded, as a URL for the file writes it.
                match url.path_segments_mut() {
                    Ok(mut segments) => {
                        segments.clear().extend(key.split('/'));
                    }
                    Err(()) => continue,
                }
                out.push((url, files[key].clone()));
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// (origin, directory) per root.
    pub fn summary(&self) -> Vec<(String, PathBuf)> {
        self.roots.iter().map(|r| (r.origin.clone(), r.dir.clone())).collect()
    }

    /// The file answering `url`, if its origin has a root and a file exists.
    /// `document`: a navigation, eligible for the `index.html` fallback.
    pub fn resolve(&self, url: &Url, document: bool) -> Option<StaticHit> {
        let origin = url.origin();
        if !matches!(origin, url::Origin::Tuple(..)) {
            return None;
        }
        let origin = origin.ascii_serialization();
        let root = self.roots.iter().find(|r| r.origin == origin)?;
        let relative = relative_path(url.path())?;
        // A file read at startup is answered without a file system call.
        if let Some(file) = root.files.ready().as_deref().and_then(|files| memory_file(files, &relative)) {
            return Some(StaticHit { path: file.path.clone(), spa_fallback: false });
        }
        // One call for a path that has nothing (an API request on the same
        // origin): its metadata, not `is_dir` and then `is_file`.
        let candidate = root.dir.join(&relative);
        let candidate = match std::fs::metadata(&candidate) {
            Ok(meta) if meta.is_dir() => Some(candidate.join("index.html")).filter(|index| index.is_file()),
            Ok(meta) if meta.is_file() => Some(candidate),
            _ => None,
        };
        if let Some(candidate) = candidate {
            return Some(StaticHit { path: candidate, spa_fallback: false });
        }
        if document && self.spa_fallback {
            let index = root.dir.join("index.html");
            if index.is_file() {
                return Some(StaticHit { path: index, spa_fallback: true });
            }
        }
        None
    }
}

/// Reads every regular file under `dir` (symlinks followed) into memory, up
/// to `budget` bytes; files that do not fit are left to disk.
fn preload(dir: &Path, budget: usize) -> HashMap<String, Arc<StaticFile>> {
    let mut files = HashMap::new();
    let mut used = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() || used.saturating_add(meta.len() as usize) > budget {
                continue;
            }
            let Ok(body) = std::fs::read(&path) else { continue };
            let Ok(relative) = path.strip_prefix(dir) else { continue };
            let Some(key) = relative.to_str() else { continue };
            used += body.len();
            let content_type = content_type(&path);
            files.insert(
                key.replace(std::path::MAIN_SEPARATOR, "/"),
                Arc::new(StaticFile { path, body: Bytes::from(body), content_type }),
            );
        }
    }
    files
}

/// The path under the root for a URL path: percent-decoded, no `..`, no
/// absolute or prefix components. `None` refuses the request to the overlay.
fn relative_path(url_path: &str) -> Option<PathBuf> {
    let decoded = percent_encoding::percent_decode_str(url_path).decode_utf8().ok()?;
    let mut out = PathBuf::new();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// `Content-Type` for a file by extension; `application/octet-stream` for
/// unknown ones.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("csv") => "text/csv",
        Some("md") => "text/markdown; charset=utf-8",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("zip") => "application/zip",
        Some("xml") => "application/xml",
        Some("wasm") => "application/wasm",
        Some("pdf") => "application/pdf",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("pyro-static-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (TempDir, StaticFiles) {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>index").unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "1").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "s").unwrap();
        let files = StaticFiles::new(vec![("http://app.test:3000/".into(), dir.path().to_path_buf())]).unwrap();
        (dir, files)
    }

    #[test]
    fn resolves_files_directories_and_spa_fallback() {
        let (dir, files) = fixture();
        let root = dir.path();
        let hit = |u: &str, doc: bool| files.resolve(&Url::parse(u).unwrap(), doc);
        assert_eq!(
            hit("http://app.test:3000/assets/app.js?v=1#x", false),
            Some(StaticHit { path: root.join("assets/app.js"), spa_fallback: false })
        );
        assert_eq!(
            hit("http://app.test:3000/", true),
            Some(StaticHit { path: root.join("index.html"), spa_fallback: false })
        );
        // A directory without index.html is not a file.
        assert_eq!(hit("http://app.test:3000/assets", false), None);
        std::fs::write(root.join("assets/index.html"), "<p>assets").unwrap();
        assert_eq!(hit("http://app.test:3000/assets/", false).map(|h| h.path), Some(root.join("assets/index.html")));
        assert_eq!(
            hit("http://app.test:3000/dashboard/settings", true),
            Some(StaticHit { path: root.join("index.html"), spa_fallback: true })
        );
        assert_eq!(hit("http://app.test:3000/dashboard/settings", false), None);
        assert_eq!(hit("http://app.test:3000/assets/missing.js", false), None);
        // Other origins (port, scheme, host) are not served.
        assert_eq!(hit("http://app.test:3001/index.html", true), None);
        assert_eq!(hit("http://other.test:3000/index.html", true), None);
        // The URL parser resolves `..` and `%2e%2e` segments within the
        // origin's path, so they never leave the root.
        assert_eq!(
            hit("http://app.test:3000/assets/%2e%2e/secret.txt", true),
            Some(StaticHit { path: root.join("secret.txt"), spa_fallback: false })
        );
        assert_eq!(
            hit("http://app.test:3000/%2e%2e/%2e%2e/etc/passwd", true),
            Some(StaticHit { path: root.join("index.html"), spa_fallback: true })
        );
        // A `..` that only appears after percent-decoding is refused outright,
        // even for a document request.
        assert_eq!(hit("http://app.test:3000/assets/..%2fsecret.txt", true), None);
        assert_eq!(hit("http://app.test:3000/assets/..%2f..%2fetc%2fpasswd", true), None);
    }

    #[test]
    fn lookup_serves_preloaded_files_without_the_file_system() {
        let (dir, files) = fixture();
        let (count, bytes) = files.preloaded();
        assert_eq!((count, bytes), (3, "<p>index".len() + 1 + 1));
        let hit = |u: &str, doc: bool| files.lookup(&Url::parse(u).unwrap(), doc);
        let app = hit("http://app.test:3000/assets/app.js?v=1", false).unwrap();
        assert_eq!(
            (&app.file.body[..], app.file.content_type, app.spa_fallback),
            (&b"1"[..], "text/javascript; charset=utf-8", false)
        );
        assert_eq!(hit("http://app.test:3000/", true).unwrap().file.path, dir.path().join("index.html"));
        assert_eq!(hit("http://app.test:3000/index.html", false).unwrap().file.path, dir.path().join("index.html"));
        let route = hit("http://app.test:3000/dashboard/x", true).unwrap();
        assert!(route.spa_fallback);
        assert!(hit("http://app.test:3000/dashboard/x", false).is_none());
        assert!(hit("http://app.test:3000/assets", false).is_none());
        assert!(hit("http://app.test:3001/", true).is_none());
        // Files written after startup are not in memory (resolve still finds them).
        std::fs::write(dir.path().join("late.txt"), "late").unwrap();
        assert!(hit("http://app.test:3000/late.txt", false).is_none());
        assert!(files.resolve(&Url::parse("http://app.test:3000/late.txt").unwrap(), false).is_some());
    }

    #[test]
    fn validates_roots() {
        let (dir, _) = fixture();
        assert!(StaticFiles::new(vec![("data:text/html,x".into(), dir.path().to_path_buf())]).is_err());
        assert!(StaticFiles::new(vec![("http://a.test".into(), dir.path().join("nope"))]).is_err());
        assert!(StaticFiles::new(vec![("nonsense".into(), dir.path().to_path_buf())]).is_err());
        let files = StaticFiles::new(vec![
            ("http://a.test:80".into(), dir.path().to_path_buf()),
            ("http://a.test".into(), dir.path().join("assets")),
        ])
        .unwrap();
        assert_eq!(files.summary(), vec![("http://a.test".to_string(), dir.path().join("assets"))]);
        assert!(StaticFiles::default().is_empty());
    }

    #[test]
    fn network_fallback_serves_files_but_not_missing_documents() {
        let (_dir, files) = fixture();
        let network = files.clone().with_spa_fallback(false);
        let dynamic = Url::parse("http://app.test:3000/reports/dynamic?signature=value&disposition=inline").unwrap();
        assert!(files.lookup(&dynamic, true).unwrap().spa_fallback);
        assert!(files.resolve(&dynamic, true).unwrap().spa_fallback);
        assert!(network.lookup(&dynamic, true).is_none());
        assert!(network.resolve(&dynamic, true).is_none());
        for document in [false, true] {
            let asset = Url::parse("http://app.test:3000/assets/app.js?v=1").unwrap();
            assert!(!network.lookup(&asset, document).unwrap().spa_fallback);
            assert!(!network.resolve(&asset, document).unwrap().spa_fallback);
        }
        assert!(network.lookup(&Url::parse("http://app.test:3000/").unwrap(), true).is_some());
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a/b.JS")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("x.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("x.unknown")), "application/octet-stream");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }
}
