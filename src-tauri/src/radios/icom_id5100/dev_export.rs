//! THROWAWAY (issue #49, Phase 3 gate): a real codeplug from the dev database,
//! resolved by the app's own slot and bank resolvers against the SEEDED ID-5100
//! row, built into a real clone read of Tim's radio — and counted.
//!
//! Needs the dev SQLite DB and a real read, neither of which is in the repo.
//! The DB is COPIED to a temp file and the app's own seeder is run on the copy,
//! so the ID-5100 row exists without the dev app having restarted and the dev
//! DB itself is never written. Any codeplug's lists can be resolved as an
//! ID-5100 codeplug: the model is taken from the seed, not the codeplug.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=3 CPM_ID5100_BASE=../scratchpad/id5100/id5100_07_rust_read.img \
//! CPM_ID5100_OUT=../scratchpad/id5100/id5100_dev_export.img \
//!   cargo test --lib icom_id5100::dev_export -- --ignored --nocapture
//! ```

use std::collections::HashSet;

use crate::commands::export::{
    banks_for_slots, exclusion_reason, expand_for_export, expanded_names, resolve_codeplug_groups,
    SlotChannel,
};
use crate::models::RadioModel;

use super::layout::IMAGE_LEN;
use super::memory::{decode_memories, read_bank};
use super::program::build_codeplug;

#[tokio::test]
#[ignore = "needs the dev database and a real ID-5100 read"]
async fn a_real_codeplug_builds_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let base = std::fs::read(std::env::var("CPM_ID5100_BASE").expect("CPM_ID5100_BASE")).unwrap();
    let base = &base[..IMAGE_LEN];

    let copy = std::env::temp_dir().join("id5100_dev_export.sqlite3");
    std::fs::copy(&db, &copy).expect("copy dev db");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", copy.display()))
        .await
        .expect("open copy");
    crate::seed::seed_radio_models(&pool).await.expect("seed");
    let model: RadioModel =
        sqlx::query_as("SELECT * FROM radio_models WHERE model = 'ID-5100'")
            .fetch_one(&pool)
            .await
            .expect("seeded ID-5100 row");

    // `resolve_codeplug_slots`, with the model swapped for the ID-5100's.
    let groups = resolve_codeplug_groups(&pool, codeplug_id).await.expect("groups");
    let mut seen = HashSet::new();
    let channels: Vec<_> = groups
        .iter()
        .flat_map(|g| g.channels.iter().cloned())
        .filter(|c| seen.insert(c.id))
        .collect();
    let expanded = expand_for_export(&pool, channels).await.expect("expand");
    let mut excluded = Vec::new();
    let included: Vec<_> = expanded
        .iter()
        .filter(|ec| match exclusion_reason(&ec.channel, &model) {
            Some(why) => {
                excluded.push(format!("  {:>10.4}  {why}", ec.channel.rx_freq));
                false
            }
            None => true,
        })
        .collect();
    let names = expanded_names(included.iter().copied(), &model);
    let slots: Vec<SlotChannel> = included
        .into_iter()
        .zip(names)
        .enumerate()
        .map(|(slot, (ec, name))| SlotChannel { slot, name, channel: ec.channel.clone() })
        .collect();
    let banks = banks_for_slots(&groups, &slots);

    println!("{} channels in, {} excluded:", expanded.len(), excluded.len());
    excluded.iter().for_each(|l| println!("{l}"));
    for b in &banks {
        println!("list {:?}: {} slots", b.name, b.slots.len());
    }

    let built = build_codeplug(&model, &slots, &banks, base).expect("build");
    for w in &built.warnings {
        println!("warning: {w}");
    }
    let decoded = decode_memories(&built.image);
    for m in &decoded {
        println!(
            "{:4} {:<16} {:>10.4} {:>8} {:<14} {:<4} {:?}",
            m.slot, m.name, m.rx_mhz, m.shift, m.tone, m.mode, m.bank
        );
    }
    // What went in is what came out: every slot, at its frequency, in its bank.
    assert_eq!(decoded.len(), slots.len());
    for (sc, m) in slots.iter().zip(&decoded) {
        assert_eq!(sc.slot, m.slot);
        assert!((sc.channel.rx_freq - m.rx_mhz).abs() < 1e-5, "{} {}", sc.channel.rx_freq, m.rx_mhz);
    }
    let banked = (0..slots.len()).filter(|&s| read_bank(&built.image, s).is_some()).count();
    println!(
        "{} memories written, {banked} banked, {} banks named",
        decoded.len(),
        built.banks_written
    );
    if let Ok(out) = std::env::var("CPM_ID5100_OUT") {
        std::fs::write(&out, &built.image).unwrap();
        println!("wrote {out}");
    }
}
