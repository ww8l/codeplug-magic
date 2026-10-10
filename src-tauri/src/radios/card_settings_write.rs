//! The settings-only card write (s136), proven against real files the radios
//! wrote — no radio needed. For every file in the scratchpad folders:
//!
//! 1. Writing a file's OWN settings back changes nothing at all: everything
//!    the settings write leaves alone, it leaves byte-identical.
//! 2. Flipping one on/off setting changes only that setting's byte(s) — plus,
//!    on the FT5D, the checksum, which must still validate.
//!
//! `#[ignore]`d: the files are personal radio saves in gitignored scratchpad/.
//!
//! ```sh
//! cargo test --lib radios::card_settings_write -- --ignored --nocapture
//! ```

use serde_json::{Map, Value};

use crate::radios::driver::RadioDriver;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../scratchpad");

fn files(dir: &str, ext: &str, prefix: &str) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(format!("{ROOT}/{dir}"))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
                && p.file_name().unwrap().to_string_lossy().starts_with(prefix)
        })
        .collect();
    v.sort();
    assert!(!v.is_empty(), "no .{ext} files in {dir}");
    v
}

/// The first on/off setting in a decoded map, flipped.
fn flip_one(decoded: &Value) -> (String, Map<String, Value>) {
    let map = decoded.as_object().unwrap();
    let (k, v) = map.iter().find(|(_, v)| v.is_boolean()).expect("an on/off setting");
    let mut one = Map::new();
    one.insert(k.clone(), Value::Bool(!v.as_bool().unwrap()));
    (k.clone(), one)
}

fn differing(a: &[u8], b: &[u8]) -> Vec<usize> {
    assert_eq!(a.len(), b.len());
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cpm-card-settings-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
#[ignore = "needs the real card files in scratchpad/"]
fn ft5d_settings_only_writes_touch_settings_and_checksum_alone() {
    use crate::radios::yaesu_ft5d::{settings::decode_settings, DRIVER};
    let ex = DRIVER.as_codeplug_exporter().unwrap();
    let w = ex.as_card_settings_writer().expect("a card settings writer");
    for src in files("ft5d", "dat", "ft5d_01") {
        let orig = std::fs::read(&src).unwrap();
        let dir = scratch("ft5d");
        let target = dir.join("BACKUP.dat");
        std::fs::write(&target, &orig).unwrap();
        let t = target.to_str().unwrap();

        let own = decode_settings(&orig);
        w.export_settings(t, own.as_object().unwrap()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), orig, "{src:?}: own settings changed bytes");
        assert!(dir.join("BACKUP.dat.orig").exists(), "the .orig is kept");

        let (key, one) = flip_one(&own);
        w.export_settings(t, &one).unwrap();
        let after = std::fs::read(&target).unwrap();
        let d = differing(&orig, &after);
        let body: Vec<_> = d.iter().filter(|&&i| i < orig.len() - 4).collect();
        assert!(!body.is_empty() && body.len() <= 2, "{key}: {d:x?}");
        assert_eq!(decode_settings(&after)[&key], one[&key]);
        assert!(crate::radios::yaesu_ft5d::sd_image::patch_settings(&after, &Map::new()).is_ok(), "checksum validates");
        println!("{src:?}: own settings = 0 bytes; {key} flipped = {body:x?} + checksum");
    }
}

#[test]
#[ignore = "needs the real card files in scratchpad/"]
fn id52_settings_only_writes_touch_settings_alone() {
    use crate::radios::icom_id52::{icf::IcfFile, settings::decode_settings, DRIVER};
    let ex = DRIVER.as_codeplug_exporter().unwrap();
    let w = ex.as_card_settings_writer().expect("a card settings writer");
    for src in files("id52", "icf", "id52_0") {
        let text = std::fs::read_to_string(&src).unwrap();
        let orig = IcfFile::parse(&text).unwrap().image().to_vec();
        let dir = scratch("id52");
        let target = dir.join("Setting.icf");
        std::fs::write(&target, &text).unwrap();
        let t = target.to_str().unwrap();

        let own = decode_settings(&orig);
        w.export_settings(t, own.as_object().unwrap()).unwrap();
        let back = IcfFile::parse(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(back.image(), orig.as_slice(), "{src:?}: own settings changed bytes");

        let (key, one) = flip_one(&own);
        w.export_settings(t, &one).unwrap();
        let after = IcfFile::parse(&std::fs::read_to_string(&target).unwrap()).unwrap();
        let d = differing(&orig, after.image());
        assert!(!d.is_empty() && d.len() <= 2, "{key}: {d:x?}");
        assert_eq!(decode_settings(after.image())[&key], one[&key]);
        println!("{src:?}: own settings = 0 bytes; {key} flipped = {d:x?}");
    }
}

#[test]
#[ignore = "needs the real card files in scratchpad/"]
fn thd75_settings_only_writes_a_new_file_touching_settings_alone() {
    use crate::radios::kenwood_thd75::{d75::D75File, settings::decode_settings, DRIVER};
    let ex = DRIVER.as_codeplug_exporter().unwrap();
    let w = ex.as_card_settings_writer().expect("a card settings writer");
    // Only the radio's own saves: the folder also holds an MCP-D75 file, which
    // the driver refuses by design (it is a different, unmeasured layout).
    for src in files("thd75/card", "d75", "") {
        let raw = std::fs::read(&src).unwrap();
        let Ok(parsed) = D75File::parse(&raw) else {
            println!("{src:?}: not a radio save — skipped, as the driver refuses it");
            continue;
        };
        let orig = parsed.body().to_vec();
        let dir = scratch("thd75");
        std::fs::write(dir.join(src.file_name().unwrap()), &raw).unwrap();

        let own = decode_settings(&orig);
        let t1 = ex.resolve_target(dir.to_str().unwrap()).unwrap();
        w.export_settings(&t1, own.as_object().unwrap()).unwrap();
        assert_eq!(std::fs::read(&t1).unwrap(), raw, "{src:?}: own settings changed bytes");

        let (key, one) = flip_one(&own);
        std::fs::remove_file(&t1).unwrap(); // keep the template the newest
        let t2 = ex.resolve_target(dir.to_str().unwrap()).unwrap();
        w.export_settings(&t2, &one).unwrap();
        let after = D75File::parse(&std::fs::read(&t2).unwrap()).unwrap();
        let d = differing(&orig, after.body());
        assert!(!d.is_empty() && d.len() <= 2, "{key}: {d:x?}");
        assert_eq!(decode_settings(after.body())[&key], one[&key]);
        assert_eq!(std::fs::read(dir.join(src.file_name().unwrap())).unwrap(), raw, "the radio's own save is untouched");
        println!("{src:?}: own settings = 0 bytes; {key} flipped = {d:x?}; new file {t2}");
    }
}
