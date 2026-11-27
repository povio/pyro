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
}

fn index_at(track: &TrackBuffer, t: f64) -> Option<usize> {
    track
        .samples()
        .iter()
        .enumerate()
        .filter(|(_, b)| b.sample.pts <= t && t < b.sample.pts + b.sample.duration)
        .max_by(|(_, a), (_, b)| a.sample.pts.total_cmp(&b.sample.pts))
        .map(|(i, _)| i)
}

impl Presentation {
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
                    decoded = decoder.decode(&s.sample.data, j as u64).ok()?.into_iter().find(|f| f.tag == i as u64);
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
                    if let Ok(Some(p)) = decoder.decode((s.sample.pts * 1e6).round() as i64, s.sample.data.clone()) {
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
            // The controller admits one video and one audio track across its
            // SourceBuffers. A multiplexed buffer can supply both.
            let video_buffer =
                buffers.iter().find(|b| b.borrow().track_buffers().iter().any(|t| t.track().width > 0)).cloned();
            let audio_buffer =
                buffers.iter().find(|b| b.borrow().track_buffers().iter().any(|t| t.track().sample_rate > 0)).cloned();
            let t = now(&p);
            let mut events = Vec::new();
            with(&p, node, |st| {
                st.fold(t);
                let m = st.mse.as_mut().unwrap();
                m.buffered = buffered;
                m.seekable = seekable;
                m.ended = ended;
                m.presentation = Presentation { video_buffer, audio_buffer, ..Default::default() };
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
                    st.error = Some((
                        if status == "decode" { 3 } else { 2 },
                        "MediaSource end of stream error",
                        st.error_serial,
                    ));
                    events.push("error");
                }
            });
            let initialized = events.contains(&"loadedmetadata");
            let truncated = events.contains(&"durationchange")
                && peek(&p, node, |st| st.ready >= HAVE_METADATA && duration.is_finite() && st.position > duration);
            for event in events {
                queue_event(&p, node, gen, event);
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
        // range when it begins within kSeekToStartFudgeRoom (1 s) of the
        // position, so a first PTS a fraction after 0 (edit list, composition
        // offset) does not stall playback in HAVE_METADATA.
        const START_FUDGE: f64 = 1.0;
        let first_start = m.buffered.first().map(|r| r.0);
        let covered = m.buffered.iter().any(|&(a, b)| a <= position && (position < b || m.ended && b == st.duration && position == b))
            || first_start.is_some_and(|a| position < a && a <= START_FUDGE && m.buffered[0].1 > a);
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
