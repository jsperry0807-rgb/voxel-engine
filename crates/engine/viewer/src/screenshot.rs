//! One-shot screenshot capture for the viewer (M3-08).
//!
//! `VOXEL_SCREENSHOT=path.png` makes the viewer copy the presented frame into a
//! map-read buffer once the ring is resident, write it as a PNG and exit with
//! code 0. `VOXEL_SCREENSHOT_FRAMES` sets how many loaded frames to render
//! first (default [`DEFAULT_SETTLE_FRAMES`]), so a capture can wait for the
//! streaming upload to finish.
//!
//! The capture reads the real swap-chain texture: it is the frame the viewer
//! presents, not a re-render through a second pipeline. The copy is recorded in
//! the frame's own encoder after the draw pass and before `present`, so no
//! extra submission and no second frame are needed.

use std::path::PathBuf;

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, MapMode, PollType, TextureFormat};

use crate::camera::Camera;
use crate::pose::View;
use crate::streaming::RING_CHUNKS;

/// Path of the PNG to write, or unset for a normal run.
pub const PATH_ENV: &str = "VOXEL_SCREENSHOT";
/// How many fully loaded frames to render before capturing.
pub const FRAMES_ENV: &str = "VOXEL_SCREENSHOT_FRAMES";
/// Optional `x,y,z` eye override, in world units.
pub const EYE_ENV: &str = "VOXEL_SCREENSHOT_EYE";
/// Optional `x,y,z` look-at override, in world units.
pub const TARGET_ENV: &str = "VOXEL_SCREENSHOT_TARGET";
/// The default settle time: long enough for the last upload frames to present.
pub const DEFAULT_SETTLE_FRAMES: u32 = 12;

/// One pending screenshot request.
#[derive(Clone, Debug)]
pub struct Capture {
    path: PathBuf,
    settle: u32,
    frames: u32,
    written: bool,
    /// Optional fixed pose, so the image does not depend on the interactive
    /// start camera.
    view: Option<View>,
}

impl Capture {
    /// Reads [`PATH_ENV`] and [`FRAMES_ENV`], or `None` for a normal run.
    ///
    /// A malformed frame count is a startup error: a capture that silently
    /// shoots the wrong frame is worse than a failed launch.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os(PATH_ENV)?;
        let settle = match std::env::var(FRAMES_ENV) {
            Ok(text) => match text.trim().parse::<u32>() {
                Ok(frames) => frames,
                Err(_) => panic!("{FRAMES_ENV}={text:?} is not a u32"),
            },
            Err(_) => DEFAULT_SETTLE_FRAMES,
        };
        let view = View::from_env(EYE_ENV, TARGET_ENV)
            .unwrap_or_else(|error| panic!("screenshot pose: {error}"));
        Some(Self {
            path: PathBuf::from(path),
            settle,
            frames: 0,
            written: false,
            view,
        })
    }

    /// The file this capture will write.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Frames to render after the ring loads before capturing.
    pub const fn settle(&self) -> u32 {
        self.settle
    }

    /// The fixed capture pose, or `None` to use the interactive camera.
    pub fn camera(&self, base: &Camera) -> Option<Camera> {
        self.view.as_ref().map(|view| view.camera(base))
    }

    /// Counts one successfully presented frame.
    pub fn tick(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }

    /// True once the ring is resident and the settle frames have elapsed.
    ///
    /// `loaded` is the streaming ring's resident chunk count. Waiting for the
    /// full ring keeps the image reproducible: a capture taken mid-load depends
    /// on how many upload frames happened to land.
    pub fn wants_copy(&self, loaded: usize) -> bool {
        !self.written && loaded >= RING_CHUNKS && self.frames >= self.settle
    }

    /// True once the PNG is on disk; the caller then exits.
    pub const fn written(&self) -> bool {
        self.written
    }
}

/// Creates the readback buffer for one frame of `width`×`height`.
///
/// `bytes_per_row` is padded to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`], which the
/// texture-to-buffer copy requires.
pub fn readback_buffer(device: &Device, width: u32, height: u32) -> (Buffer, u32) {
    let padded = padded_row_bytes(width);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(padded) * u64::from(height),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    (buffer, padded)
}

/// The row stride a texture-to-buffer copy must use for `width` RGBA8 pixels.
pub fn padded_row_bytes(width: u32) -> u32 {
    let unpadded = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}

impl Capture {
    /// Maps `buffer`, converts it to RGBA8, writes the PNG and marks the capture
    /// done.
    ///
    /// This is the one place the viewer blocks on the GPU: it happens once, on
    /// the capture frame, and the process exits immediately afterwards.
    pub fn save(
        &mut self,
        device: &Device,
        buffer: &Buffer,
        width: u32,
        height: u32,
        padded_row: u32,
        format: TextureFormat,
    ) -> Result<PathBuf, String> {
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| format!("poll after screenshot copy: {error}"))?;
        receiver
            .recv()
            .map_err(|_| "screenshot map callback did not run".to_owned())?
            .map_err(|error| format!("map the screenshot buffer: {error}"))?;

        let pixels = {
            let mapped = slice
                .get_mapped_range()
                .map_err(|error| format!("read the screenshot buffer: {error}"))?;
            let pixels = to_rgba8(&mapped, width, height, padded_row, format);
            drop(mapped);
            pixels
        };
        buffer.unmap();

        write_png(&self.path, width, height, &pixels)?;
        self.written = true;
        Ok(self.path.clone())
    }
}

/// Converts a padded, possibly BGRA, texture readback into packed RGBA8.
pub fn to_rgba8(
    bytes: &[u8],
    width: u32,
    height: u32,
    padded_row: u32,
    format: TextureFormat,
) -> Vec<u8> {
    let row = (width * 4) as usize;
    let padded = padded_row as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * padded;
        let line = &bytes[start..start + row];
        match format {
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => {
                for pixel in line.as_chunks::<4>().0 {
                    out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            _ => out.extend_from_slice(line),
        }
    }
    out
}

/// Writes `pixels` as an 8-bit RGBA PNG.
pub fn write_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("write {} header: {error}", path.display()))?;
    writer
        .write_image_data(pixels)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    // `png::Writer`'s Drop ignores the IEND error, so a full disk would look
    // like success. `finish` surfaces it.
    writer
        .finish()
        .map_err(|error| format!("finish {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_padded_to_the_copy_alignment() {
        assert_eq!(padded_row_bytes(1), 256);
        assert_eq!(padded_row_bytes(64), 256);
        assert_eq!(padded_row_bytes(65), 512);
        assert_eq!(padded_row_bytes(1920), 7680);
    }

    #[test]
    fn bgra_is_swizzled_and_padding_is_dropped() {
        // One 2×1 image: red, then blue, in BGRA, with 2 pixels of padding.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0, 0, 255, 255, 255, 0, 0, 255]);
        bytes.extend_from_slice(&[0u8; 8]);
        let pixels = to_rgba8(&bytes, 2, 1, 16, TextureFormat::Bgra8UnormSrgb);
        assert_eq!(pixels, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn rgba_is_copied_through() {
        let mut bytes = vec![1, 2, 3, 4, 5, 6, 7, 8];
        bytes.extend_from_slice(&[0u8; 8]);
        let pixels = to_rgba8(&bytes, 2, 1, 16, TextureFormat::Rgba8UnormSrgb);
        assert_eq!(pixels, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn a_capture_waits_for_the_ring_and_the_settle_frames() {
        let mut capture = Capture {
            path: PathBuf::from("/tmp/shot.png"),
            settle: 2,
            frames: 0,
            written: false,
            view: None,
        };
        assert!(!capture.wants_copy(RING_CHUNKS));
        capture.tick();
        assert!(!capture.wants_copy(RING_CHUNKS));
        capture.tick();
        assert!(
            !capture.wants_copy(RING_CHUNKS - 1),
            "a partially loaded ring is not reproducible"
        );
        assert!(capture.wants_copy(RING_CHUNKS));
        capture.written = true;
        assert!(!capture.wants_copy(RING_CHUNKS), "only one frame is copied");
    }
}
