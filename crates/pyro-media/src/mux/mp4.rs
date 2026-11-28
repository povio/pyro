//! A fragmented MP4 writer (ISO/IEC 14496-12 movie fragments): at most one H.264 video track (`avcC`
//! of ISO/IEC 14496-15) and one Opus audio track (`dOps` of "Encapsulation of Opus in ISO Base Media").
//! The first [`FragmentWriter::flush`] that has samples returns the initialization segment (`ftyp` +
//! `moov`: no samples, `mvex` announces the fragments) and then one `moof` + `mdat`; every later flush
//! returns one more fragment, so the concatenation of everything returned is one playable file. A
//! flush without samples returns nothing, so an immediate `requestData()` yields an empty blob.

/// What a track carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Video,
    Audio,
}

/// One encoded sample on the recording timeline (microseconds from the start of the recording; no
/// B pictures, so decode time is presentation time).
pub struct Sample {
    pub data: Vec<u8>,
    pub time_us: u64,
    /// Used only when this is the last sample of a fragment (the others last until the next one).
    pub duration_us: u64,
    /// A sync sample (IDR picture); every Opus packet is one.
    pub key: bool,
}

struct Track {
    kind: Kind,
    id: u32,
    timescale: u32,
    width: u32,
    height: u32,
    channels: u16,
    /// The sample entry's four-character code (`avc1`, `avc3`, `Opus`).
    entry: [u8; 4],
    /// `avcC` or `dOps` payload: known when the encoder has produced its first output.
    config: Option<Vec<u8>>,
    samples: Vec<Sample>,
}

pub struct FragmentWriter {
    tracks: Vec<Track>,
    sequence: u32,
    started: bool,
}

impl Default for FragmentWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentWriter {
    pub fn new() -> Self {
        Self { tracks: Vec::new(), sequence: 0, started: false }
    }

    /// Declares the video track (`entry` is `avc1` or `avc3`); returns its index for [`Self::push`].
    pub fn add_video(&mut self, width: u32, height: u32, entry: [u8; 4]) -> usize {
        self.add(Track {
            kind: Kind::Video,
            id: 0,
            timescale: 90_000,
            width,
            height,
            channels: 0,
            entry,
            config: None,
            samples: Vec::new(),
        })
    }

    /// Declares the Opus track.
    pub fn add_audio(&mut self, channels: u16) -> usize {
        self.add(Track {
            kind: Kind::Audio,
            id: 0,
            timescale: 48_000,
            width: 0,
            height: 0,
            channels,
            entry: *b"Opus",
            config: None,
            samples: Vec::new(),
        })
    }

    fn add(&mut self, mut track: Track) -> usize {
        track.id = self.tracks.len() as u32 + 1;
        self.tracks.push(track);
        self.tracks.len() - 1
    }

    /// The decoder configuration record of a track (`avcC` payload, or the `OpusHead` the Opus
    /// encoder describes itself with, which this converts to `dOps`).
    pub fn set_config(&mut self, track: usize, config: Vec<u8>) {
        let t = &mut self.tracks[track];
        if t.config.is_none() {
            t.config = if t.kind == Kind::Audio { opus_head_to_dops(&config) } else { Some(config) };
        }
    }

    pub fn push(&mut self, track: usize, sample: Sample) {
        self.tracks[track].samples.push(sample);
    }

    /// The pieces of file to deliver now. Empty when there are no samples, or while a declared track still
    /// lacks its configuration (unless `last`: a track that never produced anything is then left out of
    /// the file). The first delivery is the initialization segment alone (the samples wait for the next
    /// one), and a delivery that is not the last keeps back each track's newest sample: its duration is
    /// the distance to the sample after it, so the fragments follow each other exactly (a nominal
    /// duration would leave gaps and overlaps that split a player's buffered range). `last` writes
    /// everything, the kept samples with their nominal duration.
    pub fn flush(&mut self, last: bool) -> Vec<Vec<u8>> {
        if self.tracks.iter().any(|t| t.config.is_none()) {
            if !last {
                return Vec::new();
            }
            self.tracks.retain(|t| t.config.is_some());
        }
        if self.tracks.iter().all(|t| t.samples.is_empty()) {
            return Vec::new();
        }
        let mut parts = Vec::new();
        if !self.started {
            self.started = true;
            let mut init = Vec::new();
            write_init(&mut init, &self.tracks);
            parts.push(init);
            if !last {
                return parts;
            }
        }
        let mut kept = Vec::new();
        if !last {
            for (i, t) in self.tracks.iter_mut().enumerate() {
                if let Some(sample) = t.samples.pop() {
                    if let Some(before) = t.samples.last_mut() {
                        before.duration_us = sample.time_us.saturating_sub(before.time_us);
                    }
                    kept.push((i, sample));
                }
            }
        }
        if !self.tracks.iter().all(|t| t.samples.is_empty()) {
            self.sequence += 1;
            let mut fragment = Vec::new();
            write_fragment(&mut fragment, &mut self.tracks, self.sequence);
            parts.push(fragment);
        }
        for (i, sample) in kept {
            self.tracks[i].samples.push(sample);
        }
        parts
    }
}

/// `OpusHead` (RFC 7845 §5.1, little endian) to `dOps` (big endian, no signature).
fn opus_head_to_dops(head: &[u8]) -> Option<Vec<u8>> {
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return None;
    }
    let mut out = vec![0, head[9]];
    out.extend_from_slice(&u16::from_le_bytes([head[10], head[11]]).to_be_bytes());
    out.extend_from_slice(&u32::from_le_bytes([head[12], head[13], head[14], head[15]]).to_be_bytes());
    out.extend_from_slice(&i16::from_le_bytes([head[16], head[17]]).to_be_bytes());
    out.push(head[18]);
    out.extend_from_slice(&head[19..]);
    Some(out)
}

fn put_box(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - at) as u32;
    out[at..at + 4].copy_from_slice(&size.to_be_bytes());
}

fn full(out: &mut Vec<u8>, version: u8, flags: u32) {
    out.push(version);
    out.extend_from_slice(&flags.to_be_bytes()[1..]);
}

fn u16_(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn u32_(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn zeros(out: &mut Vec<u8>, n: usize) {
    out.resize(out.len() + n, 0);
}

fn matrix(out: &mut Vec<u8>) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        u32_(out, v);
    }
}

fn write_init(out: &mut Vec<u8>, tracks: &[Track]) {
    put_box(out, b"ftyp", |o| {
        o.extend_from_slice(b"isom");
        u32_(o, 0x200);
        for brand in [b"isom", b"iso5", b"iso6", b"avc1", b"mp41"] {
            o.extend_from_slice(brand);
        }
    });
    put_box(out, b"moov", |o| {
        put_box(o, b"mvhd", |o| {
            full(o, 0, 0);
            zeros(o, 8); // creation, modification
            u32_(o, 1000);
            u32_(o, 0); // duration: unknown in a fragmented file
            u32_(o, 0x0001_0000);
            u16_(o, 0x0100);
            zeros(o, 10);
            matrix(o);
            zeros(o, 24);
            u32_(o, tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1); // next_track_ID
        });
        for t in tracks {
            write_trak(o, t);
        }
        put_box(o, b"mvex", |o| {
            for t in tracks {
                put_box(o, b"trex", |o| {
                    full(o, 0, 0);
                    u32_(o, t.id);
                    u32_(o, 1);
                    zeros(o, 12);
                });
            }
        });
    });
}

fn write_trak(out: &mut Vec<u8>, t: &Track) {
    let video = t.kind == Kind::Video;
    put_box(out, b"trak", |o| {
        put_box(o, b"tkhd", |o| {
            full(o, 0, 7);
            zeros(o, 8);
            u32_(o, t.id);
            zeros(o, 4);
            u32_(o, 0);
            zeros(o, 8);
            zeros(o, 4); // layer, alternate group
            u16_(o, if video { 0 } else { 0x0100 });
            zeros(o, 2);
            matrix(o);
            u32_(o, t.width << 16);
            u32_(o, t.height << 16);
        });
        put_box(o, b"mdia", |o| {
            put_box(o, b"mdhd", |o| {
                full(o, 0, 0);
                zeros(o, 8);
                u32_(o, t.timescale);
                u32_(o, 0);
                u16_(o, 0x55c4); // "und"
                u16_(o, 0);
            });
            put_box(o, b"hdlr", |o| {
                full(o, 0, 0);
                u32_(o, 0);
                o.extend_from_slice(if video { b"vide" } else { b"soun" });
                zeros(o, 12);
                o.extend_from_slice(if video { b"VideoHandler\0" } else { b"SoundHandler\0" });
            });
            put_box(o, b"minf", |o| {
                if video {
                    put_box(o, b"vmhd", |o| {
                        full(o, 0, 1);
                        zeros(o, 8);
                    });
                } else {
                    put_box(o, b"smhd", |o| {
                        full(o, 0, 0);
                        zeros(o, 4);
                    });
                }
                put_box(o, b"dinf", |o| {
                    put_box(o, b"dref", |o| {
                        full(o, 0, 0);
                        u32_(o, 1);
                        put_box(o, b"url ", |o| full(o, 0, 1));
                    });
                });
                put_box(o, b"stbl", |o| {
                    put_box(o, b"stsd", |o| {
                        full(o, 0, 0);
                        u32_(o, 1);
                        write_entry(o, t);
                    });
                    for kind in [b"stts", b"stsc", b"stco"] {
                        put_box(o, kind, |o| {
                            full(o, 0, 0);
                            u32_(o, 0);
                        });
                    }
                    put_box(o, b"stsz", |o| {
                        full(o, 0, 0);
                        zeros(o, 8);
                    });
                });
            });
        });
    });
}

fn write_entry(out: &mut Vec<u8>, t: &Track) {
    let config = t.config.as_deref().unwrap_or_default();
    put_box(out, &t.entry, |o| {
        zeros(o, 6);
        u16_(o, 1); // data reference index
        if t.kind == Kind::Video {
            zeros(o, 16);
            u16_(o, t.width as u16);
            u16_(o, t.height as u16);
            u32_(o, 0x0048_0000);
            u32_(o, 0x0048_0000);
            u32_(o, 0);
            u16_(o, 1); // frame count
            zeros(o, 32); // compressor name
            u16_(o, 0x0018);
            u16_(o, 0xffff);
            put_box(o, b"avcC", |o| o.extend_from_slice(config));
        } else {
            zeros(o, 8);
            u16_(o, t.channels);
            u16_(o, 16);
            zeros(o, 4);
            u32_(o, 48_000 << 16); // an Opus track's rate field is always 48 kHz
            put_box(o, b"dOps", |o| o.extend_from_slice(config));
        }
    });
}

/// Ticks of `us` microseconds in `timescale`, rounded.
fn ticks(us: u64, timescale: u32) -> u64 {
    (u128::from(us) * u128::from(timescale) + 500_000).div_euclid(1_000_000) as u64
}

fn write_fragment(out: &mut Vec<u8>, tracks: &mut [Track], sequence: u32) {
    let moof_at = out.len();
    // Where each traf's `data_offset` sits in `out`, and how many sample bytes precede its data.
    let mut patches: Vec<(usize, usize)> = Vec::new();
    let mut before = 0usize;
    put_box(out, b"moof", |o| {
        put_box(o, b"mfhd", |o| {
            full(o, 0, 0);
            u32_(o, sequence);
        });
        for t in tracks.iter().filter(|t| !t.samples.is_empty()) {
            put_box(o, b"traf", |o| {
                put_box(o, b"tfhd", |o| {
                    full(o, 0, 0x02_0000); // default-base-is-moof
                    u32_(o, t.id);
                });
                let start = ticks(t.samples[0].time_us, t.timescale);
                put_box(o, b"tfdt", |o| {
                    full(o, 1, 0);
                    o.extend_from_slice(&start.to_be_bytes());
                });
                put_box(o, b"trun", |o| {
                    full(o, 0, 0x0007_01); // data offset, duration, size, flags per sample
                    u32_(o, t.samples.len() as u32);
                    patches.push((o.len(), before));
                    u32_(o, 0);
                    for (i, s) in t.samples.iter().enumerate() {
                        let from = ticks(s.time_us, t.timescale);
                        let to = match t.samples.get(i + 1) {
                            Some(next) => ticks(next.time_us, t.timescale),
                            None => ticks(s.time_us + s.duration_us, t.timescale),
                        };
                        u32_(o, to.saturating_sub(from).max(1) as u32);
                        u32_(o, s.data.len() as u32);
                        // sample_depends_on 2 (a sync sample) / depends on others and non-sync.
                        u32_(o, if s.key { 0x0200_0000 } else { 0x0101_0000 });
                    }
                });
                before += t.samples.iter().map(|s| s.data.len()).sum::<usize>();
            });
        }
    });
    let moof_len = out.len() - moof_at;
    for (at, preceding) in patches {
        let offset = (moof_len + 8 + preceding) as u32;
        out[at..at + 4].copy_from_slice(&offset.to_be_bytes());
    }
    put_box(out, b"mdat", |o| {
        for t in tracks.iter_mut() {
            for s in std::mem::take(&mut t.samples) {
                o.extend_from_slice(&s.data);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boxes at one level of `data`: (type, payload).
    fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < data.len() {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            assert!(size >= 8 && at + size <= data.len(), "box overruns its parent");
            out.push((data[at + 4..at + 8].try_into().unwrap(), &data[at + 8..at + size]));
            at += size;
        }
        out
    }

    fn head() -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
        head
    }

    #[test]
    fn delivers_the_header_alone_then_exact_fragments() {
        let mut w = FragmentWriter::new();
        let v = w.add_video(64, 32, *b"avc1");
        let a = w.add_audio(2);
        assert!(w.flush(false).is_empty(), "no samples");
        w.set_config(v, vec![1, 66, 0xc0, 30, 0xff, 0xe0, 0]);
        w.push(v, Sample { data: vec![1; 10], time_us: 0, duration_us: 99_000, key: true });
        assert!(w.flush(false).is_empty(), "the audio track has no configuration yet");
        w.set_config(a, head());
        w.push(a, Sample { data: vec![2; 5], time_us: 0, duration_us: 20_000, key: true });
        w.push(a, Sample { data: vec![3; 6], time_us: 20_000, duration_us: 20_000, key: true });
        let header = w.flush(false);
        assert_eq!(header.len(), 1, "the initialization segment goes alone");
        let kinds: Vec<_> = boxes(&header[0]).iter().map(|b| b.0).collect();
        assert_eq!(kinds, [*b"ftyp", *b"moov"]);
        w.push(v, Sample { data: vec![4; 3], time_us: 50_000, duration_us: 50_000, key: false });
        let fragment = w.flush(false).concat();
        let top = boxes(&fragment);
        let kinds: Vec<_> = top.iter().map(|b| b.0).collect();
        assert_eq!(kinds, [*b"moof", *b"mdat"]);
        // The newest sample of each track stays: the video track delivers its first picture with the
        // exact distance to the second, the audio track its first packet.
        let expected: Vec<u8> = [vec![1u8; 10], vec![2; 5]].concat();
        assert_eq!(top[1].1, expected.as_slice());
        let traf: Vec<_> = boxes(top[0].1).into_iter().filter(|b| &b.0 == b"traf").collect();
        assert_eq!(traf.len(), 2);
        let trun = |t: &[u8]| boxes(t).into_iter().find(|b| &b.0 == b"trun").unwrap().1.to_vec();
        let video = trun(traf[0].1);
        assert_eq!(u32::from_be_bytes(video[12..16].try_into().unwrap()), 4500, "50 ms at 90 kHz");
        let audio = trun(traf[1].1);
        let offset = u32::from_be_bytes(audio[8..12].try_into().unwrap()) as usize;
        assert_eq!(fragment[offset], 2, "audio data follows the video data");
        let rest = w.flush(true).concat();
        let kinds: Vec<_> = boxes(&rest).iter().map(|b| b.0).collect();
        assert_eq!(kinds, [*b"moof", *b"mdat"]);
        assert_eq!(boxes(&rest)[1].1.len(), 3 + 6, "the kept samples");
        assert!(w.flush(true).is_empty());
    }

    #[test]
    fn a_track_without_output_is_left_out_of_the_last_flush() {
        let mut w = FragmentWriter::new();
        let _v = w.add_video(64, 32, *b"avc1");
        let a = w.add_audio(1);
        w.set_config(a, head());
        w.push(a, Sample { data: vec![9; 4], time_us: 0, duration_us: 20_000, key: true });
        assert!(w.flush(false).is_empty());
        let file = w.flush(true).concat();
        let top = boxes(&file);
        let kinds: Vec<_> = top.iter().map(|b| b.0).collect();
        assert_eq!(kinds, [*b"ftyp", *b"moov", *b"moof", *b"mdat"]);
        assert_eq!(boxes(top[1].1).iter().filter(|b| &b.0 == b"trak").count(), 1);
    }

    #[test]
    fn converts_opus_head_to_dops() {
        let dops = opus_head_to_dops(&head()).unwrap();
        assert_eq!(dops, [0, 2, 0x01, 0x38, 0, 0, 0xbb, 0x80, 0, 0, 0]);
    }
}
