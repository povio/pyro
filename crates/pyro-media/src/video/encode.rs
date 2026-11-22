//! Low-delay baseline AVC access units from packed, limited-range BT.709 I420.
//! One input produces one access unit. OpenH264 uses no B pictures or delayed
//! output in this configuration, so draining requires no extra codec call.

pub struct EncoderConfig {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: Option<u32>,
    pub framerate: Option<f64>,
    pub bitrate_mode: String,
    pub format: String,
}

pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub key: bool,
    pub timestamp: i64,
    pub duration: Option<u64>,
    pub description: Option<Vec<u8>>,
}

pub struct PacketEncoder {
    #[cfg(feature = "h264")]
    inner: avc::Encoder,
}

impl PacketEncoder {
    pub fn supports(config: &EncoderConfig) -> bool {
        #[cfg(feature = "h264")]
        {
            avc::settings(config).is_some()
        }
        #[cfg(not(feature = "h264"))]
        {
            let _ = config;
            false
        }
    }

    pub fn new(config: &EncoderConfig) -> Result<Self, String> {
        #[cfg(feature = "h264")]
        {
            Ok(Self { inner: avc::Encoder::new(config)? })
        }
        #[cfg(not(feature = "h264"))]
        {
            let _ = config;
            Err("AVC encoder unavailable".into())
        }
    }

    pub fn encode(
        &mut self,
        timestamp: i64,
        duration: Option<u64>,
        key: bool,
        i420: &[u8],
    ) -> Result<EncodedPacket, String> {
        #[cfg(feature = "h264")]
        {
            self.inner.encode(timestamp, duration, key, i420)
        }
        #[cfg(not(feature = "h264"))]
        {
            let _ = (timestamp, duration, key, i420);
            Err("AVC encoder unavailable".into())
        }
    }
}

#[cfg(feature = "h264")]
mod avc {
    use super::{EncodedPacket, EncoderConfig};
    use openh264::encoder::{
        BitRate, EncoderConfig as NativeConfig, FrameRate, Level, Profile, RateControlMode, VuiConfig,
    };
    use openh264::formats::YUVSlices;
    use openh264::{OpenH264API, Timestamp};

    pub(super) fn settings(c: &EncoderConfig) -> Option<(NativeConfig, u8)> {
        let hex = c.codec.strip_prefix("avc1.").or_else(|| c.codec.strip_prefix("avc3."))?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let profile = u32::from_str_radix(hex, 16).ok()?;
        // RFC 6184 Baseline and Constrained Baseline. OpenH264 emits a
        // constrained baseline stream (profile_idc 66, constraint flags C0).
        if profile >> 16 != 66 || !matches!((profile >> 8) & 255, 0 | 0x80 | 0x40 | 0xc0 | 0xe0) {
            return None;
        }
        let id = profile as u8;
        // Annex A MaxFS, MaxMBPS and baseline MaxBR (kbit/s).
        let (level, fs, mbps, kbps) = match id {
            10 => (Level::Level_1_0, 99, 1485, 64),
            11 => (Level::Level_1_1, 396, 3000, 192),
            12 => (Level::Level_1_2, 396, 6000, 384),
            13 => (Level::Level_1_3, 396, 11880, 768),
            20 => (Level::Level_2_0, 396, 11880, 2000),
            21 => (Level::Level_2_1, 792, 19800, 4000),
            22 => (Level::Level_2_2, 1620, 20250, 4000),
            30 => (Level::Level_3_0, 1620, 40500, 10000),
            31 => (Level::Level_3_1, 3600, 108000, 14000),
            32 => (Level::Level_3_2, 5120, 216000, 20000),
            40 => (Level::Level_4_0, 8192, 245760, 20000),
            41 => (Level::Level_4_1, 8192, 245760, 50000),
            42 => (Level::Level_4_2, 8704, 522240, 50000),
            50 => (Level::Level_5_0, 22080, 589824, 135000),
            51 => (Level::Level_5_1, 36864, 983040, 240000),
            52 => (Level::Level_5_2, 36864, 2073600, 240000),
            _ => return None,
        };
        if c.width.min(c.height) < 16
            || c.width.max(c.height) > 3840
            || c.width.min(c.height) > 2160
            || c.width % 2 != 0
            || c.height % 2 != 0
            || c.bitrate_mode != "variable"
            || !matches!(c.format.as_str(), "avc" | "annexb")
        {
            return None;
        }
        let x = c.width.div_ceil(16);
        let y = c.height.div_ceil(16);
        let fps = c.framerate.unwrap_or(30.0);
        let bitrate = c.bitrate.unwrap_or((c.width * c.height * 3).clamp(64000, kbps * 1000));
        if x * y > fs
            || x.max(y).pow(2) > 8 * fs
            || !(1.0..=60.0).contains(&fps)
            || f64::from(x * y) * fps > f64::from(mbps)
            || !(1000..=kbps * 1000).contains(&bitrate)
        {
            return None;
        }
        Some((
            NativeConfig::new()
                .profile(Profile::Baseline)
                .level(level)
                .bitrate(BitRate::from_bps(bitrate))
                .max_frame_rate(FrameRate::from_hz(fps as f32))
                .rate_control_mode(RateControlMode::Bitrate)
                .vui(VuiConfig::bt709())
                .skip_frames(false)
                .num_threads(1),
            id,
        ))
    }

    pub(super) struct Encoder {
        codec: openh264::encoder::Encoder,
        width: usize,
        height: usize,
        annex_b: bool,
        level: u8,
        first_timestamp: Option<i64>,
    }

    impl Encoder {
        pub(super) fn new(c: &EncoderConfig) -> Result<Self, String> {
            let (settings, level) = settings(c).ok_or("unsupported AVC configuration")?;
            let codec = openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), settings)
                .map_err(|e| e.to_string())?;
            Ok(Self {
                codec,
                width: c.width as usize,
                height: c.height as usize,
                annex_b: c.format == "annexb",
                level,
                first_timestamp: None,
            })
        }

        pub(super) fn encode(
            &mut self,
            timestamp: i64,
            duration: Option<u64>,
            key: bool,
            i420: &[u8],
        ) -> Result<EncodedPacket, String> {
            let pixels = self.width * self.height;
            if i420.len() != pixels + pixels / 2 {
                return Err("expected packed I420 matching encoder dimensions".into());
            }
            let (y, uv) = i420.split_at(pixels);
            let (u, v) = uv.split_at(pixels / 4);
            let source =
                YUVSlices::new((y, u, v), (self.width, self.height), (self.width, self.width / 2, self.width / 2));
            if key && self.first_timestamp.is_some() {
                self.codec.force_intra_frame();
            }
            let origin = *self.first_timestamp.get_or_insert(timestamp);
            let millis = (i128::from(timestamp) - i128::from(origin)).max(0) as u64 / 1000;
            let stream = self.codec.encode_at(&source, Timestamp::from_millis(millis)).map_err(|e| e.to_string())?;
            let mut data = Vec::new();
            let mut sps = None;
            let mut pps = None;
            let mut idr = false;
            let mut picture = false;
            for l in 0..stream.num_layers() {
                let layer = stream.layer(l).ok_or("missing AVC layer")?;
                for n in 0..layer.nal_count() {
                    let unit = layer.nal_unit(n).ok_or("missing AVC NAL")?;
                    let nal = unit
                        .strip_prefix(&[0, 0, 0, 1])
                        .or_else(|| unit.strip_prefix(&[0, 0, 1]))
                        .ok_or("missing NAL prefix")?;
                    match nal.first().ok_or("empty AVC NAL")? & 31 {
                        7 => {
                            if nal.len() < 4 || nal[1] != 66 || nal[2] & 0xc0 != 0xc0 || nal[3] != self.level {
                                return Err("unexpected AVC profile or level".into());
                            }
                            sps = Some(nal.to_vec());
                        }
                        8 => pps = Some(nal.to_vec()),
                        5 => {
                            idr = true;
                            picture = true;
                        }
                        1 => picture = true,
                        _ => {}
                    }
                    if self.annex_b {
                        data.extend_from_slice(unit);
                    } else if !matches!(nal[0] & 31, 7 | 8) {
                        data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        data.extend_from_slice(nal);
                    }
                }
            }
            if !picture || (key && !idr) {
                return Err("encoder did not emit requested picture".into());
            }
            let description = if idr && !self.annex_b {
                let sps = sps.ok_or("missing AVC SPS")?;
                let pps = pps.ok_or("missing AVC PPS")?;
                let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
                avcc.extend_from_slice(&u16::try_from(sps.len()).map_err(|_| "oversized SPS")?.to_be_bytes());
                avcc.extend_from_slice(&sps);
                avcc.push(1);
                avcc.extend_from_slice(&u16::try_from(pps.len()).map_err(|_| "oversized PPS")?.to_be_bytes());
                avcc.extend_from_slice(&pps);
                Some(avcc)
            } else {
                None
            };
            Ok(EncodedPacket { data, key: idr, timestamp, duration, description })
        }
    }
}
