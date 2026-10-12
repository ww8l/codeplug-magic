//! THROWAWAY (#48, step 4): run the app's own export pipeline against the dev
//! database and write a `CLNFTM300D.dat` from a real codeplug.
//!
//! A harness, not a unit test: it needs Tim's dev SQLite DB and his radio's own
//! backup, neither of which is in the repo. The channels come from any codeplug
//! (no FTM-300 codeplug exists until the app has seeded the model), filtered
//! through the FTM-300's own seeded row exactly as `generate_codeplug` would.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=3 FTM300_DAT=scratchpad/ftm300/captures/ftm300_01_real.dat \
//! cargo test --lib yaesu_ftm300::dev_export -- --ignored --nocapture
//! ```

use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel};
use crate::radios::driver::ExportRequest;

#[tokio::test]
#[ignore = "needs the dev database and a real CLNFTM300D.dat"]
async fn a_real_codeplug_exports_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let dat = std::env::var("FTM300_DAT").expect("FTM300_DAT");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.expect("open dev db");

    let model = crate::seed::test_model("FTM-300DR");
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

    let dir = std::env::temp_dir().join("ftm300_dev_export");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("CLNFTM300D.dat");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::fs::copy(root.join(&dat), &target).expect("copy template");

    let exporter = crate::radios::registry::exporter_for_format("yaesu_ftm300_sd").expect("exporter");
    let n = exporter
        .export(
            target.to_str().unwrap(),
            &ExportRequest { channels: &included, groups: &groups, model: &model, profile_settings: None, memory_group: &[] },
        )
        .expect("export");
    let out = std::fs::read(&target).unwrap();
    let template = std::fs::read(root.join(&dat)).unwrap();

    // Count what landed, the way the radio decides: byte 0 bit 7 of each of the 999 records.
    let used = (0..999).filter(|i| out[0x800 + i * 16] & 0x80 != 0).count();
    println!("wrote {n}; {used} records marked used");
    for (slot, ec) in included.iter().enumerate().take(8) {
        let r = &out[0x800 + slot * 16..0x800 + slot * 16 + 16];
        let name = String::from_utf8_lossy(out[0x10000 + slot * 16..0x10010 + slot * 16].split(|&b| b == 0xFF).next().unwrap()).into_owned();
        println!("  {:3} {:>10.4} {:02x?} {name:?}", slot + 1, ec.channel.rx_freq, r);
    }
    assert_eq!(used, included.len());
    assert_eq!(n, included.len());
    // Everything outside the three memory tables is the template's.
    let outside = |i: usize| !(0x800..0x800 + 999 * 16).contains(&i)
        && !(0x4CB0..0x4CB0 + 999 * 2).contains(&i)
        && !(0x10000..0x10000 + 999 * 16).contains(&i);
    let strays: Vec<usize> = (0..out.len()).filter(|&i| outside(i) && out[i] != template[i]).collect();
    assert!(strays.is_empty(), "bytes outside the memory tables moved: {strays:x?}");
    println!("nothing outside the memory tables moved; settings, PMS, Home, APRS untouched");
}

