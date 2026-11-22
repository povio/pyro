//! Elementary audio decoding with independent sample vectors and the real
//! compressed sine-wave fixtures also used by the whole-file decoder.
use std::io::Cursor;

use pyro_media::audio::packet::PacketDecoder;
use symphonia::core::formats::{probe::Hint, TrackType};
use symphonia::core::io::MediaSourceStream;

#[test]
fn pcm_packets_preserve_channel_order_and_signed_sample_values() {
    let mut decoder = PacketDecoder::new("pcm-s16", 44100, 2, None).unwrap();
    let bytes: Vec<_> = [i16::MIN, 16384, 0, -16384].iter().flat_map(|x| x.to_le_bytes()).collect();
    let out = decoder.decode(-1200, &bytes).unwrap().unwrap();
    assert_eq!(out.timestamp, -1200);
    assert_eq!(out.audio.sample_rate, 44100);
    assert_eq!(out.audio.channels, [vec![-1.0, 0.0], vec![0.5, -0.5]]);
    assert!(decoder.decode(0, &bytes[..3]).is_err());
    decoder.flush().unwrap();
    assert_eq!(decoder.decode(123, &bytes).unwrap().unwrap().audio.channels, out.audio.channels);

    for (codec, data, expected) in [
        ("pcm-u8", &b"\x00\x80"[..], vec![-1.0, 0.0]),
        ("pcm-s24", &b"\x00\x00\x80\x00\x00\x40"[..], vec![-1.0, 0.5]),
        ("pcm-s32", &b"\x00\x00\x00\x80\x00\x00\x00\x40"[..], vec![-1.0, 0.5]),
        ("pcm-f32", &b"\x00\x00\x80\xbf\x00\x00\x00\x3f"[..], vec![-1.0, 0.5]),
        ("ulaw", &b"\xff\x80\x00"[..], vec![0.0, 32124.0 / 32768.0, -32124.0 / 32768.0]),
        ("alaw", &b"\xd5\x55"[..], vec![8.0 / 32768.0, -8.0 / 32768.0]),
    ] {
        let mut decoder = PacketDecoder::new(codec, 8000, 1, None).unwrap();
        assert_eq!(decoder.decode(0, data).unwrap().unwrap().audio.channels[0], expected, "{codec}");
    }
}

#[test]
fn opus_header_priming_uses_the_decoder_rate_and_survives_flush() {
    let mut head = b"OpusHead\x01\x01\x38\x01\x80\xbb\x00\x00\x00\x00\x00".to_vec();
    // RFC 6716 20 ms silence packet. The 312-frame pre-skip shifts the
    // output timestamp by 6.5 ms, including at a nonnegative origin.
    for pts in [-42, 42] {
        let mut decoder = PacketDecoder::new("opus", 44100, 1, Some(&head)).unwrap();
        let out = decoder.decode(pts, &[0xf8, 0xff, 0xfe]).unwrap().unwrap();
        assert_eq!((out.timestamp, out.audio.sample_rate, out.audio.len()), (pts + 6500, 48000, 648));
        decoder.flush().unwrap();
        let out = decoder.decode(pts, &[0xf8, 0xff, 0xfe]).unwrap().unwrap();
        assert_eq!((out.timestamp, out.audio.len()), (pts, 960));
    }
    head[9] = 3;
    assert!(PacketDecoder::new("opus", 48000, 3, Some(&head)).is_err());
    assert!(PacketDecoder::new("opus", 48000, 3, None).is_err());
}

#[test]
fn codec_specific_configuration_and_packet_boundaries_are_checked() {
    assert!(!PacketDecoder::supports(" mp3 ", 48000, 1));
    assert!(!PacketDecoder::supports("mp4a.40.5", 48000, 2));
    assert!(PacketDecoder::new("flac", 48000, 1, None).is_err());
    assert!(PacketDecoder::new("vorbis", 48000, 1, None).is_err());
    let mut decoder = PacketDecoder::new("mp4a.40.2", 48000, 1, None).unwrap();
    assert!(decoder.decode(0, &[]).is_err());
}

#[test]
fn compressed_packets_decode_to_the_fixture_tone() {
    for (codec, file) in [
        ("mp3", "sine.mp3"),
        ("mp4a.40.2", "sine.m4a"),
        ("adts", "sine.m4a"),
        ("opus", "sine.opus"),
        ("vorbis", "sine.ogg"),
        ("flac", "sine.flac"),
    ] {
        let bytes = std::fs::read(format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.clone())), Default::default());
        let mut container = symphonia::default::get_probe()
            .probe(&Hint::new(), source, Default::default(), Default::default())
            .unwrap();
        let track = container.default_track(TrackType::Audio).unwrap();
        let id = track.id;
        let params = track.codec_params.as_ref().unwrap().audio().unwrap();
        let description = match codec {
            "mp3" | "adts" => None,
            "flac" => Some(bytes[..42].to_vec()),
            "opus" => {
                let start = bytes.windows(8).position(|m| m == b"OpusHead").unwrap();
                Some(bytes[start..start + 19].to_vec())
            }
            "vorbis" => {
                // Symphonia's Ogg demuxer joins the fixed 30-byte identification
                // header and the setup header. Package them around an empty
                // Vorbis comment header with the WebCodecs Xiph lengths.
                let headers = params.extra_data.as_deref().unwrap();
                let comment = b"\x03vorbis\x00\x00\x00\x00\x00\x00\x00\x00\x01";
                let mut data = vec![2, 30, comment.len() as u8];
                data.extend_from_slice(&headers[..30]);
                data.extend_from_slice(comment);
                data.extend_from_slice(&headers[30..]);
                Some(data)
            }
            _ => params.extra_data.as_deref().map(<[u8]>::to_vec),
        };
        // Compressed stream headers determine output rate and channel count.
        let public_codec = if codec == "adts" { "mp4a.40.2" } else { codec };
        let mut decoder = PacketDecoder::new(public_codec, 8000, 1, description.as_deref()).unwrap();
        let mut samples = Vec::new();
        let expected_rate = if codec == "opus" { 48000 } else { 44100 };
        while let Some(packet) = container.next_packet().unwrap() {
            if packet.track_id != id {
                continue;
            }
            let data = if codec == "adts" {
                // AAC LC, 44.1 kHz, mono, one raw data block, no CRC.
                let len = packet.data.len() + 7;
                let mut framed = vec![
                    0xff,
                    0xf1,
                    0x50,
                    0x40 | ((len >> 11) as u8),
                    (len >> 3) as u8,
                    ((len as u8 & 7) << 5) | 0x1f,
                    0xfc,
                ];
                framed.extend_from_slice(&packet.data);
                framed
            } else {
                packet.data.into_vec()
            };
            if let Some(out) = decoder.decode(0, &data).unwrap_or_else(|e| panic!("{codec}: {e}")) {
                assert_eq!(out.audio.sample_rate, expected_rate, "{codec}");
                assert_eq!(out.audio.channels.len(), if codec == "vorbis" { 2 } else { 1 }, "{codec}");
                samples.extend_from_slice(&out.audio.channels[0]);
            }
        }
        assert!(samples.len() >= 20000, "{codec}: {} samples", samples.len());
        let middle = &samples[samples.len() / 10..samples.len() * 9 / 10];
        let crossings = middle.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count();
        let frequency = crossings as f64 * f64::from(expected_rate) / middle.len() as f64;
        assert!((frequency - 440.0).abs() < 15.0, "{codec}: {frequency} Hz");
        let peak = middle.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!((0.08..0.2).contains(&peak), "{codec}: peak {peak}");
    }
}
