//! THROWAWAY (issue #42, step 7): the hardware ladder, one rung per run.
//!
//! WRITES TO A REAL MD-380. Each rung asserts its property on the image BEFORE
//! anything is sent, takes a fresh read as the backup first, and reads back in
//! the same session. Then a SECOND, fresh session reads again — a write that
//! only lived in a buffer would pass the first read-back and fail this one.
//!
//! ```sh
//! CPM_MD380_RUNG=identity CPM_MD380_DIR=../scratchpad/tyt_md380/ladder \
//!   cargo test --lib tyt_md380::hw_ladder -- --ignored --nocapture
//! ```
//!
//! Rungs: `identity` (the radio's own image back, unchanged), `onebyte`
//! (channel 1's name, last letter changed), `image` (write the file in
//! CPM_MD380_IMAGE, e.g. the dev export).

use super::layout::*;
use super::memory::Channel;
use super::protocol::{self, Link};

/// Logs every control transfer with its time, for working out which block a
/// write loses and what the radio said about it.
struct Tracing<'a> {
    inner: &'a mut dyn Link,
    t0: std::time::Instant,
    log: Vec<String>,
}

impl Link for Tracing<'_> {
    fn ctl_out(&mut self, request: u8, value: u16, data: &[u8]) -> Result<(), String> {
        let r = self.inner.ctl_out(request, value, data);
        let head: String = data.iter().take(5).map(|b| format!("{b:02x}")).collect();
        self.log.push(format!("{:9.3} OUT r{request} v{value} len{} {head} -> {r:?}", self.t0.elapsed().as_secs_f64(), data.len()));
        r
    }
    fn ctl_in(&mut self, request: u8, value: u16, len: u16) -> Result<Vec<u8>, String> {
        let r = self.inner.ctl_in(request, value, len);
        let shown = match &r {
            Ok(v) if v.len() <= 8 => format!("{:02x?}", v),
            Ok(v) => format!("{} bytes", v.len()),
            Err(e) => e.clone(),
        };
        self.log.push(format!("{:9.3} IN  r{request} v{value} -> {shown}", self.t0.elapsed().as_secs_f64()));
        r
    }
    fn sleep(&mut self, d: std::time::Duration) {
        self.log.push(format!("{:9.3} sleep {} ms", self.t0.elapsed().as_secs_f64(), d.as_millis()));
        std::thread::sleep(d);
    }
}

#[test]
#[ignore = "WRITES to a real MD-380"]
fn ladder_rung() {
    let rung = std::env::var("CPM_MD380_RUNG").expect("CPM_MD380_RUNG");
    let dir = std::path::PathBuf::from(std::env::var("CPM_MD380_DIR").expect("CPM_MD380_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");

    let mut link = protocol::open().expect("open");
    let l = &mut *link;
    let t0 = std::time::Instant::now();
    type Images = (Vec<u8>, Vec<u8>, Vec<u8>);
    let result = (|| -> Result<Images, String> {
        let id = protocol::begin(l)?;
        println!("ident {} {}-{} MHz", id.model, id.low_mhz, id.high_mhz);
        let base = protocol::read_image(l)?;
        std::fs::write(dir.join(format!("{stamp}-{rung}-backup.img")), &base).unwrap();
        let image = match rung.as_str() {
            "identity" => base.clone(),
            "onebyte" => {
                let mut img = base.clone();
                let at = at(CHANNELS, CHANNEL_LEN, 0);
                let mut c = Channel::decode(&img[at..at + CHANNEL_LEN]).expect("channel 1");
                let mut name: Vec<char> = c.name.chars().collect();
                let last = name.last_mut().expect("channel 1 has a name");
                *last = if *last == 'X' { 'Y' } else { 'X' };
                c.name = name.into_iter().collect();
                println!("channel 1 renamed to {:?}", c.name);
                img[at..at + CHANNEL_LEN].copy_from_slice(&c.encode());
                let diff: Vec<usize> = (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).collect();
                let name = at + 32..at + 34;
                if diff.is_empty() || diff.iter().any(|i| !name.contains(i)) {
                    return Err(format!("the edit is not one name unit: {diff:x?}"));
                }
                img
            }
            "probe" => {
                // Rung 4: a PROBE zone in free slots, the operator's channels untouched.
                use super::memory::{encode_freq, mhz_to_10hz, Admit, Bandwidth, Mode, Tone, Zone};
                let mut img = base.clone();
                let used = (0..CHANNEL_COUNT)
                    .filter(|&i| Channel::is_used(&img[at(CHANNELS, CHANNEL_LEN, i)..][..CHANNEL_LEN]))
                    .count();
                let probes = [400.000, 480.000, 399.990, 480.010, 446.000, 146.520];
                let mut members = [0u16; 16];
                for (k, mhz) in probes.iter().enumerate() {
                    let slot = used + k;
                    let at0 = at(CHANNELS, CHANNEL_LEN, slot);
                    assert!(!Channel::is_used(&img[at0..at0 + CHANNEL_LEN]), "slot {slot} is in use");
                    let mut t = Channel::template();
                    encode_freq(mhz_to_10hz(*mhz), &mut t[16..20]);
                    encode_freq(mhz_to_10hz(*mhz), &mut t[20..24]);
                    let mut c = Channel::decode(&t).unwrap();
                    c.name = format!("P{mhz:.3}");
                    c.mode = Mode::Analog;
                    c.bandwidth = Bandwidth::Wide;
                    c.high_power = false;
                    c.admit = Admit::Always;
                    c.rx_tone = Tone::NONE;
                    c.tx_tone = Tone::NONE;
                    c.slot = 1;
                    c.color_code = 1;
                    img[at0..at0 + CHANNEL_LEN].copy_from_slice(&c.encode());
                    members[k] = slot as u16 + 1;
                    println!("slot {} = {}", slot + 1, c.name);
                }
                let z = (0..ZONE_COUNT)
                    .find(|&i| !Zone::is_used(&img[at(ZONES, ZONE_LEN, i)..][..ZONE_LEN]))
                    .expect("a free zone");
                let zone = Zone { name: "PROBE".into(), members };
                img[at(ZONES, ZONE_LEN, z)..][..ZONE_LEN].copy_from_slice(&zone.encode());
                println!("zone {} = PROBE", z + 1);
                img
            }
            "image" => {
                let p = std::env::var("CPM_MD380_IMAGE").expect("CPM_MD380_IMAGE");
                let img = std::fs::read(&p).unwrap();
                assert_eq!(img.len(), IMAGE_LEN);
                img
            }
            other => panic!("unknown rung {other}"),
        };
        std::fs::write(dir.join(format!("{stamp}-{rung}-sent.img")), &image).unwrap();
        let w = std::time::Instant::now();
        let mut t = Tracing { inner: &mut *l, t0: w, log: Vec::new() };
        let r = protocol::write_image(&mut t, &image);
        std::fs::write(dir.join(format!("{stamp}-{rung}-trace.txt")), t.log.join("\n")).unwrap();
        r?;
        println!("write took {:.1} s", w.elapsed().as_secs_f64());
        let back = protocol::read_image(l)?;
        Ok((base, image, back))
    })();
    protocol::reboot(l);
    // Release the device before the verify session reopens it.
    drop(link);
    let (_base, sent, back) = result.expect("rung");
    let same = (0..IMAGE_LEN).filter(|&i| sent[i] != back[i]).count();
    println!("in-session read-back: {same} bytes differ ({:.1} s total)", t0.elapsed().as_secs_f64());

    let (verified, fresh, log) = super::verify_after_write(&sent).expect("verify");
    std::fs::write(dir.join(format!("{stamp}-{rung}-fresh.img")), &fresh).unwrap();
    for l in &log {
        println!("fresh-session {l}");
    }
    assert!(verified, "the radio does not hold what was sent");
    assert_eq!(same, 0, "in-session read-back differs");
}

/// Rung 3: a real codeplug from (a copy of) the dev database, through the
/// driver's own `CodeplugProgrammer::program` — the code the Program button
/// runs, backup and verify included.
///
/// ```sh
/// CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
/// CPM_CODEPLUG=1 CPM_MD380_DIR=../scratchpad/tyt_md380/ladder \
///   cargo test --lib tyt_md380::hw_ladder::ladder_program -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "WRITES to a real MD-380"]
async fn ladder_program() {
    use crate::radios::driver::CodeplugProgrammer;
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let dir = std::path::PathBuf::from(std::env::var("CPM_MD380_DIR").expect("CPM_MD380_DIR"));
    let copy = std::env::temp_dir().join("md380_ladder_program.sqlite3");
    std::fs::copy(&db, &copy).expect("copy dev db");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", copy.display())).await.unwrap();
    crate::seed::seed_radio_models(&pool).await.unwrap();
    let mut resolved = crate::commands::export::resolve_codeplug_payload(&pool, codeplug_id)
        .await
        .unwrap();
    resolved.model = sqlx::query_as("SELECT * FROM radio_models WHERE model = 'MD-380'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let report = tokio::task::spawn_blocking(move || {
        super::DRIVER.program("usb:tyt_md380", &resolved.payload(), &dir)
    })
    .await
    .unwrap()
    .expect("program");
    println!(
        "channels {} (cleared {}), zones {} (cleared {}), contacts {} (cleared {}), scan lists {} (cleared {})",
        report.channels_written,
        report.slots_cleared,
        report.zones_written,
        report.zones_cleared,
        report.contacts_written,
        report.contacts_cleared,
        report.scan_lists_written,
        report.scan_lists_cleared
    );
    println!("verified: {:?} — {}", report.verified, report.note);
    println!("backup {}", report.backup_path);
    println!("read back {} channels; skipped {}", report.channels.len(), report.skipped.len());
    for c in report.channels.iter().take(5) {
        println!("  {:3} {:<16} {:.5} {:?} {} {:?}", c.index, c.name, c.rx_mhz, c.shift, c.tone, c.mode);
    }
    assert_eq!(report.verified, Some(true));
}

/// Settings pass: write the profile-shaped JSON in CPM_MD380_SETTINGS through
/// the driver's own `SettingsWriter`, then print what the radio holds after.
///
/// ```sh
/// CPM_MD380_SETTINGS=../scratchpad/tyt_md380/settings-w1.json CPM_MD380_DIR=../scratchpad/tyt_md380/ladder \
///   cargo test --lib tyt_md380::hw_ladder::ladder_settings -- --ignored --nocapture
/// ```
#[test]
#[ignore = "WRITES to a real MD-380"]
fn ladder_settings() {
    use crate::radios::driver::SettingsWriter;
    let path = std::env::var("CPM_MD380_SETTINGS").expect("CPM_MD380_SETTINGS");
    let dir = std::path::PathBuf::from(std::env::var("CPM_MD380_DIR").expect("CPM_MD380_DIR"));
    let want: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let report = super::DRIVER.write_settings("usb:tyt_md380", &want, "", &dir).expect("write");
    println!("fields {} verified {:?} note {:?}", report.fields_written, report.verified, report.note);
    println!("backup {}", report.backup_path);
    protocol::wait_for_radio().expect("back");
    let got = super::settings::decode(&{
        let mut l = protocol::open().unwrap();
        let l = &mut *l;
        let r = protocol::begin(l).and_then(|_| protocol::read_image(l));
        protocol::reboot(l);
        r.unwrap()
    });
    for (k, v) in want.as_object().unwrap() {
        let mark = if &got[k] == v { "ok " } else { "MISMATCH" };
        println!("{mark} {k:45} {v}  (radio: {})", got[k]);
    }
}

/// Rung 4b: replace one channel slot (CPM_MD380_SLOT, 1-based) with a 2 m
/// repeater test channel — RX 145.115, TX 144.515, 100.0 Hz encode only, low
/// power — to see whether a UHF MD-380 reaches a 2 m repeater at all.
#[test]
#[ignore = "WRITES to a real MD-380"]
fn ladder_vhf_repeater_channel() {
    use super::memory::{encode_freq, mhz_to_10hz, Bandwidth, Mode, Tone};
    let slot: usize = std::env::var("CPM_MD380_SLOT").expect("CPM_MD380_SLOT").parse().unwrap();
    let dir = std::path::PathBuf::from(std::env::var("CPM_MD380_DIR").expect("CPM_MD380_DIR"));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = super::session(|link, _| protocol::read_image(link)).expect("read");
    std::fs::write(dir.join(format!("{stamp}-vhf-backup.img")), &base).unwrap();
    let at0 = at(CHANNELS, CHANNEL_LEN, slot - 1);
    let mut t = Channel::template();
    encode_freq(mhz_to_10hz(145.115), &mut t[16..20]);
    encode_freq(mhz_to_10hz(144.515), &mut t[20..24]);
    let mut c = Channel::decode(&t).unwrap();
    c.name = "P145.115 RPT".into();
    c.mode = Mode::Analog;
    c.bandwidth = Bandwidth::Wide;
    c.high_power = false;
    c.slot = 1;
    c.color_code = 1;
    c.tx_tone = Tone::Ctcss(1000);
    c.rx_tone = Tone::NONE;
    let mut img = base.clone();
    img[at0..at0 + CHANNEL_LEN].copy_from_slice(&c.encode());
    let diff: Vec<usize> = (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).collect();
    assert!(diff.iter().all(|&i| (at0..at0 + CHANNEL_LEN).contains(&i)), "only the one slot changes");
    protocol::wait_for_radio().unwrap();
    super::session(|link, _| protocol::write_image(link, &img)).expect("write");
    let (ok, _, log) = super::verify_after_write(&img).expect("verify");
    for l in &log {
        println!("fresh-session {l}");
    }
    assert!(ok, "verify");
    println!("slot {slot} = {} RX 145.115 TX 144.515 T100.0 low", c.name);
}

/// Identify must not touch the radio: descriptor only.
#[test]
#[ignore = "needs a real MD-380 on USB"]
fn identify_is_descriptor_only() {
    use crate::radios::driver::RadioDriver;
    let t = std::time::Instant::now();
    let id = super::DRIVER.identify("usb:tyt_md380").expect("identify");
    println!("identify: {:?} in {:.2}s", id.ident_ascii, t.elapsed().as_secs_f64());
}
