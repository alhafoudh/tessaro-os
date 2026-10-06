//! Where the ctl's text goes. Every module prints through these macros,
//! which shadow the std ones (`use crate::out::{eprintln, println}`): to
//! stdout and stderr through anstream, which drops the colors when they are
//! off, or - on a thread running a command for one of several devices
//! (`capture`) - into that device's buffer, so the devices' output does not
//! mix and is printed whole, one device after the other.

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::io::Write as _;

/// What one command printed, colors kept: they are dropped, or not, when
/// the buffer is printed.
#[derive(Debug, Default)]
pub struct Captured {
    pub out: String,
    pub err: String,
}

thread_local! {
    static CAPTURE: RefCell<Option<Captured>> = const { RefCell::new(None) };
}

/// `work`, with everything it prints on this thread kept instead.
pub fn capture<T>(work: impl FnOnce() -> T) -> (T, Captured) {
    CAPTURE.with(|capture| *capture.borrow_mut() = Some(Captured::default()));
    let value = work();
    let captured = CAPTURE.with(|capture| capture.borrow_mut().take());
    (value, captured.unwrap_or_default())
}

/// Whether this thread's output is being kept: nothing can be asked at the
/// keyboard then, and nothing redrawn in place.
pub fn capturing() -> bool {
    CAPTURE.with(|capture| capture.borrow().is_some())
}

#[doc(hidden)]
pub fn out(args: fmt::Arguments) {
    CAPTURE.with(|capture| match capture.borrow_mut().as_mut() {
        Some(captured) => {
            let _ = captured.out.write_fmt(args);
        }
        None => {
            let _ = anstream::stdout().write_fmt(args);
        }
    });
}

#[doc(hidden)]
pub fn err(args: fmt::Arguments) {
    CAPTURE.with(|capture| match capture.borrow_mut().as_mut() {
        Some(captured) => {
            let _ = captured.err.write_fmt(args);
        }
        None => {
            let _ = anstream::stderr().write_fmt(args);
        }
    });
}

macro_rules! print {
    ($($arg:tt)*) => { $crate::out::out(format_args!($($arg)*)) };
}

macro_rules! println {
    () => { $crate::out::out(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::out(format_args!("{}\n", format_args!($($arg)*))) };
}

macro_rules! eprint {
    ($($arg:tt)*) => { $crate::out::err(format_args!($($arg)*)) };
}

macro_rules! eprintln {
    () => { $crate::out::err(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::err(format_args!("{}\n", format_args!($($arg)*))) };
}

pub(crate) use {eprint, eprintln, print, println};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thread_keeps_its_own_output() {
        let run = |name: &'static str| {
            std::thread::spawn(move || {
                capture(|| {
                    for at in 0..50 {
                        println!("{name} {at}");
                        eprintln!("{name} note");
                    }
                })
                .1
            })
        };
        let (a, b) = (run("a"), run("b"));
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert!(a.out.lines().all(|line| line.starts_with("a ")));
        assert_eq!(b.out.lines().count(), 50);
        assert_eq!(b.err.lines().count(), 50);
        assert!(!capturing());
    }
}
