//! AV1 through rav1d (the Rust port of dav1d) and its dav1d C API, one
//! thread and one frame of delay: a picture comes out for each temporal
//! unit sent, tagged with the unit's timestamp.
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT2020_NCL, DAV1D_MC_BT470BG, DAV1D_MC_BT601, DAV1D_MC_BT709, DAV1D_MC_UNKNOWN,
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};

use super::yuv::{self, Matrix, Picture, Plane};
use super::{Decoded, Decoder};

pub(super) struct Av1 {
    ctx: Option<Dav1dContext>,
}

fn eagain() -> i32 {
    -libc::EAGAIN
}

impl Av1 {
    pub(super) fn new() -> Option<Av1> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: dav1d_default_settings initializes every field of the
        // settings; dav1d_open reads them and writes the context out.
        unsafe {
            dav1d_default_settings(NonNull::new(settings.as_mut_ptr())?);
            let mut settings = settings.assume_init();
            settings.n_threads = 1;
            settings.max_frame_delay = 1;
            if dav1d_open(NonNull::new(&mut ctx), NonNull::new(&mut settings)).0 != 0 {
                return None;
            }
        }
        ctx.is_some().then_some(Av1 { ctx })
    }

    /// Takes every picture ready now.
    fn drain(&mut self, want: &mut dyn FnMut(u64) -> bool, out: &mut Vec<Decoded>) -> Result<(), String> {
        loop {
            let mut pic = MaybeUninit::<Dav1dPicture>::zeroed();
            // SAFETY: the context is open; the picture is written by
            // dav1d_get_picture and released with dav1d_picture_unref.
            let r = unsafe { dav1d_get_picture(self.ctx.clone(), NonNull::new(pic.as_mut_ptr())) }.0;
            if r == eagain() {
                return Ok(());
            }
            if r < 0 {
                return Err(format!("AV1 decode error {r}"));
            }
            // SAFETY: a successful dav1d_get_picture filled the picture.
            let mut pic = unsafe { pic.assume_init() };
            let tag = pic.m.timestamp as u64;
            let image = if want(tag) { convert(&pic) } else { None };
            // SAFETY: the picture came from dav1d_get_picture.
            unsafe { dav1d_picture_unref(NonNull::new(&mut pic)) };
            out.push(Decoded { tag, image });
        }
    }
}

fn convert(pic: &Dav1dPicture) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (usize::try_from(pic.p.w).ok()?, usize::try_from(pic.p.h).ok()?);
    let depth = u32::try_from(pic.p.bpc).ok()?;
    let (ss, mono): ((u32, u32), bool) = match pic.p.layout {
        DAV1D_PIXEL_LAYOUT_I400 => ((1, 1), true),
        DAV1D_PIXEL_LAYOUT_I420 => ((1, 1), false),
        DAV1D_PIXEL_LAYOUT_I422 => ((1, 0), false),
        _ => ((0, 0), false),
    };
    let (ys, cs) = (usize::try_from(pic.stride[0]).ok()?, usize::try_from(pic.stride[1]).ok()?);
    // SAFETY: the sequence header lives as long as the picture.
    let seq = pic.seq_hdr.map(|s| unsafe { s.as_ref() });
    let full_range = seq.is_some_and(|s| s.color_range != 0);
    let matrix = match seq.map(|s| s.mtrx) {
        Some(DAV1D_MC_BT709) => Matrix::Bt709,
        Some(DAV1D_MC_BT2020_NCL) => Matrix::Bt2020,
        Some(DAV1D_MC_BT601 | DAV1D_MC_BT470BG) => Matrix::Bt601,
        Some(DAV1D_MC_UNKNOWN) | None => Matrix::untagged(h as u32),
        _ => Matrix::Bt709,
    };
    let ch = (h + ss.1 as usize) >> ss.1;
    // SAFETY: dav1d's planes hold `stride * rows` bytes each.
    let plane = |i: usize, stride: usize, rows: usize| -> Option<Plane<'_>> {
        let ptr = pic.data[i]?;
        Some(Plane { data: unsafe { std::slice::from_raw_parts(ptr.as_ptr() as *const u8, stride * rows) }, stride })
    };
    let y = plane(0, ys, h)?;
    let uv = if mono { None } else { Some((plane(1, cs, ch)?, plane(2, cs, ch)?)) };
    let rgba = yuv::to_rgba(&Picture { width: w, height: h, y, uv, ss, depth, full_range, matrix });
    Some((w as u32, h as u32, rgba))
}

impl Decoder for Av1 {
    fn feed(
        &mut self,
        sample: &[u8],
        tag: u64,
        want: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<Decoded>,
    ) -> Result<(), String> {
        if sample.is_empty() {
            return Ok(());
        }
        let mut data = MaybeUninit::<Dav1dData>::zeroed();
        // SAFETY: dav1d_data_create allocates `len` bytes owned by `data`,
        // filled here before it is sent; dav1d_data_unref frees what is left.
        unsafe {
            let ptr = dav1d_data_create(NonNull::new(data.as_mut_ptr()), sample.len());
            if ptr.is_null() {
                return Err("AV1 allocation failed".into());
            }
            std::ptr::copy_nonoverlapping(sample.as_ptr(), ptr, sample.len());
        }
        // SAFETY: initialized by dav1d_data_create.
        let mut data = unsafe { data.assume_init() };
        data.m.timestamp = tag as i64;
        let result = loop {
            // SAFETY: the context is open and `data` is a dav1d buffer.
            let r = unsafe { dav1d_send_data(self.ctx.clone(), NonNull::new(&mut data)) }.0;
            if r < 0 && r != eagain() {
                break Err(format!("AV1 decode error {r}"));
            }
            if let Err(e) = self.drain(want, out) {
                break Err(e);
            }
            if data.sz == 0 {
                break Ok(());
            }
        };
        if data.sz != 0 {
            // SAFETY: what dav1d did not consume is still ours to free.
            unsafe { dav1d_data_unref(NonNull::new(&mut data)) };
        }
        result
    }

    fn finish(&mut self, want: &mut dyn FnMut(u64) -> bool, out: &mut Vec<Decoded>) -> Result<(), String> {
        self.drain(want, out)
    }
}

impl Drop for Av1 {
    fn drop(&mut self) {
        // SAFETY: the context came from dav1d_open and is closed once.
        unsafe { dav1d_close(NonNull::new(&mut self.ctx)) };
    }
}
