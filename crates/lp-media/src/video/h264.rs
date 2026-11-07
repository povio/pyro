//! H.264 through OpenH264 (Cisco's decoder, built from source by
//! `openh264-sys2`): length-prefixed samples become Annex B access units,
//! each tagged with its sample through the decoder's input timestamp, which
//! comes back on the picture (OpenH264 reorders B-frames itself).
use std::ptr::null_mut;

use openh264::decoder::Decoder as OpenH264;
use openh264_sys2::{
    videoFormatI420, SBufferInfo, DECODER_OPTION, DECODER_OPTION_END_OF_STREAM,
    DECODER_OPTION_NUM_OF_FRAMES_REMAINING_IN_BUFFER,
};

use super::yuv::{self, Matrix, Picture, Plane};
use super::{Decoded, Decoder};

pub(super) struct H264 {
    decoder: OpenH264,
    length_size: usize,
    parameter_sets: Vec<u8>,
    /// Annex B scratch.
    unit: Vec<u8>,
}

impl H264 {
    pub(super) fn new(length_size: usize, parameter_sets: Vec<u8>) -> Option<H264> {
        let decoder = OpenH264::new().ok()?;
        Some(H264 { decoder, length_size, parameter_sets, unit: Vec::new() })
    }

    fn take(
        &mut self,
        info: &SBufferInfo,
        dst: &[*mut u8; 3],
        want: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<Decoded>,
    ) {
        if info.iBufferStatus != 1 || dst.iter().any(|p| p.is_null()) {
            return;
        }
        let tag = info.uiOutYuvTimeStamp;
        let image = if want(tag) { convert(info, dst) } else { None };
        out.push(Decoded { tag, image });
    }

    fn remaining(&mut self) -> usize {
        let mut n: DECODER_OPTION = 0;
        // SAFETY: the option is an int the decoder writes.
        unsafe {
            self.decoder.raw_api().get_option(DECODER_OPTION_NUM_OF_FRAMES_REMAINING_IN_BUFFER, (&raw mut n).cast());
        }
        usize::try_from(n).unwrap_or(0)
    }
}

fn convert(info: &SBufferInfo, dst: &[*mut u8; 3]) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY: a ready buffer describes its system-memory planes.
    let buf = unsafe { info.UsrData.sSystemBuffer };
    if buf.iFormat != videoFormatI420 {
        return None;
    }
    let (w, h) = (usize::try_from(buf.iWidth).ok()?, usize::try_from(buf.iHeight).ok()?);
    let (ys, cs) = (usize::try_from(buf.iStride[0]).ok()?, usize::try_from(buf.iStride[1]).ok()?);
    let ch = h.div_ceil(2);
    // SAFETY: OpenH264's planes hold `stride * rows` bytes until the next call.
    let plane = |p: *mut u8, stride: usize, rows: usize| Plane {
        data: unsafe { std::slice::from_raw_parts(p as *const u8, stride * rows) },
        stride,
    };
    let picture = Picture {
        width: w,
        height: h,
        y: plane(dst[0], ys, h),
        uv: Some((plane(dst[1], cs, ch), plane(dst[2], cs, ch))),
        ss: (1, 1),
        depth: 8,
        full_range: false,
        matrix: Matrix::untagged(h as u32),
    };
    Some((w as u32, h as u32, yuv::to_rgba(&picture)))
}

impl Decoder for H264 {
    fn feed(
        &mut self,
        sample: &[u8],
        tag: u64,
        want: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<Decoded>,
    ) -> Result<(), String> {
        self.unit.clear();
        // A key frame carries the parameter sets, so decoding can start at
        // any of them.
        let mut at = 0;
        let mut idr = false;
        let mut nals = Vec::new();
        while at + self.length_size <= sample.len() {
            let len = sample[at..at + self.length_size].iter().fold(0usize, |v, &b| (v << 8) | usize::from(b));
            at += self.length_size;
            let Some(nal) = sample.get(at..at + len) else { break };
            idr |= nal.first().is_some_and(|b| b & 0x1f == 5);
            nals.push(nal);
            at += len;
        }
        if idr {
            self.unit.extend_from_slice(&self.parameter_sets);
        }
        for nal in nals {
            self.unit.extend_from_slice(&[0, 0, 0, 1]);
            self.unit.extend_from_slice(nal);
        }
        let mut dst = [null_mut::<u8>(); 3];
        let mut info = SBufferInfo { uiInBsTimeStamp: tag, ..Default::default() };
        let unit = std::mem::take(&mut self.unit);
        // SAFETY: the decoder reads `unit` and writes plane pointers and info.
        let state = unsafe {
            self.decoder.raw_api().decode_frame_no_delay(unit.as_ptr(), unit.len() as i32, dst.as_mut_ptr(), &mut info)
        };
        self.unit = unit;
        // Errors are concealed frames; the picture, if any, still counts.
        let _ = state;
        self.take(&info, &dst, want, out);
        Ok(())
    }

    fn finish(&mut self, want: &mut dyn FnMut(u64) -> bool, out: &mut Vec<Decoded>) -> Result<(), String> {
        let mut end: DECODER_OPTION = 1;
        // SAFETY: the option is an int the decoder reads.
        unsafe {
            self.decoder.raw_api().set_option(DECODER_OPTION_END_OF_STREAM, (&raw mut end).cast());
        }
        for _ in 0..self.remaining().max(1) {
            let mut dst = [null_mut::<u8>(); 3];
            let mut info = SBufferInfo::default();
            // SAFETY: as in `feed`.
            unsafe { self.decoder.raw_api().flush_frame(dst.as_mut_ptr(), &mut info) };
            self.take(&info, &dst, want, out);
        }
        Ok(())
    }
}
