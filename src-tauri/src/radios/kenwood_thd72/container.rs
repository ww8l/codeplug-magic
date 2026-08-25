//! The TH-D72 image container: parsing, the MCP-4A `.mc4` wrapper, the model
//! guard, and which 256-byte blocks an upload is allowed to write.
//!
//! A TH-D72 codeplug is a flat 64 KiB image with no length field and no
//! revision stamp. **Nothing checksums the regions this driver writes**, and
//! that is measured rather than inherited: in the controlled series, two
//! single-channel edits made *by the radio itself* moved 25 bytes each and not
//! one of them fell outside the flag records, the channel records, the name
//! cells and one byte of UI state. A stored checksum over any of those would
//! have had to change. That test has the power to catch the failure that cost
//! this project a factory reset on the FT5D, whose checksum sat at a fixed
//! image address where any edit would have moved it.
//!
//! ⚠ It proves nothing about regions we never touch, and nothing about the
//! upload protocol — CHIRP's `write_block` sends no checksum and takes a
//! per-block ACK, unlike the TD-H3, whose reads carry one. Phase 5 step 2
//! (change one memory name, upload, see if the radio accepts it) stays in the
//! ladder; it is now expected to pass rather than hoped to.
//!
//! ## Why `patch` is the only mutator
//!
//! The clone protocol moves 256-byte blocks and an upload may write any subset
//! of them, so this driver can put a codeplug on the radio while never touching
//! the APRS, GPS, TNC or bitmap regions. That safety argument only holds if the
//! dirty-block set is exactly the set of blocks that actually changed — so the
//! bytes and the bookkeeping are updated in one place, together, and [`body`]
//! hands out `&[u8]` rather than `&mut [u8]`.
//!
//! [`body`]: Thd72Image::body

use super::layout::{
    read_prog_vfo_table, ProgVfoTable, BLOCK_COUNT, BLOCK_LEN, CALIBRATION_BASE, IMAGE_LEN,
    MC4_HEADER_LEN,
};

/// Total length of an MCP-4A `.mc4` file: its header, then the image.
pub(crate) const MC4_LEN: usize = MC4_HEADER_LEN + IMAGE_LEN;

/// How an image reached us, which decides what `to_mc4` re-emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageSource {
    /// A bare 64 KiB clone image — what the cable download produces.
    Raw,
    /// An MCP-4A `.mc4` file, whose header is carried along verbatim.
    Mc4,
}

/// A parsed TH-D72 image plus the bookkeeping needed for a partial upload.
pub(crate) struct Thd72Image {
    /// Exactly [`IMAGE_LEN`] bytes.
    body: Vec<u8>,
    /// The `.mc4` header exactly as it arrived, or `None` for a raw image.
    /// [`ImageSource`] is derived from this rather than stored alongside it —
    /// two fields encoding one fact can disagree, and this one cannot.
    header: Option<Box<[u8; MC4_HEADER_LEN]>>,
    /// One flag per 256-byte block; set only when a byte in it actually changed.
    dirty: [bool; BLOCK_COUNT],
}

/// Summarised rather than derived: a derived `Debug` would dump 64 KiB of image
/// into every failing assertion.
impl std::fmt::Debug for Thd72Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thd72Image")
            .field("source", &self.source())
            .field("len", &self.body.len())
            .field("dirty_blocks", &self.dirty_blocks())
            .finish()
    }
}

impl Thd72Image {
    /// Parse a raw 64 KiB clone image or an MCP-4A `.mc4` file.
    ///
    /// The model guard runs here, so a file that is not a TH-D72 image is
    /// refused before anything can patch it, name it, or hand it to a radio.
    pub(crate) fn parse(data: &[u8]) -> Result<Self, String> {
        let (header, body) = match data.len() {
            IMAGE_LEN => (None, data),
            MC4_LEN => {
                let (h, b) = data.split_at(MC4_HEADER_LEN);
                let mut owned = Box::new([0u8; MC4_HEADER_LEN]);
                owned.copy_from_slice(h);
                (Some(owned), b)
            }
            other => {
                return Err(format!(
                    "this file is {other} bytes — a TH-D72 clone image is {IMAGE_LEN} and an \
                     MCP-4A .mc4 is {MC4_LEN}. Pick a file read from a TH-D72 (radio-backups/ \
                     also holds images for other radios, which must not be written to this one)."
                ));
            }
        };

        check_thd72_image(body)?;

        Ok(Self {
            body: body.to_vec(),
            header,
            dirty: [false; BLOCK_COUNT],
        })
    }

    /// How this image arrived.
    pub(crate) fn source(&self) -> ImageSource {
        if self.header.is_some() {
            ImageSource::Mc4
        } else {
            ImageSource::Raw
        }
    }

    /// The 64 KiB image. Read-only on purpose — see the module header.
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }

    /// Write `bytes` at `off`, marking every block they actually change.
    ///
    /// Bytes equal to what is already there are not "changed": a patch that
    /// rewrites a memory with its own contents leaves the block clean and out of
    /// the upload. That keeps a write to the radio as small as the edit was,
    /// which is the whole safety argument for uploading a subset of blocks.
    pub(crate) fn patch(&mut self, off: usize, bytes: &[u8]) -> Result<(), String> {
        let end = off
            .checked_add(bytes.len())
            .ok_or_else(|| format!("patch at {off:#X} of {} bytes overflows", bytes.len()))?;

        if end > IMAGE_LEN {
            return Err(format!(
                "patch at {off:#06X} of {} bytes runs past the end of a {IMAGE_LEN}-byte TH-D72 \
                 image",
                bytes.len()
            ));
        }
        if end > CALIBRATION_BASE {
            return Err(format!(
                "patch at {off:#06X} of {} bytes reaches into {CALIBRATION_BASE:#06X}-{:#06X}, \
                 which holds per-radio data (it is identical across every image from one radio, \
                 including a factory reset, and different for every radio). CHIRP never writes \
                 those two blocks and neither does this driver.",
                bytes.len(),
                IMAGE_LEN - 1
            ));
        }

        for (i, &b) in bytes.iter().enumerate() {
            let at = off + i;
            if self.body[at] != b {
                self.body[at] = b;
                self.dirty[at / BLOCK_LEN] = true;
            }
        }
        Ok(())
    }

    /// The blocks an upload needs to write, ascending. Empty means the image is
    /// unchanged and there is nothing to send.
    pub(crate) fn dirty_blocks(&self) -> Vec<usize> {
        self.dirty
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| if d { Some(i) } else { None })
            .collect()
    }

    /// The image as the cable wants it.
    pub(crate) fn into_raw(self) -> Vec<u8> {
        self.body
    }

    /// The image as an MCP-4A `.mc4` file.
    ///
    /// A file that arrived as `.mc4` keeps its own header **byte for byte** —
    /// it carries a version stamp we do not understand and must not normalise.
    /// A raw image gets [`synthesised_mc4_header`].
    pub(crate) fn to_mc4(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MC4_LEN);
        match &self.header {
            Some(h) => out.extend_from_slice(&h[..]),
            None => out.extend_from_slice(&synthesised_mc4_header()),
        }
        out.extend_from_slice(&self.body);
        out
    }

    /// This radio's own programmable-VFO ranges — the ones a channel's band
    /// index must agree with. See `layout.rs` for why they are read from the
    /// image rather than assumed.
    pub(crate) fn prog_vfo_table(&self) -> Result<ProgVfoTable, String> {
        read_prog_vfo_table(&self.body)
    }
}

/// Refuse a file that is not a TH-D72 image, by name.
///
/// Byte `0x00` is `0x1B` and bytes `0x10..0x18` are `00 00 00 00 02 00 30 30`
/// in all eight real images examined. Bytes `0x01..0x04` are deliberately **not**
/// checked: they read `ff ff ff` on the 2017 radio and `01 32 ff` on the 2014
/// one, so a fixed value there would refuse somebody else's radio.
///
/// ⚠ Grade this honestly: eight files from **three** radios, of unknown variant
/// and firmware, none of them ours. That is better than the two samples which
/// once cost this project a factory reset, and it is still not a proven rule. If
/// a legitimate TH-D72 image is ever refused here, the guard is wrong, not the
/// file.
///
/// ⚠ CHIRP's `thd72.py` calls byte `0x02` `shouldbe32`, and only the 2014 radio
/// has `0x32` there — the other six images read `0xFF`. Whatever that field
/// means, the name is a claim the real files do not support. Do not guard on it.
pub(crate) fn check_thd72_image(body: &[u8]) -> Result<(), String> {
    if body.len() != IMAGE_LEN {
        return Err(format!(
            "this file is {} bytes — a TH-D72 clone image is {IMAGE_LEN}.",
            body.len()
        ));
    }
    if body[0x00] != 0x1B || body[0x10..0x18] != [0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x30, 0x30] {
        return Err(
            "this file is the right size for a TH-D72 image but does not carry a TH-D72 \
             front matter block — it is not one. Pick a file read from a TH-D72."
                .into(),
        );
    }
    Ok(())
}

/// The header MCP-4A writes, per CHIRP's `D72_FILE_HEADER`: `0xFF` fill with
/// `MCP-4A` at 0x00, `V1.04` at 0x08, `TH-D72` at 0x10, `1` at 0x20 and `AMB0`
/// at 0x80.
///
/// ⚠ **Unverified.** There is no real `.mc4` file on this machine — this layout
/// comes from one driver's `save_mmap` and nothing else, and the `V1.04` stamp
/// in particular is a version this project has never seen a file of. It is here
/// so a `.mc4` can be written at all; the first real one should be diffed
/// against it before anyone believes it.
fn synthesised_mc4_header() -> [u8; MC4_HEADER_LEN] {
    let mut h = [0xFFu8; MC4_HEADER_LEN];
    h[0x00..0x06].copy_from_slice(b"MCP-4A");
    h[0x08..0x0D].copy_from_slice(b"V1.04");
    h[0x10..0x16].copy_from_slice(b"TH-D72");
    h[0x20] = b'1';
    h[0x80..0x84].copy_from_slice(b"AMB0");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 0x20 bytes of `002-set-channel-1-from-radio.img` (2017 radio).
    const FRONTMATTER_2017: [u8; 0x20] = [
        0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x30, 0x30, 0x30, 0x00, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff,
    ];

    /// The same bytes of `th-d72a.img` (2014 radio) — note 0x01/0x02/0x03.
    const FRONTMATTER_2014: [u8; 0x20] = [
        0x1b, 0x01, 0x32, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x30, 0x30, 0x30, 0x00, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff,
    ];

    /// CHIRP's `D72_FILE_HEADER`, expanded from its source expression rather
    /// than from this module's constructor, so the test can disagree with it.
    const CHIRP_MC4_HEADER_HEX: &str = concat!(
        "4d43502d3441ffff56312e3034ffffff54482d443732ffffffffffffffffffff",
        "31ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "414d4230ffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    );

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// An image shaped like a real one: 0xFF fill, real front matter, and
    /// per-radio bytes in the calibration region so it is never all-0xFF.
    fn image_with(frontmatter: &[u8; 0x20]) -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        image[..0x20].copy_from_slice(frontmatter);
        image[CALIBRATION_BASE] = 0x17;
        image[CALIBRATION_BASE + 1] = 0x03;
        image
    }

    fn real_image() -> Vec<u8> {
        image_with(&FRONTMATTER_2017)
    }

    #[test]
    fn a_raw_image_parses_and_comes_back_byte_identical() {
        let raw = real_image();
        let image = Thd72Image::parse(&raw).unwrap();
        assert_eq!(image.source(), ImageSource::Raw);
        assert_eq!(image.body(), &raw[..]);
        assert!(image.dirty_blocks().is_empty(), "parsing dirties nothing");
        assert_eq!(image.into_raw(), raw);
    }

    /// The whole point of not guarding bytes 0x01..0x04: both real radios pass.
    #[test]
    fn both_real_frontmatter_variants_are_accepted() {
        Thd72Image::parse(&image_with(&FRONTMATTER_2017)).expect("2017 radio");
        Thd72Image::parse(&image_with(&FRONTMATTER_2014)).expect("2014 radio");
    }

    #[test]
    fn an_mc4_file_keeps_its_header_byte_for_byte() {
        // A header that is deliberately NOT the one we would synthesise, so a
        // normalising bug cannot pass this test.
        let mut header = vec![0xFFu8; MC4_HEADER_LEN];
        header[0x00..0x06].copy_from_slice(b"MCP-4A");
        header[0x08..0x0D].copy_from_slice(b"V9.99");
        header[0x10..0x16].copy_from_slice(b"TH-D72");
        header[0xFE] = 0x42;

        let mut file = header.clone();
        file.extend_from_slice(&real_image());

        let image = Thd72Image::parse(&file).unwrap();
        assert_eq!(image.source(), ImageSource::Mc4);
        assert_eq!(image.body(), &real_image()[..]);
        assert_eq!(image.to_mc4(), file, "header and body both re-emitted as-is");
    }

    #[test]
    fn a_raw_image_written_as_mc4_gets_the_documented_header() {
        let image = Thd72Image::parse(&real_image()).unwrap();
        let mc4 = image.to_mc4();
        assert_eq!(mc4.len(), MC4_LEN);
        assert_eq!(&mc4[..MC4_HEADER_LEN], &unhex(CHIRP_MC4_HEADER_HEX)[..]);
        assert_eq!(&mc4[MC4_HEADER_LEN..], &real_image()[..]);
    }

    #[test]
    fn a_patch_marks_exactly_the_blocks_it_changed() {
        let mut image = Thd72Image::parse(&real_image()).unwrap();
        // Straddle a block boundary: last byte of block 0x15, first of 0x16.
        image.patch(0x15FF, &[0xAA, 0xBB]).unwrap();
        assert_eq!(image.dirty_blocks(), vec![0x15, 0x16]);
        assert_eq!(image.body()[0x15FF], 0xAA);
        assert_eq!(image.body()[0x1600], 0xBB);
    }

    /// A patch that changes nothing must not enlarge the upload.
    #[test]
    fn rewriting_a_byte_with_its_own_value_leaves_the_block_clean() {
        let mut image = Thd72Image::parse(&real_image()).unwrap();
        image.patch(0x1500, &[0xFF; 16]).unwrap();
        assert!(image.dirty_blocks().is_empty());

        image.patch(0x1500, &[0x00; 16]).unwrap();
        assert_eq!(image.dirty_blocks(), vec![0x15]);
    }

    #[test]
    fn a_patch_into_the_calibration_region_is_refused() {
        let mut image = Thd72Image::parse(&real_image()).unwrap();
        let err = image.patch(CALIBRATION_BASE, &[0x00]).unwrap_err();
        assert!(err.contains("per-radio data"), "{err}");
        // And the byte before it is fine.
        image.patch(CALIBRATION_BASE - 1, &[0x00]).unwrap();
        // Including a write that only *reaches* into it.
        assert!(image.patch(CALIBRATION_BASE - 1, &[0x00, 0x01]).is_err());
    }

    #[test]
    fn a_patch_past_the_end_is_refused() {
        let mut image = Thd72Image::parse(&real_image()).unwrap();
        assert!(image.patch(IMAGE_LEN, &[0x00]).is_err());
        assert!(image.patch(IMAGE_LEN - 1, &[0x00, 0x01]).is_err());
        assert!(image.patch(usize::MAX, &[0x00]).is_err(), "no overflow panic");
    }

    #[test]
    fn a_file_of_the_wrong_length_is_refused_by_name() {
        // 0x2008 is a TD-H3 backup's length — a real file in radio-backups/.
        let err = Thd72Image::parse(&[0u8; 0x2008]).unwrap_err();
        assert!(err.contains("TH-D72"), "names the radio: {err}");
        assert!(err.contains("8200"), "names the length it got: {err}");
        assert!(err.contains("65536"), "names the length it wanted: {err}");
    }

    /// A TD-H3 backup padded to 64 KiB is the right size and the wrong radio.
    #[test]
    fn a_right_sized_file_that_is_not_a_d72_image_is_refused() {
        let err = Thd72Image::parse(&vec![0u8; IMAGE_LEN]).unwrap_err();
        assert!(err.contains("not one"), "{err}");
    }

    #[test]
    fn the_prog_vfo_table_is_read_out_of_the_image_being_patched() {
        let mut raw = real_image();
        raw[0x02C0..0x02C4].copy_from_slice(&144_000_000u32.to_le_bytes());
        raw[0x02C4..0x02C8].copy_from_slice(&146_000_000u32.to_le_bytes());
        let image = Thd72Image::parse(&raw).unwrap();
        let table = image.prog_vfo_table().unwrap();
        assert_eq!(table[0].start_hz, 144_000_000);
        assert_eq!(table[0].end_hz, 146_000_000);
    }
}
