//! THROWAWAY (#52, step 4): run the app's own export pipeline against the dev
//! database and write a `CLNFTM400D.dat` from a real codeplug.
//!
//! A harness, not a unit test: it needs Tim's dev SQLite DB and his radio's own
//! backup, neither of which is in the repo. The channels come from any codeplug,
//! filtered through the FTM-400's own seeded row exactly as `generate_codeplug`
//! would.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=2 FTM400_DAT=scratchpad/yaesu_ftm400/captures/CLNFTM400D_card_20260201.dat \
//! cargo test --lib yaesu_ftm400::dev_export -- --ignored --nocapture
//! ```

use super::sd_image::{decode_name, BANKS, MEM_SLOTS, MEM_STRIDE, NAME_LEN};
use crate::commands::export::{exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel};
use crate::radios::driver::ExportRequest;

#[tokio::test]
#[ignore = "needs the dev database and a real CLNFTM400D.dat"]
async fn a_real_codeplug_exports_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let dat = std::env::var("FTM400_DAT").expect("FTM400_DAT");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro")).await.expect("open dev db");

    let model = crate::seed::test_model("FTM-400");
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

    let dir = std::env::temp_dir().join("ftm400_dev_export");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("CLNFTM400D.dat");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::fs::copy(root.join(&dat), &target).expect("copy template");

    let exporter = crate::radios::registry::exporter_for_format("yaesu_ftm400_sd").expect("exporter");
    let n = exporter
        .export(
            target.to_str().unwrap(),
            &ExportRequest { channels: &included, groups: &groups, model: &model, profile_settings: None, memory_group: &[] },
        )
        .expect("export");
    let out = std::fs::read(&target).unwrap();
    let template = std::fs::read(root.join(&dat)).unwrap();

    // Count what landed, the way the radio decides: byte 0 bit 7, in each band.
    for (b, (bank, want)) in BANKS.iter().zip([included.len(), included.len()]).enumerate() {
        let used = (0..MEM_SLOTS).filter(|i| out[bank.mem + i * MEM_STRIDE] & 0x80 != 0).count();
        println!("band {}: {used} records marked used", "AB".as_bytes()[b] as char);
        assert_eq!(used, want);
    }
    for (slot, ec) in included.iter().enumerate().take(10) {
        let r = &out[BANKS[0].mem + slot * MEM_STRIDE..BANKS[0].mem + (slot + 1) * MEM_STRIDE];
        let name = decode_name(&out[BANKS[0].name + slot * NAME_LEN..BANKS[0].name + (slot + 1) * NAME_LEN]);
        println!("  {:3} {:>10.4} {:02x?} {name:?}", slot + 1, ec.channel.rx_freq, r);
    }
    assert_eq!(n, included.len());
    // Everything outside the four memory tables is the template's.
    let outside = |i: usize| {
        !BANKS.iter().any(|b| {
            (b.mem..b.mem + MEM_SLOTS * MEM_STRIDE).contains(&i) || (b.name..b.name + MEM_SLOTS * NAME_LEN).contains(&i)
        })
    };
    // Each band's RX COVERAGE (0xE3 / 0xF3) may be switched to WIDE for out-of-ham channels.
    let strays: Vec<usize> = (0..out.len()).filter(|&i| outside(i) && i != 0xE3 && i != 0xF3 && out[i] != template[i]).collect();
    assert!(strays.is_empty(), "bytes outside the memory tables moved: {strays:x?}");
    println!("nothing outside the memory tables moved; header, settings, PMS, Home, APRS untouched");
    std::fs::copy(&target, root.join("scratchpad/yaesu_ftm400/out/dev_export_CLNFTM400D.dat")).ok();
}
