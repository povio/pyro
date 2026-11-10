//! JSON and CSS module scripts (`import x from "./a.json" with { type: "json" }`)
//! and the MIME type check every module script fetch makes.
//!
//! JavaScriptCore keys its module map by (URL, type) and hands each fetch's
//! `type` import attribute to the realm's [`lp_js::ModuleFetcher`] (patched
//! JavaScriptCore, `Capabilities::typed_modules`). The fetcher loads the
//! response, checks its MIME type here, and completes the request: JavaScript
//! and JSON as source text, CSS as a constructed `CSSStyleSheet`
//! ([`complete`]). Nothing here runs unless a module is fetched.

use lp_js::{JsError, JsResult, ModuleRequest, ModuleType, Scope};

/// HTML "fetch a single module script": the response's MIME type essence
/// (lowercased, as `lp_net::parse_content_type` gives it) must match the
/// request's module type. `None` when it does, else the TypeError message.
pub fn mime_error(ty: ModuleType, mime: &str, url: &str) -> Option<String> {
    let (ok, expected) = match ty {
        ModuleType::JavaScript => (lp_net::is_javascript_mime(mime), "a JavaScript"),
        ModuleType::Json => (lp_net::is_json_mime(mime), "a JSON"),
        ModuleType::Css => (mime == "text/css", "a CSS"),
        ModuleType::Unsupported => (false, "a supported"),
    };
    if ok {
        return None;
    }
    let mut message = format!(
        "Failed to load module script {url}: Expected {expected} module script but the server responded with a MIME \
         type of \"{mime}\". Strict MIME type checking is enforced for module scripts per HTML spec."
    );
    // Without import attributes every request looks like a JavaScript one;
    // say why a JSON or CSS module did not load instead of mis-parsing it.
    if ty == ModuleType::JavaScript && (lp_net::is_json_mime(mime) || mime == "text/css") && !typed_modules() {
        message.push_str(
            " JSON and CSS module scripts (import attributes) need the source-built JavaScriptCore \
             (scripts/jsc/build.sh); the system framework does not pass the type attribute to the loader.",
        );
    }
    Some(message)
}

/// Encoding "UTF-8 decode", which module scripts of every type use whatever
/// the response's charset: a UTF-8 BOM is dropped, nothing else is sniffed.
pub fn utf8_decode(body: &[u8]) -> String {
    let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    String::from_utf8_lossy(body).into_owned()
}

/// Whether the engine passes import attributes to module fetchers.
fn typed_modules() -> bool {
    // SAFETY: plain capability probe without arguments.
    unsafe { lp_js::sys::modules::lp_jsc_typed_modules_available() == 1 }
}

/// The error a module request fails with before any fetch: a module type
/// this global does not allow (HTML "module type allowed": CSS needs
/// `CSSStyleSheet`, which workers do not have; JavaScriptCore's
/// `"webassembly"` is no HTML module type).
pub fn disallowed(ty: ModuleType, window: bool) -> Option<JsError> {
    match ty {
        ModuleType::Css if !window => {
            Some(JsError::Type("CSS module scripts are not supported in workers (no CSSStyleSheet)".into()))
        }
        ModuleType::Unsupported => Some(JsError::Type("Module type not supported by environment".into())),
        _ => None,
    }
}

/// Completes `request` with a fetched response body whose MIME type passed
/// [`mime_error`]: JavaScript and JSON as source, CSS as a new constructed
/// `CSSStyleSheet` of the realm's document (`@import` rules dropped, as
/// `replaceSync` does).
pub fn complete(scope: &Scope<'_>, request: ModuleRequest, text: &str) -> JsResult<()> {
    match request.module_type() {
        ModuleType::JavaScript => request.resolve(scope, text),
        ModuleType::Json => request.resolve_json(scope, text),
        ModuleType::Css => match crate::css::module_sheet(scope, text) {
            Ok(sheet) => request.resolve_object(scope, sheet),
            Err(e) => request.reject(scope, &e),
        },
        ModuleType::Unsupported => {
            request.reject(scope, &JsError::Type("Module type not supported by environment".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{disallowed, mime_error};
    use lp_js::ModuleType;

    #[test]
    fn module_types_need_their_mime_types() {
        let url = "https://site.test/a";
        assert_eq!(mime_error(ModuleType::JavaScript, "text/javascript", url), None);
        assert_eq!(mime_error(ModuleType::Json, "application/json", url), None);
        assert_eq!(mime_error(ModuleType::Json, "application/manifest+json", url), None);
        assert_eq!(mime_error(ModuleType::Css, "text/css", url), None);
        for (ty, mime) in [
            (ModuleType::JavaScript, ""),
            (ModuleType::JavaScript, "application/json"),
            (ModuleType::JavaScript, "text/css"),
            (ModuleType::Json, "text/javascript"),
            (ModuleType::Json, "text/plain"),
            (ModuleType::Css, "application/css"),
            (ModuleType::Css, "application/json"),
            (ModuleType::Unsupported, "application/wasm"),
        ] {
            let e = mime_error(ty, mime, url).unwrap_or_else(|| panic!("{ty:?} {mime}"));
            assert!(e.contains(&format!("MIME type of \"{mime}\"")), "{e}");
        }
        // Without import attributes, a JSON or CSS response to what looks like
        // a JavaScript request says why it did not load.
        let e = mime_error(ModuleType::JavaScript, "application/json", url).unwrap();
        let typed = unsafe { lp_js::sys::modules::lp_jsc_typed_modules_available() == 1 };
        assert_eq!(e.contains("need the source-built JavaScriptCore"), !typed, "{e}");
    }

    #[test]
    fn css_modules_need_a_window() {
        assert!(disallowed(ModuleType::Css, false).is_some());
        assert!(disallowed(ModuleType::Css, true).is_none());
        assert!(disallowed(ModuleType::Json, false).is_none());
        assert!(disallowed(ModuleType::Unsupported, true).is_some());
    }
}
