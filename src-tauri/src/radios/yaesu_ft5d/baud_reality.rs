//! Does this USB-serial adapter actually change speed when we ask it to?
//!
//! Written after four clone captures returned the byte-identical `35 52 FE` at
//! nominally 4800, 9600, 19200, 38400, 57600 and 115200. Live UART data cannot
//! do that — misframing is a function of the sampling rate — so the rate was
//! never really changing. The adapter is a Prolific PL2303 (`067B:2303`) on
//! Apple's `AppleUSBPLCOM` driver, a family with a large counterfeit
//! population.
//!
//! Reading the rate back with `SerialPort::baud_rate()` does **not** detect
//! this: that returns `termios`, i.e. the value we ourselves stored. It reports
//! our request, not the hardware's behaviour, so it agrees with us even when
//! the chip ignores the setting entirely.
//!
//! ## The measurement
//!
//! Time a blocking write. A UART takes `bytes * 10 / rate` seconds to shift out
//! a buffer (8 data + start + stop), and `flush()` is `tcdrain(3)` on Unix — it
//! returns only once the last bit is on the wire. So elapsed time reveals the
//! **physical** rate whatever `termios` claims:
//!
//! | requested | 4096 bytes should take |
//! |-----------|------------------------|
//! | 4800      | 8.53 s |
//! | 9600      | 4.27 s |
//! | 19200     | 2.13 s |
//! | 38400     | 1.07 s |
//! | 115200    | 0.36 s |
//!
//! If every rate takes about the same time, the chip is pinned and no capture
//! it produced means anything. If the times track the table, the adapter is
//! honest and the FT5D's silence is a protocol problem instead.
//!
//! No radio is involved: this measures the adapter alone. Power the radio off
//! or unplug the radio end first, so the bytes go nowhere.
//!
//! ```text
//! cargo test --lib ft5d_baud_reality -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

const RATES: &[u32] = &[4800, 9600, 19200, 38400, 57600, 115200];

/// Enough bytes that even 115200 takes long enough to time reliably, but small
/// enough that 4800 does not make the run tedious.
const PAYLOAD: usize = 4096;

/// How far off the predicted time we tolerate before calling a rate a liar.
/// Generous: USB adds latency, and we only care about order-of-magnitude
/// agreement (is it 0.36 s or 4.3 s?), not about a few percent.
const TOLERANCE: f64 = 0.35;

fn expected_secs(rate: u32) -> f64 {
    (PAYLOAD * 10) as f64 / rate as f64
}

#[test]
#[ignore = "requires the USB-serial adapter (radio off / unplugged)"]
fn ft5d_baud_reality() {
    let port = super::hw_probe::pick_port().expect("port");
    println!("\n=== is the adapter's baud rate real? ===");
    println!("port: {port}");
    println!("writing {PAYLOAD} bytes at each rate and timing the drain.\n");
    println!("{:>8}  {:>10}  {:>10}  {}", "rate", "expected", "measured", "implied actual rate");

    let payload = vec![0x00u8; PAYLOAD];
    let mut measured = Vec::new();

    for &rate in RATES {
        let mut p = match serialport::new(&port, rate)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_secs(30))
            .open()
        {
            Ok(p) => p,
            Err(e) => {
                println!("{rate:>8}  could not open: {e}");
                continue;
            }
        };

        let start = Instant::now();
        if let Err(e) = std::io::Write::write_all(&mut *p, &payload) {
            println!("{rate:>8}  write failed: {e}");
            continue;
        }
        // tcdrain: returns only when the last bit has physically left.
        if let Err(e) = std::io::Write::flush(&mut *p) {
            println!("{rate:>8}  flush failed: {e}");
            continue;
        }
        let secs = start.elapsed().as_secs_f64();
        let implied = (PAYLOAD * 10) as f64 / secs;
        println!(
            "{rate:>8}  {:>9.2}s  {:>9.2}s  {implied:>9.0} baud",
            expected_secs(rate),
            secs
        );
        measured.push((rate, secs, implied));
    }

    println!();
    if measured.len() < 2 {
        println!("Not enough rates opened to conclude anything.");
        return;
    }

    let honest: Vec<&(u32, f64, f64)> = measured
        .iter()
        .filter(|(rate, secs, _)| {
            let want = expected_secs(*rate);
            (secs - want).abs() / want <= TOLERANCE
        })
        .collect();

    let spread = {
        let times: Vec<f64> = measured.iter().map(|(_, s, _)| *s).collect();
        let lo = times.iter().cloned().fold(f64::MAX, f64::min);
        let hi = times.iter().cloned().fold(0.0, f64::max);
        if lo > 0.0 { hi / lo } else { f64::MAX }
    };

    println!("slowest/fastest ratio: {spread:.1}x  (should be ~{:.0}x across this range)",
             expected_secs(RATES[0]) / expected_secs(RATES[RATES.len() - 1]));

    if spread < 2.0 {
        println!(
            "\n!! THE ADAPTER IS NOT CHANGING SPEED. Every rate took about the same time, so the\n\
             PL2303 is pinned to one physical rate and ignoring the setting. Every FT5D capture\n\
             so far is an artifact of that, not evidence about the radio's protocol. Fix the\n\
             cable (an FTDI or CP210x adapter, or a genuine Prolific) before reading anything\n\
             into those bytes."
        );
    } else if honest.len() == measured.len() {
        println!(
            "\nThe adapter honours every rate it was given, so the baud setting is NOT the\n\
             problem and the FT5D's 3-byte reply is a real protocol observation."
        );
    } else {
        println!(
            "\nSome rates track and others do not -- the chip supports a subset. Trust only the\n\
             ones whose measured time matched, and drive the radio at one of those."
        );
    }
}
