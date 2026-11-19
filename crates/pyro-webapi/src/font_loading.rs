//! CSS Font Loading 3: `FontFace`, `FontFaceSet` (`document.fonts`, `self.fonts` in workers) and
//! `FontFaceSetLoadEvent`, made in JS (font_loading.js) on a realm's first use. Stylo parses
//! descriptors, sources and the `font` shorthand (`pyro_dom::font_face`); font data is checked
//! with skrifa after WOFF/WOFF2 decompression.
//!
//! A document's set renders: its script-added faces go to the style engine as `@font-face`
//! rules (`StyleEngine::set_script_fonts`), a loaded face's bytes as a `data:` URL, an unloaded
//! one's own `url()` sources (the layout engine loads those when text uses them, as for a
//! CSS-connected face). The set's CSS-connected faces are the `@font-face` rules of the
//! document's sheets.

use std::rc::Rc;

use base64::Engine as _;
use pyro_js::{JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpFontLoading",
    source: include_str!("font_loading.js"),
    url: "pyro://internal/font_loading.js",
    natives: Some(natives),
};

/// `FontFace`, `FontFaceSet` and `FontFaceSetLoadEvent` on a window or worker global.
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    crate::lazy_module::lazy_globals!(def, MODULE, "FontFace", "FontFaceSet", "FontFaceSetLoadEvent");
}

/// `document.fonts` / `self.fonts`: the realm's set.
pub(crate) fn fonts<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    MODULE.module(s, page)?.get("fonts")
}

/// Whether `bytes` is font data a face can use: an OpenType/TrueType font or collection, WOFF or
/// WOFF2 compressed or not, whose tables parse.
pub fn is_font(bytes: &[u8]) -> bool {
    let decompressed;
    let data = match bytes.get(..4) {
        Some(b"wOFF") => {
            let Ok(d) = wuff::decompress_woff1(bytes) else { return false };
            decompressed = d;
            &decompressed[..]
        }
        Some(b"wOF2") => {
            let Ok(d) = wuff::decompress_woff2(bytes) else { return false };
            decompressed = d;
            &decompressed[..]
        }
        _ => bytes,
    };
    use skrifa::raw::{FileRef, TableProvider};
    let font = match FileRef::new(data) {
        Ok(FileRef::Font(f)) => f,
        Ok(FileRef::Collection(c)) => match c.get(0) {
            Ok(f) => f,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    // What a renderer needs of every font: its header, metrics and character map.
    font.head().is_ok() && font.hhea().is_ok() && font.maxp().is_ok() && font.cmap().is_ok()
}

fn strings<'s>(s: &Scope<'s>, items: &[&str]) -> JsResult<Object<'s>> {
    let values: Vec<Value<'s>> = items.iter().map(|t| s.string(t)).collect();
    s.array(&values)
}

fn source_list<'s>(s: &Scope<'s>, list: &[pyro_dom::font_face::FontSource]) -> JsResult<Object<'s>> {
    use pyro_dom::font_face::FontSource;
    let mut out = Vec::new();
    for source in list {
        out.push(match source {
            FontSource::Url(url, _) => strings(s, &["url", url])?.value(),
            FontSource::Local(name) => strings(s, &["local", name])?.value(),
        });
    }
    s.array(&out)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // descriptor(name, value): `value` as the `@font-face` descriptor `name`, serialized, or null.
    let f = s.function("descriptor", |s, _this, a| {
        let name = crate::util::dom_string(a.get(0))?;
        let value = crate::util::dom_string(a.get(1))?;
        Ok(match pyro_dom::font_face::descriptor(&name, &value) {
            Some(v) => s.string(&v),
            None => s.null(),
        })
    })?;
    natives.set_atom("descriptor", f.value())?;
    // sources(text, base): the source string as [kind, url-or-name] pairs, or null.
    let f = s.function("sources", |s, _this, a| {
        let text = crate::util::dom_string(a.get(0))?;
        let base = crate::util::dom_string(a.get(1))?;
        Ok(match pyro_dom::font_face::sources(&text, &base) {
            Some(list) => source_list(s, &list)?.value(),
            None => s.null(),
        })
    })?;
    natives.set_atom("sources", f.value())?;
    // isFont(buffer): whether the bytes are a usable font.
    let f = s.function("isFont", |s, _this, a| {
        let bytes = crate::encoding::buffer_source_bytes(s, a.get(0))?.unwrap_or_default();
        Ok(s.boolean(is_font(&bytes)))
    })?;
    natives.set_atom("isFont", f.value())?;
    // fontQuery(font): {families, style, weight, stretch} of the `font` shorthand, or null.
    let f = s.function("fontQuery", |s, _this, a| {
        let font = crate::util::dom_string(a.get(0))?;
        let Some(q) = pyro_dom::font_face::font_query(&font) else { return Ok(s.null()) };
        let o = s.object();
        let families: Vec<&str> = q.families.iter().map(String::as_str).collect();
        o.set_atom("families", strings(s, &families)?.value())?;
        o.set_atom("style", s.string(&q.style))?;
        o.set_atom("weight", s.string(&q.weight))?;
        o.set_atom("stretch", s.string(&q.stretch))?;
        Ok(o.value())
    })?;
    natives.set_atom("fontQuery", f.value())?;
    // cssFaces(): the document's `@font-face` rules as [family, [[descriptor, value]...],
    // [[kind, url-or-name]...]], in order.
    let f = s.function("cssFaces", |s, _this, _a| {
        let page = crate::target::page_of(s)?;
        let faces = page.css_font_faces();
        let mut out = Vec::new();
        for face in faces.iter() {
            let mut descriptors = Vec::new();
            for (name, value) in &face.descriptors {
                descriptors.push(strings(s, &[name, value])?.value());
            }
            out.push(
                s.array(&[s.string(&face.family), s.array(&descriptors)?.value(), source_list(s, &face.sources)?.value()])?
                    .value(),
            );
        }
        Ok(s.array(&out)?.value())
    })?;
    natives.set_atom("cssFaces", f.value())?;
    // urlState(url): "loaded" or "loading" when the layout engine has or is fetching the font at
    // `url`, else null.
    let f = s.function("urlState", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let url = crate::util::dom_string(a.get(0))?;
        Ok(match page.font_url_state(&url) {
            Some(true) => s.string("loaded"),
            Some(false) => s.string("loading"),
            None => s.null(),
        })
    })?;
    natives.set_atom("urlState", f.value())?;
    // enginePending(): whether web fonts the layout engine uses are still loading.
    let f = s.function("enginePending", |s, _this, _a| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(page.fonts_pending()))
    })?;
    natives.set_atom("enginePending", f.value())?;
    // loading(delta): a `FontFace` load of the document started (+1) or ended (-1); loads in
    // flight delay the load event (PageDom::fonts_delay_load).
    let f = s.function("loading", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let delta = a.get(0).to_number()? as i64;
        page.font_loads.set((i64::from(page.font_loads.get()) + delta).max(0) as u32);
        Ok(s.undefined())
    })?;
    natives.set_atom("loading", f.value())?;
    // register(entries): the document set's script-added faces, each [family, [[descriptor,
    // value]...], buffer-or-null, [[kind, url]...]], become the style engine's font faces.
    let f = s.function("register", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let entries = a.get(0).as_object().ok_or_else(|| pyro_js::JsError::Type("register: not an array".into()))?;
        let count = crate::util::to_u32(entries.get("length")?)?;
        let mut css = String::new();
        for i in 0..count {
            let Some(entry) = entries.get_index(i)?.as_object() else { continue };
            let family = crate::util::dom_string(entry.get_index(0)?)?;
            let mut descriptors = Vec::new();
            if let Some(list) = entry.get_index(1)?.as_object() {
                for j in 0..crate::util::to_u32(list.get("length")?)? {
                    let Some(pair) = list.get_index(j)?.as_object() else { continue };
                    descriptors
                        .push((crate::util::dom_string(pair.get_index(0)?)?, crate::util::dom_string(pair.get_index(1)?)?));
                }
            }
            if let Some(bytes) = crate::encoding::buffer_source_bytes(s, entry.get_index(2)?)? {
                let url = format!(
                    "data:application/octet-stream;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                );
                css.push_str(&pyro_dom::font_face::face_rule(&family, &descriptors, &[url]));
            } else if let Some(list) = entry.get_index(3)?.as_object() {
                let mut urls = Vec::new();
                for j in 0..crate::util::to_u32(list.get("length")?)? {
                    let Some(pair) = list.get_index(j)?.as_object() else { continue };
                    if crate::util::dom_string(pair.get_index(0)?)? == "url" {
                        urls.push(crate::util::dom_string(pair.get_index(1)?)?);
                    }
                }
                if !urls.is_empty() {
                    css.push_str(&pyro_dom::font_face::face_rule(&family, &descriptors, &urls));
                }
            }
        }
        page.set_script_fonts(if css.is_empty() { None } else { Some(css.into()) });
        Ok(s.undefined())
    })?;
    natives.set_atom("register", f.value())?;
    Ok(natives)
}

#[cfg(test)]
mod tests {
    #[test]
    fn font_data_is_checked() {
        assert!(!super::is_font(&[0; 8]));
        assert!(!super::is_font(b"wOF2\0\0\0\0"));
        assert!(!super::is_font(&[]));
    }
}
