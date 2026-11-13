//! BPD's player: the interface beets' `gstplayer` GstPlayer has, with a
//! pure-Rust audio backend (rodio + symphonia, behind the `bpd-audio`
//! feature) and a null player for builds without audio output and for the
//! tests.

use anyhow::Result;
use std::path::Path;

/// What the server asks a player to do (beets' `gstplayer.GstPlayer`).
pub trait Player: Send {
    /// Start playing `path` from the beginning.
    fn play_file(&mut self, path: &Path) -> Result<()>;
    /// Resume after a pause.
    fn play(&mut self) -> Result<()>;
    fn pause(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    /// Seek within the current file, in seconds.
    fn seek(&mut self, pos: f64) -> Result<()>;
    /// `0.0`..=`1.0`.
    fn set_volume(&mut self, volume: f64);
    /// `(position, duration)` in seconds of the current file; a zero
    /// duration means "unknown" (the server then uses the item's length).
    fn time(&self) -> (f64, f64);
    fn playing(&self) -> bool;
    /// Whether a track finished since this was last called (the player's
    /// `play_finished` callback).
    fn take_finished(&mut self) -> bool;
    /// The player's name, for the "Server ready" log line.
    fn name(&self) -> &'static str;
}

/// The player without an audio device: it tracks what the server told it,
/// and never finishes a track on its own.
#[derive(Debug, Default)]
pub struct NullPlayer {
    playing: bool,
    path: Option<std::path::PathBuf>,
    duration: f64,
}

impl NullPlayer {
    pub fn new() -> NullPlayer {
        NullPlayer::default()
    }
}

impl Player for NullPlayer {
    fn play_file(&mut self, path: &Path) -> Result<()> {
        self.path = Some(path.to_path_buf());
        // The duration of a file is not known without decoding it; the
        // server falls back on the item's length.
        self.duration = 0.0;
        self.playing = true;
        Ok(())
    }

    fn play(&mut self) -> Result<()> {
        self.playing = true;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.playing = false;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.playing = false;
        self.path = None;
        self.duration = 0.0;
        Ok(())
    }

    fn seek(&mut self, _pos: f64) -> Result<()> {
        Ok(())
    }

    fn set_volume(&mut self, _volume: f64) {}

    fn time(&self) -> (f64, f64) {
        (0.0, self.duration)
    }

    fn playing(&self) -> bool {
        self.playing
    }

    fn take_finished(&mut self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "null"
    }
}

/// The pure-Rust player: rodio's output stream (cpal) with symphonia
/// decoding.
#[cfg(feature = "bpd-audio")]
pub struct RodioPlayer {
    // The stream stops when it is dropped; the sink plays through it.
    _stream: rodio::OutputStream,
    sink: rodio::Sink,
    playing: bool,
    duration: f64,
    volume: f32,
    started: bool,
}

#[cfg(feature = "bpd-audio")]
impl RodioPlayer {
    pub fn new() -> Result<RodioPlayer> {
        let stream = rodio::OutputStreamBuilder::open_default_stream()?;
        let sink = rodio::Sink::connect_new(stream.mixer());
        Ok(RodioPlayer {
            _stream: stream,
            sink,
            playing: false,
            duration: 0.0,
            volume: 1.0,
            started: false,
        })
    }
}

#[cfg(feature = "bpd-audio")]
impl Player for RodioPlayer {
    fn play_file(&mut self, path: &Path) -> Result<()> {
        use rodio::Source as _;
        self.sink.stop();
        self.sink = rodio::Sink::connect_new(self._stream.mixer());
        self.sink.set_volume(self.volume);
        let file = std::fs::File::open(path)?;
        let source = rodio::Decoder::new(std::io::BufReader::new(file))?;
        self.duration = source
            .total_duration()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.sink.append(source);
        self.sink.play();
        self.playing = true;
        self.started = true;
        Ok(())
    }

    fn play(&mut self) -> Result<()> {
        self.sink.play();
        self.playing = true;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.sink.pause();
        self.playing = false;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.sink.stop();
        self.playing = false;
        self.started = false;
        self.duration = 0.0;
        Ok(())
    }

    fn seek(&mut self, pos: f64) -> Result<()> {
        self.sink
            .try_seek(std::time::Duration::from_secs_f64(pos.max(0.0)))?;
        Ok(())
    }

    fn set_volume(&mut self, volume: f64) {
        self.volume = volume as f32;
        self.sink.set_volume(self.volume);
    }

    fn time(&self) -> (f64, f64) {
        (self.sink.get_pos().as_secs_f64(), self.duration)
    }

    fn playing(&self) -> bool {
        self.playing && !self.sink.is_paused()
    }

    fn take_finished(&mut self) -> bool {
        // `Sink::empty` is true once the queue drains.
        if self.started && self.playing && self.sink.empty() {
            self.playing = false;
            self.started = false;
            return true;
        }
        false
    }

    fn name(&self) -> &'static str {
        "rodio"
    }
}

/// The player this build can use: rodio when the `bpd-audio` feature is
/// on and an output device opens, the null player otherwise.
pub fn default_player(log: &crate::plugins::PluginLog) -> Box<dyn Player> {
    #[cfg(feature = "bpd-audio")]
    {
        match RodioPlayer::new() {
            Ok(p) => return Box::new(p),
            Err(e) => log.warning(&format!(
                "could not open an audio output ({e}); running without playback"
            )),
        }
    }
    #[cfg(not(feature = "bpd-audio"))]
    {
        let _ = log;
        log.warning(
            "built without the bpd-audio feature: no audio output (the server still works)",
        );
    }
    Box::new(NullPlayer::new())
}
