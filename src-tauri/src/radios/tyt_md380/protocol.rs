//! TYT's codeplug protocol over USB DFU, as the MD-380 speaks it.
//!
//! The radio has no serial port. Switched on normally with the cable in, the
//! running firmware enumerates as an STM32 DFU device (0483:DF11) and the
//! codeplug — a 256 KiB SPI-flash image — moves over DFU class control
//! requests. The sequence is dmrconfig's (`dfu-libusb.c` + `md380.c`, BSD-3),
//! cross-checked against md380tools and qdmr.
//!
//! What has been run against Tim's radio and what has not (s136, 2026-10-09):
//!
//! - **Identify + read** — measured, twice, through `scratchpad/tyt_md380/
//!   md380_read.py`, which sends exactly the commands below. 0.9 s per read;
//!   two reads 30 s apart differed in zero bytes. The reboot at the end drops
//!   the device (its GETSTATUS fails, as expected) and the radio re-enumerates
//!   on its own, so a second session needs no hands.
//! - **Write** — NOT run against a radio. dmrconfig's erase + block sequence,
//!   checked against the fake below; the hardware ladder is what proves it.
//!
//! ## Safety
//!
//! A write erases before it writes, so an interrupted one leaves a blank or
//! partial codeplug; the caller takes a backup first. The brick path is the
//! FIRMWARE, reached by `91 31` in the bootloader. This module has no way to
//! send that, and every erase and block number it can produce lies inside the
//! 256 KiB codeplug — see [`ERASE_BASES`] and the asserts in [`write_image`].

use std::time::Duration;

use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use nusb::MaybeFuture;

use super::layout::IMAGE_LEN;

pub(crate) const VID: u16 = 0x0483;
pub(crate) const PID: u16 = 0xDF11;

/// The model token the ident reply starts with. `"MD390"` and the dual-band
/// models share the VID:PID and are refused — this driver knows one layout.
pub(crate) const MODEL: &str = "DR780";

const DNLOAD: u8 = 1;
const UPLOAD: u8 = 2;
const GETSTATUS: u8 = 3;
const CLRSTATUS: u8 = 4;
const GETSTATE: u8 = 5;
const ABORT: u8 = 6;
const DETACH: u8 = 0;

const STATE_APP_IDLE: u8 = 0;
const STATE_APP_DETACH: u8 = 1;
const STATE_DFU_IDLE: u8 = 2;
const STATE_DNBUSY: u8 = 4;
const STATE_MANIFEST_WAIT_RESET: u8 = 8;
const STATE_DNLOAD_SYNC: u8 = 3;
const STATE_DNLOAD_IDLE: u8 = 5;
#[cfg(test)]
const STATE_UPLOAD_IDLE: u8 = 9;
const STATE_ERROR: u8 = 10;

const BLOCK: usize = 1024;
const BLOCKS: usize = IMAGE_LEN / BLOCK;
/// The four 64 KiB sectors that hold the codeplug, and nothing else.
pub(crate) const ERASE_BASES: [u32; 4] = [0x0_0000, 0x1_0000, 0x2_0000, 0x3_0000];

const TIMEOUT: Duration = Duration::from_millis(5000);

/// The radio's control endpoint, abstracted so the sequencing can be tested
/// against a fake without a radio.
pub(crate) trait Link {
    fn ctl_out(&mut self, request: u8, value: u16, data: &[u8]) -> Result<(), String>;
    fn ctl_in(&mut self, request: u8, value: u16, len: u16) -> Result<Vec<u8>, String>;
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The real radio, through nusb. macOS needs no driver; Linux needs read/write
/// access to the device node (a udev rule).
pub(crate) struct UsbLink {
    iface: nusb::Interface,
}

impl Link for UsbLink {
    fn ctl_out(&mut self, request: u8, value: u16, data: &[u8]) -> Result<(), String> {
        self.iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request,
                    value,
                    index: 0,
                    data,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(|e| format!("USB request {request} failed: {e}"))
    }

    fn ctl_in(&mut self, request: u8, value: u16, len: u16) -> Result<Vec<u8>, String> {
        self.iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request,
                    value,
                    index: 0,
                    length: len,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(|e| format!("USB request {request} failed: {e}"))
    }
}

/// Find the radio and open it through whatever driver the system already has.
///
/// On Windows that is first ST's STTub30 (installed by TYT's CPS; see
/// `stt.rs`), then WinUSB through nusb. Elsewhere it is nusb: macOS needs no
/// driver, Linux needs read/write access to the device node. The app never
/// installs or changes a driver; it says which one is missing and stops.
pub(crate) fn open() -> Result<Box<dyn Link + Send>, String> {
    #[cfg(windows)]
    if let Some(link) = super::stt::open()? {
        return Ok(Box::new(link));
    }
    open_usb().map(|l| Box::new(l) as Box<dyn Link + Send>)
}

/// Refuse, from the USB descriptor alone and before any TYT command is sent, a
/// 0483:DF11 device that is not a radio in normal mode. The ID is ST's generic
/// DFU id, shared by every STM32 in its ROM bootloader and by the radio's own
/// firmware-update mode — and the brick path runs through the latter.
///
/// md380tools tells the modes apart the same way: manufacturer "AnyRoad
/// Technology" is the TYT bootloader (`md380_dfu.py`). ST's ROM bootloader
/// calls itself "STM32  BOOTLOADER". Windows does not cache the manufacturer
/// string (nusb), so there only the product string is checked.
pub(crate) fn check_descriptor(manufacturer: Option<&str>, product: Option<&str>) -> Result<(), String> {
    if manufacturer.is_some_and(|m| m.trim() == "AnyRoad Technology") {
        return Err("The MD-380 is in firmware-update mode (its LED flashes red and green). \
                    Switch it off, then on normally without holding any buttons. Nothing was \
                    sent to it."
            .into());
    }
    if product.is_some_and(|p| p.to_ascii_uppercase().contains("BOOTLOADER")) {
        return Err(format!(
            "The USB device 0483:DF11 here is an STM32 bootloader ({}), not an MD-380. \
             Nothing was sent to it.",
            product.unwrap_or_default().trim()
        ));
    }
    Ok(())
}

fn find_usb() -> Result<nusb::DeviceInfo, String> {
    let info = nusb::list_devices()
        .wait()
        .map_err(|e| format!("could not list USB devices: {e}"))?
        .find(|d| d.vendor_id() == VID && d.product_id() == PID)
        .ok_or_else(|| {
            "No MD-380 found on USB. Connect the programming cable and switch the radio \
             on normally — not in firmware-update mode (PTT + the top side button at \
             power-on), which it must never be in for programming."
                .to_string()
        })?;
    check_descriptor(info.manufacturer_string(), info.product_string())?;
    Ok(info)
}

/// What is on the bus, without opening it or sending the radio anything. The
/// radio's ident needs programming mode, which ends in a restart, so Identify
/// stops at the descriptor; the model and band are checked by every read and
/// write instead.
pub(crate) fn probe() -> Result<String, String> {
    #[cfg(windows)]
    if super::stt::present() {
        return Ok("MD-380 found through TYT's USB driver".into());
    }
    let info = find_usb()?;
    Ok(format!(
        "MD-380 found on USB ({})",
        info.product_string().unwrap_or("no product name").trim()
    ))
}

fn open_usb() -> Result<UsbLink, String> {
    let info = find_usb()?;
    let device = info.open().wait().map_err(|e| no_driver(&e.to_string()))?;
    let iface = device
        .detach_and_claim_interface(0)
        .wait()
        .map_err(|e| no_driver(&e.to_string()))?;
    Ok(UsbLink { iface })
}

/// The radio is on the bus but cannot be opened: on Windows that means no
/// usable driver, on Linux usually no permission on the device node.
fn no_driver(e: &str) -> String {
    if cfg!(windows) {
        format!(
            "The MD-380 is connected, but Windows has no driver installed for it that this \
             app can use. TYT's CPS installs one. ({e})"
        )
    } else if cfg!(target_os = "linux") {
        format!(
            "The MD-380 is connected, but this user cannot open it. Linux needs read/write \
             access to the radio's USB device (0483:df11), usually granted by a udev rule. ({e})"
        )
    } else {
        format!("could not open the MD-380's USB device: {e}")
    }
}

/// What the radio says it is. The ident reply is 32 bytes and is, byte for
/// byte, the 0x125..0x145 block TYT's CPS writes into an `.rdt` header —
/// measured on Tim's radio: `"DR780"`, then range index 2 and 400.0 / 480.0.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ident {
    pub raw: Vec<u8>,
    pub model: String,
    /// The CPS frequency-range index: 0 = 136-174, 1 = 350-400, 2 = 400-480,
    /// 3 = 450-520.
    pub range_index: u8,
    pub low_mhz: f64,
    pub high_mhz: f64,
}

/// Two bytes of little-endian BCD in 100 kHz units (`00 40` = 400.0 MHz).
fn bcd_100khz(lo: u8, hi: u8) -> Option<f64> {
    let digits = [hi >> 4, hi & 0xF, lo >> 4, lo & 0xF];
    if digits.iter().any(|&d| d > 9) {
        return None;
    }
    let n = digits.iter().fold(0u32, |a, &d| a * 10 + d as u32);
    Some(n as f64 / 10.0)
}

pub(crate) fn parse_ident(raw: &[u8]) -> Result<Ident, String> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len()).min(16);
    let model = String::from_utf8_lossy(&raw[..end]).into_owned();
    if model != MODEL {
        return Err(format!(
            "This radio identifies as {model:?}, not an MD-380 ({MODEL:?}). The MD-390 \
             and the dual-band TYT models share its USB id but not its memory layout, \
             so this driver will not touch it."
        ));
    }
    if raw.len() < 0x18 {
        return Err(format!("the MD-380's ident reply was only {} bytes", raw.len()));
    }
    let range_index = raw[0x11];
    let (low_mhz, high_mhz) = match (bcd_100khz(raw[0x14], raw[0x15]), bcd_100khz(raw[0x16], raw[0x17])) {
        (Some(l), Some(h)) if l < h => (l, h),
        _ => {
            return Err(format!(
                "the MD-380 reported a band this driver cannot read: {}",
                hex(&raw[0x10..0x18])
            ))
        }
    };
    Ok(Ident { raw: raw.to_vec(), model, range_index, low_mhz, high_mhz })
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn get_state(l: &mut dyn Link) -> Result<u8, String> {
    l.ctl_in(GETSTATE, 0, 1)?
        .first()
        .copied()
        .ok_or_else(|| "the MD-380 returned an empty DFU state".to_string())
}

fn get_status(l: &mut dyn Link) -> Result<Vec<u8>, String> {
    l.ctl_in(GETSTATUS, 0, 6)
}

/// Drive the DFU state machine back to dfuIDLE (dmrconfig `wait_idle`).
fn wait_idle(l: &mut dyn Link) -> Result<(), String> {
    for _ in 0..100 {
        match get_state(l)? {
            STATE_DFU_IDLE => return Ok(()),
            STATE_APP_IDLE => l.ctl_out(DETACH, 1000, &[])?,
            STATE_ERROR => l.ctl_out(CLRSTATUS, 0, &[])?,
            STATE_APP_DETACH | STATE_DNBUSY | STATE_MANIFEST_WAIT_RESET => {
                l.sleep(Duration::from_millis(100))
            }
            _ => l.ctl_out(ABORT, 0, &[])?,
        }
    }
    Err("the MD-380 never returned to DFU idle".into())
}

/// After a DNLOAD, ask for status until the device reports the request DONE
/// (dfuDNLOAD_IDLE), sleeping the poll timeout it asks for between asks.
///
/// This is md380tools' order and the DFU spec's: a request is committed by the
/// GETSTATUS that follows its busy period, and an ABORT before that may
/// discard it. dmrconfig instead ABORTs any state that is not idle.
///
/// ⚠ Not proven necessary. It went in when ladder rung 1 (s136) seemed to lose
/// a block under dmrconfig's order; the "lost" block turned out to be the
/// verify read's (see `verify_after_write` in mod.rs). It stays because it is
/// the order two tools and the spec agree on, and it costs nothing measurable.
fn wait_dnload_idle(l: &mut dyn Link) -> Result<(), String> {
    for _ in 0..200 {
        let st = get_status(l)?;
        if st.len() < 6 {
            return Err(format!("the MD-380 returned a {}-byte DFU status", st.len()));
        }
        match st[4] {
            STATE_DNLOAD_IDLE => return Ok(()),
            STATE_ERROR => {
                return Err(format!("the MD-380 reported a DFU error (status {})", st[0]))
            }
            STATE_DNLOAD_SYNC | STATE_DNBUSY => {
                let poll = u32::from_le_bytes([st[1], st[2], st[3], 0]);
                l.sleep(Duration::from_millis(poll.clamp(1, 1000) as u64));
            }
            other => return Err(format!("the MD-380 went to DFU state {other} mid-write")),
        }
    }
    Err("the MD-380 never finished a write request".into())
}

/// A DNLOAD to block 0 — how TYT's commands, address and erase requests go.
fn command(l: &mut dyn Link, data: &[u8]) -> Result<(), String> {
    l.ctl_out(DNLOAD, 0, data)?;
    wait_dnload_idle(l)?;
    wait_idle(l)
}

fn set_address(l: &mut dyn Link, addr: u32) -> Result<(), String> {
    let a = addr.to_le_bytes();
    command(l, &[0x21, a[0], a[1], a[2], a[3]])
}

/// Enter programming mode and identify. The radio shows "PC Program USB Mode"
/// from here until [`reboot`].
pub(crate) fn begin(l: &mut dyn Link) -> Result<Ident, String> {
    wait_idle(l)?;
    command(l, &[0x91, 0x01])?;
    command(l, &[0xA2, 0x01])?;
    let raw = l.ctl_in(UPLOAD, 0, 64)?;
    get_status(l)?;
    wait_idle(l)?;
    parse_ident(&raw)
}

/// Read the whole codeplug image. Call after [`begin`].
pub(crate) fn read_image(l: &mut dyn Link) -> Result<Vec<u8>, String> {
    set_address(l, 0)?;
    let mut image = Vec::with_capacity(IMAGE_LEN);
    for b in 0..BLOCKS {
        let blk = l.ctl_in(UPLOAD, (b + 2) as u16, BLOCK as u16)?;
        if blk.len() != BLOCK {
            return Err(format!(
                "the MD-380 sent {} bytes for block {b}, expected {BLOCK}",
                blk.len()
            ));
        }
        get_status(l)?;
        image.extend_from_slice(&blk);
    }
    Ok(image)
}

/// Erase and rewrite the whole codeplug. Call after [`begin`]. Not yet run on
/// hardware (see the module header).
pub(crate) fn write_image(l: &mut dyn Link, image: &[u8]) -> Result<(), String> {
    write_sectors(l, image, &ERASE_BASES)
}

/// The 64 KiB sectors whose bytes differ between two images — what a write of
/// `after` over a radio holding `before` actually has to erase and rewrite.
pub(crate) fn changed_sectors(before: &[u8], after: &[u8]) -> Vec<u32> {
    ERASE_BASES
        .into_iter()
        .filter(|&base| {
            let r = base as usize..base as usize + 0x10000;
            before[r.clone()] != after[r]
        })
        .collect()
}

/// Erase and rewrite only `sectors` (bases from [`ERASE_BASES`]) of `image`;
/// every other byte of the radio's codeplug is left exactly as it is.
///
/// ⚠ Block numbers are absolute from address 0 (block b = image byte b×1024),
/// so a sector-1 write sends blocks 64-127 without 0-63 first.
///
/// Hardware (s136): a sector-0-only write — every settings write — ran twice
/// on Tim's radio, each verified, and left sectors 1-3 byte-identical. A write
/// that STARTS past sector 0 has not run on hardware; nothing sends one yet.
pub(crate) fn write_sectors(l: &mut dyn Link, image: &[u8], sectors: &[u32]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "refusing to write a {}-byte image; the MD-380's codeplug is {IMAGE_LEN}",
            image.len()
        ));
    }
    // After a read the device sits in dfuUPLOAD_IDLE, where a DNLOAD stalls.
    // Measured on Tim's radio (ladder rung 1, s136): "endpoint stalled" on the
    // `91 01` below, before any erase — dmrconfig's `dfu_erase` opens with this
    // same GETSTATUS + wait-for-idle, which the first port of it left out.
    get_status(l)?;
    wait_idle(l)?;
    // TYT's CPS prelude, as md380tools and editcp send it (captured from the
    // CPS; the commands are undocumented). dmrconfig omits it. ⚠ Not proven
    // necessary either: it was tried against the same verify-read artefact.
    // Kept because the CPS, md380tools and editcp all send it before a write.
    command(l, &[0x91, 0x01])?;
    command(l, &[0x91, 0x01])?;
    command(l, &[0xA2, 0x02])?;
    l.sleep(Duration::from_millis(2000));
    for b in [0x02, 0x03, 0x04, 0x07] {
        command(l, &[0xA2, b])?;
    }
    for &base in sectors {
        assert!(ERASE_BASES.contains(&base), "erase outside the codeplug's sectors");
        let a = base.to_le_bytes();
        command(l, &[0x41, a[0], a[1], a[2], a[3]])?;
    }
    set_address(l, 0)?;
    for (b, chunk) in image.chunks(BLOCK).enumerate() {
        assert!(b < BLOCKS, "block outside the codeplug");
        if !sectors.contains(&((b * BLOCK) as u32 & !0xFFFF)) {
            continue;
        }
        l.ctl_out(DNLOAD, (b + 2) as u16, chunk)?;
        wait_dnload_idle(l)?;
    }
    wait_idle(l)
}

/// Wait for the radio to come back on the bus after [`reboot`]. Measured s136:
/// it re-enumerates on its own within a few seconds, on the Mac and on Windows.
pub(crate) fn wait_for_radio() -> Result<(), String> {
    // Give it time to drop off first, or this finds the old device.
    std::thread::sleep(Duration::from_millis(1500));
    for _ in 0..40 {
        if open().is_ok() {
            std::thread::sleep(Duration::from_millis(1000));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("the MD-380 did not come back on USB within 20 seconds of restarting".into())
}

/// Leave programming mode. The radio restarts and drops off the bus, so the
/// status read after the command failing is the expected outcome.
pub(crate) fn reboot(l: &mut dyn Link) {
    if wait_idle(l).is_ok() && l.ctl_out(DNLOAD, 0, &[0x91, 0x05]).is_ok() {
        let _ = get_status(l);
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// A radio that answers like Tim's did, records every request, and holds an
    /// image that writes land in. It follows the DFU spec's download states: a
    /// DNLOAD leaves the request PENDING (dfuDNLOAD_SYNC); the first GETSTATUS
    /// moves it to dfuDNBUSY; it is committed only by a GETSTATUS after that.
    /// GETSTATE during the busy period reports dfuDNLOAD_SYNC (the busy time
    /// has passed, the host has not asked yet), and an ABORT there DISCARDS the
    /// pending request — which is how a block went missing on the real radio.
    pub(crate) struct FakeRadio {
        pub ident: Vec<u8>,
        pub image: Vec<u8>,
        pub log: Vec<String>,
        state: u8,
        addr_block: Option<u32>,
        last_cmd: Vec<u8>,
        /// (wValue, data, busy seen)
        pending: Option<(u16, Vec<u8>, bool)>,
    }

    /// Tim's MD-380's ident reply, as measured s136.
    pub(crate) const TIM_IDENT: &str =
        "445237383000ffffffffffffffffffff200200330040004808888888fffdffff";

    impl FakeRadio {
        pub(crate) fn new(image: Vec<u8>) -> Self {
            let ident = (0..TIM_IDENT.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&TIM_IDENT[i..i + 2], 16).unwrap())
                .collect();
            Self {
                ident,
                image,
                log: vec![],
                state: STATE_DFU_IDLE,
                addr_block: None,
                last_cmd: vec![],
                pending: None,
            }
        }

        fn commit(&mut self, value: u16, data: Vec<u8>) -> Result<(), String> {
            if value == 0 {
                self.log.push(format!("cmd {}", hex(&data)));
                match data.as_slice() {
                    [0x41, a @ ..] => {
                        let base = u32::from_le_bytes(a.try_into().unwrap()) as usize;
                        self.image[base..base + 0x10000].fill(0xFF);
                    }
                    [0x21, a @ ..] => {
                        self.addr_block = Some(u32::from_le_bytes(a.try_into().unwrap()))
                    }
                    _ => {}
                }
                self.last_cmd = data;
            } else {
                let base = self.addr_block.ok_or("write before set_address")? as usize;
                let off = base + (value as usize - 2) * BLOCK;
                self.image[off..off + data.len()].copy_from_slice(&data);
                self.log.push(format!("write block {}", value - 2));
            }
            Ok(())
        }
    }

    impl Link for FakeRadio {
        fn ctl_out(&mut self, request: u8, value: u16, data: &[u8]) -> Result<(), String> {
            match request {
                DNLOAD => {
                    // The real device refuses a DNLOAD outside idle states
                    // (s136: "endpoint stalled" straight after a read).
                    if !matches!(self.state, STATE_DFU_IDLE | STATE_DNLOAD_IDLE) {
                        self.log.push(format!("STALL dnload in state {}", self.state));
                        return Err("USB request 1 failed: endpoint stalled".into());
                    }
                    self.pending = Some((value, data.to_vec(), false));
                    self.state = STATE_DNLOAD_SYNC;
                }
                ABORT | CLRSTATUS | DETACH => {
                    if let Some((v, d, _)) = self.pending.take() {
                        self.log.push(format!(
                            "DROPPED {}",
                            if v == 0 { format!("cmd {}", hex(&d)) } else { format!("block {}", v - 2) }
                        ));
                    }
                    self.state = STATE_DFU_IDLE;
                }
                _ => return Err(format!("unexpected OUT {request}/{value}")),
            }
            Ok(())
        }

        fn ctl_in(&mut self, request: u8, value: u16, len: u16) -> Result<Vec<u8>, String> {
            match request {
                GETSTATE => {
                    if self.state == STATE_DNBUSY {
                        self.state = STATE_DNLOAD_SYNC;
                    }
                    Ok(vec![self.state])
                }
                GETSTATUS => {
                    if let Some((v, d, busy)) = self.pending.take() {
                        if busy {
                            self.commit(v, d)?;
                            self.state = STATE_DNLOAD_IDLE;
                        } else {
                            self.pending = Some((v, d, true));
                            self.state = STATE_DNBUSY;
                        }
                    }
                    Ok(vec![0, 5, 0, 0, self.state, 0])
                }
                UPLOAD if value == 0 && self.last_cmd == [0xA2, 0x01] => {
                    self.state = STATE_UPLOAD_IDLE;
                    Ok(self.ident.clone())
                }
                UPLOAD if value >= 2 => {
                    self.state = STATE_UPLOAD_IDLE;
                    let off = (value as usize - 2) * BLOCK;
                    Ok(self.image[off..off + len as usize].to_vec())
                }
                _ => Err(format!("unexpected IN {request}/{value}")),
            }
        }

        fn sleep(&mut self, _: Duration) {}
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRadio;
    use super::*;

    #[test]
    fn a_bootloader_is_refused_by_its_descriptor() {
        assert!(check_descriptor(None, Some("Patched MD380")).is_ok());
        assert!(check_descriptor(None, Some("Digital Radio in USB mode")).is_ok());
        let e = check_descriptor(Some("AnyRoad Technology"), Some("x")).unwrap_err();
        assert!(e.contains("firmware-update mode"), "{e}");
        assert!(check_descriptor(Some("STMicroelectronics"), Some("STM32  BOOTLOADER")).is_err());
    }

    #[test]
    fn tims_ident_reads_as_a_uhf_md380() {
        let mut r = FakeRadio::new(vec![0xFF; IMAGE_LEN]);
        let id = begin(&mut r).unwrap();
        assert_eq!(id.model, "DR780");
        assert_eq!(id.range_index, 2);
        assert_eq!((id.low_mhz, id.high_mhz), (400.0, 480.0));
    }

    #[test]
    fn an_md390_is_refused_before_any_memory_is_read() {
        let mut r = FakeRadio::new(vec![0xFF; IMAGE_LEN]);
        r.ident[..5].copy_from_slice(b"MD390");
        let e = begin(&mut r).unwrap_err();
        assert!(e.contains("MD390"), "{e}");
        assert!(!r.log.iter().any(|l| l.starts_with("cmd 21")), "{:?}", r.log);
    }

    #[test]
    fn read_returns_the_whole_image_in_order() {
        let image: Vec<u8> = (0..IMAGE_LEN).map(|i| (i / BLOCK) as u8 ^ i as u8).collect();
        let mut r = FakeRadio::new(image.clone());
        begin(&mut r).unwrap();
        assert_eq!(read_image(&mut r).unwrap(), image);
    }

    #[test]
    fn write_erases_only_the_codeplug_then_writes_every_block() {
        let mut r = FakeRadio::new(vec![0x00; IMAGE_LEN]);
        begin(&mut r).unwrap();
        // The real flow reads the backup first, which leaves the device in
        // dfuUPLOAD_IDLE — the state the first hardware write stalled in.
        read_image(&mut r).unwrap();
        let image: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 7) as u8).collect();
        write_image(&mut r, &image).unwrap();
        assert_eq!(r.image, image);
        let erases: Vec<_> = r.log.iter().filter(|l| l.starts_with("cmd 41")).collect();
        assert_eq!(erases, ["cmd 4100000000", "cmd 4100000100", "cmd 4100000200", "cmd 4100000300"]);
        assert_eq!(r.log.iter().filter(|l| l.starts_with("write block")).count(), BLOCKS);
        // Nothing that could reach the firmware.
        assert!(!r.log.iter().any(|l| l.starts_with("cmd 9131")), "{:?}", r.log);
        assert!(!r.log.iter().any(|l| l.starts_with("STALL")), "{:?}", r.log);
        assert!(!r.log.iter().any(|l| l.starts_with("DROPPED")), "{:?}", r.log);
        // …and the image reads back in the same session.
        assert_eq!(read_image(&mut r).unwrap(), image);
    }

    /// A settings-sized change erases and rewrites sector 0 alone; the channel
    /// and contact tables in sectors 1-3 are never erased.
    #[test]
    fn a_sector_write_touches_only_its_sector() {
        let old: Vec<u8> = (0..IMAGE_LEN).map(|i| (i * 3) as u8).collect();
        let mut r = FakeRadio::new(old.clone());
        begin(&mut r).unwrap();
        read_image(&mut r).unwrap();
        let mut new = old.clone();
        new[0x2095] ^= 0xFF; // a General Settings byte
        let sectors = changed_sectors(&old, &new);
        assert_eq!(sectors, [0]);
        write_sectors(&mut r, &new, &sectors).unwrap();
        assert_eq!(r.image, new);
        let erases: Vec<_> = r.log.iter().filter(|l| l.starts_with("cmd 41")).collect();
        assert_eq!(erases, ["cmd 4100000000"]);
        assert_eq!(r.log.iter().filter(|l| l.starts_with("write block")).count(), 64);
    }

    #[test]
    fn a_short_image_is_refused_before_anything_is_erased() {
        let mut r = FakeRadio::new(vec![0x00; IMAGE_LEN]);
        begin(&mut r).unwrap();
        assert!(write_image(&mut r, &[0u8; 1000]).is_err());
        assert!(!r.log.iter().any(|l| l.starts_with("cmd 41")));
    }
}
