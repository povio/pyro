use super::*;
use crate::cenc::{Protection, SampleEnc, SeigEntry};

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

/// `moof` is the stream offset of the movie fragment box, `payload` that of its first child
/// (auxiliary sample information is addressed from the fragment, and lives in it).
pub(super) fn parse(data: &[u8], moof: u64, payload: u64, tracks: &BTreeMap<u32, TrackInfo>) -> Result<Vec<Span>> {
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
        let first = spans.len();
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
                    enc: None,
                };
                spans.push(Span { offset, size: length as usize, sample });
                offset = offset.checked_add(u64::from(length)).ok_or(Error::Invalid)?;
                time = time.checked_add(u64::from(delta)).ok_or(Error::Invalid)?;
            }
        }
        if let Some(protection) = &info.track.protection {
            let encs = sample_encryption(traf, protection, spans.len() - first, base, payload, data)?;
            for (span, enc) in spans[first..].iter_mut().zip(encs) {
                span.sample.enc = enc;
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

/// The Common Encryption parameters of the `count` samples of a `traf`: the sample group
/// (`sbgp`/`sgpd` `seig`) each belongs to, and its initialization vector and subsample map from
/// `senc` or from the auxiliary information (`saiz`/`saio`) when `senc` is absent.
fn sample_encryption(
    traf: &[u8],
    protection: &Protection,
    count: usize,
    base: u64,
    payload: u64,
    moof: &[u8],
) -> Result<Vec<Option<Box<SampleEnc>>>> {
    let mut local: Vec<SeigEntry> = Vec::new();
    let mut assigned: Vec<u32> = Vec::new();
    let (mut senc, mut saiz, mut saio) = (None, None, None);
    for item in Boxes(traf) {
        let (kind, b) = item?;
        match kind {
            b"sgpd" => {
                if let Some(groups) = super::init::seig_groups(b)? {
                    local = groups;
                }
            }
            b"sbgp" => {
                if b.get(4..8) == Some(&b"seig"[..]) {
                    let mut p = if b.first() == Some(&1) { 12 } else { 8 };
                    let entries = u32at(b, p)? as usize;
                    p += 4;
                    for _ in 0..entries {
                        let samples = u32at(b, p)? as usize;
                        let index = u32at(b, p + 4)?;
                        p += 8;
                        let room = count.saturating_sub(assigned.len());
                        assigned.resize(assigned.len() + samples.min(room), index);
                    }
                }
            }
            b"senc" => senc = Some(b),
            b"saiz" => saiz = Some(b),
            b"saio" => saio = Some(b),
            _ => {}
        }
    }
    // Where the per-sample records are: (bytes, whether `senc` says they carry subsamples,
    // the per-sample record sizes of `saiz` when that gave the location).
    let mut sizes: Option<(u8, &[u8])> = None;
    let (aux, subsamples_flag): (Option<&[u8]>, Option<bool>) = if let Some(b) = senc {
        let flags = u32at(b, 0)?;
        if u32at(b, 4)? as usize != count {
            return Err(Error::Invalid);
        }
        (Some(b.get(8..).ok_or(Error::Invalid)?), Some(flags & 2 != 0))
    } else if let (Some(saiz), Some(saio)) = (saiz, saio) {
        let skip = if u32at(saiz, 0)? & 1 != 0 { 8 } else { 0 };
        let default = *saiz.get(4 + skip).ok_or(Error::Invalid)?;
        if u32at(saiz, 5 + skip)? as usize != count {
            return Err(Error::Invalid);
        }
        sizes = Some((default, saiz.get(9 + skip..).ok_or(Error::Invalid)?));
        let version = *saio.first().ok_or(Error::Invalid)?;
        let skip = if u32at(saio, 0)? & 1 != 0 { 8 } else { 0 };
        // One offset: the records of every run are contiguous.
        if u32at(saio, 4 + skip)? != 1 {
            return Err(Error::Unsupported);
        }
        let offset = if version == 0 { u64::from(u32at(saio, 8 + skip)?) } else { u64at(saio, 8 + skip)? };
        let absolute = base.checked_add(offset).ok_or(Error::Invalid)?;
        let at = usize::try_from(absolute.checked_sub(payload).ok_or(Error::Unsupported)?).map_err(|_| Error::Invalid)?;
        // Auxiliary data in the media data box is not retained yet when the fragment is parsed.
        (Some(moof.get(at..).ok_or(Error::Unsupported)?), None)
    } else {
        (None, None)
    };
    let mut out = Vec::with_capacity(count);
    let mut p = 0;
    for k in 0..count {
        let entry = group_entry(k, &assigned, protection, &local)?;
        let mut iv = Vec::new();
        let mut subsamples = Vec::new();
        if let Some(aux) = aux {
            let start = p;
            let size = usize::from(entry.iv_size);
            iv = aux.get(p..p + size).ok_or(Error::Invalid)?.to_vec();
            p += size;
            let with_subsamples = match (subsamples_flag, &sizes) {
                (Some(flag), _) => flag,
                (None, Some((default, list))) => {
                    let info = if *default != 0 { usize::from(*default) } else { usize::from(*list.get(k).ok_or(Error::Invalid)?) };
                    info > size
                }
                _ => false,
            };
            if with_subsamples {
                let n = usize::from(u16at(aux, p)?);
                p += 2;
                for _ in 0..n {
                    subsamples.push((u16at(aux, p)?, u32at(aux, p + 2)?));
                    p += 6;
                }
            }
            if let Some((default, list)) = &sizes {
                let info = if *default != 0 { usize::from(*default) } else { usize::from(*list.get(k).ok_or(Error::Invalid)?) };
                p = start + info;
            }
        }
        if !entry.protected {
            out.push(None);
            continue;
        }
        if iv.is_empty() {
            // `Per_Sample_IV_Size` 0: the track's constant IV (cbcs).
            match &entry.constant_iv {
                Some(constant) => iv = constant.clone(),
                None => return Err(Error::Invalid),
            }
        }
        out.push(Some(Box::new(SampleEnc {
            scheme: protection.scheme,
            kid: entry.kid,
            iv,
            subsamples,
            crypt_blocks: entry.crypt_blocks,
            skip_blocks: entry.skip_blocks,
        })));
    }
    Ok(out)
}

/// The protection entry of sample `k`: its `seig` group (indices above 0x10000 name the
/// fragment's own `sgpd`), else the `tenc` default.
fn group_entry<'a>(k: usize, assigned: &[u32], protection: &'a Protection, local: &'a [SeigEntry]) -> Result<&'a SeigEntry> {
    match assigned.get(k).copied().unwrap_or(0) {
        0 => Ok(&protection.default),
        i if i > 0x10000 => local.get((i - 0x10001) as usize).ok_or(Error::Invalid),
        i => protection.groups.get(i as usize - 1).ok_or(Error::Invalid),
    }
}
