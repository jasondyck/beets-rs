//! A port of [langdetect](https://github.com/Mimino666/langdetect) 1.0.9
//! (Apache-2.0), which beets uses for `lyrics_language` and
//! `translation_language`.
//!
//! beets seeds langdetect with 0 (`DetectorFactory.seed = 0`), so the
//! randomized detector is deterministic; [`python_random`] reproduces
//! CPython's Mersenne Twister stream for that seed. The language profiles
//! and the normalization tables are embedded (see [`profile`] and
//! [`ngram`]); `NOTICE.md` credits them.

mod detector;
mod ngram;
mod profile;
mod python_random;

#[cfg(test)]
mod tests;

pub use detector::detect;
