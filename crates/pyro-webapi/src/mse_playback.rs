//! Local media source attachment and presentation of the retained coded frames.
use super::*;
use pyro_media::mse_buffer::{CodedBuffer, TrackBuffer};
use pyro_media::video::{Frame, PacketConfig, PacketPixels};

pub(super) struct Source {
    realm: WeakRealm,
    wrapper: Persistent,
}

pub(crate) fn register(s: &Scope<'_>, page: &PageDom, url: String, value: Value<'_>) {
    page.media
        .source_urls
        .borrow_mut()
        .insert(url, Rc::new(Source { realm: s.realm().downgrade(), wrapper: Persistent::new(s, value) }));
}

fn owner(page: &Rc<PageDom>, url: &str) -> Option<Rc<PageDom>> {
    if page.media.source_urls.borrow().contains_key(url) {
        return Some(page.clone());
    }
    page.hooks()?.frame_tree().into_iter().find(|p| {
        p.origin_key() == page.origin_key()
            && p.cookie_partition_key() == page.cookie_partition_key()
            && p.cookie_site_context() == page.cookie_site_context()
            && p.media.source_urls.borrow().contains_key(url)
    })
}

pub(crate) fn clear_urls(page: &PageDom) {
    page.media.source_urls.borrow_mut().clear();
}

pub(crate) fn revoke(page: &Rc<PageDom>, url: &str) {
    if let Some(p) = owner(page, url) {
        p.media.source_urls.borrow_mut().remove(url);
    }
}

#[derive(Default)]
pub(super) struct Presentation {
    video_buffer: Option<Rc<RefCell<CodedBuffer>>>,
    audio_buffer: Option<Rc<RefCell<CodedBuffer>>>,
    frame: Option<Frame>,
    audio: Option<(f64, pyro_media::audio::Decoded)>,
    decoder: Option<(usize, Rc<pyro_media::mse::Track>, pyro_media::audio::packet::PacketDecoder)>,
    /// EME: the keys of the element's `MediaKeys`, which its Common Encryption samples decrypt with.
    keys: Option<crate::eme::SharedKeys>,
}

/// The access unit of `sample` as the decoder takes it: a protected sample decrypted with the key of its
/// key ID (`pyro_media::cenc`); `None` while that key is missing, or for ciphertext that does not parse.
fn clear_data(sample: &pyro_media::mse::Sample, keys: Option<&crate::eme::SharedKeys>) -> Option<Bytes> {
    let Some(enc) = sample.enc.as_deref().filter(|enc| !enc.is_clear()) else { return Some(sample.data.clone()) };
    let key = keys?.borrow().key(&enc.kid)?;
    pyro_media::cenc::decrypt(enc, &crate::eme::Aes::new(&key), &sample.data).map(Bytes::from)
}

/// Whether `sample` is protected and its key is not known: EME "waiting for key".
fn lacks_key(sample: &pyro_media::mse::Sample, keys: Option<&crate::eme::SharedKeys>) -> bool {
    sample.enc.as_deref().filter(|enc| !enc.is_clear()).is_some_and(|enc| {
        !keys.is_some_and(|keys| keys.borrow().key(&enc.kid).is_some())
    })
}

fn index_at(track: &TrackBuffer, t: f64) -> Option<usize> {
    let samples = track.samples();
    let spanning = samples
        .iter()
        .enumerate()
        .filter(|(_, b)| b.sample.pts <= t && t < b.sample.pts + b.sample.duration)
        .max_by(|(_, a), (_, b)| a.sample.pts.total_cmp(&b.sample.pts))
        .map(|(i, _)| i);
    // No frame spans `t`: the first one starting within the track's fudge room after it stands in
    // (Chrome's `SourceBufferRange::CanSeekTo`: a position a little before buffered data plays it).
    spanning.or_else(|| {
        samples
            .iter()
            .enumerate()
            .filter(|(_, b)| b.sample.pts > t && b.sample.pts - t <= track.fudge_room())
            .min_by(|(_, a), (_, b)| a.sample.pts.total_cmp(&b.sample.pts))
            .map(|(i, _)| i)
    })
}

impl Presentation {
    /// How far before a buffered range the position may be and still play it: the largest fudge room
    /// of the tracks, so a seek to the 3.298333 s of a test manifest reaches a segment whose first
    /// frame is at 3.2983333 s, and data that starts a frame after a truncated duration is reached.
    pub(super) fn seek_fudge(&self) -> f64 {
        let buffers = self.video_buffer.iter().chain(self.audio_buffer.iter());
        buffers.flat_map(|b| b.borrow().track_buffers().iter().map(|t| t.fudge_room()).collect::<Vec<_>>()).fold(0., f64::max)
    }

    pub(super) fn set_keys(&mut self, keys: Option<crate::eme::SharedKeys>) {
        self.keys = keys;
        // What was decoded with the previous keys is stale.
        self.frame = None;
        self.audio = None;
        self.decoder = None;
    }

    /// The protected sample presented at `t`, in either track, has no key: playback cannot proceed.
    pub(super) fn key_missing(&self, t: f64) -> bool {
        let tracks = self.video_buffer.iter().chain(self.audio_buffer.iter());
        tracks.into_iter().any(|buffer| {
            // Clear playback: a track whose configuration has no `protection` has no protected sample,
            // so the position lookup below (a linear scan in a gap) is skipped.
            buffer.borrow().track_buffers().iter().filter(|track| track.track().protection.is_some()).any(|track| {
                let samples = track.samples();
                // Within the start fudge of the first buffered range the position precedes its first sample.
                let at = index_at(track, t).or_else(|| {
                    (0..samples.len())
                        .filter(|&i| samples[i].sample.pts > t && samples[i].sample.pts - t < 1.0)
                        .min_by(|&a, &b| samples[a].sample.pts.total_cmp(&samples[b].sample.pts))
                });
                at.is_some_and(|i| lacks_key(&samples[i].sample, self.keys.as_ref()))
            })
        })
    }

    /// The configuration of the selected track at this presentation time. While
    /// waiting in a gap, retain the dimensions of the preceding coded picture.
    pub(super) fn dimensions(&self, time: f64) -> (u32, u32) {
        for buffer in self.video_buffer.iter() {
            let buffer = buffer.borrow();
            if let Some(track) = buffer.track_buffers().iter().find(|t| t.track().width > 0) {
                let samples = track.samples();
                let sample = samples
                    .iter()
                    .filter(|s| s.sample.pts <= time)
                    .max_by(|a, b| a.sample.pts.total_cmp(&b.sample.pts))
                    .or_else(|| samples.iter().min_by(|a, b| a.sample.pts.total_cmp(&b.sample.pts)));
                let config = sample.map_or(track.track(), |s| &*s.config);
                return (config.width, config.height);
            }
        }
        (0, 0)
    }

    pub(super) fn frame(&mut self, t: f64) -> Option<Frame> {
        for b in self.video_buffer.iter() {
            let b = b.borrow();
            for track in b.track_buffers().iter().filter(|tr| tr.track().width > 0) {
                let Some(i) = index_at(track, t) else { continue };
                let samples = track.samples();
                let target = &samples[i];
                if let Some(frame) = self.frame.as_ref().filter(|f| f.time == target.sample.pts) {
                    return Some(frame.clone());
                }
                let start =
                    (0..=i).rev().find(|&n| samples[n].sample.key && Rc::ptr_eq(&samples[n].config, &target.config))?;
                let mut decoder =
                    PacketConfig::parse(&target.config.codec, Some(&target.config.description))?.decoder()?;
                let mut decoded = None;
                for (j, s) in
                    samples.iter().enumerate().skip(start).take_while(|(_, s)| Rc::ptr_eq(&s.config, &target.config))
                {
                    let data = clear_data(&s.sample, self.keys.as_ref())?;
                    decoded = decoder.decode(&data, j as u64).ok()?.into_iter().find(|f| f.tag == i as u64);
                    if decoded.is_some() {
                        break;
                    }
                }
                let f = decoded.or_else(|| decoder.flush().ok()?.into_iter().find(|f| f.tag == i as u64))?;
                let PacketPixels::Rgba(rgba) = f.pixels else {
                    return None;
                };
                let frame =
                    Frame { index: i, time: target.sample.pts, width: f.width, height: f.height, rgba: rgba.into() };
                self.frame = Some(frame.clone());
                return Some(frame);
            }
        }
        None
    }

    pub(super) fn audio_at(&mut self, t: f64, seeking: bool) {
        use pyro_media::audio::packet::{PacketAudio, PacketDecoder};
        for buffer in self.audio_buffer.iter() {
            let buffer = buffer.borrow();
            for track in buffer.track_buffers().iter().filter(|tr| tr.track().sample_rate > 0) {
                let Some(i) = index_at(track, t) else { continue };
                let samples = track.samples();
                let c = &samples[i].config;
                if self.audio.as_ref().is_some_and(|a| a.0 == samples[i].sample.pts) {
                    return;
                }
                let (start, mut decoder) = match self.decoder.take() {
                    Some((next, config, decoder)) if !seeking && next <= i && Rc::ptr_eq(&config, c) => (next, decoder),
                    _ => {
                        let Ok(decoder) = PacketDecoder::new(&c.codec, c.sample_rate, c.channels, Some(&c.description))
                        else {
                            continue;
                        };
                        // Prime the AAC overlap window when entering at a new position.
                        (i.saturating_sub(1), decoder)
                    }
                };
                for s in &samples[start..=i] {
                    if !Rc::ptr_eq(&s.config, c) {
                        continue;
                    }
                    let Some(data) = clear_data(&s.sample, self.keys.as_ref()) else { continue };
                    if let Ok(Some(p)) = decoder.decode((s.sample.pts * 1e6).round() as i64, data) {
                        if let PacketAudio::Planar(pcm) = p.audio {
                            self.audio = Some((s.sample.pts, pcm));
                        }
                    }
                }
                self.decoder = Some((i + 1, c.clone(), decoder));
                return;
            }
        }
        self.decoder = None;
        self.audio = None;
    }
}

pub(super) struct Attachment {
    source: Rc<Source>,
    pub(super) presentation: Presentation,
    pub(super) buffered: Vec<(f64, f64)>,
    pub(super) seekable: Vec<(f64, f64)>,
    pub(super) ended: bool,
    loaded_data: bool,
    /// EME: `waitingforkey` was fired for the stall that is still going on.
    waiting_key: bool,
}

impl Attachment {
    pub(super) fn video_frame(&mut self, position: f64, duration: f64) -> Option<Frame> {
        // The final frame remains visible with the clock parked at the exclusive range end.
        let position = if self.ended && position == duration && position > 0. {
            // The next double below (f64::next_down needs Rust 1.86).
            f64::from_bits(position.to_bits() - 1)
        } else {
            position
        };
        self.presentation.frame(position)
    }

    pub(super) fn reset_waiting_key(&mut self) {
        self.waiting_key = false;
    }

    pub(super) fn detach(self) {
        if let Some(realm) = self.source.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                if let Some(o) = self.source.wrapper.get(&s).as_object() {
                    let _ = crate::media_source::detach(&s, o);
                }
            }
        }
    }
}

fn ranges(v: Value<'_>) -> JsResult<Vec<(f64, f64)>> {
    let a = v.as_object().ok_or(JsError::ContextDestroyed)?;
    let len = a.get_atom("length")?.to_number()? as u32;
    (0..len / 2).map(|i| Ok((a.get_index(i * 2)?.to_number()?, a.get_index(i * 2 + 1)?.to_number()?))).collect()
}

pub(super) fn attach(page: &Rc<PageDom>, node: NodeId, gen: u32, url: &str) -> Option<bool> {
    let owner = owner(page, url)?;
    let source = owner.media.source_urls.borrow().get(url)?.clone();
    let run = || -> JsResult<bool> {
        let realm = source.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let s = realm.scope()?;
        let object = source.wrapper.get(&s).as_object().ok_or(JsError::ContextDestroyed)?;
        let weak = Rc::downgrade(page);
        let weak_source = Rc::downgrade(&source);
        let cb = s.function("mediaSourceChanged", move |s, _, a| {
            let (Some(p), Some(source)) = (weak.upgrade(), weak_source.upgrade()) else { return Ok(s.undefined()) };
            if p.is_disposed() || peek(&p, node, |st| st.generation != gen || st.mse.is_none()) {
                return Ok(s.undefined());
            }
            let status = a.get(0).to_string()?;
            let duration = a.get(1).to_number()?;
            let buffered = ranges(a.get(2))?;
            let seekable = ranges(a.get(3))?;
            let metadata = a.get(4).to_bool();
            let ended = a.get(5).to_bool();
            let object = source.wrapper.get(s).as_object().ok_or(JsError::ContextDestroyed)?;
            let buffers = crate::media_source::presentation(s, object)?;
            let encrypted = crate::media_source::take_encrypted(s, object)?;
            // The controller admits one video and one audio track across its
            // SourceBuffers. A multiplexed buffer can supply both.
            let video_buffer =
                buffers.iter().find(|b| b.borrow().track_buffers().iter().any(|t| t.track().width > 0)).cloned();
            let audio_buffer =
                buffers.iter().find(|b| b.borrow().track_buffers().iter().any(|t| t.track().sample_rate > 0)).cloned();
            let t = now(&p);
            let mut events = Vec::new();
            // MSE "end of stream" with an error while the element is at HAVE_NOTHING: the
            // dedicated media source failure steps (MEDIA_ERR_SRC_NOT_SUPPORTED, then the
            // source is detached and closes).
            let mut source_failed = false;
            with(&p, node, |st| {
                st.fold(t);
                let keys = st.keys.clone();
                let m = st.mse.as_mut().unwrap();
                m.buffered = buffered;
                m.seekable = seekable;
                m.ended = ended;
                m.presentation = Presentation { video_buffer, audio_buffer, keys, ..Default::default() };
                if duration != st.duration && !(duration.is_nan() && st.duration.is_nan()) {
                    st.duration = duration;
                    events.push("durationchange");
                }
                if metadata && st.ready == HAVE_NOTHING {
                    st.ready = HAVE_METADATA;
                    events.push("loadedmetadata");
                }
                // The end-of-stream error is reported once per load: later
                // notifications still carry the source's error status.
                if (status == "decode" || status == "network") && st.error.is_none() {
                    st.error_serial = st.error_serial.wrapping_add(1);
                    source_failed = st.ready == HAVE_NOTHING;
                    st.error = Some((
                        if source_failed {
                            4
                        } else if status == "decode" {
                            3
                        } else {
                            2
                        },
                        "MediaSource end of stream error",
                        st.error_serial,
                    ));
                    events.push("error");
                }
            });
            // EME "initialization data encountered" (init segments carrying protection information).
            for (init_data_type, init_data) in encrypted {
                queue(&p, node, gen, move |p| crate::eme::fire_encrypted(p, node, init_data_type, &init_data));
            }
            let initialized = events.contains(&"loadedmetadata");
            let truncated = events.contains(&"durationchange")
                && peek(&p, node, |st| st.ready >= HAVE_METADATA && duration.is_finite() && st.position > duration);
            for event in events {
                queue_event(&p, node, gen, event);
            }
            if source_failed {
                queue(&p, node, gen, move |p| {
                    if let Some(attachment) = with(p, node, |st| st.mse.take()) {
                        attachment.detach();
                    }
                });
                return Ok(s.undefined());
            }
            if truncated {
                seek(&p, node, duration);
            } else if initialized {
                let start = with(&p, node, |st| std::mem::take(&mut st.default_start));
                if start != 0. {
                    seek(&p, node, start);
                }
            }
            refresh(&p, node);
            Ok(s.undefined())
        })?;
        with(page, node, |st| {
            st.mse = Some(Attachment {
                source: source.clone(),
                presentation: Default::default(),
                buffered: vec![],
                seekable: vec![],
                ended: false,
                loaded_data: false,
                waiting_key: false,
            });
            st.network = NETWORK_LOADING;
        });
        let attached = crate::media_source::attach(&s, object, cb.value());
        if !matches!(attached, Ok(true)) {
            with(page, node, |st| st.mse.take());
            return attached;
        }
        set_delaying(page, node, false);
        Ok(true)
    };
    Some(run().unwrap_or(false))
}

pub(super) fn refresh(p: &Rc<PageDom>, node: NodeId) {
    let t = now(p);
    let mut size_change = None;
    let mut waiting_key = false;
    let Some((gen, old, ready, first, playing)) = with(p, node, |st| {
        let position = st.position_at(t);
        let m = st.mse.as_mut()?;
        if st.ready < HAVE_METADATA {
            return None;
        }
        let size = m.presentation.dimensions(position);
        if size != st.video {
            st.video = size;
            size_change = Some(size);
        }
        let old = st.ready;
        // Chrome's SourceBufferStream seeks to the start of the first buffered
        // range when it begins within kSeekToStartFudgeRoom (1 s, strictly less) of the
        // position, so a first PTS a fraction after 0 (edit list, composition
        // offset) does not stall playback in HAVE_METADATA.
        const START_FUDGE: f64 = 1.0;
        let first_start = m.buffered.first().map(|r| r.0);
        let fudge = m.presentation.seek_fudge();
        let covered = m.buffered.iter().any(|&(a, b)| a - fudge <= position && (position < b || m.ended && b == st.duration && position == b))
            || first_start.is_some_and(|a| position <= a && a < START_FUDGE && m.buffered[0].1 > a);
        // EME: data is there but its key is not (a protected sample at the position): the element
        // stays at HAVE_METADATA and fires `waitingforkey` until a session provides the key.
        let blocked = covered && m.presentation.key_missing(position);
        waiting_key = blocked && !m.waiting_key;
        m.waiting_key = blocked;
        let covered = covered && !blocked;
        let ready = if covered {
            if m.ended {
                HAVE_ENOUGH_DATA
            } else {
                HAVE_FUTURE_DATA
            }
        } else {
            HAVE_METADATA
        };
        let first = covered && !m.loaded_data;
        m.loaded_data |= covered;
        if covered {
            m.presentation.audio_at(position, st.seeking);
        }
        st.ready = ready;
        Some((st.generation, old, ready, first, !st.paused))
    }) else {
        return;
    };
    if waiting_key {
        queue_event(p, node, gen, "waitingforkey");
    }
    if let Some((width, height)) = size_change {
        let size = (width > 0 && height > 0).then_some((f64::from(width), f64::from(height)));
        p.dom.borrow_mut().set_natural_size(node, size);
        queue_event(p, node, gen, "resize");
    }
    if ready >= HAVE_CURRENT_DATA {
        if let Some(serial) = peek(p, node, |st| st.seeking.then_some(st.seek_serial)) {
            complete_seek(p, node, gen, serial);
        }
    }
    if first {
        queue_event(p, node, gen, "loadeddata");
    }
    if old < HAVE_FUTURE_DATA && ready >= HAVE_FUTURE_DATA {
        queue_event(p, node, gen, "canplay");
        if playing {
            queue(p, node, gen, move |p| notify_playing(p, node, gen));
        }
    }
    if old < HAVE_ENOUGH_DATA && ready == HAVE_ENOUGH_DATA {
        queue_event(p, node, gen, "canplaythrough");
        if peek(p, node, |st| st.can_autoplay && st.paused) && attr(p, node, "autoplay") && may_play(p, node) {
            with(p, node, |st| {
                st.paused = false;
                st.show_poster = false;
            });
            queue_event(p, node, gen, "play");
            queue(p, node, gen, move |p| notify_playing(p, node, gen));
        }
    }
    if playing && old >= HAVE_FUTURE_DATA && ready < HAVE_FUTURE_DATA {
        queue_event(p, node, gen, "waiting");
    }
    sync_clock(p, node);
    update_pin(p, node);
    want_frame(p, node);
}

#[cfg(feature = "test-hooks")]
impl PageDom {
    /// Decoded output held by a headless media element at its presentation position.
    pub fn media_audio_channel(&self, node: NodeId) -> Option<(f64, u32, Vec<f32>)> {
        let elements = self.media.elements.borrow();
        let (time, pcm) = elements.get(&node)?.mse.as_ref()?.presentation.audio.as_ref()?;
        Some((*time, pcm.sample_rate, pcm.channels.first()?.clone()))
    }
}
