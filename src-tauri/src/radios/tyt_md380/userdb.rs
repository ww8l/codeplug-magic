//! md380tools' user database: caller ID for radios running Travis Goodspeed's
//! patched firmware.
//!
//! The patched firmware looks a caller's DMR ID up in a text database it keeps
//! in the radio's SPI flash at 0x100000, above the codeplug (0x0-0x3FFFF) and
//! its own menu settings (0xF0000-0xFFFFF). Stock firmware ignores the region,
//! and has no commands to write it, so this is patched-firmware only.
//!
//! Sources: md380tools (`Makefile` flashdb, `applet/src/usersdb.c`,
//! `md380_tool.py`, `applet/src/tooldfu.h`) and editcp (`dfu.go`
//! `WriteMD380Users`, `userdb.go`) — see scratchpad/tyt_md380/CONTACTS-
//! RESEARCH.md. Hardware-proven on Tim's UHF MD-380 (2026-10-10): 269,452
//! users written and read back, the codeplug intact after.
//!
//! ## Format — md380tools' "linear" database
//!
//! The body's byte count in ASCII decimal, `\n`, then one line per user,
//! `ID,CALLSIGN,NAME,CITY,STATE,NICKNAME,COUNTRY\n`, **sorted by ID** (the
//! firmware binary-searches it). ASCII only, no commas inside a field, and the
//! firmware reads at most 100 bytes of a line. Built the way editcp builds it,
//! not with md380tools' awk scripts, which assume an older radioid column order.
//!
//! The indexed format newer firmware also reads is half the size but needs a
//! newer build; the linear one is read by every patched build.

use std::time::Duration;

use super::protocol::{self, Link};
use crate::radios::driver::CallsignRecord;

/// Where the database starts, and the lowest address this module will ever
/// erase or write. Everything below is the codeplug and the firmware's own
/// settings.
pub(crate) const BASE: u32 = 0x10_0000;
/// The 16 MB chip a database needs. Some VHF units carry a 1 MB part and
/// cannot hold one at all.
pub(crate) const FLASH_SIZE: u32 = 0x100_0000;
/// The firmware refuses a declared size over 15 MB − 1.
pub(crate) const MAX_BODY: usize = 15 * 1024 * 1024 - 1;
const BLOCK: u32 = 0x1_0000;
const CHUNK: usize = 1024;
/// The firmware's per-line read limit.
const MAX_LINE: usize = 100;

/// Plain ASCII with no commas, trimmed, double spaces collapsed — editcp's
/// `normalize`. Anything outside printable ASCII is dropped (a transliteration
/// table would need a dependency; the firmware cannot show it either way).
fn field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars() {
        let c = if c == ',' { ';' } else { c };
        if !(' '..='~').contains(&c) {
            continue;
        }
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// A built database and how much of the selection it holds.
pub(crate) struct Built {
    pub image: Vec<u8>,
    pub users: usize,
    /// Users past the 15 MB the firmware reads, lowest priority first to go.
    pub left_out: usize,
}

/// The database image: `<len>\n<lines>`, users only, sorted by ID, first
/// occurrence of an ID kept. Talkgroups are not callers and are left out —
/// their names come from the codeplug's own contact list.
///
/// `records` arrive in the library's priority order, so what does not fit in
/// the firmware's 15 MB is the end of that order, not the top of the ID range.
pub(crate) fn build(records: &[CallsignRecord]) -> Built {
    // Room for the header too: the whole image has to end inside the chip,
    // and "15728639\n" is 9 bytes.
    let budget = MAX_BODY - 16;
    let mut seen = std::collections::HashSet::new();
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut size = 0usize;
    let mut left_out = 0usize;
    for u in records.iter().filter(|r| !r.group_call) {
        if !seen.insert(u.dmr_id) {
            continue;
        }
        let name = field(&u.name);
        let nick = name.split(' ').next().unwrap_or("").to_string();
        let mut line = format!(
            "{},{},{},{},{},{},{}",
            u.dmr_id,
            field(&u.callsign),
            name,
            field(u.city.as_deref().unwrap_or("")),
            field(u.state.as_deref().unwrap_or("")),
            nick,
            field(u.country.as_deref().unwrap_or("")),
        );
        // A line past the firmware's read limit loses its tail fields; cut
        // it here instead, on a char boundary (it is ASCII).
        line.truncate(MAX_LINE - 1);
        if left_out > 0 || size + line.len() + 1 > budget {
            left_out += 1;
            continue;
        }
        size += line.len() + 1;
        lines.push((u.dmr_id, line));
    }
    lines.sort_by_key(|(id, _)| *id);
    let mut body = String::with_capacity(size);
    for (_, line) in &lines {
        body.push_str(line);
        body.push('\n');
    }
    Built { image: format!("{}\n{body}", body.len()).into_bytes(), users: lines.len(), left_out }
}

/// The 64 KiB blocks an image starting at [`BASE`] occupies. Every one is
/// at or above `BASE`, block-aligned, and inside the chip — asserted, because
/// an erase below `BASE` takes the codeplug with it.
pub(crate) fn blocks_for(len: usize) -> Vec<u32> {
    let n = (len as u32).div_ceil(BLOCK);
    let blocks: Vec<u32> = (0..n).map(|i| BASE + i * BLOCK).collect();
    for &b in &blocks {
        assert!(b >= BASE && b.is_multiple_of(BLOCK) && b + BLOCK <= FLASH_SIZE, "block {b:#x} out of bounds");
    }
    blocks
}

/// Write `image` at [`BASE`] and read it back. The radio must be in a
/// programming session already ([`protocol::begin`]).
///
/// Erase is md380tools' 0x03, which returns before the chip has finished — a
/// second erase sent while it is still busy is IGNORED. So each erase is given
/// time and its block read back as blank before the next goes out (editcp
/// waits 500 ms blind; this checks). Writes are md380tools' 0x04, 1 KiB at a
/// time, the last chunk padded with 0xFF as editcp does.
pub(crate) fn write(l: &mut dyn Link, image: &[u8]) -> Result<(), String> {
    let id = protocol::spi_flash_id(l)?;
    if !matches!(id, [0xEF, 0x40, 0x18] | [0x10, 0xDC, 0x01]) {
        return Err(format!(
            "This radio's flash chip ({}) is not the 16 MB part a user database needs — some \
             MD-380s have a 1 MB chip. Nothing was written.",
            protocol::hex(&id)
        ));
    }
    for block in blocks_for(image.len()) {
        erase(l, block)?;
    }
    for (i, chunk) in image.chunks(CHUNK).enumerate() {
        let mut data = chunk.to_vec();
        data.resize(CHUNK, 0xFF);
        protocol::spi_write(l, BASE + (i * CHUNK) as u32, &data)?;
    }
    verify(l, image)
}

/// Erase one block and see it blank: the first and last 1 KiB.
///
/// The firmware's erase command returns at once and does not wait for the
/// chip, so an erase that arrives while the previous one is still running is
/// dropped without a word. That is what stopped Tim's first run at 0x130000
/// (2026-10-10): three blocks erased, the fourth never did. So each attempt
/// waits editcp's 500 ms before looking, polls past the chip's 2 s worst case,
/// and a block still not blank gets its erase sent again. md380tools covers
/// the same hole by sending every block's erase sixteen times. Anything this
/// misses, the read-back at the end still catches.
fn erase(l: &mut dyn Link, block: u32) -> Result<(), String> {
    for _ in 0..3 {
        protocol::spi_erase_64k(l, block)?;
        l.sleep(Duration::from_millis(500));
        for _ in 0..10 {
            let first = protocol::spi_read(l, block)?;
            let last = protocol::spi_read(l, block + BLOCK - CHUNK as u32)?;
            if first.iter().chain(&last).all(|&b| b == 0xFF) {
                return Ok(());
            }
            l.sleep(Duration::from_millis(250));
        }
    }
    Err(format!(
        "The radio's flash block at {block:#x} would not erase. The codeplug is untouched; \
         reconnect the radio and run the write again."
    ))
}

/// Read the whole image back and compare. The tools this follows skip this.
fn verify(l: &mut dyn Link, image: &[u8]) -> Result<(), String> {
    for (i, chunk) in image.chunks(CHUNK).enumerate() {
        let addr = BASE + (i * CHUNK) as u32;
        let back = protocol::spi_read(l, addr)?;
        if back[..chunk.len()] != *chunk {
            return Err(format!(
                "The user database did not read back as written at {addr:#x}. The codeplug is \
                 untouched; run the write again."
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32, call: &str, name: &str, city: &str, group: bool) -> CallsignRecord {
        CallsignRecord {
            dmr_id: id,
            group_call: group,
            callsign: call.into(),
            name: name.into(),
            city: Some(city.into()),
            state: Some("Colorado".into()),
            country: Some("United States".into()),
            comment: None,
        }
    }

    #[test]
    fn the_image_is_a_sized_header_then_users_sorted_by_id() {
        let img = build(&[
            rec(3108176, "WW8L", "Tim  Annable", "Fort Collins, CO", false),
            rec(91, "", "Worldwide", "", true),
            rec(1023001, "K0ABC", "José Ruiz", "Denver", false),
            rec(3108176, "DUP", "Dup", "", false),
        ]);
        assert_eq!((img.users, img.left_out), (2, 0));
        let text = String::from_utf8(img.image).unwrap();
        let (head, body) = text.split_once('\n').unwrap();
        assert_eq!(head.parse::<usize>().unwrap(), body.len());
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines,
            [
                "1023001,K0ABC,Jos Ruiz,Denver,Colorado,Jos,United States",
                "3108176,WW8L,Tim Annable,Fort Collins; CO,Colorado,Tim,United States",
            ]
        );
    }

    /// The whole sequence against the simulated radio: the image lands at
    /// 0x100000, reads back, and nothing below 0x100000 — the codeplug — is
    /// erased or changed.
    /// Past 15 MB the END of the priority order is what goes, and the image
    /// still ends inside the chip.
    #[test]
    fn a_selection_past_fifteen_megabytes_drops_the_lowest_priority() {
        let long = "x".repeat(80);
        // Priority order runs DOWN the ID range, so a by-ID cut would keep the
        // wrong end.
        let records: Vec<_> = (0..200_000u32).rev().map(|i| rec(1_000_000 + i, "N0CALL", &long, "", false)).collect();
        let b = build(&records);
        assert!(b.left_out > 0 && b.users + b.left_out == 200_000);
        assert!(b.image.len() as u32 <= FLASH_SIZE - BASE);
        let _ = blocks_for(b.image.len());
        let text = String::from_utf8(b.image).unwrap();
        let first = text.lines().nth(1).unwrap();
        let kept_lowest = 1_000_000 + 200_000 - b.users as u32;
        assert!(first.starts_with(&format!("{kept_lowest},")), "{first}");
    }

    /// An erase the firmware dropped is sent again, and the write goes on.
    #[test]
    fn a_dropped_erase_is_sent_again() {
        use crate::radios::tyt_md380::layout::IMAGE_LEN;
        use crate::radios::tyt_md380::protocol::fake::FakeRadio;
        let mut r = FakeRadio::new(vec![0x5A; IMAGE_LEN]);
        r.ignore_erase = vec![0x13_0000];
        let records: Vec<_> = (0..20_000u32).map(|i| rec(1_000_000 + i, "N0CALL", "Some Operator", "Town", false)).collect();
        let img = build(&records).image;
        assert!(img.len() > 0x4_0000);
        write(&mut r, &img).unwrap();
        let erases: Vec<_> = r.log.iter().filter(|l| l.starts_with("erase 0x130000")).collect();
        assert_eq!(erases, ["erase 0x130000 ignored", "erase 0x130000"]);
        assert_eq!(&r.flash.as_ref().unwrap()[BASE as usize..][..img.len()], &img[..]);
    }

    #[test]
    fn a_write_lands_at_the_base_and_never_touches_the_codeplug() {
        use crate::radios::tyt_md380::layout::IMAGE_LEN;
        use crate::radios::tyt_md380::protocol::fake::FakeRadio;
        let codeplug: Vec<u8> = (0..IMAGE_LEN).map(|i| i as u8).collect();
        let mut r = FakeRadio::new(codeplug.clone());
        protocol::begin(&mut r).unwrap();
        let records: Vec<_> =
            (0..3000).map(|i| rec(3_100_000 + i, &format!("K{i}"), "Some Body", "Town", false)).collect();
        let img = build(&records).image;
        assert!(img.len() > 0x1_0000, "spans more than one block");
        write(&mut r, &img).unwrap();
        let flash = r.flash.as_ref().unwrap();
        assert_eq!(&flash[BASE as usize..BASE as usize + img.len()], img.as_slice());
        assert_eq!(r.image, codeplug, "the codeplug is untouched");
        assert!(r.log.iter().filter(|l| l.starts_with("erase")).all(|l| l != "erase 0x0"));
    }

    #[test]
    fn a_one_megabyte_radio_is_refused_before_anything_is_erased() {
        use crate::radios::tyt_md380::layout::IMAGE_LEN;
        use crate::radios::tyt_md380::protocol::fake::FakeRadio;
        let mut r = FakeRadio::new(vec![0; IMAGE_LEN]);
        r.flash_id = [0xEF, 0x40, 0x14];
        protocol::begin(&mut r).unwrap();
        let e = write(&mut r, &build(&[rec(1, "A", "B", "C", false)]).image).unwrap_err();
        assert!(e.contains("16 MB"), "{e}");
        assert!(!r.log.iter().any(|l| l.starts_with("erase")));
    }

    #[test]
    fn every_block_is_at_or_above_the_database_base() {
        assert_eq!(blocks_for(1), [0x10_0000]);
        assert_eq!(blocks_for(0x1_0001), [0x10_0000, 0x11_0000]);
        assert!(blocks_for(0x30_0000).iter().all(|&b| b >= BASE && b.is_multiple_of(0x1_0000)));
    }
}
