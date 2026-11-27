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
    /// Temporal layers of the L1T<n> scalability mode (0 or 1: a single layer, at most 3).
    pub temporal_layers: u8,
}

pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub key: bool,
    pub timestamp: i64,
    pub duration: Option<u64>,
    pub description: Option<Vec<u8>>,
    pub temporal_layer: u8,
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

    pub(super) fn settings(c: &EncoderConfig) -> Option<(NativeConfig, [u8; 3])> {
        let hex = c.codec.strip_prefix("avc1.").or_else(|| c.codec.strip_prefix("avc3."))?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let profile = u32::from_str_radix(hex, 16).ok()?;
        let profile_id = (profile >> 16) as u8;
        let flags = (profile >> 8) as u8;
        // WelsWriteSpsSyntax signals progressive pictures and no B slices
        // for Main and High (constraint_set4/5). Any subset of those actual
        // constraints is compatible with the requested profile.
        let (native_profile, output_flags) = match profile_id {
            66 if matches!(flags, 0 | 0x80 | 0x40 | 0xc0 | 0xe0) => (Profile::Baseline, 0xc0),
            77 if flags & !0x4c == 0 => (Profile::Main, 0x4c),
            100 if flags & !0x0c == 0 => (Profile::High, 0x0c),
            _ => return None,
        };
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
            // OpenH264's bitrate mode is constant-bitrate rate control; "variable" keeps the
            // same controller with a rate that may stray from the target.
            || !matches!(c.bitrate_mode.as_str(), "variable" | "constant")
            || c.temporal_layers > 3
            || !matches!(c.format.as_str(), "avc" | "annexb")
        {
            return None;
        }
        let x = c.width.div_ceil(16);
        let y = c.height.div_ceil(16);
        let fps = c.framerate.unwrap_or(30.0);
        // Annex A's High VCL factor is 1250 (Baseline/Main: 1000).
        // OpenH264's WelsCheckLevelLimitation applies 1200 to every profile
        // and would raise the level for the remaining High bitrate interval.
        let max_bitrate = kbps * if profile_id == 100 { 1200 } else { 1000 };
        let bitrate = c.bitrate.unwrap_or((c.width * c.height * 3).clamp(64000, max_bitrate));
        if x * y > fs
            || x.max(y).pow(2) > 8 * fs
            || !(1.0..=60.0).contains(&fps)
            || f64::from(x * y) * fps > f64::from(mbps)
            || !(1000..=max_bitrate).contains(&bitrate)
        {
            return None;
        }
        Some((
            NativeConfig::new()
                .profile(native_profile)
                .level(level)
                .bitrate(BitRate::from_bps(bitrate))
                .max_frame_rate(FrameRate::from_hz(fps as f32))
                .rate_control_mode(RateControlMode::Bitrate)
                .vui(VuiConfig::bt709())
                .skip_frames(false)
                .num_threads(1),
            [profile_id, output_flags, id],
        ))
    }

    pub(super) struct Encoder {
        codec: openh264::encoder::Encoder,
        width: usize,
        height: usize,
        annex_b: bool,
        profile_level: [u8; 3],
        first_timestamp: Option<i64>,
    }

    impl Encoder {
        pub(super) fn new(c: &EncoderConfig) -> Result<Self, String> {
            let (settings, profile_level) = settings(c).ok_or("unsupported AVC configuration")?;
            let mut codec = openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), settings)
                .map_err(|e| e.to_string())?;
            if c.temporal_layers > 1 {
                // The safe wrapper initialises with one temporal layer on the first picture.
                // Initialise now with a throwaway grey picture, raise iTemporalLayerNum
                // through the extended parameters and restart at an IDR picture.
                let pixels = (c.width * c.height) as usize;
                let grey = vec![128u8; pixels + pixels / 2];
                let (y, uv) = grey.split_at(pixels);
                let (u, v) = uv.split_at(pixels / 4);
                let (w, h) = (c.width as usize, c.height as usize);
                let source = YUVSlices::new((y, u, v), (w, h), (w, w / 2, w / 2));
                codec.encode_at(&source, Timestamp::ZERO).map_err(|e| e.to_string())?;
                let mut params = openh264_sys2::SEncParamExt::default();
                // SAFETY: the encoder is initialised and the parameter struct is the type
                // ENCODER_OPTION_SVC_ENCODE_PARAM_EXT reads and writes.
                unsafe {
                    let api = codec.raw_api();
                    let get = api.get_option(
                        openh264_sys2::ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        std::ptr::addr_of_mut!(params).cast(),
                    );
                    if get != 0 {
                        return Err("AVC encoder rejected temporal layer query".into());
                    }
                    params.iTemporalLayerNum = i32::from(c.temporal_layers);
                    let set = api.set_option(
                        openh264_sys2::ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        std::ptr::addr_of_mut!(params).cast(),
                    );
                    if set != 0 {
                        return Err("AVC encoder rejected temporal layers".into());
                    }
                }
                codec.force_intra_frame();
            }
            Ok(Self {
                codec,
                width: c.width as usize,
                height: c.height as usize,
                annex_b: c.format == "annexb",
                profile_level,
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
            let mut temporal_layer = 0;
            for l in 0..stream.num_layers() {
                let layer = stream.layer(l).ok_or("missing AVC layer")?;
                temporal_layer = temporal_layer.max(layer.raw_info().uiTemporalId);
                for n in 0..layer.nal_count() {
                    let unit = layer.nal_unit(n).ok_or("missing AVC NAL")?;
                    let nal = unit
                        .strip_prefix(&[0, 0, 0, 1])
                        .or_else(|| unit.strip_prefix(&[0, 0, 1]))
                        .ok_or("missing NAL prefix")?;
                    match nal.first().ok_or("empty AVC NAL")? & 31 {
                        7 => {
                            if nal.len() < 4
                                || nal[1] != self.profile_level[0]
                                || nal[2] & self.profile_level[1] != self.profile_level[1]
                                || nal[3] > self.profile_level[2]
                            {
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
                if sps[1] == 100 {
                    // ISO/IEC 14496-15 High-profile avcC extension. This
                    // encoder writes 4:2:0, 8-bit luma/chroma and no SPS ext.
                    avcc.extend_from_slice(&[0xfd, 0xf8, 0xf8, 0]);
                }
                Some(avcc)
            } else {
                None
            };
            Ok(EncodedPacket { data, key: idr, timestamp, duration, description, temporal_layer })
        }
    }
}
