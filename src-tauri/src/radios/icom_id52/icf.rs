//! The Icom `.icf` container: parse a settings file, patch bytes in it, and
//! write it back so the radio still accepts it.
//!
//! An `.icf` is **ASCII text**, not a binary image — a memory dump rendered as
//! hex lines with a small header:
//!
//! ```text
//! 32200000                       ← model id, 4 bytes as 8 hex chars
//! #Comment=
//! #MapRev=1
//! #EtcData=000000
//! 0000000020DEADBEEF…            ← address(8 hex) length(2 hex) data(length*2 hex)
//! 0000002020CAFEBABE…
//! …
//! #CD=8FA0…                      ← MD5 over the model magic + the raw lines
//! ```
//!
//! ## Why this is a patcher and not a generator
//!
//! Same bargain as the FT5D's `BACKUP.dat` writer: the file is the operator's
//! own radio, most of it is settings we do not model (APRS, GPS logs, the
//! opening message, Bluetooth pairings), and generating one from scratch would
//! silently revert all of it. So a real file goes in, named byte ranges are
//! overwritten, and everything else comes back out untouched — which this
//! module proves by round-tripping to a byte-identical string before anything
//! is changed.
//!
//! ## The checksum, which is the part that bites
//!
//! SD-card-era Icoms append `#CD=<md5>`. Get it wrong and the radio rejects the
//! file — and on the FT5D, whose 32-bit checksum was *not* documented anywhere,
//! guessing it wrong cost four failed hardware writes and a factory reset. Here
//! the algorithm is known up front from CHIRP's `icf.py` (`write_file`), so it
//! is implemented from a specification rather than inferred from two samples:
//!
//! - hash input starts with six "magic" bytes derived from the model id
//!   ([`model_magic`]),
//! - followed by every data line **unhexlified**, address and length fields
//!   included — not the payload bytes alone.
//!
//! [`IcfFile::parse`] verifies the digest of the incoming file, so a file we
//! cannot reproduce the checksum for is rejected at the door instead of being
//! written to a radio.
//!
//! ## What this module deliberately does not know
//!
//! Anything about the ID-52 specifically — no addresses, no field meanings, not
//! even which model ids are acceptable. It is the envelope; the letter is
//! elsewhere. That split is what lets the container be finished and tested now,
//! while the memory map is still waiting on dumps from a real radio
//! (`scratchpad/id52/FINDINGS.md` §5).

// Reached only from this module's tests until the settings path lands, which is
// the caller that reads and patches an image. Finished and proven ahead of its
// consumer on purpose: the checksum is the part that decides whether the radio
// accepts a file at all, and it was implementable before any hardware arrived.
#![allow(dead_code)]

use md5::{Digest, Md5};

/// Data-line record size Icom uses on SD-card-era models: 32 bytes per line,
/// with an 8-hex-character address. Older/smaller radios use 16 bytes and a
/// 4-character address; both are parsed, but only the wide form carries `#CD`.
const WIDE_ADDR_CHARS: usize = 8;
const NARROW_ADDR_CHARS: usize = 4;

/// A parsed `.icf` file: a flat memory image plus everything needed to write
/// the exact same file back out.
#[derive(Debug, Clone)]
pub(crate) struct IcfFile {
    /// Line 1 verbatim — the model id as 8 hex characters.
    model_line: String,
    /// Those same 4 bytes decoded, for the checksum magic.
    model: Vec<u8>,
    /// `#`-prefixed header lines in file order, verbatim, EXCLUDING the
    /// trailing `#CD` digest. Kept as text rather than parsed into fields
    /// because we round-trip far more of them than we understand: re-emitting
    /// `#Comment=` from a struct would drop anything Icom adds later.
    header: Vec<String>,
    /// Bytes per data line, from the first line.
    record_size: usize,
    /// Address field width in characters (8 or 4).
    addr_chars: usize,
    /// The memory image itself, contiguous from address 0.
    image: Vec<u8>,
    /// Whether the source carried a `#CD=` digest to reproduce.
    has_digest: bool,
    /// Whether data lines were uppercase hex, so a round-trip is byte-identical
    /// rather than merely equivalent.
    uppercase: bool,
    /// `\r\n` or `\n`, whichever the source used.
    line_ending: &'static str,
    /// Whether the source ended with a line break after its last line.
    trailing_newline: bool,
}

impl IcfFile {
    /// Parse an `.icf` file, verifying its checksum.
    ///
    /// Rejects rather than repairs: a file whose `#CD` we cannot reproduce is a
    /// file whose format we do not actually understand, and writing a patched
    /// version of it to a radio is exactly the move that bricks one.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let trailing_newline = text.ends_with('\n');

        let mut lines = text.lines().filter(|l| !l.trim().is_empty());

        let model_line = lines
            .next()
            .ok_or("not an ICF file: the file is empty")?
            .trim()
            .to_string();
        if model_line.len() != 8 || !model_line.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "not an ICF file: expected an 8-character hex model id on line 1, got {model_line:?}"
            ));
        }
        let model = unhex(&model_line)?;

        let mut header = Vec::new();
        let mut file_digest: Option<String> = None;
        let mut record_size = 0usize;
        let mut addr_chars = WIDE_ADDR_CHARS;
        let mut uppercase = true;
        let mut image: Vec<u8> = Vec::new();
        // The digest covers the raw lines, so it is accumulated during the same
        // pass that decodes them rather than re-rendering them afterwards —
        // re-rendering is what would let a formatting difference slip past the
        // check unnoticed.
        let mut hashed: Vec<u8> = model_magic(&model);

        for line in lines {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix('#') {
                match rest.split_once('=') {
                    Some(("CD", digest)) => file_digest = Some(digest.trim().to_uppercase()),
                    // Anything else is a header we carry through untouched.
                    _ => header.push(line.to_string()),
                }
                continue;
            }

            // Address width is not stated anywhere in the file: Icom's own
            // reader infers it from the line length, on the rule that the
            // payload is always a whole number of 4-byte words, so whatever is
            // left over modulo 8 characters must be the address+length prefix.
            let prefix = if line.len() % 8 == 6 {
                NARROW_ADDR_CHARS
            } else {
                WIDE_ADDR_CHARS
            };
            if image.is_empty() {
                addr_chars = prefix;
            } else if prefix != addr_chars {
                return Err("corrupt ICF: the address width changes partway through".into());
            }

            if line.len() < prefix + 2 {
                return Err(format!("corrupt ICF: data line too short: {line:?}"));
            }
            let addr = usize::from_str_radix(&line[..prefix], 16)
                .map_err(|_| format!("corrupt ICF: bad address in {line:?}"))?;
            let len = usize::from_str_radix(&line[prefix..prefix + 2], 16)
                .map_err(|_| format!("corrupt ICF: bad length in {line:?}"))?;
            let payload = &line[prefix + 2..];
            if payload.len() != len * 2 {
                return Err(format!(
                    "corrupt ICF: line claims {len} bytes but carries {}",
                    payload.len() / 2
                ));
            }
            if addr != image.len() {
                return Err(format!(
                    "unsupported ICF: expected the next line at 0x{:X} but it is at 0x{addr:X} \
                     — this writer only handles images laid out contiguously from zero",
                    image.len()
                ));
            }

            if image.is_empty() {
                record_size = len;
                uppercase = !payload.chars().any(|c| c.is_ascii_lowercase());
            }
            image.extend_from_slice(&unhex(payload)?);
            hashed.extend_from_slice(&unhex(line)?);
        }

        if image.is_empty() {
            return Err("not an ICF file: no data lines".into());
        }

        let has_digest = file_digest.is_some();
        if let Some(expected) = file_digest {
            let actual = digest_of(&hashed);
            if actual != expected {
                return Err(format!(
                    "ICF checksum mismatch: the file says {expected} but its contents hash to \
                     {actual}. Refusing to patch a file whose format does not match what this \
                     writer understands."
                ));
            }
        }

        Ok(Self {
            model_line,
            model,
            header,
            record_size,
            addr_chars,
            image,
            has_digest,
            uppercase,
            line_ending,
            trailing_newline,
        })
    }

    /// The memory image, for decoders to read.
    pub(crate) fn image(&self) -> &[u8] {
        &self.image
    }

    /// The memory image, for encoders to patch. Length is fixed — a settings
    /// write changes bytes in place and never resizes the radio's memory.
    pub(crate) fn image_mut(&mut self) -> &mut [u8] {
        &mut self.image
    }

    /// The model id as it appears on line 1, e.g. `"32200000"`. The one piece
    /// of identity a caller needs to check it has the right radio's file.
    ///
    /// The settings writer is the caller that must refuse an `.icf` belonging to
    /// some other Icom before patching it — the ID-52's own id is not known
    /// until a file comes off the radio.
    pub(crate) fn model_id(&self) -> &str {
        &self.model_line
    }

    /// The `#MapRev` value, when the file carries one.
    ///
    /// The ID-52 has a "Save Form" setting that writes an *earlier firmware
    /// version* of the file (Advanced Manual p. 2-2), which is exactly the kind
    /// of thing that moves a memory map. A caller with hard-won offsets should
    /// refuse a revision it has not seen rather than write to addresses that
    /// may have shifted.
    pub(crate) fn map_rev(&self) -> Option<u32> {
        self.header
            .iter()
            .find_map(|h| h.strip_prefix("#MapRev="))
            .and_then(|v| v.trim().parse().ok())
    }

    /// Render the file back to text, recomputing the checksum over the current
    /// image. Byte-identical to the input when nothing has been patched.
    pub(crate) fn render(&self) -> String {
        let mut out = String::with_capacity(self.image.len() * 2 + 512);
        let mut hashed = model_magic(&self.model);

        out.push_str(&self.model_line);
        out.push_str(self.line_ending);
        for h in &self.header {
            out.push_str(h);
            out.push_str(self.line_ending);
        }

        for (i, chunk) in self.image.chunks(self.record_size).enumerate() {
            let addr = i * self.record_size;
            let mut line = String::with_capacity(self.addr_chars + 2 + chunk.len() * 2);
            match self.addr_chars {
                NARROW_ADDR_CHARS => line.push_str(&format!("{addr:04X}")),
                _ => line.push_str(&format!("{addr:08X}")),
            }
            line.push_str(&format!("{:02X}", chunk.len()));
            for b in chunk {
                line.push_str(&format!("{b:02X}"));
            }
            if !self.uppercase {
                line = line.to_lowercase();
            }
            // Unhexlifying our own line rather than hashing the bytes directly
            // keeps this identical to the reader's accumulation, which is what
            // makes "parse then render" a genuine checksum round-trip.
            hashed.extend_from_slice(&unhex(&line).expect("we just wrote this hex"));
            out.push_str(&line);
            out.push_str(self.line_ending);
        }

        if self.has_digest {
            out.push_str(&format!("#CD={}", digest_of(&hashed)));
            out.push_str(self.line_ending);
        }

        if !self.trailing_newline {
            while out.ends_with('\n') || out.ends_with('\r') {
                out.pop();
            }
        }
        out
    }
}

/// The six bytes the checksum is seeded with, expanded from the model id.
///
/// Model `AB CD 00 00` becomes `AA BB CC DD AB CD`: each nibble of the first
/// two bytes doubled into a byte of its own, then those two bytes verbatim.
/// Icom's reason for this is not recorded anywhere; it is reproduced because
/// the radio checks it.
fn model_magic(model: &[u8]) -> Vec<u8> {
    let (m0, m1) = (model[0], model[1]);
    let (a, b) = ((m0 & 0xF0) >> 4, m0 & 0x0F);
    let (c, d) = ((m1 & 0xF0) >> 4, m1 & 0x0F);
    vec![
        (a << 4) | a,
        (b << 4) | b,
        (c << 4) | c,
        (d << 4) | d,
        m0,
        m1,
    ]
}

fn digest_of(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
    format!("{:X}", h.finalize())
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("corrupt ICF: odd-length hex run {s:?}"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| format!("corrupt ICF: {:?} is not hex", &s[i..i + 2]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a syntactically real ICF the way Icom writes one, so the tests
    /// exercise the same shape a radio produces. Not a capture from hardware —
    /// the checksum here is whatever our own implementation computes, which is
    /// why [`checksum_matches_chirps_worked_example`] pins the algorithm
    /// against an independent source as well.
    fn synth(model: &str, records: &[[u8; 4]]) -> String {
        let mut body = String::new();
        for (i, r) in records.iter().enumerate() {
            body.push_str(&format!("{:08X}04", i * 4));
            for b in r {
                body.push_str(&format!("{b:02X}"));
            }
            body.push_str("\r\n");
        }
        let head = format!("{model}\r\n#Comment=\r\n#MapRev=1\r\n#EtcData=000000\r\n");
        let text = format!("{head}{body}");
        // Round-trip once through the parser to learn the digest, then re-emit
        // WITH it, which is how a file from the radio arrives.
        let f = IcfFile::parse(&text).expect("synthetic file parses");
        let mut hashed = model_magic(&f.model);
        for (i, r) in records.iter().enumerate() {
            let mut line = format!("{:08X}04", i * 4);
            for b in r {
                line.push_str(&format!("{b:02X}"));
            }
            hashed.extend_from_slice(&unhex(&line).unwrap());
        }
        format!("{head}{body}#CD={}\r\n", digest_of(&hashed))
    }

    /// The whole safety argument for patching a real radio's file rests on
    /// this: parse then render, change nothing, get the same bytes back. If
    /// that does not hold, every byte we did not mean to touch is at risk.
    #[test]
    fn an_untouched_file_round_trips_byte_for_byte() {
        let text = synth("32200000", &[[0xDE, 0xAD, 0xBE, 0xEF], [0, 1, 2, 3]]);
        let f = IcfFile::parse(&text).unwrap();
        assert_eq!(f.render(), text);
    }

    /// The point of the module: change bytes, and the digest follows. A stale
    /// digest is what the radio rejects.
    #[test]
    fn patching_the_image_rewrites_the_digest() {
        let text = synth("32200000", &[[0xDE, 0xAD, 0xBE, 0xEF], [0, 1, 2, 3]]);
        let mut f = IcfFile::parse(&text).unwrap();
        let before = f.render();

        f.image_mut()[5] = 0xFF;
        let after = f.render();

        assert_ne!(before, after);
        assert_eq!(IcfFile::parse(&after).unwrap().image()[5], 0xFF);

        let digest_line = |s: &str| {
            s.lines()
                .find(|l| l.starts_with("#CD="))
                .unwrap()
                .to_string()
        };
        assert_ne!(digest_line(&before), digest_line(&after));
    }

    /// A file whose checksum we cannot reproduce is a file we do not
    /// understand. Refusing it here is what keeps a misunderstanding from
    /// reaching the radio.
    #[test]
    fn a_bad_checksum_is_refused() {
        let text = synth("32200000", &[[1, 2, 3, 4]]);
        let broken = text.replace("#CD=", "#CD=00");
        let err = IcfFile::parse(&broken).unwrap_err();
        assert!(err.contains("checksum mismatch"), "{err}");
    }

    /// The digest seed is the one piece of the algorithm with no rationale, so
    /// it is pinned against CHIRP's worked example: an ID-31 (`33 22 00 00`)
    /// hashes with a header of `33 33 22 22 33 22`.
    #[test]
    fn checksum_matches_chirps_worked_example() {
        assert_eq!(
            model_magic(&[0x33, 0x22, 0x00, 0x00]),
            vec![0x33, 0x33, 0x22, 0x22, 0x33, 0x22]
        );
    }

    /// Header lines are carried through verbatim, including ones this code has
    /// no opinion about — dropping an unrecognised header would quietly change
    /// the operator's file.
    #[test]
    fn unknown_headers_survive_a_round_trip() {
        let text = synth("32200000", &[[1, 2, 3, 4]])
            .replace("#MapRev=1\r\n", "#MapRev=3\r\n#SomethingNew=42\r\n");
        let f = IcfFile::parse(&text).unwrap();
        assert_eq!(f.map_rev(), Some(3));
        assert!(f.render().contains("#SomethingNew=42"));
    }

    /// Not every candidate file is an ICF, and the failure should say so rather
    /// than surfacing as a hex parse error three layers down.
    #[test]
    fn non_icf_input_is_rejected_by_name() {
        for bad in ["", "hello world\n", "3220000\n0000000004DEADBEEF\n"] {
            assert!(IcfFile::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    /// The same round trip against a **real file off Tim's ID-52**, which is the
    /// only source that can be wrong in ways the synthetic fixture is not.
    ///
    /// `#[ignore]`d because the file is a personal radio's dump under
    /// `scratchpad/` (gitignored), so it cannot exist in CI. Run it whenever a
    /// fresh capture lands:
    ///
    ///     cargo test --lib icf -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf under scratchpad/id52/"]
    fn a_real_radio_file_round_trips() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/id52_01_base.icf"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("skipped: no capture at {path}");
            return;
        };

        let icf = IcfFile::parse(&text).expect("a real ID-52 file must parse");
        assert_eq!(icf.model_id(), super::super::ID52_MODEL_ID);
        assert_eq!(icf.map_rev(), Some(super::super::ID52_MAP_REV));
        assert_eq!(icf.image().len(), super::super::ID52_IMAGE_LEN);
        assert_eq!(
            icf.render(),
            text,
            "an untouched real file must come back byte for byte"
        );
        eprintln!(
            "ok: model {} rev {:?}, {} bytes of image",
            icf.model_id(),
            icf.map_rev(),
            icf.image().len()
        );
    }
}
