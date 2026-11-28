use super::*;
use crate::cenc::{Scheme, SeigEntry};

type Parsed = (BTreeMap<u32, TrackInfo>, Option<f64>, Vec<Encrypted>);

pub(super) fn parse(data: &[u8]) -> Result<Parsed> {
    let (movie_scale, movie_duration) = timing(child(data, b"mvhd")?)?;
    let mvex = child(data, b"mvex")?;
    let mut duration = movie_duration;
    let mut defaults = BTreeMap::new();
    for item in Boxes(mvex) {
        let (kind, b) = item?;
        match kind {
            b"trex" => {
                if u32at(b, 8)? != 1 {
                    return Err(Error::Unsupported);
                }
                if defaults.insert(u32at(b, 4)?, (u32at(b, 12)?, u32at(b, 16)?, u32at(b, 20)?)).is_some() {
                    return Err(Error::Invalid);
                }
            }
            b"mehd" => duration = duration_field(b, 4)?,
            _ => {}
        }
    }
    // Every `pssh` box of the movie, whole, in file order.
    let mut pssh = Vec::new();
    for item in Boxes(data) {
        let (kind, b) = item?;
        if kind == b"pssh" {
            pssh.extend_from_slice(&((b.len() + 8) as u32).to_be_bytes());
            pssh.extend_from_slice(b"pssh");
            pssh.extend_from_slice(b);
        }
    }
    let mut tracks = BTreeMap::new();
    for item in Boxes(data) {
        let (kind, b) = item?;
        if kind != b"trak" {
            continue;
        }
        if tracks.len() >= 16 {
            return Err(Error::Quota);
        }
        let tkhd = child(b, b"tkhd")?;
        let id = u32at(
            tkhd,
            match tkhd.first() {
                Some(0) => 12,
                Some(1) => 20,
                _ => return Err(Error::Unsupported),
            },
        )?;
        let mdia = child(b, b"mdia")?;
        let (timescale, _) = timing(child(mdia, b"mdhd")?)?;
        let handler = child(mdia, b"hdlr")?.get(8..12).ok_or(Error::Invalid)?;
        let stbl = child(child(mdia, b"minf")?, b"stbl")?;
        let stsd = child(stbl, b"stsd")?;
        if u32at(stsd, 4)? != 1 {
            return Err(Error::Unsupported);
        }
        let (entry, e) = Boxes(&stsd[8..]).next().ok_or(Error::Invalid)??;
        let mut t = Track {
            id,
            codec: String::new(),
            description: Bytes::new(),
            timescale,
            width: 0,
            height: 0,
            sample_rate: 0,
            channels: 0,
            protection: None,
        };
        // A protected sample entry (`encv`/`enca`) wraps the original format in
        // `sinf`: `frma` names it, `schm` the scheme, `schi`/`tenc` the defaults.
        let mut format = entry;
        if entry == b"encv" || entry == b"enca" {
            let sinf = child(e.get(if entry == b"encv" { 78 } else { 28 }..).ok_or(Error::Invalid)?, b"sinf")?;
            format = child(sinf, b"frma")?.get(..4).ok_or(Error::Invalid)?;
            let scheme = Scheme::from_fourcc(child(sinf, b"schm")?.get(4..8).ok_or(Error::Invalid)?)
                .ok_or(Error::Unsupported)?;
            let tenc = child(child(sinf, b"schi")?, b"tenc")?;
            let (default, _) = seig_entry(tenc.get(4..).ok_or(Error::Invalid)?)?;
            let mut groups = Vec::new();
            if let Ok(stbl_groups) = stbl_seig(stbl) {
                groups = stbl_groups;
            }
            t.protection = Some(crate::cenc::Protection { scheme, default, groups });
        }
        match (handler, format) {
            (b"vide", b"avc1" | b"avc3") => {
                if !crate::video::H264 {
                    return Err(Error::Unsupported);
                }
                let desc = child(e.get(78..).ok_or(Error::Invalid)?, b"avcC")?;
                crate::video::demux::avc_config(desc).ok_or(Error::Invalid)?;
                if desc.first() != Some(&1) {
                    return Err(Error::Invalid);
                }
                t.codec =
                    format!("{}.{:02x}{:02x}{:02x}", std::str::from_utf8(format).map_err(|_| Error::Invalid)?, desc[1], desc[2], desc[3]);
                t.width = u32::from(u16at(e, 24)?);
                t.height = u32::from(u16at(e, 26)?);
                if t.width == 0 || t.height == 0 {
                    return Err(Error::Invalid);
                }
                t.description = Bytes::copy_from_slice(desc);
            }
            (b"soun", b"mp4a") => {
                let esds = child(e.get(28..).ok_or(Error::Invalid)?, b"esds")?;
                let asc = audio_specific_config(esds)?;
                let bits = u16at(asc, 0)?;
                if bits >> 11 != 2 {
                    return Err(Error::Unsupported);
                }
                let index = (bits >> 7) & 15;
                t.sample_rate =
                    *[96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]
                        .get(index as usize)
                        .ok_or(Error::Unsupported)?;
                t.channels = match (bits >> 3) & 15 {
                    n @ 1..=6 => u32::from(n),
                    7 => 8,
                    _ => return Err(Error::Unsupported),
                };
                t.codec = "mp4a.40.2".into();
                t.description = Bytes::copy_from_slice(asc);
            }
            _ => return Err(Error::Unsupported),
        }
        let (duration, size, flags) = defaults.remove(&id).ok_or(Error::Invalid)?;
        let edit = if let Ok(edts) = child(b, b"edts") {
            edit_offset(child(edts, b"elst")?, movie_scale, timescale)?
        } else {
            0.0
        };
        if tracks.insert(id, TrackInfo { track: t, duration, size, flags, edit }).is_some() {
            return Err(Error::Invalid);
        }
    }
    if tracks.is_empty() {
        return Err(Error::Unsupported);
    }
    let duration = duration.map(|ticks| ticks as f64 / f64::from(movie_scale));
    let mut kids: Vec<[u8; 16]> = Vec::new();
    for info in tracks.values() {
        if let Some(p) = &info.track.protection {
            for entry in std::iter::once(&p.default).chain(p.groups.iter()) {
                if entry.protected && !kids.contains(&entry.kid) {
                    kids.push(entry.kid);
                }
            }
        }
    }
    let mut encrypted = Vec::new();
    if !kids.is_empty() || !pssh.is_empty() {
        // The `pssh` boxes of the file, then a Common SystemID one (W3C cenc-format) naming the
        // `tenc` key IDs, which is what a key system without its own box (Clear Key) reads.
        if !kids.is_empty() {
            pssh.extend_from_slice(&(32 + 16 * kids.len() as u32).to_be_bytes());
            pssh.extend_from_slice(b"pssh");
            pssh.extend_from_slice(&[1, 0, 0, 0]);
            pssh.extend_from_slice(&[
                0x10, 0x77, 0xef, 0xec, 0xc0, 0xb2, 0x4d, 0x02, 0xac, 0xe3, 0x3c, 0x1e, 0x52, 0xe2, 0xfb, 0x4b,
            ]);
            pssh.extend_from_slice(&(kids.len() as u32).to_be_bytes());
            for kid in &kids {
                pssh.extend_from_slice(kid);
            }
            pssh.extend_from_slice(&[0, 0, 0, 0]);
        }
        encrypted.push(Encrypted { init_data_type: "cenc", init_data: pssh });
    }
    Ok((tracks, duration, encrypted))
}

/// A protection entry of `tenc` (from its reserved byte on) or of a `seig`
/// sample group: the entry and the bytes it spans.
pub(super) fn seig_entry(b: &[u8]) -> Result<(SeigEntry, usize)> {
    let crypt_skip = *b.get(1).ok_or(Error::Invalid)?;
    let protected = *b.get(2).ok_or(Error::Invalid)? != 0;
    let iv_size = *b.get(3).ok_or(Error::Invalid)?;
    if !matches!(iv_size, 0 | 8 | 16) {
        return Err(Error::Invalid);
    }
    let kid: [u8; 16] = b.get(4..20).ok_or(Error::Invalid)?.try_into().unwrap();
    let mut used = 20;
    let mut constant_iv = None;
    if protected && iv_size == 0 {
        let n = usize::from(*b.get(20).ok_or(Error::Invalid)?);
        if n != 8 && n != 16 {
            return Err(Error::Invalid);
        }
        constant_iv = Some(b.get(21..21 + n).ok_or(Error::Invalid)?.to_vec());
        used = 21 + n;
    }
    Ok((SeigEntry { protected, iv_size, kid, constant_iv, crypt_blocks: crypt_skip >> 4, skip_blocks: crypt_skip & 15 }, used))
}

/// The entries of a `seig` `sgpd` box, `None` for another grouping type.
pub(super) fn seig_groups(b: &[u8]) -> Result<Option<Vec<SeigEntry>>> {
    let version = *b.first().ok_or(Error::Invalid)?;
    if b.get(4..8) != Some(&b"seig"[..]) {
        return Ok(None);
    }
    let mut p = 8;
    let mut default_length = 0;
    if version >= 1 {
        default_length = u32at(b, p)? as usize;
        p += 4;
    }
    if version >= 2 {
        p += 4;
    }
    let count = u32at(b, p)? as usize;
    p += 4;
    if count > 4096 {
        return Err(Error::Quota);
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut length = (version >= 1).then_some(default_length);
        if version >= 1 && default_length == 0 {
            length = Some(u32at(b, p)? as usize);
            p += 4;
        }
        let (entry, used) = seig_entry(b.get(p..).ok_or(Error::Invalid)?)?;
        p += length.unwrap_or(used);
        out.push(entry);
    }
    Ok(Some(out))
}

/// The `seig` entries in a track's sample table (`stbl`).
fn stbl_seig(stbl: &[u8]) -> Result<Vec<SeigEntry>> {
    for item in Boxes(stbl) {
        let (kind, b) = item?;
        if kind == b"sgpd" {
            if let Some(groups) = seig_groups(b)? {
                return Ok(groups);
            }
        }
    }
    Ok(Vec::new())
}

fn edit_offset(elst: &[u8], movie: u32, track: u32) -> Result<f64> {
    let count = u32at(elst, 4)?;
    if count > 2 {
        return Err(Error::Unsupported);
    }
    let mut offset = 0.0;
    for i in 0..count as usize {
        let (length, time, rate) = match elst[0] {
            0 => {
                let p = 8 + 12 * i;
                (u64::from(u32at(elst, p)?), i64::from(u32at(elst, p + 4)? as i32), u32at(elst, p + 8)?)
            }
            1 => {
                let p = 8 + 20 * i;
                (u64at(elst, p)?, u64at(elst, p + 8)? as i64, u32at(elst, p + 16)?)
            }
            _ => return Err(Error::Unsupported),
        };
        if rate != 65536 {
            return Err(Error::Unsupported);
        }
        if time == -1 && i == 0 {
            offset += length as f64 / f64::from(movie)
        } else if time >= 0 && i + 1 == count as usize {
            offset -= time as f64 / f64::from(track)
        } else {
            return Err(Error::Unsupported);
        }
    }
    Ok(offset)
}

// MPEG-4 systems descriptors use a one-to-four-octet base-128 length.
fn descriptor(mut b: &[u8], tag: u8) -> Result<&[u8]> {
    while !b.is_empty() {
        let actual = b[0];
        let mut len = 0;
        let mut p = 1;
        loop {
            let v = *b.get(p).ok_or(Error::Invalid)?;
            p += 1;
            len = (len << 7) | usize::from(v & 127);
            if v & 128 == 0 {
                break;
            }
            if p == 5 {
                return Err(Error::Invalid);
            }
        }
        let value = b.get(p..p + len).ok_or(Error::Invalid)?;
        if actual == tag {
            return Ok(value);
        }
        b = &b[p + len..];
    }
    Err(Error::Invalid)
}
fn audio_specific_config(esds: &[u8]) -> Result<&[u8]> {
    let es = descriptor(esds.get(4..).ok_or(Error::Invalid)?, 3)?;
    let flags = *es.get(2).ok_or(Error::Invalid)?;
    let mut p = 3;
    if flags & 0x80 != 0 {
        p += 2
    }
    if flags & 0x40 != 0 {
        p += 1 + usize::from(*es.get(p).ok_or(Error::Invalid)?)
    }
    if flags & 0x20 != 0 {
        p += 2
    }
    let cfg = descriptor(es.get(p..).ok_or(Error::Invalid)?, 4)?;
    if cfg.first() != Some(&0x40) {
        return Err(Error::Unsupported);
    }
    descriptor(cfg.get(13..).ok_or(Error::Invalid)?, 5)
}
