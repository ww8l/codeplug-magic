//! THROWAWAY (#50, step 4): run the app's own export pipeline against the dev
//! database and write a new `.icf` from a real codeplug into a folder holding a
//! copy of the radio's own file — the folder → new `SetYYYYMMDD_NN.icf` path the
//! card action takes.
//!
//! A harness, not a unit test: it needs Tim's dev SQLite DB and his radio's
//! file, neither of which is in the repo.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=3 cargo test --lib icom_id51::dev_export -- --ignored --nocapture
//! ```

use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel};
use crate::radios::driver::ExportRequest;
use crate::radios::icom_id52::icf::IcfFile;

use super::layout::{BANK_NAMES, BANK_TABLE, CHANNEL_COUNT, EMPTY_BITMAP, REC_LEN};

#[tokio::test]
#[ignore = "needs the dev database and scratchpad/icom_id51/id51_01_base.icf"]
async fn a_real_codeplug_exports_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.expect("open dev db");

    let model = crate::seed::test_model("ID-51 Plus");
    let groups = resolve_codeplug_groups(&pool, codeplug_id).await.expect("groups");
    let mut seen = std::collections::HashSet::new();
    let channels: Vec<_> = groups
        .iter()
        .flat_map(|g| g.channels.iter().cloned())
        .filter(|c| seen.insert(c.id))
        .collect();
    let expanded = expand_for_export(&pool, channels).await.expect("expand");
    let mut excluded = Vec::new();
    let included: Vec<&ExpandedChannel> = expanded
        .iter()
        .filter(|ec| match exclusion_reason(&ec.channel, &model) {
            Some(why) => {
                excluded.push(format!("  {:>10.4}  {}", ec.channel.rx_freq, why));
                false
            }
            None => true,
        })
        .collect();
    println!("{} channels in, {} included, {} excluded", expanded.len(), included.len(), excluded.len());
    for line in &excluded {
        println!("{line}");
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let template = root.join("scratchpad/icom_id51/id51_01_base.icf");
    let dir = std::env::temp_dir().join("id51_dev_export/ID-51/Setting");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&template, dir.join("Set20261104_01.icf")).expect("copy template");

    let exporter = crate::radios::registry::exporter_for_format("icom_id51_icf").expect("exporter");
    let target = exporter.resolve_target(dir.to_str().unwrap()).expect("resolve");
    println!("target {target}");
    let n = exporter
        .export(
            &target,
            &ExportRequest { channels: &included, groups: &groups, model: &model, profile_settings: None, memory_group: &[] },
        )
        .expect("export");

    let out = IcfFile::parse(&std::fs::read_to_string(&target).unwrap()).expect("our file parses and its #CD verifies");
    super::check_is_an_id51_file(&out).unwrap();
    let before = IcfFile::parse(&std::fs::read_to_string(&template).unwrap()).unwrap();
    let (o, b) = (out.image(), before.image());
    let used = (0..CHANNEL_COUNT).filter(|&s| o[EMPTY_BITMAP + s / 8] & (1 << (s % 8)) == 0).count();
    println!("wrote {n}; {used} memories marked used");
    for bank in 0..26 {
        let name = String::from_utf8_lossy(&o[BANK_NAMES + 16 * bank..BANK_NAMES + 16 * bank + 16]).trim_end().to_string();
        if !name.is_empty() {
            let members = (0..CHANNEL_COUNT).filter(|&s| o[BANK_TABLE + 2 * s] & 0x1F == bank as u8).count();
            println!("  bank {} {name:?}: {members}", (b'A' + bank as u8) as char);
        }
    }
    assert_eq!((n, used), (included.len(), included.len()));
    // Everything outside the memory tables is the radio's own file.
    let tables = |i: usize| i < CHANNEL_COUNT * REC_LEN || (EMPTY_BITMAP..BANK_NAMES + 26 * 16).contains(&i);
    let strays: Vec<usize> = (0..o.len()).filter(|&i| !tables(i) && o[i] != b[i]).collect();
    assert!(strays.is_empty(), "bytes outside the memory tables moved: {strays:x?}");
    // The radio's own file was not touched.
    assert_eq!(std::fs::read(dir.join("Set20261104_01.icf")).unwrap(), std::fs::read(&template).unwrap());
    println!("nothing outside the memory tables moved; the radio's own file is untouched");
}
