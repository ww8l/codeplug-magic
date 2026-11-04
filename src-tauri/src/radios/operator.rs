//! Telling the operator what to do at the radio, mid-operation, and waiting for
//! them when the radio cannot say they have done it.
//!
//! Every radio command is one blocking session under one port lock. Most radios
//! never need the operator during it. The FT-857D/FT-897 does, three times in a
//! program: [C] SEND for the backup read, [A] RCV before the write, [C] SEND
//! again for the read-back. A radio that has had SEND pressed announces itself —
//! its first block arrives — so [`say`] is enough. A radio waiting in RCV is
//! silent, so before sending a byte the driver must [`wait`] for the operator to
//! click Continue, which is the step CHIRP takes with an OK dialog.
//!
//! The command layer installs a sink for the session with [`with_sink`]; the
//! sink is thread-local because a driver runs on the same blocking thread as the
//! command that called it. With no sink installed — tests, `#[ignore]`d
//! hardware harnesses — [`say`] prints and [`wait`] proceeds at once; a write to
//! a radio that was not actually put in RCV then fails at block 0 with nothing
//! written, which is the safe way for that to go wrong.

use std::cell::RefCell;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;

/// What the dialog shows. `text: None` clears it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OperatorStep {
    pub text: Option<String>,
    /// The dialog offers Continue and Cancel, and the operation is paused until
    /// one is clicked.
    pub needs_continue: bool,
}

pub(crate) type Sink = Box<dyn Fn(&OperatorStep)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Idle,
    Waiting,
    Continue,
    Cancel,
}

static GATE: Mutex<Gate> = Mutex::new(Gate::Idle);
static GATE_CHANGED: Condvar = Condvar::new();

/// How long [`wait`] holds the port before giving up on the operator.
pub(crate) const WAIT_LIMIT: Duration = Duration::from_secs(600);

fn emit(step: &OperatorStep) {
    SINK.with(|s| match s.borrow().as_ref() {
        Some(sink) => sink(step),
        None => {
            if let Some(t) = &step.text {
                eprintln!("[operator] {t}");
            }
        }
    });
}

/// Run `body` with `sink` receiving every step, and clear the dialog after.
pub(crate) fn with_sink<T>(sink: Sink, body: impl FnOnce() -> T) -> T {
    SINK.with(|s| *s.borrow_mut() = Some(sink));
    let out = body();
    emit(&OperatorStep { text: None, needs_continue: false });
    SINK.with(|s| *s.borrow_mut() = None);
    out
}

/// Show an instruction the radio itself will confirm (its data arriving).
pub(crate) fn say(text: &str) {
    emit(&OperatorStep { text: Some(text.to_string()), needs_continue: false });
}

/// Show an instruction and block until the operator clicks Continue. Cancel, or
/// no answer within `limit`, is an error the caller returns before touching
/// the radio.
pub(crate) fn wait(text: &str, limit: Duration) -> Result<(), String> {
    let installed = SINK.with(|s| s.borrow().is_some());
    if !installed {
        say(text);
        return Ok(());
    }
    {
        let mut g = GATE.lock().map_err(|_| "operator gate poisoned".to_string())?;
        if *g == Gate::Waiting {
            return Err("Another radio operation is waiting for the operator.".into());
        }
        *g = Gate::Waiting;
    }
    emit(&OperatorStep { text: Some(text.to_string()), needs_continue: true });
    let g = GATE.lock().map_err(|_| "operator gate poisoned".to_string())?;
    let (mut g, timed_out) = GATE_CHANGED
        .wait_timeout_while(g, limit, |g| *g == Gate::Waiting)
        .map_err(|_| "operator gate poisoned".to_string())?;
    let answer = *g;
    *g = Gate::Idle;
    drop(g);
    emit(&OperatorStep { text: None, needs_continue: false });
    match answer {
        Gate::Continue => Ok(()),
        _ if timed_out.timed_out() => {
            Err("No answer from the operator, so nothing was sent to the radio.".into())
        }
        _ => Err("Cancelled — nothing was sent to the radio.".into()),
    }
}

/// The dialog's Continue (`true`) or Cancel (`false`). Ignored when nothing is
/// waiting, so a stray click cannot pre-answer the next step.
pub(crate) fn answer(proceed: bool) {
    if let Ok(mut g) = GATE.lock() {
        if *g == Gate::Waiting {
            *g = if proceed { Gate::Continue } else { Gate::Cancel };
            GATE_CHANGED.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    // The gate is process-wide; these tests take turns.
    static SERIAL: StdMutex<()> = StdMutex::new(());

    fn recording() -> (Sink, Arc<StdMutex<Vec<OperatorStep>>>) {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let l = log.clone();
        (Box::new(move |s: &OperatorStep| l.lock().unwrap().push(s.clone())), log)
    }

    fn answer_soon(proceed: bool) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            // Answer once the waiter is actually waiting.
            for _ in 0..200 {
                if *GATE.lock().unwrap() == Gate::Waiting {
                    answer(proceed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("nobody waited");
        })
    }

    #[test]
    fn continue_releases_the_wait_and_the_dialog_is_cleared() {
        let _s = SERIAL.lock().unwrap();
        let (sink, log) = recording();
        let t = answer_soon(true);
        let r = with_sink(sink, || wait("press RCV", Duration::from_secs(5)));
        t.join().unwrap();
        assert_eq!(r, Ok(()));
        let log = log.lock().unwrap();
        assert_eq!(log[0], OperatorStep { text: Some("press RCV".into()), needs_continue: true });
        assert_eq!(log.last().unwrap().text, None, "the dialog is cleared at the end");
    }

    #[test]
    fn cancel_and_silence_are_errors_that_say_nothing_was_sent() {
        let _s = SERIAL.lock().unwrap();
        let (sink, _) = recording();
        let t = answer_soon(false);
        let e = with_sink(sink, || wait("press RCV", Duration::from_secs(5))).unwrap_err();
        t.join().unwrap();
        assert!(e.contains("Cancelled") && e.contains("nothing was sent"), "{e}");

        let (sink, _) = recording();
        let e = with_sink(sink, || wait("press RCV", Duration::from_millis(30))).unwrap_err();
        assert!(e.contains("No answer"), "{e}");
        assert_eq!(*GATE.lock().unwrap(), Gate::Idle);
    }

    #[test]
    fn a_stray_click_does_not_pre_answer_the_next_step() {
        let _s = SERIAL.lock().unwrap();
        answer(true);
        let (sink, _) = recording();
        let e = with_sink(sink, || wait("press RCV", Duration::from_millis(30))).unwrap_err();
        assert!(e.contains("No answer"), "{e}");
    }

    #[test]
    fn with_no_sink_a_wait_proceeds() {
        let _s = SERIAL.lock().unwrap();
        assert_eq!(wait("press RCV", Duration::from_millis(1)), Ok(()));
    }
}
