use super::*;

pub(super) struct Text {
    decoder: encoding_rs::Decoder,
    chunks: usize,
    done: bool,
    value: String,
}
impl Xhr {
    /// XHR's final MIME type and final encoding are resolved separately. An
    /// override without a charset preserves the transport charset. An unknown
    /// override charset still supersedes the transport label.
    fn response_mime(&self) -> (Mime, Option<&'static encoding_rs::Encoding>) {
        let response = self
            .head
            .borrow()
            .as_ref()
            .and_then(|head| {
                let header = head.headers.get(header::CONTENT_TYPE)?;
                // Fetch decodes header bytes as isomorphic Latin-1.
                let value: String = header.as_bytes().iter().copied().map(char::from).collect();
                value.parse::<Mime>().ok()
            })
            .unwrap_or_else(|| Mime::new("text", "xml"));
        let override_mime = self.override_mime.borrow();
        let label = override_mime
            .as_ref()
            .and_then(|m| m.get_parameter("charset"))
            .or_else(|| response.get_parameter("charset"));
        let encoding = label.and_then(|v| encoding_rs::Encoding::for_label(v.as_bytes()));
        (override_mime.clone().unwrap_or(response), encoding)
    }
    fn text<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        if self.ready.get() < 3 || self.head.borrow().is_none() {
            return Ok(s.string(""));
        }
        let Some(wire) = self.wire.borrow().clone() else { return Ok(s.string("")) };
        let mut cache = self.text.borrow_mut();
        let text = cache.get_or_insert_with(|| {
            let (_, encoding) = self.response_mime();
            let encoding = encoding.unwrap_or(encoding_rs::UTF_8);
            Text { decoder: encoding.new_decoder(), chunks: 0, done: false, value: String::new() }
        });
        let wire = wire.lock().unwrap();
        // Only decode the owner task's snapshot. I/O may append more bytes
        // or record a failure while JS is running, but cannot change this view.
        let visible = self.visible_chunks.get();
        for chunk in &wire.chunks[text.chunks..visible] {
            text.value
                .reserve(text.decoder.max_utf8_buffer_length(chunk.len()).unwrap_or(chunk.len().saturating_mul(3)));
            let (result, _, _) = text.decoder.decode_to_string(chunk, &mut text.value, false);
            debug_assert_eq!(result, encoding_rs::CoderResult::InputEmpty);
        }
        text.chunks = visible;
        if self.ready.get() == 4 && !text.done {
            text.value.reserve(16);
            let _ = text.decoder.decode_to_string(&[], &mut text.value, true);
            text.done = true;
        }
        drop(wire);
        Ok(s.string(&text.value))
    }
    pub(super) fn response_text<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        if !matches!(self.response_type.borrow().as_str(), "" | "text") {
            return Err(invalid("responseText requires text responseType"));
        }
        self.text(s)
    }
    fn body_bytes(&self) -> Option<Vec<u8>> {
        let wire = self.wire.borrow();
        let wire = wire.as_ref()?.lock().unwrap();
        let mut out = Vec::with_capacity(wire.bytes);
        for c in &wire.chunks {
            out.extend_from_slice(c);
        }
        Some(out)
    }
    pub(super) fn response_xml<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        if !matches!(self.response_type.borrow().as_str(), "" | "document") {
            return Err(invalid("responseXML requires an empty or document responseType"));
        }
        self.document_response(s)
    }
    /// XHR "set a document response": HTML only for responseType "document",
    /// XML for any XML MIME type, null otherwise and for malformed XML. The
    /// document is inert (no scripts, no subresources) and parsed once.
    fn document_response<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        if self.ready.get() != 4 || self.head.borrow().is_none() {
            return Ok(s.null());
        }
        let wrapper = self.wrapper(s)?;
        if let Some(value) = s.get_hidden(wrapper, RESPONSE)?.filter(|v| !v.is_undefined()) {
            return Ok(value);
        }
        let Some(bytes) = self.body_bytes() else { return Ok(s.null()) };
        let (mime, encoding) = self.response_mime();
        let essence = format!("{}/{}", mime.type_, mime.subtype);
        let charset = encoding.map(|enc| enc.name());
        let url = self.head.borrow().as_ref().map_or(String::new(), |h| h.url.to_string());
        let page = self.page()?;
        let document = if essence == "text/html" {
            (self.response_type.borrow().as_str() == "document")
                .then(|| pyro_dom::parse::parse_document_bytes(&page.dom, &url, &bytes, charset))
        } else if pyro_dom::parse::is_xml_mime(&essence) {
            pyro_dom::xml::parse_bytes(&mut page.dom.borrow_mut(), &url, &bytes, charset, &essence)
        } else {
            None
        };
        let value = match document {
            Some(document) => crate::node::wrap_node(s, &page, document)?.value(),
            None => s.null(),
        };
        s.set_hidden(wrapper, RESPONSE, value)?;
        Ok(value)
    }
    pub(super) fn response<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let kind = self.response_type.borrow().clone();
        if matches!(kind.as_str(), "" | "text") {
            return self.text(s);
        }
        if kind == "document" {
            return self.document_response(s);
        }
        if self.ready.get() != 4 || self.head.borrow().is_none() {
            return Ok(s.null());
        }
        let wrapper = self.wrapper(s)?;
        if let Some(value) = s.get_hidden(wrapper, RESPONSE)?.filter(|v| !v.is_undefined()) {
            return Ok(value);
        }
        let Some(bytes) = self.body_bytes() else { return Ok(s.null()) };
        let value = match kind.as_str() {
            "arraybuffer" => crate::encoding::array_buffer(s, &bytes)?,
            "blob" => s
                .wrap::<crate::file::Blob>(
                    crate::file::BlobData {
                        bytes: Bytes::from(bytes),
                        mime: self.response_mime().0.to_string(),
                        file: None,
                    },
                    None,
                )?
                .value(),
            "json" => {
                let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes));
                let text = pyro_js::JsString::new(text.as_ref());
                // SAFETY: the scope/string are live. Wrap and trace the result
                // before another JS call; invalid JSON is the null response.
                let raw = unsafe { pyro_js::sys::JSValueMakeFromJSONString(s.ctx(), text.as_raw()) };
                if raw.is_null() {
                    s.null()
                } else {
                    unsafe { Value::from_raw(s.ctx(), raw) }
                }
            }
            _ => s.null(),
        };
        s.set_hidden(wrapper, RESPONSE, value)?;
        Ok(value)
    }
}
