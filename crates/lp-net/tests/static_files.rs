//! `NetConfig::static_files`: an origin answered from a directory.

use std::path::PathBuf;

use http::Method;
use lp_net::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lp-net-static-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(url: &str, method: Method, destination: Destination) -> Request {
    let mut req = Request::get(Url::parse(url).unwrap(), destination);
    req.method = method;
    req
}

#[test]
fn static_origin_is_served_from_disk_and_misses_fall_through() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.0.join("assets")).unwrap();
    std::fs::write(dir.0.join("index.html"), "<p>index</p>").unwrap();
    std::fs::write(dir.0.join("assets/app.js"), "globalThis.app = 1").unwrap();
    // Port 1 has no listener: a miss must fail like any connection refused.
    let origin = "http://127.0.0.1:1";
    let config = NetConfig {
        static_files: StaticFiles::new(vec![(origin.to_owned(), dir.0.clone())]).unwrap(),
        ..NetConfig::default()
    };
    let client = NetClient::new(config).unwrap();

    let (head, body) = client
        .fetch_blocking(request(&format!("{origin}/assets/app.js?v=2"), Method::GET, Destination::Script))
        .unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(head.headers[http::header::CONTENT_TYPE], "text/javascript; charset=utf-8");
    assert_eq!(head.headers[http::header::CONTENT_LENGTH], "18");
    assert_eq!(head.mime, "text/javascript");
    assert_eq!(&body[..], b"globalThis.app = 1");

    let (head, body) =
        client.fetch_blocking(request(&format!("{origin}/"), Method::GET, Destination::Document)).unwrap();
    assert_eq!((head.status.as_u16(), &body[..]), (200, &b"<p>index</p>"[..]));
    assert_eq!(head.headers[http::header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert!(!head.headers.contains_key("x-lightningpanda-spa-fallback"));

    // SPA fallback for navigations only.
    let (head, body) =
        client.fetch_blocking(request(&format!("{origin}/app/route"), Method::GET, Destination::Document)).unwrap();
    assert_eq!((head.status.as_u16(), &body[..]), (200, &b"<p>index</p>"[..]));
    assert_eq!(head.headers["x-lightningpanda-spa-fallback"], "1");
    let err =
        client.fetch_blocking(request(&format!("{origin}/app/route"), Method::GET, Destination::Script)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_CONNECTION_REFUSED");

    // HEAD: headers without a body. POST: network.
    let (head, body) =
        client.fetch_blocking(request(&format!("{origin}/assets/app.js"), Method::HEAD, Destination::Other)).unwrap();
    assert_eq!((head.status.as_u16(), body.len()), (200, 0));
    assert_eq!(head.headers[http::header::CONTENT_LENGTH], "18");
    let err = client
        .fetch_blocking(request(&format!("{origin}/assets/app.js"), Method::POST, Destination::Other))
        .unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_CONNECTION_REFUSED");

    // `%2e%2e` is resolved by the URL parser within the origin (`/x`): no such
    // file. Other origins are not the overlay's business.
    let err =
        client.fetch_blocking(request(&format!("{origin}/%2e%2e/x"), Method::GET, Destination::Script)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_CONNECTION_REFUSED");
    let err = client
        .fetch_blocking(request("http://127.0.0.1:2/index.html", Method::GET, Destination::Document))
        .unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_CONNECTION_REFUSED");
}

#[test]
fn static_files_satisfy_cross_origin_cors_requests() {
    let dir = TempDir::new();
    std::fs::write(dir.0.join("data.json"), "{}").unwrap();
    let origin = "http://127.0.0.1:1";
    let config = NetConfig {
        static_files: StaticFiles::new(vec![(origin.to_owned(), dir.0.clone())]).unwrap(),
        ..NetConfig::default()
    };
    let client = NetClient::new(config).unwrap();
    let mut req = request(&format!("{origin}/data.json"), Method::GET, Destination::Fetch);
    req.mode = RequestMode::Cors;
    req.origin = Some("http://other.test".into());
    let (head, body) = client.fetch_blocking(req.clone()).unwrap();
    assert_eq!((head.status.as_u16(), &body[..]), (200, &b"{}"[..]));
    assert_eq!(head.kind, ResponseKind::Cors);
    assert_eq!(head.headers[http::header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

    req.credentials = Credentials::Include;
    let (head, _) = client.fetch_blocking(req).unwrap();
    assert_eq!(head.headers[http::header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://other.test");
    assert_eq!(head.headers[http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
}

#[test]
fn file_urls_serve_local_files_like_chrome() {
    let dir = TempDir::new();
    std::fs::write(dir.0.join("page.html"), "<p>local</p>").unwrap();
    std::fs::write(dir.0.join("app.js"), "1").unwrap();
    let client = NetClient::new(NetConfig::default()).unwrap();
    let url = |name: &str| Url::from_file_path(dir.0.join(name)).unwrap().to_string();

    let (head, body) = client.fetch_blocking(request(&url("page.html"), Method::GET, Destination::Document)).unwrap();
    assert_eq!((head.status.as_u16(), &body[..]), (200, &b"<p>local</p>"[..]));
    assert_eq!(head.mime, "text/html");
    let (head, body) = client.fetch_blocking(request(&url("app.js"), Method::HEAD, Destination::Script)).unwrap();
    assert_eq!((head.status.as_u16(), body.len()), (200, 0));
    assert_eq!(head.headers[http::header::CONTENT_LENGTH], "1");

    // Missing files and directories.
    let err = client.fetch_blocking(request(&url("missing.html"), Method::GET, Destination::Document)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_FILE_NOT_FOUND");
    let dir_url = Url::from_directory_path(&dir.0).unwrap().to_string();
    let err = client.fetch_blocking(request(&dir_url, Method::GET, Destination::Document)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_FILE_NOT_FOUND");

    // fetch/XHR do not support the scheme; web pages cannot reach local files.
    let err = client.fetch_blocking(request(&url("app.js"), Method::GET, Destination::Fetch)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_UNKNOWN_URL_SCHEME");
    let mut from_web = request(&url("app.js"), Method::GET, Destination::Script);
    from_web.origin = Some("https://example.com".into());
    assert_eq!(client.fetch_blocking(from_web).unwrap_err().error_text(), "net::ERR_ACCESS_DENIED");
    let mut from_file = request(&url("app.js"), Method::GET, Destination::Script);
    from_file.origin = Some("null".into());
    assert!(client.fetch_blocking(from_file).is_ok());

    // Off: the scheme is unknown, as before.
    let off = NetClient::new(NetConfig { file_urls: false, ..NetConfig::default() }).unwrap();
    let err = off.fetch_blocking(request(&url("page.html"), Method::GET, Destination::Document)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_UNKNOWN_URL_SCHEME");
}

#[test]
fn redirects_into_file_urls_are_refused() {
    use std::io::{Read, Write};
    let dir = TempDir::new();
    std::fs::write(dir.0.join("secret.txt"), "local").unwrap();
    let target = Url::from_file_path(dir.0.join("secret.txt")).unwrap().to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let response =
            format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(response.as_bytes()).unwrap();
    });
    let client = NetClient::new(NetConfig::default()).unwrap();
    let url = format!("http://127.0.0.1:{port}/");
    let err = client.fetch_blocking(request(&url, Method::GET, Destination::Document)).unwrap_err();
    assert_eq!(err.error_text(), "net::ERR_UNSAFE_REDIRECT");
    server.join().unwrap();
}
