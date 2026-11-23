//! Decoding the committed fixtures (made by tests/fixtures/make.sh): half a
//! second of a 440 Hz sine in every audio format, and ten 64x48 frames at
//! 10 fps where frame N is rgb(N*25, 255-N*25, 128).
use bytes::Bytes;
use pyro_media::audio;
use pyro_media::video::{self, Player};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The frequency of a sine from its zero crossings.
fn frequency(samples: &[f32], rate: u32) -> f64 {
    let crossings = samples.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f64 * f64::from(rate) / samples.len() as f64
}

fn check_sine(name: &str, rate: u32, min_frames: usize, max_frames: usize) {
    let decoded = audio::decode(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(decoded.sample_rate, rate, "{name}");
    let len = decoded.len();
    assert!((min_frames..=max_frames).contains(&len), "{name}: {len} frames");
    let ch = &decoded.channels[0];
    // Skip the codec's ramp-up; the sine is 1/8 of full scale.
    let body = &ch[len / 10..len * 9 / 10];
    let f = frequency(body, rate);
    assert!((f - 440.0).abs() < 15.0, "{name}: {f} Hz");
    let peak = body.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!((0.08..0.2).contains(&peak), "{name}: peak {peak}");
}

#[test]
fn decodes_wav() {
    let d = audio::decode(&fixture("sine.wav")).unwrap();
    assert_eq!((d.sample_rate, d.channels.len(), d.len()), (44_100, 1, 22_050));
    check_sine("sine.wav", 44_100, 22_050, 22_050);
}

#[test]
fn decodes_mp3() {
    // LAME's gapless info trims the encoder delay and padding.
    check_sine("sine.mp3", 44_100, 22_050, 23_500);
}

#[test]
fn decodes_aac_m4a() {
    check_sine("sine.m4a", 44_100, 22_050, 24_576);
}

#[test]
fn decodes_ogg_vorbis_stereo() {
    let d = audio::decode(&fixture("sine.ogg")).unwrap();
    assert_eq!(d.channels.len(), 2);
    check_sine("sine.ogg", 44_100, 22_050, 22_100);
}

#[test]
fn decodes_ogg_opus() {
    check_sine("sine.opus", 48_000, 24_000, 24_000);
}

#[test]
fn decodes_webm_opus() {
    check_sine("sine.webm", 48_000, 24_000, 24_700);
}

#[test]
fn decodes_flac() {
    check_sine("sine.flac", 44_100, 22_050, 22_050);
}

#[test]
fn rejects_garbage() {
    assert!(audio::decode(b"<!doctype html><p>not audio").is_err());
    assert!(audio::decode(&[]).is_err());
}

#[test]
fn resamples_to_the_context_rate() {
    let d = audio::decode(&fixture("sine.wav")).unwrap();
    let up = audio::resample(&d.channels[0], 44_100, 48_000);
    assert_eq!(up.len(), 24_000);
    assert!((frequency(&up[2400..21600], 48_000) - 440.0).abs() < 5.0);
    let down = audio::resample(&d.channels[0], 44_100, 22_050);
    assert_eq!(down.len(), 11_025);
    assert!((frequency(&down[1000..10000], 22_050) - 440.0).abs() < 5.0);
}

/// The frame index a decoded pixel stands for.
fn frame_of(rgba: &[u8]) -> usize {
    let (r, g, b) = (f64::from(rgba[0]), f64::from(rgba[1]), f64::from(rgba[2]));
    assert!((b - 128.0).abs() < 16.0, "blue {b}");
    let n = (r / 25.0 + (255.0 - g) / 25.0) / 2.0;
    n.round() as usize
}

fn check_video(name: &str) {
    let mut player = Player::open(Bytes::from(fixture(name))).unwrap_or_else(|| panic!("{name}: no player"));
    assert_eq!(player.size(), (64, 48));
    assert_eq!(player.frame_count(), 10, "{name}");
    // In order (playback), then seeking back and around.
    for (t, want) in [(0.0, 0), (0.05, 0), (0.1, 1), (0.25, 2), (0.3, 3), (0.42, 4), (0.5, 5), (0.99, 9), (5.0, 9)] {
        let frame = player.frame_at(t).unwrap_or_else(|| panic!("{name}: no frame at {t}"));
        assert_eq!((frame.width, frame.height), (64, 48));
        assert_eq!(frame.index, want, "{name} at {t}");
        assert_eq!(frame_of(&frame.rgba[(24 * 64 + 32) * 4..]), want, "{name} pixel at {t}");
    }
    for (t, want) in [(0.2, 2), (0.75, 7), (0.0, 0), (0.6, 6), (0.35, 3)] {
        let frame = player.frame_at(t).unwrap();
        assert_eq!(frame.index, want, "{name} seeking to {t}");
        assert_eq!(frame_of(&frame.rgba[(24 * 64 + 32) * 4..]), want, "{name} pixel seeking to {t}");
    }
}

#[test]
#[cfg(feature = "av1")]
fn decodes_av1_mp4() {
    check_video("frames-av1.mp4");
}

#[test]
#[cfg(feature = "av1")]
fn decodes_av1_webm() {
    check_video("frames-av1.webm");
}

#[test]
#[cfg(feature = "h264")]
fn decodes_h264_mp4_with_b_frames() {
    check_video("frames-h264.mp4");
}

#[test]
fn reports_codecs_by_build() {
    let track = video::demux::video_track(&fixture("frames-h264.mp4")).unwrap();
    assert!(matches!(track.codec, video::Codec::H264 { length_size: 4, .. }));
    assert_eq!(video::decodes(&track.codec), video::H264);
    let track = video::demux::video_track(&fixture("frames-av1.webm")).unwrap();
    assert_eq!(track.codec, video::Codec::Av1);
    assert_eq!(track.samples.len(), 10);
    assert!(track.samples[0].key && track.samples[5].key && !track.samples[1].key);
}

#[test]
fn a_cut_body_lists_the_samples_it_holds() {
    let data = fixture("frames-av1.webm");
    let track = video::demux::video_track(&data[..data.len() * 3 / 4]).unwrap();
    assert!(!track.samples.is_empty() && track.samples.len() < 10, "{}", track.samples.len());
}

/// A version 0 `ctts` box with negative offsets (common: WPT `media/white.mp4`) orders frames as
/// a signed one does: read unsigned, the negative offsets put their frames past the end and no
/// frame after the first decoded (a black `<video>` in screenshots taken once it could play).
#[test]
#[cfg(feature = "h264")]
fn a_version_0_ctts_with_negative_offsets_is_signed() {
    let mut bytes = fixture("frames-h264.mp4");
    let at = bytes.windows(4).position(|w| w == b"ctts").unwrap();
    assert_eq!(bytes[at + 4], 0, "the fixture's ctts is version 0");
    let entries = u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
    // Every offset 2048 lower (order unchanged, some now negative), the version left at 0.
    for k in 0..entries {
        let o = at + 16 + 8 * k;
        let off = i32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) - 2048;
        bytes[o..o + 4].copy_from_slice(&off.to_be_bytes());
    }
    let mut shifted = Player::open(Bytes::from(bytes)).expect("a player");
    assert_eq!(shifted.frame_count(), 10);
    // Each frame, at its own presentation time, is the frame it was.
    for index in 0..10 {
        let t = shifted.frame_time(index);
        let frame = shifted.frame_at(t).unwrap_or_else(|| panic!("no frame {index} at {t}"));
        assert_eq!(frame.index, index, "frame at {t}");
        assert_eq!(frame_of(&frame.rgba[(24 * 64 + 32) * 4..]), index, "pixel of frame {index}");
    }
}

/// A complete ten-frame access-unit stream through the packet API. Tags
/// identify input packets, even when the backend emits them in another order.
fn check_packets(config: video::PacketConfig, samples: Vec<(Vec<u8>, usize)>, file: &[u8]) {
    let mut player = Player::open(Bytes::copy_from_slice(file)).unwrap();
    assert!(config.is_key(&samples[0].0));
    let mut decoder = config.decoder().unwrap();
    for _round in 0..2 {
        let mut pictures = Vec::new();
        for (i, (data, _)) in samples.iter().enumerate() {
            pictures.extend(decoder.decode(data, 100 + i as u64).unwrap());
        }
        pictures.extend(decoder.flush().unwrap());
        assert_eq!(pictures.len(), samples.len());
        for (display_index, picture) in pictures.iter().enumerate() {
            assert_eq!((picture.width, picture.height), (64, 48));
            assert_eq!(samples[(picture.tag - 100) as usize].1, display_index);
            // Player retains video-signal code values. Packet output normalizes
            // that RGB to sRGB, so compare in the target transfer domain.
            let origin = (24 * 64 + 32) * 4;
            let reference = player.frame_at(display_index as f64 / 10.).unwrap();
            assert_eq!(frame_of(&reference.rgba[origin..]), display_index);
            let pyro_media::video::PacketPixels::Rgba(rgba) = &picture.pixels else { panic!("expected RGBA") };
            for channel in 0..3 {
                let signal = f64::from(reference.rgba[origin + channel]) / 255.;
                let linear = if signal < 0.081 { signal / 4.5 } else { ((signal + 0.099) / 1.099).powf(1. / 0.45) };
                let srgb = if linear <= 0.0031308 { 12.92 * linear } else { 1.055 * linear.powf(1. / 2.4) - 0.055 };
                // Player already rounded to 8-bit; the packet conversion
                // applies the curve before that rounding.
                assert!(rgba[origin + channel].abs_diff((255. * srgb).round() as u8) <= 2);
            }
        }
        assert!(decoder.flush().unwrap().is_empty());
    }
}

#[cfg(feature = "av1")]
#[test]
fn packet_av1_keeps_sample_tags_and_restarts_after_flush() {
    let data = fixture("frames-av1.mp4");
    let track = video::demux::video_track(&data).unwrap();
    let config = video::PacketConfig::parse("av01.0.04M.08", None).unwrap();
    let samples = track
        .samples
        .iter()
        .map(|s| (data[s.offset..s.offset + s.size].to_vec(), (s.pts * 10.).round() as usize))
        .collect();
    check_packets(config, samples, &data);
}

#[cfg(feature = "h264")]
#[test]
fn packet_avc_and_annex_b_keep_sample_tags_and_restart_after_flush() {
    let data = fixture("frames-h264.mp4");
    let track = video::demux::video_track(&data).unwrap();
    let video::Codec::H264 { length_size, parameter_sets } = track.codec else { panic!("H.264 fixture") };
    let pos = data.windows(4).position(|p| p == b"avcC").unwrap();
    let size = u32::from_be_bytes(data[pos - 4..pos].try_into().unwrap()) as usize;
    let config = &data[pos + 4..pos - 4 + size];
    let codec = format!("avc1.{:02X}{:02X}{:02X}", config[1], config[2], config[3]);
    let mut avc = Vec::new();
    let mut annex = Vec::new();
    for s in track.samples {
        let mut sample = &data[s.offset..s.offset + s.size];
        let display = (s.pts * 10.).round() as usize;
        avc.push((sample.to_vec(), display));
        let mut unit = if s.key { parameter_sets.clone() } else { Vec::new() };
        while !sample.is_empty() {
            let (prefix, body) = sample.split_at(length_size);
            let len = prefix.iter().fold(0usize, |v, &b| (v << 8) | b as usize);
            unit.extend_from_slice(&[0, 0, 0, 1]);
            unit.extend_from_slice(&body[..len]);
            sample = &body[len..];
        }
        annex.push((unit, display));
    }
    check_packets(video::PacketConfig::parse(&codec, Some(config)).unwrap(), avc, &data);
    check_packets(video::PacketConfig::parse(&codec, None).unwrap(), annex, &data);
}

#[test]
#[cfg(feature = "av1")]
fn packet_av1_capability_matches_supported_sdr_colorimetry() {
    use video::PacketConfig;
    for codec in [
        "av01.0.04M.08",
        "av01.0.04M.08.0.110.01.01.01.0", // limited BT.709
        "av01.0.08M.10.0.110.01.13.01.1", // full-range sRGB
        "av01.0.04M.08.0.110.01.01.06.0", // BT.601 matrix, BT.709 primaries
    ] {
        assert!(PacketConfig::parse(codec, None).is_some(), "{codec}");
    }
    for codec in [
        "av01.0.08M.10.0.110.09.16.09.0", // BT.2020 / PQ
        "av01.0.08M.10.0.110.09.18.09.0", // BT.2020 / HLG
        "av01.0.08M.10.0.110.09.01.09.0", // wide-gamut SDR
        "av01.0.08M.10.0.110.01.08.01.0", // linear transfer
        "av01.0.08M.10.0.110.01.01.14.0", // ICtCp matrix
    ] {
        assert!(PacketConfig::parse(codec, None).is_none(), "{codec}");
    }
}
