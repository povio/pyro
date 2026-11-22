//! Elementary audio decoding with independent sample vectors and the real
//! compressed sine-wave fixtures also used by the whole-file decoder.
use std::io::Cursor;

use bytes::Bytes;
use pyro_media::audio::packet::{PacketAudio, PacketDecoder, PacketOutput, PcmFormat};
use pyro_media::audio::Decoded;
use symphonia::core::formats::{probe::Hint, TrackType};
use symphonia::core::io::MediaSourceStream;

fn planes(output: PacketOutput) -> Decoded {
    match output.audio {
        PacketAudio::Planar(audio) => audio,
        PacketAudio::Pcm { .. } => panic!("expected decoded float planes"),
    }
}

#[test]
fn pcm_packets_preserve_formats_integer_precision_and_channel_order() {
    let s16 = [i16::MIN, -1, 0, 1, 8192, i16::MAX];
    let s32 = [i32::MIN, -16777217, -1, 0, 16777217, i32::MAX];
    // Preserve the exact f32 bit pattern while transposing stereo L/R into two planes.
    let f32_bits = [0x80000000u32, 0x40000000, 0x7fc12345, 0xbf800000];
    for (codec, format, channels, raw, expected) in [
        ("pcm-u8", PcmFormat::U8, 1, vec![0, 1, 127, 128, 254, 255], vec![0, 1, 127, 128, 254, 255]),
        (
            "pcm-s16",
            PcmFormat::S16,
            2,
            s16.iter().flat_map(|v| v.to_le_bytes()).collect(),
            s16.iter().flat_map(|v| v.to_ne_bytes()).collect(),
        ),
        (
            "pcm-s24",
            PcmFormat::S32,
            1,
            vec![0, 0, 128, 255, 255, 255, 1, 0, 0, 255, 255, 127],
            [i32::MIN, -256, 256, 0x7fffff00].iter().flat_map(|v| v.to_ne_bytes()).collect(),
        ),
        (
            "pcm-s32",
            PcmFormat::S32,
            2,
            s32.iter().flat_map(|v| v.to_le_bytes()).collect(),
            s32.iter().flat_map(|v| v.to_ne_bytes()).collect(),
        ),
        (
            "pcm-f32",
            PcmFormat::F32Planar,
            2,
            f32_bits.iter().flat_map(|v| v.to_le_bytes()).collect(),
            [f32_bits[0], f32_bits[2], f32_bits[1], f32_bits[3]].iter().flat_map(|v| v.to_ne_bytes()).collect(),
        ),
    ] {
        let frames_per_packet = match codec {
            "pcm-u8" => 6,
            "pcm-s16" | "pcm-s32" => 3,
            "pcm-s24" => 4,
            _ => 2,
        };
        let mut decoder = PacketDecoder::new(codec, 44100, channels, None).unwrap();
        let raw = Bytes::from(raw);
        for timestamp in [-1200, 123] {
            let output = decoder.decode(timestamp, raw.clone()).unwrap().unwrap();
            assert_eq!(output.timestamp, timestamp);
            let PacketAudio::Pcm { format: actual, sample_rate, channels: count, frames, data } = output.audio else {
                panic!("expected typed PCM for {codec}");
            };
            assert_eq!((actual, sample_rate, count, frames), (format, 44100, channels, frames_per_packet), "{codec}");
            assert_eq!(data.as_ref(), &expected[..], "{codec}");
            if cfg!(target_endian = "little") && matches!(codec, "pcm-u8" | "pcm-s16" | "pcm-s32") {
                assert_eq!(data.as_ptr(), raw.as_ptr(), "{codec} should share the chunk’s bytes");
            }
            decoder.flush().unwrap();
        }
        if channels > 1 {
            assert!(decoder.decode(0, raw.slice(1..)).is_err());
        }
    }
}

#[test]
fn g711_packets_expand_to_linear_samples() {
    for (codec, data, expected) in [
        ("ulaw", &b"\xff\x80\x00"[..], vec![0.0, 32124.0 / 32768.0, -32124.0 / 32768.0]),
        ("alaw", &b"\xd5\x55"[..], vec![8.0 / 32768.0, -8.0 / 32768.0]),
    ] {
        let mut decoder = PacketDecoder::new(codec, 8000, 1, None).unwrap();
        let out = planes(decoder.decode(0, Bytes::from_static(data)).unwrap().unwrap());
        assert_eq!(out.channels[0], expected, "{codec}");
    }
}

#[test]
fn opus_header_priming_uses_the_decoder_rate_and_survives_flush() {
    let mut head = b"OpusHead\x01\x01\x38\x01\x80\xbb\x00\x00\x00\x00\x00".to_vec();
    // RFC 6716 20 ms silence packet. The 312-frame pre-skip shifts the
    // output timestamp by 6.5 ms, including at a nonnegative origin.
    for pts in [-42, 42] {
        let mut decoder = PacketDecoder::new("opus", 44100, 1, Some(&head)).unwrap();
        let out = decoder.decode(pts, Bytes::from_static(&[0xf8, 0xff, 0xfe])).unwrap().unwrap();
        assert_eq!(out.timestamp, pts + 6500);
        let audio = planes(out);
        assert_eq!((audio.sample_rate, audio.len()), (48000, 648));
        decoder.flush().unwrap();
        let out = decoder.decode(pts, Bytes::from_static(&[0xf8, 0xff, 0xfe])).unwrap().unwrap();
        assert_eq!(out.timestamp, pts);
        assert_eq!(planes(out).len(), 960);
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
    assert!(decoder.decode(0, Bytes::new()).is_err());
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
            if let Some(out) = decoder.decode(0, Bytes::from(data)).unwrap_or_else(|e| panic!("{codec}: {e}")) {
                let audio = planes(out);
                assert_eq!(audio.sample_rate, expected_rate, "{codec}");
                assert_eq!(audio.channels.len(), if codec == "vorbis" { 2 } else { 1 }, "{codec}");
                samples.extend_from_slice(&audio.channels[0]);
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
