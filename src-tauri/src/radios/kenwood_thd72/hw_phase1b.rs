//! THROWAWAY (issue #55, Phase 1b): the first clone download off a real TH-D72.
//!
//! Read-only, but not risk-free the way `hw_phase1` is: it sends `0M PROGRAM`,
//! which puts the radio into clone mode at 57600 and only leaves it on `E`. If
//! this driver mis-sequences, the radio can sit in program mode until it is
//! power-cycled. Nothing is written to it either way.
//!
//! ```sh
//! CPM_THD72_PORT=/dev/cu.SLAB_USBtoUART \
//! CPM_THD72_OUT=scratchpad/kenwood_thd72/images/ww8l-<label>.img \
//!   cargo test --lib kenwood_thd72::hw_phase1b -- --ignored --nocapture
//! ```

use super::{container, layout, memory, protocol};

#[test]
#[ignore = "needs a real TH-D72 on the cable"]
fn download_this_radios_own_image() {
    let port = std::env::var("CPM_THD72_PORT").expect("CPM_THD72_PORT");
    let out = std::env::var("CPM_THD72_OUT").expect("CPM_THD72_OUT");

    let mut p = protocol::open_port(&port).expect("open the port");
    let ident = protocol::identify(&mut *p).expect("identify");
    println!("radio: {} ({})", ident.matched, ident.ident_hex);

    let started = std::time::Instant::now();
    let image = protocol::download(&mut *p).expect("clone download");
    println!("read {} bytes in {:.1}s", image.len(), started.elapsed().as_secs_f64());
    assert_eq!(image.len(), layout::IMAGE_LEN);

    std::fs::write(&out, &image).expect("save the image");
    println!("saved to {out}");

    // The guard was written against eight images from three OTHER radios. This
    // is the first time it has met the radio this app will actually program.
    let parsed = container::Thd72Image::parse(&image)
        .unwrap_or_else(|e| panic!("the model guard refused this radio's own image: {e}"));

    // Two independent readings of the same memories: the `ME` lines from
    // hw_phase1 and the clone image. No other radio here can be cross-examined.
    println!("\nprogrammable-VFO table, out of THIS radio's image:");
    let table = parsed.prog_vfo_table().expect("prog vfo table");
    for (i, b) in table.iter().enumerate() {
        println!("  idx {i}: {:.3} - {:.3} MHz", b.start_hz as f64 / 1e6, b.end_hz as f64 / 1e6);
    }

    println!("\nfirst memories, decoded out of the image:");
    let mut count = 0usize;
    let mut mismatched = 0usize;
    for slot in 0..layout::CHANNEL_COUNT {
        let Some(rec) = memory::read_record(&image, slot) else {
            continue;
        };
        count += 1;
        let m = memory::decode_memory(&rec.memory);
        if layout::prog_vfo_index(&table, m.freq_hz) != Some(rec.prog_vfo()) {
            mismatched += 1;
        }
        if count <= 12 {
            let name: String = rec.name.iter().take_while(|&&b| b != 0xFF).map(|&b| b as char).collect();
            println!(
                "  {slot:3} {name:<9} {:9.4} MHz  band {}  step nibble {:X}",
                m.freq_hz as f64 / 1e6,
                rec.prog_vfo(),
                m.tune_step
            );
        }
    }
    println!("\n{count} memories programmed; {mismatched} mis-banded");

    // Every memory re-encodes byte-identically — the Phase 2 gate, now against
    // the radio this app will program rather than someone else's.
    for slot in 0..layout::CHANNEL_COUNT {
        let Some(rec) = memory::read_record(&image, slot) else {
            continue;
        };
        assert_eq!(
            memory::encode_memory(&memory::decode_memory(&rec.memory)),
            rec.memory,
            "memory {slot} does not re-encode byte-identically"
        );
    }
    println!("all {count} memories re-encode byte-identically");
}
