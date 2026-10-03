//! Hardware check of the Rust clone READ against the real radio. Read-only:
//! identify + clone-out, nothing sent that writes.
//!
//! ```sh
//! CPM_ID5100_PORT=/dev/cu.usbserial-RT1SQ1OS \
//! CPM_ID5100_OUT=../scratchpad/id5100/id5100_07_rust_read.img \
//!   cargo test --lib icom_id5100::hw_read -- --ignored --nocapture
//! ```

use super::layout::{IMAGE_LEN, VOLATILE_BASE};
use super::protocol::{download, identify, open_port};

#[test]
#[ignore = "talks to a real ID-5100 on CPM_ID5100_PORT"]
fn rust_clone_read_matches_the_radio() {
    let port = std::env::var("CPM_ID5100_PORT").expect("CPM_ID5100_PORT");
    let out = std::env::var("CPM_ID5100_OUT").expect("CPM_ID5100_OUT");
    let mut p = open_port(&port).unwrap();
    let id = identify(&mut *p).unwrap();
    println!("identify: {} {}", id.matched, id.ident_hex);
    let image = download(&mut *p).unwrap();
    assert_eq!(image.len(), IMAGE_LEN);
    std::fs::write(&out, &image).unwrap();
    println!("wrote {out}");
    if let Ok(reference) = std::env::var("CPM_ID5100_COMPARE") {
        let r = std::fs::read(&reference).unwrap();
        let low = (0..VOLATILE_BASE).filter(|&i| image[i] != r[i]).count();
        let high = (VOLATILE_BASE..IMAGE_LEN).filter(|&i| image[i] != r[i]).count();
        println!("vs {reference}: {low} bytes differ below {VOLATILE_BASE:#x}, {high} above");
        assert_eq!(low, 0);
    }
}
