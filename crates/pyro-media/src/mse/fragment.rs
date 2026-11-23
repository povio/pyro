use super::*;

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}
impl Cursor<'_> {
    fn word(&mut self) -> Result<u32> {
        let n = u32at(self.b, self.pos)?;
        self.pos += 4;
        Ok(n)
    }
    fn optional(&mut self, flags: u32, mask: u32, default: u32) -> Result<u32> {
        if flags & mask != 0 { self.word() } else { Ok(default) }
    }
}

pub(super) fn parse(data: &[u8], moof: u64, tracks: &BTreeMap<u32, TrackInfo>) -> Result<Vec<Span>> {
    let mut spans = Vec::new();
    let mut previous_end = moof;
    for item in Boxes(data) {
        let (kind, traf) = item?;
        if kind != b"traf" {
            continue;
        }
        let mut h = Cursor { b: child(traf, b"tfhd")?, pos: 0 };
        let flags = h.word()?;
        let id = h.word()?;
        let info = tracks.get(&id).ok_or(Error::Invalid)?;
        // MSE media segments use movie-fragment-relative addressing. An
        // explicit base offset would refer to the original file byte stream.
        if flags & 1 != 0 {
            return Err(Error::Unsupported);
        }
        let base = if flags & 0x20000 != 0 { moof } else { previous_end };
        if h.optional(flags, 2, 1)? != 1 {
            return Err(Error::Unsupported);
        }
        let duration = h.optional(flags, 8, info.duration)?;
        let size = h.optional(flags, 16, info.size)?;
        let default_flags = h.optional(flags, 32, info.flags)?;
        let tfdt = child(traf, b"tfdt")?;
        let mut time = match tfdt.first() {
            Some(0) => u64::from(u32at(tfdt, 4)?),
            Some(1) => u64at(tfdt, 4)?,
            _ => return Err(Error::Unsupported),
        };
        let scale = f64::from(info.track.timescale);
        let mut offset = base;
        for item in Boxes(traf) {
            let (kind, b) = item?;
            if kind != b"trun" {
                continue;
            }
            let mut r = Cursor { b, pos: 0 };
            let flags = r.word()?;
            let version = flags >> 24;
            if version > 1 {
                return Err(Error::Unsupported);
            }
            let count = r.word()? as usize;
            if count > MAX_SAMPLES - spans.len() {
                return Err(Error::Quota);
            }
            if flags & 1 != 0 {
                offset = base.checked_add_signed(i64::from(r.word()? as i32)).ok_or(Error::Invalid)?;
            }
            let first_flags = r.optional(flags, 4, default_flags)?;
            for i in 0..count {
                let delta = r.optional(flags, 0x100, duration)?;
                let length = r.optional(flags, 0x200, size)?;
                let sf = r.optional(flags, 0x400, if i == 0 { first_flags } else { default_flags })?;
                let cto = r.optional(flags, 0x800, 0)?;
                let cto = if version == 1 { i64::from(cto as i32) } else { i64::from(cto) };
                if length == 0 || delta == 0 {
                    return Err(Error::Unsupported);
                }
                if length as usize > MAX_BOX {
                    return Err(Error::Quota);
                }
                let sample = Sample {
                    track_id: id,
                    dts: time as f64 / scale + info.edit,
                    pts: (i128::from(time) + i128::from(cto)) as f64 / scale + info.edit,
                    duration: f64::from(delta) / scale,
                    key: sf & 0x10000 == 0,
                    data: Bytes::new(),
                };
                spans.push(Span { offset, size: length as usize, sample });
                offset = offset.checked_add(u64::from(length)).ok_or(Error::Invalid)?;
                time = time.checked_add(u64::from(delta)).ok_or(Error::Invalid)?;
            }
        }
        previous_end = offset;
    }
    spans.sort_by_key(|span| span.offset);
    let mut end = moof;
    for span in &spans {
        if span.offset < end {
            return Err(Error::Invalid);
        }
        end = span.offset.checked_add(span.size as u64).ok_or(Error::Invalid)?;
    }
    if spans.is_empty() {
        return Err(Error::Unsupported);
    }
    Ok(spans)
}
