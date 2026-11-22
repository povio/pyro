use pyro_media::mse::{Event, Mp4Parser, Sample, Track};

fn read(input: &[u8], chunk: usize) -> (Vec<Track>, Vec<Sample>) {
    let mut parser = Mp4Parser::default();
    let mut tracks = Vec::new();
    let mut samples = Vec::new();
    for bytes in input.chunks(chunk) {
        for event in parser.append(bytes).unwrap() {
            match event {
                Event::Init(t, _) => {
                    assert!(tracks.is_empty());
                    tracks = t
                }
                Event::Samples(s) => samples.extend(s),
            }
        }
    }
    assert!(!parser.partial_segment());
    assert_eq!(parser.buffered_bytes(), 0);
    (tracks, samples)
}

#[cfg(feature = "h264")]
#[test]
fn fragmented_video_decodes_two_color_segments() {
    let (tracks, samples) = read(include_bytes!("fixtures/fragmented-red-blue.mp4"), 13);
    let t = &tracks[0];
    assert_eq!((t.width, t.height), (64, 48));
    assert!(t.codec.starts_with("avc1.42"));
    assert_eq!(samples.len(), 10);
    let mut decoder =
        pyro_media::video::PacketConfig::parse(&t.codec, Some(&t.description)).unwrap().decoder().unwrap();
    let mut frames = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        assert_eq!(s.track_id, t.id);
        assert_eq!(s.key, i == 0 || i == 5);
        assert!((s.pts - i as f64 * 0.1).abs() < 1e-8);
        assert_eq!(s.dts, s.pts);
        assert!((s.duration - 0.1).abs() < 1e-8);
        frames.extend(decoder.decode(&s.data, i as u64).unwrap());
    }
    frames.extend(decoder.flush().unwrap());
    assert_eq!(frames.len(), 10);
    for f in frames {
        use pyro_media::video::PacketPixels;
        assert_eq!((f.width, f.height), (64, 48));
        match f.pixels {
            PacketPixels::Rgba(bytes) => {
                let rgb = &bytes[(24 * 64 + 32) * 4..][..3];
                if f.tag < 5 {
                    assert!(rgb[0] > 200 && rgb[2] < 40);
                } else {
                    assert!(rgb[2] > 200 && rgb[0] < 40);
                }
            }
            PacketPixels::I420LimitedBt709(bytes) => {
                let chroma_index = 12 * 32 + 16;
                let u = bytes[64 * 48 + chroma_index];
                let v = bytes[64 * 48 + 32 * 24 + chroma_index];
                if f.tag < 5 {
                    assert!(v > 200 && u < 125);
                } else {
                    assert!(u > 200 && v < 130);
                }
            }
        }
    }
}

#[test]
fn fragmented_aac_decodes_440_hz_mono() {
    use pyro_media::audio::packet::{PacketAudio, PacketDecoder};
    let (tracks, samples) = read(include_bytes!("fixtures/fragmented-tone.mp4"), 7);
    let t = &tracks[0];
    assert_eq!((t.codec.as_str(), t.sample_rate, t.channels), ("mp4a.40.2", 48000, 1));
    assert_eq!(samples.len(), 48);
    let mut decoder = PacketDecoder::new(&t.codec, t.sample_rate, t.channels, Some(&t.description)).unwrap();
    let mut pcm = Vec::new();
    for s in samples {
        assert!(s.key && s.duration > 0.0);
        if let Some(output) = decoder.decode((s.pts * 1000000.0).round() as i64, s.data).unwrap() {
            let PacketAudio::Planar(audio) = output.audio else { panic!("expected AAC PCM") };
            pcm.extend_from_slice(&audio.channels[0]);
        }
    }
    let mid = &pcm[12000..36000];
    let crossings = mid.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count();
    assert!(crossings.abs_diff(220) <= 2);
    assert!(mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32 > 0.0001);
}

#[cfg(feature = "h264")]
#[test]
fn multiplexed_runs_keep_separate_configuration_and_payload() {
    let input = include_bytes!("fixtures/fragmented-tone-pattern.mp4");
    let (tracks, samples) = read(input, input.len());
    let (_, split) = read(input, 1);
    assert_eq!(tracks.len(), 2);
    let video = tracks.iter().find(|t| t.width == 64).unwrap();
    let audio = tracks.iter().find(|t| t.channels == 1).unwrap();
    assert_eq!(samples.iter().filter(|s| s.track_id == video.id).count(), 4);
    assert_eq!(samples.iter().filter(|s| s.track_id == audio.id).count(), 25);
    assert_eq!(samples.len(), split.len());
    for (a, b) in samples.iter().zip(split) {
        assert_eq!((a.track_id, a.pts, &a.data), (b.track_id, b.pts, &b.data));
    }
}

#[test]
fn each_init_reports_its_own_duration_across_append_boundaries() {
    // The 48 kHz fixture has an empty movie header duration and no mehd.
    let init = &include_bytes!("fixtures/fragmented-tone.mp4")[..728];
    fn with_mehd(init: &[u8], version: u8, ticks: u64) -> Vec<u8> {
        let mut data = init.to_vec();
        let mut mehd = vec![0, 0, 0, 0, b'm', b'e', b'h', b'd', version, 0, 0, 0];
        if version == 0 {
            mehd.extend_from_slice(&(ticks as u32).to_be_bytes());
        } else {
            mehd.extend_from_slice(&ticks.to_be_bytes());
        }
        let added = mehd.len() as u32;
        mehd[..4].copy_from_slice(&added.to_be_bytes());
        let offset = |kind: &[u8]| data.windows(4).position(|x| x == kind).unwrap() - 4;
        let moov = offset(b"moov");
        let mvex = offset(b"mvex");
        for p in [moov, mvex] {
            let size = u32::from_be_bytes(data[p..p + 4].try_into().unwrap());
            data[p..p + 4].copy_from_slice(&(size + added).to_be_bytes());
        }
        data.splice(mvex + 8..mvex + 8, mehd);
        data
    }
    let a = with_mehd(init, 0, 48000);
    let b = with_mehd(init, 1, u64::from(u32::MAX));
    let c = with_mehd(init, 0, u64::from(u32::MAX));
    let d = with_mehd(init, 1, u64::MAX);
    let all = [a, b, c, d].concat();
    for chunk in [13, all.len()] {
        let mut parser = Mp4Parser::default();
        let mut durations = Vec::new();
        for part in all.chunks(chunk) {
            for event in parser.append(part).unwrap() {
                let Event::Init(_, duration) = event else { panic!("expected init") };
                durations.push(duration);
            }
        }
        assert_eq!(durations, [Some(1.0), Some(f64::from(u32::MAX) / 48000.0), None, None]);
    }
}
