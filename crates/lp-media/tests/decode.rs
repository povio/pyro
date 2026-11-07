//! Decoding the committed fixtures (made by tests/fixtures/make.sh): half a
//! second of a 440 Hz sine in every audio format, and ten 64x48 frames at
//! 10 fps where frame N is rgb(N*25, 255-N*25, 128).
use bytes::Bytes;
use lp_media::audio;
use lp_media::video::{self, Player};

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
