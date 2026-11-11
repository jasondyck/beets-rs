//! A minimal logger for `-v/--verbose`, like beets' `beets` logger: debug
//! messages go to stderr from `-v` on; `-vv` adds lower-level detail (such
//! as each MusicBrainz request).
//!
//! A thread can [`capture`] what it would log, so work done ahead of time
//! on a worker thread (the threaded importer's lookups) can be replayed on
//! the main thread when its result is used, in the sequential order.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};

static VERBOSITY: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static CAPTURED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

pub fn set_verbosity(level: u8) {
    VERBOSITY.store(level, Ordering::Relaxed);
}

pub fn verbosity() -> u8 {
    VERBOSITY.load(Ordering::Relaxed)
}

/// Write a line to stderr, or keep it if this thread is capturing.
pub fn emit(line: String) {
    let line = CAPTURED.with(|c| match c.borrow_mut().as_mut() {
        Some(buf) => {
            buf.push(line);
            None
        }
        None => Some(line),
    });
    if let Some(line) = line {
        eprintln!("{line}");
    }
}

/// Run `f`, returning what it logged (with [`emit`]) instead of printing it.
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let outer = CAPTURED.with(|c| c.borrow_mut().replace(vec![]));
    let r = f();
    let lines = CAPTURED.with(|c| std::mem::replace(&mut *c.borrow_mut(), outer));
    (r, lines.unwrap_or_default())
}

/// Print lines captured earlier, now.
pub fn replay(lines: &[String]) {
    for l in lines {
        emit(l.clone());
    }
}

/// Log a message at `-v` and above.
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::logging::verbosity() >= 1 {
            $crate::logging::emit(format!($($arg)*));
        }
    };
}

/// Log a message at `-vv` and above.
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::logging::verbosity() >= 2 {
            $crate::logging::emit(format!($($arg)*));
        }
    };
}

pub(crate) use {debug, trace};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_collects_and_nests() {
        let ((inner, n), outer) = capture(|| {
            emit("a".into());
            let r = capture(|| emit("b".into()));
            emit("c".into());
            r
        });
        assert_eq!(inner, ());
        assert_eq!(n, vec!["b"]);
        assert_eq!(outer, vec!["a", "c"]);
    }
}
