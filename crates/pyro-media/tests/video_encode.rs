#![cfg(feature = "h264")]

use pyro_media::video::{
    PacketConfig,
    encode::{EncoderConfig, PacketEncoder},
};

fn config(format: &str) -> EncoderConfig {
    EncoderConfig {
        codec: "avc1.42001e".into(),
        width: 64,
        height: 48,
        bitrate: Some(200000),
        framerate: Some(30.0),
        bitrate_mode: "variable".into(),
        format: format.into(),
    }
}

#[test]
fn avc_packets_decode_to_input_pattern_and_honor_key_requests() {
    for (codec, profile, flags) in [("avc1.42001e", 66, 0xc0), ("avc1.4d001e", 77, 0x4c), ("avc1.64001e", 100, 0x0c)] {
        for format in ["avc", "annexb"] {
            let mut c = config(format);
            c.codec = codec.into();
            let mut encoder = PacketEncoder::new(&c).unwrap();
            let mut pcm = vec![128; 64 * 48 * 3 / 2];
            for row in pcm[..64 * 48].chunks_exact_mut(64) {
                row[..32].fill(48);
                row[32..].fill(200);
            }
            let first = encoder.encode(-4000, Some(33333), false, &pcm).unwrap();
            assert!(first.key);
            assert_eq!((first.timestamp, first.duration), (-4000, Some(33333)));
            if format == "avc" {
                let avcc = first.description.as_ref().unwrap();
                assert_eq!(&avcc[..6], &[1, profile, flags, 30, 255, 225]);
                assert_eq!(&avcc[8..12], &[0x67, profile, flags, 30]);
            } else {
                assert!(first.description.is_none());
            }
            let stream = PacketConfig::parse(&c.codec, first.description.as_deref()).unwrap();
            assert!(stream.is_key(&first.data));
            let mut decoder = stream.decoder().unwrap();
            let mut frames = decoder.decode(&first.data, 1).unwrap();
            let second = encoder.encode(29333, None, false, &pcm).unwrap();
            assert!(!second.key);
            frames.extend(decoder.decode(&second.data, 2).unwrap());
            let third = encoder.encode(62666, None, true, &pcm).unwrap();
            assert!(third.key && stream.is_key(&third.data));
            frames.extend(decoder.decode(&third.data, 3).unwrap());
            frames.extend(decoder.flush().unwrap());
            assert_eq!(frames.len(), 3);
            for frame in frames {
                assert_eq!((frame.width, frame.height), (64, 48));
                assert!(frame.rgba[(24 * 64 + 16) * 4] < 70);
                assert!(frame.rgba[(24 * 64 + 48) * 4] > 180);
            }
        }
    }
}

#[test]
fn rejects_unimplemented_modes_and_out_of_level_picture_rates() {
    let mut c = config("avc");
    c.codec = "avc1.42001e".into();
    c.width = 1280;
    c.height = 720;
    assert!(!PacketEncoder::supports(&c));
    c.codec = "avc1.42c01f".into();
    assert!(PacketEncoder::supports(&c));
    c.framerate = Some(60.0);
    assert!(!PacketEncoder::supports(&c));
    c.framerate = Some(30.0);
    c.bitrate_mode = "constant".into();
    assert!(!PacketEncoder::supports(&c));
}

#[test]
fn profile_level_bitrate_and_constraints() {
    let mut c = config("avc");
    for (codec, rate, supported) in [
        ("avc1.4d401e", 10000000, true),
        ("avc1.4d401e", 10000001, false),
        ("avc1.640c1e", 12000000, true),
        ("avc1.64001e", 12000001, false),
        ("avc1.64801e", 200000, false),
        ("avc1.4d801e", 200000, false),
        ("avc1.64003e", 200000, false),
        ("avc1.6e001e", 200000, false),
    ] {
        c.codec = codec.into();
        c.bitrate = Some(rate);
        assert_eq!(PacketEncoder::supports(&c), supported, "{codec} at {rate}");
    }
}
