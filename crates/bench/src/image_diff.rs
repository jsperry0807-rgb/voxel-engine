//! M3-12: the visual regression comparison.
//!
//! [`compare`] measures a candidate capture against the committed reference and
//! decides against [`Threshold`]. The reference for the M3 scene is
//! `docs/images/m3-12-reference.png`, captured with the M3-08 screenshot pose;
//! [`docs/m3-12-visual-regression.md`](../../../docs/m3-12-visual-regression.md)
//! records the measured run-to-run variance and why the threshold is what it is.
//!
//! The comparison is deliberately geometric, not a perceptual hash: the renderer
//! is deterministic on the reference machine (two captures are byte-identical),
//! so a per-channel delta with a small tolerance is both stricter and easier to
//! explain than a similarity score.

use std::path::Path;

/// Maximum fraction of pixels allowed to differ beyond the tolerance: 0.01%.
pub const DEFAULT_MAX_FRACTION: f64 = 0.0001;
/// Per-channel delta ignored as capture noise, in 0–255 levels.
pub const DEFAULT_CHANNEL_TOLERANCE: u8 = 2;
/// Largest single-channel delta allowed anywhere, in 0–255 levels.
pub const DEFAULT_MAX_DELTA: u8 = 32;
/// Largest mean absolute channel delta allowed, in 0–255 levels.
pub const DEFAULT_MAX_MEAN_ABS: f64 = 0.5;

/// One RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA rows, top to bottom.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Reads an 8-bit RGBA PNG.
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        let decoder = png::Decoder::new(file);
        let mut reader = decoder
            .read_info()
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| format!("decode {}: {error}", path.display()))?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err(format!(
                "{} is {:?}/{:?}, expected RGBA8",
                path.display(),
                info.color_type,
                info.bit_depth
            ));
        }
        buffer.truncate(info.buffer_size());
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba: buffer,
        })
    }

    /// Pixel count.
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// The limits a capture must meet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Threshold {
    /// Maximum fraction of pixels whose channel delta exceeds the tolerance.
    pub max_fraction: f64,
    /// Per-channel delta treated as noise.
    pub channel_tolerance: u8,
    /// Largest single-channel delta allowed anywhere.
    pub max_delta: u8,
    /// Largest mean absolute channel delta allowed.
    pub max_mean_abs: f64,
}

impl Default for Threshold {
    fn default() -> Self {
        Self {
            max_fraction: DEFAULT_MAX_FRACTION,
            channel_tolerance: DEFAULT_CHANNEL_TOLERANCE,
            max_delta: DEFAULT_MAX_DELTA,
            max_mean_abs: DEFAULT_MAX_MEAN_ABS,
        }
    }
}

/// What the comparison found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diff {
    /// Pixels compared.
    pub pixels: u64,
    /// Pixels with a channel delta above the tolerance.
    pub differing: u64,
    /// `differing / pixels`.
    pub differing_fraction: f64,
    /// Largest single-channel delta anywhere.
    pub max_delta: u8,
    /// Mean absolute channel delta over all channels.
    pub mean_abs: f64,
}

impl Diff {
    /// True when the capture is within `threshold`.
    pub fn passes(&self, threshold: &Threshold) -> bool {
        self.differing_fraction <= threshold.max_fraction
            && self.max_delta <= threshold.max_delta
            && self.mean_abs <= threshold.max_mean_abs
    }

    /// The one line a reviewer reads.
    pub fn line(&self, threshold: &Threshold) -> String {
        format!(
            "{} of {} pixels differ beyond {} ({}), max delta {}, mean abs {:.4}; threshold: fraction <= {} ({} px), max delta <= {}, mean abs <= {:.2} -> {}",
            self.differing,
            self.pixels,
            threshold.channel_tolerance,
            percent(self.differing_fraction),
            self.max_delta,
            self.mean_abs,
            threshold.max_fraction,
            allowed_pixels(self.pixels, threshold.max_fraction),
            threshold.max_delta,
            threshold.max_mean_abs,
            if self.passes(threshold) {
                "PASS"
            } else {
                "FAIL"
            },
        )
    }
}

/// Compares two images and reports the deltas.
///
/// Both images must have the same dimensions; a size change is a hard error
/// rather than a difference, because it means the capture configuration moved.
pub fn compare(
    reference: &Image,
    candidate: &Image,
    threshold: &Threshold,
) -> Result<Diff, String> {
    if reference.width != candidate.width || reference.height != candidate.height {
        return Err(format!(
            "size mismatch: reference {}x{}, candidate {}x{}",
            reference.width, reference.height, candidate.width, candidate.height
        ));
    }
    let pixels = reference.pixels();
    let mut differing = 0u64;
    let mut max_delta = 0u8;
    let mut total = 0f64;
    for (a, b) in reference
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(candidate.rgba.as_chunks::<4>().0)
    {
        let mut worst = 0u8;
        for channel in 0..4 {
            let delta = a[channel].abs_diff(b[channel]);
            worst = worst.max(delta);
            total += f64::from(delta);
        }
        if worst > threshold.channel_tolerance {
            differing += 1;
        }
        max_delta = max_delta.max(worst);
    }
    let channels = (pixels * 4).max(1) as f64;
    Ok(Diff {
        pixels,
        differing,
        differing_fraction: differing as f64 / pixels.max(1) as f64,
        max_delta,
        mean_abs: total / channels,
    })
}

/// Pixels the fraction allows, rounded down, at least one.
fn allowed_pixels(pixels: u64, fraction: f64) -> u64 {
    ((pixels as f64 * fraction) as u64).max(1)
}

fn percent(fraction: f64) -> String {
    format!("{:.4}%", fraction * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, fill: u8) -> Image {
        Image {
            width,
            height,
            rgba: vec![fill; (width * height * 4) as usize],
        }
    }

    #[test]
    fn identical_images_pass_with_zero_delta() {
        let a = image(4, 3, 128);
        let diff = compare(&a, &a.clone(), &Threshold::default()).expect("same size");
        assert_eq!(diff.differing, 0);
        assert_eq!(diff.max_delta, 0);
        assert!(diff.passes(&Threshold::default()));
    }

    #[test]
    fn one_pixel_beyond_the_tolerance_is_allowed_by_the_fraction() {
        let mut b = image(1920, 1080, 128);
        b.rgba[0] = 128 + 18;
        let diff = compare(&image(1920, 1080, 128), &b, &Threshold::default()).expect("same size");
        assert_eq!(diff.differing, 1);
        assert_eq!(diff.max_delta, 18);
        assert!(diff.passes(&Threshold::default()), "one pixel is 0.00005%");
    }

    #[test]
    fn a_whole_region_changing_fails() {
        let a = image(1920, 1080, 128);
        let mut b = a.clone();
        // 1% of the frame goes black, as a dropped chunk would.
        let pixels = (1920 * 1080) as usize;
        for pixel in 0..pixels / 100 {
            for channel in 0..4 {
                b.rgba[pixel * 4 + channel] = 0;
            }
        }
        let diff = compare(&a, &b, &Threshold::default()).expect("same size");
        assert!(!diff.passes(&Threshold::default()));
        assert!(diff.differing_fraction > 0.009);
    }

    #[test]
    fn a_small_delta_inside_the_tolerance_is_ignored() {
        let mut b = image(8, 8, 100);
        b.rgba[5] = 102;
        let diff = compare(&image(8, 8, 100), &b, &Threshold::default()).expect("same size");
        assert_eq!(diff.differing, 0, "a 2-level delta is capture noise");
        assert_eq!(diff.max_delta, 2);
        assert!(diff.passes(&Threshold::default()));
    }

    #[test]
    fn a_size_change_is_an_error_not_a_difference() {
        let error = compare(&image(4, 4, 0), &image(5, 4, 0), &Threshold::default())
            .expect_err("sizes differ");
        assert!(error.contains("size mismatch"), "{error}");
    }

    #[test]
    fn the_threshold_is_configurable() {
        let mut b = image(10, 10, 0);
        b.rgba[0] = 40;
        let strict = Threshold {
            max_delta: 32,
            ..Threshold::default()
        };
        // The one changed pixel is 1% of this tiny image, so a loose check has
        // to widen the fraction as well as the delta.
        let loose = Threshold {
            max_delta: 64,
            max_fraction: 0.02,
            ..Threshold::default()
        };
        assert!(
            !compare(&image(10, 10, 0), &b, &strict)
                .expect("sized")
                .passes(&strict)
        );
        assert!(
            compare(&image(10, 10, 0), &b, &loose)
                .expect("sized")
                .passes(&loose)
        );
    }
}
