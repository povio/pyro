//! pyro: tagged images are drawn in their colour space (CSS Color 4, "Color spaces of tagged
//! images"): a raster image's embedded ICC profile converts its pixels to sRGB when it is
//! decoded, as Chromium does for an sRGB display. An untagged image is sRGB already.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use moxcms::{ColorProfile, DataColorSpace, Layout, TransformExecutor, TransformOptions};

type Transform = Arc<dyn TransformExecutor<u8> + Send + Sync>;

/// The transform to sRGB of each profile seen, by its bytes: `None` for a profile that is sRGB
/// already (or not an RGB one moxcms reads), whose pixels stay as they are.
static TRANSFORMS: LazyLock<Mutex<HashMap<Vec<u8>, Option<Transform>>>> =
    LazyLock::new(Default::default);

/// Converts `rgba` (RGBA8, unpremultiplied) from the colour space of the ICC profile `icc` to sRGB.
pub(crate) fn to_srgb(icc: &[u8], rgba: &mut [u8]) {
    const MAX_PROFILES: usize = 32;
    let transform = {
        let mut transforms = TRANSFORMS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(transform) = transforms.get(icc) {
            transform.clone()
        } else {
            if transforms.len() >= MAX_PROFILES {
                transforms.clear();
            }
            let transform = transform_to_srgb(icc);
            transforms.insert(icc.to_vec(), transform.clone());
            transform
        }
    };
    if let Some(transform) = transform {
        let source = rgba.to_vec();
        let _ = transform.transform(&source, rgba);
    }
}

/// The transform from `icc`'s colour space to sRGB, `None` when there is nothing to convert:
/// the profile is not an RGB one moxcms reads, or it maps every probe colour to itself within a
/// level (an sRGB profile, which Skia also leaves alone), so that its images stay exact.
fn transform_to_srgb(icc: &[u8]) -> Option<Transform> {
    let profile = ColorProfile::new_from_slice(icc).ok()?;
    if profile.color_space != DataColorSpace::Rgb {
        return None;
    }
    let srgb = ColorProfile::new_srgb();
    let transform = profile
        .create_transform_8bit(Layout::Rgba, &srgb, Layout::Rgba, TransformOptions::default())
        .ok()?;
    // Ramps of each primary and of gray.
    let probe: Vec<u8> = (0..=255u8)
        .step_by(5)
        .flat_map(|v| [[v, 0, 0, 255], [0, v, 0, 255], [0, 0, v, 255], [v, v, v, 255]])
        .flatten()
        .collect();
    let mut mapped = vec![0; probe.len()];
    transform.transform(&probe, &mut mapped).ok()?;
    let identity = probe.iter().zip(&mapped).all(|(a, b)| a.abs_diff(*b) <= 1);
    (!identity).then_some(transform)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An sRGB profile changes nothing; one with red and green swapped swaps them.
    #[test]
    fn profiles_convert_to_srgb() {
        let srgb = ColorProfile::new_srgb().encode().unwrap();
        let mut pixels = vec![0x99, 0, 0, 255, 10, 200, 30, 128];
        let original = pixels.clone();
        to_srgb(&srgb, &mut pixels);
        assert_eq!(pixels, original);

        let mut swapped = ColorProfile::new_srgb();
        std::mem::swap(&mut swapped.red_colorant, &mut swapped.green_colorant);
        let swapped = swapped.encode().unwrap();
        let mut pixels = vec![0x99, 0, 0, 255];
        to_srgb(&swapped, &mut pixels);
        assert!(pixels[0] <= 1 && pixels[1].abs_diff(0x99) <= 1 && pixels[2] <= 1, "{pixels:?}");
        assert_eq!(pixels[3], 255, "alpha stays");

        // Not a profile: the pixels stay.
        let mut pixels = original.clone();
        to_srgb(b"not a profile", &mut pixels);
        assert_eq!(pixels, original);
    }
}
