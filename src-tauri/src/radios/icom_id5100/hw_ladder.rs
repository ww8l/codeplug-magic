//! THROWAWAY (issue #49, Phase 5): the hardware ladder's write steps, run
//! through the driver's own protocol functions.
//!
//! Every step: read the radio and save that read as the backup, build the file
//! to write and ASSERT its property before a byte goes out, upload, read back,
//! and compare. One process per step.
//!
//! - `identity` — upload the read unchanged. Proves the upload path.
//! - `rename`   — change one memory's name (`CPM_ID5100_SLOT`, default 0) to
//!   `LADDER STEP 2`. Proves a changed image is taken.
//! - `file`     — upload `CPM_ID5100_FILE` (a built codeplug, or a backup to
//!   restore). Only the memory area and bank tables may differ from the fresh
//!   read, or the step refuses.
//!
//! ```sh
//! CPM_ID5100_PORT=/dev/cu.usbserial-RT1SQ1OS CPM_ID5100_STEP=identity \
//! CPM_ID5100_DIR=../scratchpad/id5100 \
//!   cargo test --lib icom_id5100::hw_ladder -- --ignored --nocapture
//! ```

use super::layout::{BANK_NAMES, BANK_NAME_LEN, BANK_COUNT, IMAGE_LEN, NAME_LEN, VOLATILE_BASE};
use super::memory::{is_used, read_record, store, read_bank};
use super::protocol::{download, identify, open_port, upload};

fn read(port: &str) -> Vec<u8> {
    let mut p = open_port(port).unwrap();
    identify(&mut *p).unwrap();
    download(&mut *p).unwrap()
}

#[test]
#[ignore = "WRITES to a real ID-5100 on CPM_ID5100_PORT"]
fn ladder_step() {
    let port = std::env::var("CPM_ID5100_PORT").expect("CPM_ID5100_PORT");
    let step = std::env::var("CPM_ID5100_STEP").expect("CPM_ID5100_STEP");
    let dir = std::path::PathBuf::from(std::env::var("CPM_ID5100_DIR").expect("CPM_ID5100_DIR"));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");

    let base = read(&port);
    let backup = dir.join(format!("id5100_ladder_{step}_before_{stamp}.img"));
    std::fs::write(&backup, &base).unwrap();
    println!("backup: {}", backup.display());

    let built = match step.as_str() {
        "identity" => base.clone(),
        "rename" => {
            let slot: usize = std::env::var("CPM_ID5100_SLOT").map(|s| s.parse().unwrap()).unwrap_or(0);
            assert!(is_used(&base, slot), "memory {slot} is empty; pick a used one");
            let mut rec = read_record(&base, slot);
            println!("memory {slot}: {:?} -> \"LADDER STEP 2\"", rec.name());
            rec.name = [b' '; NAME_LEN];
            rec.name[..13].copy_from_slice(b"LADDER STEP 2");
            let mut img = base.clone();
            store(&mut img, slot, &rec, read_bank(&base, slot));
            // The property, asserted before the write: exactly the name bytes
            // moved (a skipped memory also loses its skip bit via `store`).
            let moved: Vec<usize> = (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).collect();
            println!("bytes changed: {}", moved.len());
            assert!(moved.len() <= NAME_LEN + 1 && !moved.is_empty());
            img
        }
        "bandprobe" => {
            // Memories 990-999 in bank Z "BAND PROBE": the manual's coverage
            // edges, and one step past each. Built by the driver's own encoder
            // — the app's `rx_bands` would never let the outside ones through,
            // which is exactly why they have to be asked of the radio directly.
            let probes: [(f64, &str, &str); 10] = [
                (118.000, "AM", "P1 118.000 AM"),
                (136.990, "AM", "P2 136.990 AM"),
                (137.000, "FM", "P3 137.000"),
                (174.000, "FM", "P4 174.000"),
                (375.000, "FM", "P5 375.000"),
                (550.000, "FM", "P6 550.000"),
                (117.975, "AM", "X1 117.975 AM"),
                (174.005, "FM", "X2 174.005"),
                (374.995, "FM", "X3 374.995"),
                (550.005, "FM", "X4 550.005"),
            ];
            let mut img = base.clone();
            for (i, (f, mode, name)) in probes.iter().enumerate() {
                let slot = 990 + i;
                assert!(!is_used(&base, slot), "memory {slot} is in use; pick other slots");
                let ec = crate::commands::export::ExpandedChannel {
                    channel: crate::models::Channel {
                        rx_freq: *f,
                        mode: Some((*mode).into()),
                        dcs_polarity: "NN".into(),
                        ..Default::default()
                    },
                    tg_label: None,
                    timeslot: None,
                    tg_number: None,
                    tg_call_type: None,
                    tg_inline: false,
                };
                let rec = super::memory::encode_channel(&ec, name).unwrap();
                store(&mut img, slot, &rec, Some((25, i as u8)));
            }
            super::memory::set_bank_name(&mut img, 25, "BAND PROBE");
            let outside: Vec<usize> = (0..IMAGE_LEN)
                .filter(|&i| img[i] != base[i] && i >= BANK_NAMES + BANK_COUNT * BANK_NAME_LEN)
                .collect();
            assert!(outside.is_empty(), "the probe build touched more than memories");
            println!(
                "bytes changed: {}",
                (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).count()
            );
            img
        }
        "settings" => {
            // A profile from CPM_ID5100_PROFILE (JSON), applied by the same
            // `settings::apply` the program path uses. Asserted before the
            // write: nothing outside the named fields' bytes moved.
            let profile: serde_json::Value =
                serde_json::from_str(&std::env::var("CPM_ID5100_PROFILE").expect("CPM_ID5100_PROFILE")).unwrap();
            let mut img = base.clone();
            let (n, notes) = super::settings::apply(&mut img, &profile);
            println!("settings applied: {n}, notes: {notes:?}");
            assert!(notes.is_empty());
            let decoded = super::settings::decode(&img);
            for k in profile.as_object().unwrap().keys() {
                println!("  {k}: {} -> {}", super::settings::decode(&base)[k], decoded[k]);
            }
            let moved: Vec<usize> = (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).collect();
            println!("bytes changed: {} {:x?}", moved.len(), moved);
            // Every changed byte belongs to a field the profile named.
            let mut owned = std::collections::HashSet::new();
            for k in profile.as_object().unwrap().keys() {
                let f = super::settings::ID5100_SETTINGS_FIELDS.iter().find(|f| f.key == k).expect(k);
                let len = match &f.kind {
                    crate::radios::icom_id52::settings::SK::Text { len }
                    | crate::radios::icom_id52::settings::SK::TextFill { len, .. } => *len as usize,
                    crate::radios::icom_id52::settings::SK::Uint { width }
                    | crate::radios::icom_id52::settings::SK::Enum { width, .. } => *width as usize,
                    _ => 1,
                };
                owned.extend(f.byte as usize..f.byte as usize + len);
            }
            assert!(moved.iter().all(|i| owned.contains(i)), "a byte outside the named fields moved");
            img
        }
        "file" => {
            let f = std::env::var("CPM_ID5100_FILE").expect("CPM_ID5100_FILE");
            let img = std::fs::read(&f).unwrap()[..IMAGE_LEN].to_vec();
            let names_end = BANK_NAMES + BANK_COUNT * BANK_NAME_LEN;
            let outside: Vec<usize> = (0..IMAGE_LEN)
                .filter(|&i| img[i] != base[i] && i >= names_end && i < VOLATILE_BASE)
                .collect();
            println!(
                "{f}: {} bytes differ from the radio, {} of them outside the memory area",
                (0..IMAGE_LEN).filter(|&i| img[i] != base[i]).count(),
                outside.len()
            );
            if std::env::var("CPM_ID5100_RESTORE").is_err() {
                assert!(outside.is_empty(), "refusing: the file changes more than memories");
            }
            img
        }
        other => panic!("unknown step {other}"),
    };
    let target = dir.join(format!("id5100_ladder_{step}_written_{stamp}.img"));
    std::fs::write(&target, &built).unwrap();

    // The radio is deaf for ~7 s after the backup read; `reopen` waits it out.
    let t = std::time::Instant::now();
    let mut p = super::reopen(&port).unwrap();
    println!("write session opened after {:.1}s", t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    upload(&mut *p, &built).unwrap();
    drop(p);
    println!("upload confirmed by the radio in {:.1}s", t.elapsed().as_secs_f64());

    // How soon the radio answers again after a clone-in is not known; the
    // driver retries identify. Measure it here.
    let t = std::time::Instant::now();
    let after = loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let attempt = open_port(&port).and_then(|mut p| {
            identify(&mut *p)?;
            download(&mut *p)
        });
        match attempt {
            Ok(img) => break img,
            Err(e) if t.elapsed().as_secs() < 60 => println!("  not yet ({e})"),
            Err(e) => panic!("no read-back within 60 s: {e}"),
        }
    };
    println!("read back after {:.1}s", t.elapsed().as_secs_f64());
    std::fs::write(dir.join(format!("id5100_ladder_{step}_after_{stamp}.img")), &after).unwrap();
    let low = (0..VOLATILE_BASE).filter(|&i| after[i] != built[i]).count();
    let high = (VOLATILE_BASE..IMAGE_LEN).filter(|&i| after[i] != built[i]).count();
    println!("read-back vs written: {low} bytes differ below {VOLATILE_BASE:#x}, {high} above");
    assert_eq!(low, 0, "the memory area did not read back as written");
}
