//! CRC-32/ISO-HDLC (the zlib/PNG polynomial), shared by every on-disk format.
//!
//! The storage region log and the worldgen macro tile both protect their
//! payloads with the same checksum. Keeping one implementation here means a
//! tile written by worldgen can be verified by a tool that links only
//! `voxel-core`, and the two formats cannot drift apart.
//!
//! The incremental [`Crc32`] exists because a file often protects a header and
//! a payload that are not adjacent in memory: the tile CRC covers the header
//! (minus the CRC field) and then the payload, without copying either.

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// Incremental CRC-32/ISO-HDLC state.
#[derive(Clone, Copy, Debug)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// Start a new checksum.
    #[inline]
    pub const fn new() -> Self {
        Self { state: !0 }
    }

    /// Fold another slice into the checksum.
    #[inline]
    pub fn update(&mut self, data: &[u8]) {
        let mut state = self.state;
        for &byte in data {
            state = TABLE[((state ^ byte as u32) & 0xff) as usize] ^ (state >> 8);
        }
        self.state = state;
    }

    /// Finish and return the checksum.
    #[inline]
    pub const fn finish(&self) -> u32 {
        !self.state
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32/ISO-HDLC of one contiguous slice.
#[inline]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_published_check_values() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn crc32_is_incremental() {
        let whole = crc32(b"voxel-worldgen");
        let mut split = Crc32::new();
        split.update(b"voxel-");
        split.update(b"world");
        split.update(b"gen");
        assert_eq!(split.finish(), whole);
    }
}
