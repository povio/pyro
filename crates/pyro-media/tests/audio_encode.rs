use pyro_media::audio::encode::{EncoderConfig, PacketEncoder};
use pyro_media::audio::packet::{PacketAudio, PacketDecoder};

fn config(rate: u32, channels: u32) -> EncoderConfig {
    EncoderConfig {
        codec: "opus".into(),
        sample_rate: rate,
        channels,
        bitrate: Some(64000 * channels),
        constant_bitrate: false,
        frame_duration: 20000,
        complexity: 5,
        packet_loss: 0,
        fec: false,
        dtx: false,
        signal: "music".into(),
        application: "audio".into(),
        format: if channels > 2 { "ogg" } else { "opus" }.into(),
    }
}

#[test]
fn opus_tones_round_trip_across_input_boundaries() {
    for (rate, channels) in [(48000, 1), (44100, 2), (48000, 6)] {
        let mut encoder = PacketEncoder::new(&config(rate, channels)).unwrap();
        let head = encoder.description().unwrap();
        assert_eq!(&head[..8], b"OpusHead");
        let skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        assert!(skip > 0 && skip < 960);
        let mut decoder = PacketDecoder::new("opus", rate, channels, Some(head)).unwrap();
        let frames = rate as usize / 5;
        let mut packets = Vec::new();
        for start in (0..frames).step_by(317) {
            let mut pcm = Vec::new();
            for frame in start..(start + 317).min(frames) {
                for ch in 0..channels {
                    let frequency = (440 + 110 * ch) as f32;
                    pcm.push(0.25 * (std::f32::consts::TAU * frequency * frame as f32 / rate as f32).sin());
                }
            }
            let ts = -5000 + start as i64 * 1000000 / i64::from(rate);
            packets.extend(encoder.encode(ts, rate, channels, &pcm).unwrap());
        }
        packets.extend(encoder.flush().unwrap());
        assert_eq!(packets.len(), 11);
        assert!(encoder.flush().unwrap().is_empty());
        let mut planes = vec![Vec::<f32>::new(); channels as usize];
        for (i, packet) in packets.into_iter().enumerate() {
            assert_eq!(packet.timestamp, -5000 + i as i64 * 20000);
            assert_eq!(packet.duration, 20000);
            let output = decoder.decode(packet.timestamp, packet.data).unwrap().unwrap();
            let PacketAudio::Planar(audio) = output.audio else { panic!("expected decoded Opus") };
            assert_eq!(audio.sample_rate, 48000);
            assert_eq!(audio.channels.len(), planes.len());
            for (dst, src) in planes.iter_mut().zip(audio.channels) {
                dst.extend(src);
            }
        }
        for (ch, plane) in planes.iter().enumerate() {
            assert_eq!(plane.len(), 11 * 960 - skip);
            let mid = &plane[2400..7200];
            let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
            assert!(rms > 0.04, "channel {ch}: {rms}");
            let cycles = mid.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count();
            assert!(cycles.abs_diff(44 + 11 * ch) <= 4, "channel {ch}: {cycles}");
        }
        let second = encoder.encode(900000, rate, channels, &vec![0.0; (rate * channels / 25) as usize]).unwrap();
        assert_eq!(second[0].timestamp, 900000);
    }
}

#[test]
fn opus_packet_durations_and_input_validation() {
    for duration in [7500, 30000, 120000] {
        let mut cfg = config(48000, 1);
        cfg.frame_duration = duration;
        let mut encoder = PacketEncoder::new(&cfg).unwrap();
        let mut decoder = PacketDecoder::new("opus", 48000, 1, None).unwrap();
        let frames = duration as usize * 48 / 1000;
        assert!(encoder.encode(0, 44100, 1, &[0.0]).is_err());
        let packets = encoder.encode(0, 48000, 1, &vec![0.1; frames]).unwrap();
        assert_eq!(packets.len(), 1);
        let output = decoder.decode(0, packets.into_iter().next().unwrap().data).unwrap().unwrap();
        let PacketAudio::Planar(audio) = output.audio else { panic!("expected decoded Opus") };
        assert_eq!(audio.channels[0].len(), frames);
    }
    assert!(!PacketEncoder::supports(&config(48000, 16)));
}
