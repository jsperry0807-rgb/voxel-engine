//! The one PNG encoder used by every tool in the pipeline.
//!
//! Settings are fixed by `docs/generator.spec.md` section 5.4: RGBA8,
//! non-interlaced, `Compression::Best`, Paeth filtering, adaptive filtering
//! off, and no ancillary chunks. If a `png` bump changes bytes,
//! `texgen update-manifest` is the only sanctioned fix.

use std::error::Error;
use std::fmt;

/// Encoder recipe recorded in the pipeline docs. Not a semver promise; it is a
/// human-readable label for the byte stream.
pub const PNG_RECIPE: &str = "png-0.17.16-rgba8-best-paeth-noadaptive-noancillary";

/// A failure while encoding a PNG.
#[derive(Debug)]
pub enum PngError {
    Encode(png::EncodingError),
    Invalid(String),
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => write!(f, "PNG encode failed: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for PngError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

/// Encodes row-major RGBA8 pixels to PNG bytes with the locked settings.
pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, PngError> {
    let expected = width as usize * height as usize * 4;
    if pixels.len() != expected {
        return Err(PngError::Invalid(format!(
            "expected {expected} RGBA bytes for {width}x{height}, got {}",
            pixels.len()
        )));
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Best);
        encoder.set_filter(png::FilterType::Paeth);
        encoder.set_adaptive_filter(png::AdaptiveFilterType::NonAdaptive);
        let mut writer = encoder.write_header().map_err(PngError::Encode)?;
        writer.write_image_data(pixels).map_err(PngError::Encode)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_exactly_one_idat_and_no_ancillary_chunks() {
        let bytes = encode_rgba(2, 2, &[255; 16]).expect("encode");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");

        let mut chunks = Vec::new();
        let mut offset = 8;
        while offset + 8 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &bytes[offset + 4..offset + 8];
            chunks.push(kind.to_vec());
            offset += 12 + length;
        }
        assert!(chunks.iter().any(|chunk| chunk == b"IDAT"));
        assert!(
            chunks
                .iter()
                .all(|chunk| matches!(chunk.as_slice(), b"IHDR" | b"IDAT" | b"IEND"))
        );
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(encode_rgba(2, 2, &[0; 15]).is_err());
    }
}
