//! M3-12: the committed visual-regression reference is readable and stable.
//!
//! The comparison itself is pinned by `image_diff`'s unit tests. This test
//! guards the artifact: it must exist, be an 8-bit RGBA 1920×1080 PNG, and
//! compare clean against itself. A capture regenerated from the same build is
//! compared by `voxel-bench image-diff` on the reference machine, not here.

use std::path::PathBuf;

use voxel_bench::image_diff::{Image, Threshold, compare};

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/images/m3-12-reference.png")
}

#[test]
fn the_committed_reference_is_a_readable_frame() {
    let image = Image::load(&reference_path()).expect("the M3-12 reference loads");
    assert_eq!(image.width, 1920);
    assert_eq!(image.height, 1080);
    assert_eq!(
        image.rgba.len(),
        (image.width * image.height * 4) as usize,
        "packed RGBA rows"
    );
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] == 255),
        "an opaque pixel exists"
    );
}

#[test]
fn the_reference_matches_itself_within_the_threshold() {
    let reference = Image::load(&reference_path()).expect("the M3-12 reference loads");
    let diff =
        compare(&reference, &reference.clone(), &Threshold::default()).expect("same dimensions");
    assert_eq!(diff.differing, 0);
    assert_eq!(diff.max_delta, 0);
    assert!(diff.passes(&Threshold::default()));
}
