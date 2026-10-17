use super::*;

pub(super) struct Text {
    decoder: encoding_rs::Decoder,
    chunks: usize,
    done: bool,
    value: String,
}
impl Xhr {
    pub(super) fn mime(&self) -> String {
        self.override_mime
            .borrow()
            .clone()
            .or_else(|| {
                self.head
                    .borrow()
                    .as_ref()
                    .and_then(|h| h.headers.get(header::CONTENT_TYPE))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }
    fn text<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        if self.ready.get() < 3 || self.head.borrow().is_none() {
            return Ok(s.string(""));
        }
        let Some(wire) = self.wire.borrow().clone() else { return Ok(s.string("")) };
        let mut cache = self.text.borrow_mut();
        let text = cache.get_or_insert_with(|| {
            let mime = self.mime();
            let charset = mime
                .split(';')
                .skip(1)
                .filter_map(|part| part.trim().split_once('='))
                .find(|(k, _)| k.eq_ignore_ascii_case("charset"))
                .map(|(_, v)| v.trim().trim_matches('"'));
            let encoding =
                charset.and_then(|v| encoding_rs::Encoding::for_label(v.as_bytes())).unwrap_or(encoding_rs::UTF_8);
            Text { decoder: encoding.new_decoder_with_bom_removal(), chunks: 0, done: false, value: String::new() }
        });
        let wire = wire.lock().unwrap();
        // Failure can release buffers on the I/O thread before its browser
        // notification changes readyState. Never reuse a cursor into that
        // released buffer.
        if matches!(wire.terminal, Some(Err(_))) {
            return Ok(s.string(""));
        }
        for chunk in &wire.chunks[text.chunks..] {
            text.value
                .reserve(text.decoder.max_utf8_buffer_length(chunk.len()).unwrap_or(chunk.len().saturating_mul(3)));
            let (result, _, _) = text.decoder.decode_to_string(chunk, &mut text.value, false);
            debug_assert_eq!(result, encoding_rs::CoderResult::InputEmpty);
        }
        text.chunks = wire.chunks.len();
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
    pub(super) fn response<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let kind = self.response_type.borrow().clone();
        if matches!(kind.as_str(), "" | "text") {
            return self.text(s);
        }
        if self.ready.get() != 4 || self.head.borrow().is_none() {
            return Ok(s.null());
        }
        let wrapper = self.wrapper(s)?;
        if let Some(value) = s.get_hidden(wrapper, RESPONSE)?.filter(|v| !v.is_undefined()) {
            return Ok(value);
        }
        let bytes = {
            let wire = self.wire.borrow();
            let Some(wire) = wire.as_ref() else { return Ok(s.null()) };
            let wire = wire.lock().unwrap();
            let mut out = Vec::with_capacity(wire.bytes);
            for c in &wire.chunks {
                out.extend_from_slice(c);
            }
            out
        };
        let value = match kind.as_str() {
            "arraybuffer" => crate::encoding::array_buffer(s, &bytes)?,
            "blob" => s
                .wrap::<crate::file::Blob>(
                    crate::file::BlobData { bytes: Bytes::from(bytes), mime: self.mime(), file: None },
                    None,
                )?
                .value(),
            "json" => {
                let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes));
                let text = lp_js::JsString::new(text.as_ref());
                // SAFETY: the scope/string are live. Wrap and trace the result
                // before another JS call; invalid JSON is the null response.
                let raw = unsafe { lp_js::sys::JSValueMakeFromJSONString(s.ctx(), text.as_raw()) };
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
