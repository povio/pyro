//! HTTP forward proxy of one context: `Target.createBrowserContext proxyServer`
//! and `serve --proxy`. https goes through a CONNECT tunnel, http as an
//! absolute-form request. The connector sits under hyper-rustls, so TLS (and its
//! ALPN) runs end to end over the tunnel, and connections stay pooled per target
//! in the context's client.

use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, Uri};
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower_service::Service;
use url::Url;

use crate::auth::auth_challenge;
use crate::timing::{ConnInfo, ConnTimes, TimedResolver, CONNECTING};

/// Largest CONNECT response head accepted from a proxy.
const MAX_TUNNEL_HEAD: usize = 64 * 1024;

/// A parsed proxy setting. Credentials in the URL are sent preemptively; others
/// come from the `Fetch.authRequired` challenge. Two are equal when they send
/// the same requests the same way (server, credentials and bypass rules).
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    uri: Uri,
    origin: String,
    authorization: Option<HeaderValue>,
    bypass: BypassRules,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No credentials in logs.
        f.debug_struct("ProxyConfig").field("origin", &self.origin).field("bypass", &self.bypass).finish()
    }
}

impl ProxyConfig {
    /// Parses Chrome's `--proxy-server` value for a single HTTP proxy
    /// (`[http://][user:pass@]host[:port]`) and `--proxy-bypass-list`.
    /// `direct://` means no proxy (`Ok(None)`). Every other form is an error
    /// rather than a silently direct connection.
    pub fn parse(server: &str, bypass: Option<&str>) -> Result<Option<Self>, String> {
        let server = server.trim();
        if server == "direct://" {
            return Ok(None);
        }
        let address = server.rsplit_once('@').map_or(server, |(_, address)| address);
        if address.contains([';', '=']) {
            return Err(format!("per-scheme proxy lists are not supported: '{address}'"));
        }
        let scheme = server.split_once("://").map_or("http", |(scheme, _)| scheme).to_ascii_lowercase();
        match scheme.as_str() {
            "http" => {}
            "socks" | "socks4" | "socks4a" | "socks5" | "socks5h" => {
                return Err(format!("SOCKS proxies are not supported: {scheme}://{}", host_only(address)))
            }
            "https" => return Err(format!("HTTPS proxies are not supported: https://{}", host_only(address))),
            other => return Err(format!("unknown proxy scheme '{other}'")),
        }
        let invalid = || format!("invalid proxy server '{}'", host_only(address));
        let full = if server.contains("://") { server.to_owned() } else { format!("http://{server}") };
        let url = Url::parse(&full).map_err(|_| invalid())?;
        let host = url.host_str().filter(|h| !h.is_empty()).ok_or_else(invalid)?;
        if !matches!(url.path(), "" | "/") || url.query().is_some() {
            return Err(invalid());
        }
        let authority = format!("{host}:{}", url.port().unwrap_or(80));
        let uri: Uri = format!("http://{authority}/").parse().map_err(|_| invalid())?;
        let authorization = if url.username().is_empty() {
            None
        } else {
            let decode = |s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
            Some(basic_authorization(&decode(url.username()), &decode(url.password().unwrap_or("")))?)
        };
        Ok(Some(ProxyConfig {
            uri,
            origin: format!("http://{authority}"),
            authorization,
            bypass: BypassRules::parse(bypass.unwrap_or(""))?,
        }))
    }
}

/// `address` of a server string without its scheme, for error messages (the
/// credentials are already gone).
fn host_only(address: &str) -> &str {
    address.split_once("://").map_or(address, |(_, rest)| rest)
}

/// `Proxy-Authorization` / `Authorization` value for Basic credentials.
pub(crate) fn basic_authorization(username: &str, password: &str) -> Result<HeaderValue, String> {
    use base64::Engine;
    if username.contains(':') {
        return Err("Basic authentication username contains ':'".into());
    }
    let value = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    HeaderValue::from_str(&format!("Basic {value}")).map_err(|_| "invalid credentials".into())
}

/// `--proxy-bypass-list`, Chrome's syntax. Loopback hosts bypass the proxy
/// unless the list says `<-loopback>`, which Playwright always does.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BypassRules {
    loopback: bool,
    rules: Vec<BypassRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BypassRule {
    /// `<local>`: hostnames without a dot.
    Local,
    Cidr(IpAddr, u8),
    /// `[scheme://]pattern[:port]`; the pattern is lowercase, `*` matches any run.
    Host {
        scheme: Option<String>,
        pattern: String,
        port: Option<u16>,
    },
}

impl BypassRules {
    fn parse(list: &str) -> Result<Self, String> {
        let mut out = BypassRules { loopback: true, rules: Vec::new() };
        for entry in list.split([',', ';']).map(str::trim).filter(|e| !e.is_empty()) {
            let lower = entry.to_ascii_lowercase();
            match lower.as_str() {
                "<-loopback>" => out.loopback = false,
                "<local>" => out.rules.push(BypassRule::Local),
                _ => out.rules.push(parse_bypass_rule(&lower)?),
            }
        }
        Ok(out)
    }

    /// `host` without IPv6 brackets.
    fn matches(&self, scheme: &str, host: &str, port: u16) -> bool {
        let ip = host.parse::<IpAddr>().ok();
        if self.loopback && (ip.is_some_and(|ip| ip.is_loopback()) || is_localhost(host)) {
            return true;
        }
        self.rules.iter().any(|rule| match rule {
            BypassRule::Local => ip.is_none() && !host.contains('.'),
            BypassRule::Cidr(net, bits) => ip.is_some_and(|ip| in_cidr(ip, *net, *bits)),
            BypassRule::Host { scheme: s, pattern, port: p } => {
                s.as_deref().is_none_or(|s| s == scheme) && p.is_none_or(|p| p == port) && glob(pattern, host)
            }
        })
    }
}

fn is_localhost(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

fn parse_bypass_rule(rule: &str) -> Result<BypassRule, String> {
    let invalid = || format!("invalid proxy bypass rule '{rule}'");
    if !rule.contains("://") {
        if let Some((ip, bits)) = rule.split_once('/') {
            let ip: IpAddr = ip.trim_matches(['[', ']']).parse().map_err(|_| invalid())?;
            let bits: u8 = bits.parse().map_err(|_| invalid())?;
            if bits > if ip.is_ipv4() { 32 } else { 128 } {
                return Err(invalid());
            }
            return Ok(BypassRule::Cidr(ip, bits));
        }
    }
    let (scheme, rest) = rule.split_once("://").map_or((None, rule), |(s, r)| (Some(s.to_owned()), r));
    // A bare IPv6 literal has colons that are not a port.
    if rest.parse::<std::net::Ipv6Addr>().is_ok() {
        return Ok(BypassRule::Host { scheme, pattern: rest.to_owned(), port: None });
    }
    let (host, port) = if let Some(after) = rest.strip_prefix('[') {
        let (host, tail) = after.split_once(']').ok_or_else(invalid)?;
        (host, tail.strip_prefix(':'))
    } else {
        rest.rsplit_once(':').map_or((rest, None), |(h, p)| (h, Some(p)))
    };
    let port = port.map(str::parse).transpose().map_err(|_| invalid())?;
    let pattern = match host {
        "" => "*".to_owned(),
        host if host.starts_with('.') => format!("*{host}"),
        host => host.to_owned(),
    };
    Ok(BypassRule::Host { scheme, pattern, port })
}

fn in_cidr(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    let (ip, net, width) = match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (u128::from(u32::from(a)), u128::from(u32::from(b)), 32),
        (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), 128),
        _ => return false,
    };
    bits == 0 || ip >> (width - u32::from(bits)) == net >> (width - u32::from(bits))
}

/// Case-insensitive match where `*` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p].eq_ignore_ascii_case(&text[t]) {
            p += 1;
            t += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == b'*')
}

/// One client's proxy: the configuration plus the credentials answered so far.
/// A restarted I/O thread keeps them.
#[derive(Debug)]
pub(crate) struct ProxyState {
    config: ProxyConfig,
    authorization: Mutex<Option<HeaderValue>>,
}

impl ProxyState {
    pub(crate) fn new(config: ProxyConfig) -> Self {
        let authorization = Mutex::new(config.authorization.clone());
        ProxyState { config, authorization }
    }

    pub(crate) fn origin(&self) -> &str {
        &self.config.origin
    }

    pub(crate) fn applies_to(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
        !self.config.bypass.matches(url.scheme(), host, url.port_or_known_default().unwrap_or(0))
    }

    pub(crate) fn authorization(&self) -> Option<HeaderValue> {
        self.authorization.lock().unwrap().clone()
    }

    pub(crate) fn set_authorization(&self, value: HeaderValue) {
        *self.authorization.lock().unwrap() = Some(value);
    }

    fn applies_to_uri(&self, uri: &Uri) -> bool {
        let https = uri.scheme_str() == Some("https");
        let host = uri.host().unwrap_or("").trim_matches(['[', ']']);
        let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
        !self.config.bypass.matches(if https { "https" } else { "http" }, host, port)
    }
}

/// A connect failure attributed to the proxy, read back by `classify`.
#[derive(Debug)]
pub(crate) enum ProxyError {
    /// The proxy itself could not be reached.
    Unreachable(BoxError),
    /// 407 to CONNECT: there is no response to deliver, so it surfaces here.
    Challenge { scheme: String, realm: String },
    /// CONNECT refused or malformed.
    Tunnel(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyError::Unreachable(e) => write!(f, "proxy connection failed: {e}"),
            ProxyError::Challenge { scheme, .. } => write!(f, "proxy requires {scheme} authentication"),
            ProxyError::Tunnel(m) => write!(f, "proxy tunnel failed: {m}"),
        }
    }
}

impl std::error::Error for ProxyError {}

/// TCP connector under hyper-rustls: a direct connection, or one to the proxy.
#[derive(Clone)]
pub(crate) struct ProxyConnector {
    tcp: HttpConnector<TimedResolver>,
    proxy: Option<Arc<ProxyState>>,
    timeout: Duration,
    /// Tunnel http targets too (WebSocket: the upgrade must reach the target,
    /// not the proxy).
    tunnel_plain: bool,
}

impl ProxyConnector {
    pub(crate) fn new(tcp: HttpConnector<TimedResolver>, proxy: Option<Arc<ProxyState>>, timeout: Duration) -> Self {
        ProxyConnector { tcp, proxy, timeout, tunnel_plain: false }
    }

    /// Opens a CONNECT tunnel for every target, `http` ones included.
    pub(crate) fn tunneling(mut self) -> Self {
        self.tunnel_plain = true;
        self
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl Service<Uri> for ProxyConnector {
    type Response = ProxyStream;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<ProxyStream, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.tcp.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        // Resource Timing: the resolver notes its lookup on `times` (a
        // task-local while the TCP connector runs), the stream the rest.
        let times = ConnTimes::new(dst.scheme_str() == Some("https"));
        let Some(proxy) = self.proxy.clone().filter(|p| p.applies_to_uri(&dst)) else {
            let connecting = CONNECTING.scope(times.clone(), self.tcp.call(dst));
            return Box::pin(async move {
                let io = connecting.await?;
                times.tcp_connected();
                Ok(ProxyStream { io, absolute_form: false, times })
            });
        };
        let connecting = CONNECTING.scope(times.clone(), self.tcp.call(proxy.config.uri.clone()));
        let timeout = self.timeout;
        let tunnel_plain = self.tunnel_plain;
        Box::pin(async move {
            let mut io = connecting.await.map_err(|e| ProxyError::Unreachable(e.into()))?;
            let https = dst.scheme_str() == Some("https");
            if !https && !tunnel_plain {
                times.tcp_connected();
                return Ok(ProxyStream { io, absolute_form: true, times });
            }
            let host = dst.host().ok_or_else(|| ProxyError::Tunnel("no host".into()))?;
            let target = format!("{host}:{}", dst.port_u16().unwrap_or(if https { 443 } else { 80 }));
            let authorization = proxy.authorization();
            let opening = open_tunnel(io.inner_mut(), &target, authorization.as_ref());
            tokio::time::timeout(timeout, opening).await.map_err(|_| ProxyError::Tunnel("timed out".into()))??;
            times.tcp_connected();
            Ok(ProxyStream { io, absolute_form: false, times })
        })
    }
}

async fn open_tunnel(
    stream: &mut TcpStream,
    target: &str,
    authorization: Option<&HeaderValue>,
) -> Result<(), ProxyError> {
    let fail = |e: io::Error| ProxyError::Tunnel(e.to_string());
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Some(value) = authorization.and_then(|v| v.to_str().ok()) {
        request.push_str("Proxy-Authorization: ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.map_err(fail)?;
    let mut head = Vec::with_capacity(512);
    loop {
        if stream.read_buf(&mut head).await.map_err(fail)? == 0 {
            return Err(ProxyError::Tunnel("connection closed".into()));
        }
        let mut parsed = [httparse::EMPTY_HEADER; 64];
        let mut response = httparse::Response::new(&mut parsed);
        match response.parse(&head) {
            Ok(httparse::Status::Partial) if head.len() < MAX_TUNNEL_HEAD => continue,
            Ok(httparse::Status::Complete(len)) => {
                return tunnel_answer(response.code.unwrap_or(0), response.headers, head.len() - len)
            }
            _ => return Err(ProxyError::Tunnel("malformed response".into())),
        }
    }
}

fn tunnel_answer(status: u16, headers: &[httparse::Header<'_>], extra: usize) -> Result<(), ProxyError> {
    if (200..300).contains(&status) {
        // The TLS client speaks first: anything here is not from the target.
        return match extra {
            0 => Ok(()),
            _ => Err(ProxyError::Tunnel("data before the tunnel was used".into())),
        };
    }
    if status == 407 {
        let mut map = HeaderMap::new();
        for h in headers {
            if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(h.name.as_bytes()), HeaderValue::from_bytes(h.value))
            {
                map.append(name, value);
            }
        }
        if let Some((scheme, realm)) = auth_challenge(&map, http::header::PROXY_AUTHENTICATE) {
            return Err(ProxyError::Challenge { scheme, realm });
        }
    }
    Err(ProxyError::Tunnel(format!("status {status}")))
}

/// A TCP stream that tells hyper to send absolute-form requests when it goes to
/// a proxy without a tunnel.
pub(crate) struct ProxyStream {
    io: TokioIo<TcpStream>,
    absolute_form: bool,
    times: Arc<ConnTimes>,
}

impl Connection for ProxyStream {
    /// Called by hyper once the connection is ready (after TLS for https):
    /// its times go with every response over it.
    fn connected(&self) -> Connected {
        self.times.ready();
        self.io.connected().proxy(self.absolute_form).extra(ConnInfo(self.times.clone()))
    }
}

impl Read for ProxyStream {
    #[inline]
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl Write for ProxyStream {
    #[inline]
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    #[inline]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
    #[inline]
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bypass(list: &str, scheme: &str, host: &str, port: u16) -> bool {
        BypassRules::parse(list).unwrap().matches(scheme, host, port)
    }

    #[test]
    fn parses_http_servers_and_refuses_others() {
        let plain = ProxyConfig::parse("proxy.test:3128", None).unwrap().unwrap();
        assert_eq!((plain.origin.as_str(), plain.authorization.is_none()), ("http://proxy.test:3128", true));
        let port80 = ProxyConfig::parse("http://proxy.test", None).unwrap().unwrap();
        assert_eq!(port80.origin, "http://proxy.test:80");
        let auth = ProxyConfig::parse("http://us%40er:p%3Ass@127.0.0.1:8080", None).unwrap().unwrap();
        assert_eq!(auth.authorization.unwrap().to_str().unwrap(), "Basic dXNAZXI6cDpzcw==");
        assert!(!format!("{:?}", ProxyConfig::parse("http://u:secret@p.test:1", None).unwrap()).contains("secret"));
        assert!(ProxyConfig::parse("direct://", None).unwrap().is_none());

        for socks in ["socks5://p.test:1080", "socks4://p.test:1080", "socks://p.test:1"] {
            assert!(ProxyConfig::parse(socks, None).unwrap_err().contains("SOCKS"), "{socks}");
        }
        assert!(!ProxyConfig::parse("socks5://u:secret@p.test:1", None).unwrap_err().contains("secret"));
        assert!(ProxyConfig::parse("https://p.test:1", None).unwrap_err().contains("HTTPS"));
        assert!(ProxyConfig::parse("http=a:1;https=b:2", None).is_err());
        assert!(ProxyConfig::parse("ftp://p.test", None).is_err());
        assert!(ProxyConfig::parse("http://p.test:1/path", None).is_err());
        assert!(ProxyConfig::parse("http://p.test:1", Some("10.0.0.0/99")).is_err());
    }

    #[test]
    fn bypass_rules_follow_chrome() {
        // Loopback is implicit, and `<-loopback>` removes it.
        for host in ["localhost", "a.localhost", "LOCALHOST", "127.0.0.1", "127.3.0.1", "::1"] {
            assert!(bypass("", "http", host, 80), "{host}");
            assert!(!bypass("<-loopback>", "http", host, 80), "{host}");
        }
        assert!(!bypass("", "http", "example.com", 80));
        assert!(!bypass("", "http", "notlocalhost", 80));

        assert!(bypass("example.com", "https", "example.com", 443));
        assert!(!bypass("example.com", "https", "a.example.com", 443));
        assert!(bypass(".example.com", "https", "a.example.com", 443));
        assert!(!bypass(".example.com", "https", "example.com", 443));
        assert!(bypass("*.EXAMPLE.com, other.test", "http", "a.b.example.com", 80));
        assert!(bypass("foo.test;*example*", "http", "myexample.org", 80));
        assert!(bypass("*", "http", "any.test", 80));

        assert!(bypass("https://secure.test", "https", "secure.test", 443));
        assert!(!bypass("https://secure.test", "http", "secure.test", 80));
        assert!(bypass("port.test:8080", "http", "port.test", 8080));
        assert!(!bypass("port.test:8080", "http", "port.test", 80));

        assert!(bypass("<local>", "http", "intranet", 80));
        assert!(!bypass("<local>", "http", "example.com", 80));
        assert!(!bypass("<local>", "http", "10.0.0.1", 80));

        assert!(bypass("192.168.0.0/16", "http", "192.168.7.7", 80));
        assert!(!bypass("192.168.0.0/16", "http", "192.169.7.7", 80));
        assert!(bypass("fd00::/8", "http", "fd12::1", 80));
        assert!(!bypass("fd00::/8", "http", "10.0.0.1", 80));
        assert!(bypass("0.0.0.0/0", "http", "8.8.8.8", 80));
        assert!(bypass("2001:db8::1", "http", "2001:db8::1", 80));
        assert!(bypass("[2001:db8::1]:8443", "https", "2001:db8::1", 8443));
    }
}
