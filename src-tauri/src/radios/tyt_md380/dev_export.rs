//! THROWAWAY (issue #42, step 4 gate): a real codeplug from the dev database,
//! resolved by the app's own `resolve_codeplug_payload` against the SEEDED
//! MD-380 row, planned and patched into a real read of Tim's radio — then
//! decoded back out of the image and counted against what went in.
//!
//! The dev DB is COPIED and the app's seeder run on the copy, so the MD-380 row
//! exists without the dev app restarting and the dev DB is never written.
//!
//! ```sh
//! CPM_DEV_DB="$HOME/Library/Application Support/com.ww8l.codeplugmagic.dev/codeplug_manager.sqlite3" \
//! CPM_CODEPLUG=1 CPM_MD380_BASE=../scratchpad/tyt_md380/captures/md380-read-20261009-173549.img \
//! CPM_MD380_OUT=../scratchpad/tyt_md380/md380_dev_export.img \
//!   cargo test --lib tyt_md380::dev_export -- --ignored --nocapture
//! ```

use crate::commands::export::resolve_codeplug_payload;
use crate::models::RadioModel;

use super::layout::*;
use super::memory::{Channel, Contact, ScanList, Zone};
use super::program::{count_used, plan_program};

#[tokio::test]
#[ignore = "needs the dev database and a real MD-380 read"]
async fn a_real_codeplug_builds_through_the_app_pipeline() {
    let db = std::env::var("CPM_DEV_DB").expect("CPM_DEV_DB");
    let codeplug_id: i64 = std::env::var("CPM_CODEPLUG").expect("CPM_CODEPLUG").parse().unwrap();
    let base = std::fs::read(std::env::var("CPM_MD380_BASE").expect("CPM_MD380_BASE")).unwrap();

    let copy = std::env::temp_dir().join("md380_dev_export.sqlite3");
    std::fs::copy(&db, &copy).expect("copy dev db");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", copy.display()))
        .await
        .expect("open copy");
    crate::seed::seed_radio_models(&pool).await.expect("seed");
    let model: RadioModel = sqlx::query_as("SELECT * FROM radio_models WHERE model = 'MD-380'")
        .fetch_one(&pool)
        .await
        .expect("seeded MD-380 row");

    let mut resolved = resolve_codeplug_payload(&pool, codeplug_id).await.expect("payload");
    resolved.model = model;
    let rows_in = resolved.channels.len();
    let plan = plan_program(&resolved.payload()).expect("plan");
    let img = plan.apply(&base).expect("apply");
    if let Ok(out) = std::env::var("CPM_MD380_OUT") {
        std::fs::write(out, &img).unwrap();
    }

    println!("rows in: {rows_in}; programmed {}; skipped {}", plan.channels.len(), plan.skipped.len());
    for s in &plan.skipped {
        println!("  skipped {:<20} {}", s.name, s.reason);
    }
    for w in &plan.warnings {
        println!("  warning: {w}");
    }
    assert_eq!(plan.channels.len() + plan.skipped.len(), rows_in, "every row is programmed or skipped");

    // Decode the image back and hold it against the plan, table by table.
    let used = count_used(&img);
    assert_eq!(used.channels, plan.channels.len());
    assert_eq!(used.contacts, plan.contacts.len());
    assert_eq!(used.zones, plan.zones.len());
    assert_eq!(used.scan_lists, plan.scan_lists.len());
    for (i, want) in plan.channels.iter().enumerate() {
        let raw = &img[at(CHANNELS, CHANNEL_LEN, i)..][..CHANNEL_LEN];
        let got = Channel::decode(raw).expect("decodes");
        assert_eq!(&got, &Channel::decode(&want.encode()).unwrap(), "channel {}", i + 1);
        if (got.contact as usize) > plan.contacts.len() {
            panic!("channel {} points at contact {} of {}", i + 1, got.contact, plan.contacts.len());
        }
    }
    for i in 0..plan.zones.len() {
        let z = Zone::decode(&img[at(ZONES, ZONE_LEN, i)..][..ZONE_LEN]);
        for &m in z.members.iter().filter(|&&m| m != 0) {
            assert!((m as usize) <= plan.channels.len(), "zone {} member {m} beyond the channels", z.name);
        }
        let names: Vec<_> = z
            .members
            .iter()
            .filter(|&&m| m != 0)
            .map(|&m| plan.channels[m as usize - 1].name.clone())
            .collect();
        println!("zone {:2} {:<16} {:2} ch: {}", i + 1, z.name, names.len(), names.join(", "));
    }
    for i in 0..plan.contacts.len() {
        let c = Contact::decode(&img[at(CONTACTS, CONTACT_LEN, i)..][..CONTACT_LEN]).unwrap();
        println!("contact {:2} {:>8} {:?} {}", i + 1, c.id, c.call_type, c.name);
    }
    for i in 0..plan.scan_lists.len() {
        let s = ScanList::decode(&img[at(SCAN_LISTS, SCAN_LIST_LEN, i)..][..SCAN_LIST_LEN]);
        let n = s.members.iter().filter(|&&m| m != 0).count();
        println!("scan {:2} {:<16} {n} members, prio {:#06x}/{:#06x}", i + 1, s.name, s.priority_1, s.priority_2);
    }
    // Everything outside the four tables is the radio's own.
    let tables = [
        // Rewritten with the contact table: quick keys remapped, and the
        // radio's current-zone byte kept inside the new zone count.
        (ONE_TOUCH, 4 * ONE_TOUCH_COUNT),
        (NUMBER_KEYS, 2 * NUMBER_KEY_COUNT),
        (CURRENT_ZONE, 1),
        (CONTACTS, CONTACT_LEN * CONTACT_COUNT),
        (RX_GROUPS, RX_GROUP_LEN * RX_GROUP_COUNT),
        (ZONES, ZONE_LEN * ZONE_COUNT),
        (SCAN_LISTS, SCAN_LIST_LEN * SCAN_LIST_COUNT),
        (CHANNELS, CHANNEL_LEN * CHANNEL_COUNT),
    ];
    let outside = (0..IMAGE_LEN)
        .filter(|&i| !tables.iter().any(|&(b, l)| (b..b + l).contains(&i)))
        .filter(|&i| img[i] != base[i])
        .count();
    assert_eq!(outside, 0, "bytes changed outside the programmed tables");
    println!("outside the tables: 0 bytes changed");
}
