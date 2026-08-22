//! Fetch the BrandMeister talkgroup list exactly as the app does, and print
//! what came back — no database, no writes.
//!
//! The app's own "BrandMeister" button is the same call; this is how the live
//! service gets exercised without a GUI session and without touching anyone's
//! talkgroup library.
//!
//! Usage: cargo run --example brandmeister_dryrun

#[tokio::main]
async fn main() {
    match ww8l_codeplug_magic_lib::commands::talkgroups::fetch_brandmeister_list_for_diagnostics()
        .await
    {
        Ok(list) => {
            let mut keys: Vec<&String> = list.keys().collect();
            keys.sort_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX));
            println!("fetched {} talkgroups", list.len());
            for k in keys.iter().take(5) {
                println!("  {k:>7}  {}", list[*k]);
            }
            println!("  ...");
            // The one the app renames, so the dry run shows what it starts as.
            match list.get("9990") {
                Some(name) => println!("  9990 as served: {name:?} (app renames to \"Parrot (Echo Test)\")"),
                None => println!("  9990 not in the list"),
            }
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
