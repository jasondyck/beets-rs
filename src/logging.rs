//! A minimal logger for `-v/--verbose`, like beets' `beets` logger: debug
//! messages go to stderr from `-v` on; `-vv` adds lower-level detail (such
//! as each MusicBrainz request).

use std::sync::atomic::{AtomicU8, Ordering};

static VERBOSITY: AtomicU8 = AtomicU8::new(0);

pub fn set_verbosity(level: u8) {
    VERBOSITY.store(level, Ordering::Relaxed);
}

pub fn verbosity() -> u8 {
    VERBOSITY.load(Ordering::Relaxed)
}

/// Log a message at `-v` and above.
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::logging::verbosity() >= 1 {
            eprintln!($($arg)*);
        }
    };
}

/// Log a message at `-vv` and above.
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::logging::verbosity() >= 2 {
            eprintln!($($arg)*);
        }
    };
}

pub(crate) use {debug, trace};
