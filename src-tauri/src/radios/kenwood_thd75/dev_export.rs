//! THROWAWAY (issue #40, Phase 2): run the app's own export pipeline against
//! the dev database and write a `.d75` from a real codeplug.
//!
//! Not a unit test — a harness, in the style of the AnyTone's `examples/`
//! diagnostics: it needs Tim's dev SQLite DB and a real radio save, neither of
//! which is in the repo. It exists to answer the one question Phase 2's unit
//! tests cannot: does a codeplug of real channels, filtered by the model row
//! actually seeded in the running app, reach the exporter intact — 220 MHz
//! repeaters included, which is exactly what the ID-52 threw away.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=3 \
//! cargo test --lib kenwood_thd75::dev_export -- --ignored --nocapture
//! ```

use crate::commands::export::{
    codeplug_model, exclusion_reason, expand_for_export, resolve_codeplug_groups, ExpandedChannel,
};
use crate::radios::driver::ExportRequest;

#[tokio::test]
#[ignore = "needs the dev database and a real .d75 template"]
async fn a_real_codeplug_exports_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG")
        .expect("CPM_CODEPLUG")
        .parse()
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db}?mode=ro"))
        .await
        .expect("open dev db");

    // Exactly what `generate_codeplug` does, in the same order. `codeplug_channels`
    // is private to the export module, so its one line — flatten the groups,
    // first occurrence of a channel wins — is repeated here rather than widening
    // that function's visibility for a harness.
    let model = codeplug_model(&pool, codeplug_id).await.expect("model");
    let groups = resolve_codeplug_groups(&pool, codeplug_id)
        .await
        .expect("groups");
    let mut seen = std::collections::HashSet::new();
    let channels: Vec<_> = groups
        .iter()
        .flat_map(|g| g.channels.iter().cloned())
        .filter(|c| seen.insert(c.id))
        .collect();
    let expanded = expand_for_export(&pool, channels).await.expect("expand");

    println!("model: {} ({:?})", model.display_name, model.export_format);
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
    println!("{} channels in, {} excluded", expanded.len(), excluded.len());
    for line in &excluded {
        println!("{line}");
    }
    for g in &groups {
        println!("group {:?}: {} channels", g.list_name, g.channels.len());
    }

    // The 220 MHz repeaters the ID-52 silently dropped. This radio transmits
    // there, so they must survive the filter.
    let two_twenty: Vec<f64> = included
        .iter()
        .map(|ec| ec.channel.rx_freq)
        .filter(|f| (216.0..260.0).contains(f))
        .collect();
    println!("220 MHz memories kept: {two_twenty:?}");

    let dir = std::env::temp_dir().join("thd75_dev_export");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let template = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scratchpad/thd75/card/08142026_204448.d75"
    );
    std::fs::copy(template, dir.join("08142026_204448.d75")).expect("copy template");

    let exporter = crate::radios::registry::exporter_for_format(
        model.export_format.as_deref().expect("an export format"),
    )
    .expect("an exporter claims this format");
    let target = exporter
        .resolve_target(&dir.to_string_lossy())
        .expect("resolve_target");
    let n = exporter
        .export(
            &target,
            &ExportRequest {
                channels: &included,
                groups: &groups,
                model: &model,
                profile_settings: None,
            },
        )
        .expect("export");

    println!("wrote {n} channels to {target}");
    let raw = std::fs::read(&target).unwrap();
    let file = super::d75::D75File::parse(&raw).expect("the driver takes its own output back");
    println!("container ok, body {:#X} bytes", file.body().len());

    // Read the names back out of the file the way the radio would.
    let body = file.body();
    for slot in 0..included.len().min(6) {
        let at = 0x10000 + slot * 16;
        let name: String = body[at..at + 16]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        println!("  slot {slot:3}: {name}");
    }
    assert_eq!(n, included.len());
}
