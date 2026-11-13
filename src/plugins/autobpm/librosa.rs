//! The pieces of librosa that beets' `autobpm` plugin uses, ported to Rust:
//! `librosa.load` (a decode to mono 22.05 kHz float32, resampled the way
//! resampy's `kaiser_fast` filter does), `onset_strength` with
//! `aggregate=np.median` (a Slaney mel spectrogram, `power_to_db`,
//! spectral flux), and `beat_track`'s tempo estimate (a windowed
//! autocorrelation tempogram with librosa's log-normal prior around 120
//! BPM).
//!
//! Everything is float64 after loading, as librosa computes in float64 for
//! the float inputs it converts; the FFTs are rustfft's unnormalised
//! real-to-complex transforms, scaled the way numpy's are.

use anyhow::{Result, bail};
use rustfft::{FftPlanner, num_complex::Complex};
use std::path::Path;

/// librosa's default sample rate (`librosa.load(sr=22050)`).
pub const SAMPLE_RATE: u32 = 22050;
/// `librosa.beat.beat_track`'s default hop length.
pub const HOP_LENGTH: usize = 512;
/// `librosa.feature.melspectrogram`'s default `n_fft`.
pub const N_FFT: usize = 2048;

// ------------------------------------------------------------------ loading

/// `librosa.load(path, sr=22050, mono=True, res_type="kaiser_fast")`:
/// decode, mix down to mono, resample to 22.05 kHz.
pub fn load(path: &Path) -> Result<Vec<f32>> {
    let decoded = decode(path)?;
    let mono = to_mono(&decoded.samples, decoded.channels);
    Ok(if decoded.sample_rate == SAMPLE_RATE {
        mono
    } else {
        resample_kaiser_fast(&mono, decoded.sample_rate, SAMPLE_RATE)
    })
}

/// A decoded stream: interleaved samples and its format.
pub struct Decoded {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
}

/// Decode any format symphonia reads into interleaved float samples.
pub fn decode(path: &Path) -> Result<Decoded> {
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), symphonia::core::io::MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("no supported audio tracks"))?;
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut samples: Vec<f32> = vec![];
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(SAMPLE_RATE);
    let mut channels = track
        .codec_params
        .channels
        .map_or(1, symphonia::core::audio::Channels::count);
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                sample_rate = spec.rate;
                channels = spec.channels.count();
                let mut sbuf = symphonia::core::audio::SampleBuffer::<f32>::new(
                    buf.capacity() as u64,
                    spec,
                );
                sbuf.copy_interleaved_ref(buf);
                samples.extend_from_slice(sbuf.samples());
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    if samples.is_empty() {
        bail!("no audio samples");
    }
    Ok(Decoded {
        samples,
        sample_rate,
        channels,
    })
}

/// `librosa.util.to_mono`: the mean of the channels.
fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

// -------------------------------------------------------------- resampling

/// resampy's `kaiser_fast` filter (its precomputed half-window: 24
/// zero-crossings, 512 samples per crossing, roll-off 0.8682120388…, a
/// Kaiser window with beta 9.90322).
fn kaiser_fast_window() -> (Vec<f64>, usize) {
    const NUM_ZEROS: usize = 24;
    const PRECISION: usize = 9;
    const ROLLOFF: f64 = 0.8682120388377784;
    const BETA: f64 = 9.90322;
    let num_bits = 1usize << PRECISION;
    let n = num_bits * NUM_ZEROS;
    // `kaiser(2n+1, beta)[n:]`: the right wing of the symmetric window.
    let i0 = |x: f64| -> f64 {
        // The zeroth-order modified Bessel function (scipy's `i0`).
        let mut sum = 1.0;
        let mut term = 1.0;
        let y = x * x / 4.0;
        for k in 1..1000 {
            term *= y / (f64::from(k) * f64::from(k));
            sum += term;
            if term < 1e-18 * sum {
                break;
            }
        }
        sum
    };
    let norm = i0(BETA);
    let mut win = vec![0.0; n + 1];
    for (i, w) in win.iter_mut().enumerate() {
        // Position `n + i` of the 2n+1-sample window.
        let m = (n + i) as f64;
        let arg = BETA * (1.0 - (2.0 * m / (2 * n) as f64 - 1.0).powi(2)).max(0.0).sqrt();
        // `sinc_window`'s sinc part: rolloff * sinc(rolloff * x), x = i/num_bits.
        let x = ROLLOFF * (i as f64 / num_bits as f64);
        let sinc = if x == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        };
        *w = i0(arg) / norm * ROLLOFF * sinc;
    }
    (win, num_bits)
}

/// resampy's `resample(x, sr_orig, sr_new, filter="kaiser_fast")` for one
/// channel (its `_resample_loop`).
pub fn resample_kaiser_fast(x: &[f32], sr_orig: u32, sr_new: u32) -> Vec<f32> {
    if sr_orig == sr_new {
        return x.to_vec();
    }
    let sample_ratio = f64::from(sr_new) / f64::from(sr_orig);
    let n_out = (x.len() as f64 * f64::from(sr_new) / f64::from(sr_orig)) as usize;
    let (mut interp_win, num_table) = kaiser_fast_window();
    if sample_ratio < 1.0 {
        for w in &mut interp_win {
            *w *= sample_ratio;
        }
    }
    // `np.diff(interp_win, append=interp_win[-1])`: the last delta is 0.
    let mut interp_delta = vec![0.0; interp_win.len()];
    for i in 0..interp_win.len() - 1 {
        interp_delta[i] = interp_win[i + 1] - interp_win[i];
    }
    let scale = sample_ratio.min(1.0);
    let index_step = (scale * num_table as f64) as usize;
    let time_increment = 1.0 / sample_ratio;
    let nwin = interp_win.len();
    let n_orig = x.len();
    let mut y = vec![0.0f32; n_out];
    for (t, out) in y.iter_mut().enumerate() {
        let time_register = t as f64 * time_increment;
        let n = time_register as usize;
        if n >= n_orig {
            break;
        }
        let mut acc = 0.0f64;
        let mut frac = scale * (time_register - n as f64);
        let mut index_frac = frac * num_table as f64;
        let mut offset = index_frac as usize;
        let mut eta = index_frac - offset as f64;
        let i_max = (n + 1).min((nwin - offset) / index_step);
        for i in 0..i_max {
            let idx = offset + i * index_step;
            let weight = interp_win[idx] + eta * interp_delta[idx];
            acc += weight * f64::from(x[n - i]);
        }
        frac = scale - frac;
        index_frac = frac * num_table as f64;
        offset = index_frac as usize;
        eta = index_frac - offset as f64;
        let k_max = (n_orig - n - 1).min((nwin - offset) / index_step);
        for k in 0..k_max {
            let idx = offset + k * index_step;
            let weight = interp_win[idx] + eta * interp_delta[idx];
            acc += weight * f64::from(x[n + k + 1]);
        }
        *out = acc as f32;
    }
    y
}

// ---------------------------------------------------------- the spectrogram

/// The real FFT of `x` (length `n`, zero-padded), as `np.fft.rfft` has it.
fn rfft(x: &[f64], n: usize) -> Vec<Complex<f64>> {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f64>> = x.iter().map(|&v| Complex::new(v, 0.0)).collect();
    buf.resize(n, Complex::new(0.0, 0.0));
    fft.process(&mut buf);
    buf.truncate(n / 2 + 1);
    buf
}

/// The inverse real FFT (`np.fft.irfft`) of a half spectrum.
fn irfft(spectrum: &[Complex<f64>], n: usize) -> Vec<f64> {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_inverse(n);
    let mut buf = vec![Complex::new(0.0, 0.0); n];
    for (i, c) in spectrum.iter().enumerate() {
        buf[i] = *c;
        if i > 0 && i < n - i {
            buf[n - i] = c.conj();
        }
    }
    fft.process(&mut buf);
    buf.iter().map(|c| c.re / n as f64).collect()
}

/// `scipy.fft.next_fast_len(n, real=True)`: the smallest 5-smooth number
/// (and, for real transforms, also the smallest product of 2s, 3s and 5s)
/// at least `n`.
pub fn next_fast_len(mut n: usize) -> usize {
    loop {
        let mut m = n;
        for p in [2, 3, 5] {
            while m.is_multiple_of(p) {
                m /= p;
            }
        }
        if m == 1 {
            return n;
        }
        n += 1;
    }
}

/// `scipy.signal.get_window("hann", n, fftbins=True)`: the periodic Hann
/// window.
pub fn hann_periodic(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
        .collect()
}

/// `librosa.filters.mel(sr, n_fft, n_mels=128, fmin=0, fmax=sr/2)` with
/// htk=False and norm="slaney": the (`n_mels`, `n_fft/2+1`) filterbank.
pub fn mel_filterbank(sr: u32, n_fft: usize, n_mels: usize) -> Vec<Vec<f64>> {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    let logstep = (6.4f64).ln() / 27.0;
    let hz_to_mel = |f: f64| -> f64 {
        if f >= MIN_LOG_HZ {
            MIN_LOG_MEL + (f / MIN_LOG_HZ).ln() / logstep
        } else {
            f / F_SP
        }
    };
    let mel_to_hz = |m: f64| -> f64 {
        if m >= MIN_LOG_MEL {
            MIN_LOG_HZ * (logstep * (m - MIN_LOG_MEL)).exp()
        } else {
            F_SP * m
        }
    };
    let fmin = 0.0;
    let fmax = f64::from(sr) / 2.0;
    let mel_min = hz_to_mel(fmin);
    let mel_max = hz_to_mel(fmax);
    let mel_f: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();
    let fftfreqs: Vec<f64> = (0..n_fft / 2 + 1)
        .map(|k| k as f64 * f64::from(sr) / n_fft as f64)
        .collect();
    let mut weights = vec![vec![0.0; n_fft / 2 + 1]; n_mels];
    for m in 0..n_mels {
        let fdiff_lower = mel_f[m + 1] - mel_f[m];
        let fdiff_upper = mel_f[m + 2] - mel_f[m + 1];
        let enorm = 2.0 / (mel_f[m + 2] - mel_f[m]);
        for (k, f) in fftfreqs.iter().enumerate() {
            let lower = (f - mel_f[m]) / fdiff_lower;
            let upper = (mel_f[m + 2] - f) / fdiff_upper;
            weights[m][k] = lower.min(upper).max(0.0) * enorm;
        }
    }
    weights
}

/// `librosa.feature.melspectrogram(y, sr, n_fft=2048, hop_length=512,
/// window="hann", center=True, pad_mode="constant", power=2.0,
/// n_mels=128, fmax=sr/2)`: the power mel spectrogram.
pub fn melspectrogram(y: &[f32], sr: u32, n_fft: usize, hop: usize, n_mels: usize) -> Vec<Vec<f64>> {
    let window = hann_periodic(n_fft);
    // `center=True`: n_fft//2 zeros at each end, then frames every `hop`.
    let n_frames = 1 + y.len() / hop;
    let filters = mel_filterbank(sr, n_fft, n_mels);
    let mut out = vec![vec![0.0; n_frames]; n_mels];
    for (frame, _) in (0..n_frames).enumerate() {
        let start = frame * hop;
        // `center=True, pad_mode="constant"`: n_fft//2 zeros at each end.
        let mut buf = vec![0.0; n_fft];
        for (i, b) in buf.iter_mut().enumerate() {
            // Position in the padded signal (n_fft//2 zeros in front).
            let padded = start + i;
            if let Some(sample) = padded.checked_sub(n_fft / 2)
                && sample < y.len()
            {
                *b = f64::from(y[sample]) * window[i];
            }
        }
        let spec = rfft(&buf, n_fft);
        let power: Vec<f64> = spec.iter().map(Complex::norm_sqr).collect();
        for m in 0..n_mels {
            out[m][frame] = filters[m]
                .iter()
                .zip(power.iter())
                .map(|(w, p)| w * p)
                .sum();
        }
    }
    out
}

/// `librosa.power_to_db(S, ref=1.0, amin=1e-10, top_db=80.0)`.
pub fn power_to_db(s: &[Vec<f64>], top_db: f64) -> Vec<Vec<f64>> {
    const AMIN: f64 = 1e-10;
    let mut out: Vec<Vec<f64>> = s
        .iter()
        .map(|row| {
            row.iter()
                .map(|&v| 10.0 * v.max(AMIN).log10())
                .collect::<Vec<f64>>()
        })
        .collect();
    // `ref=1.0` (a scalar): subtract 10*log10(max(amin, 1.0)) = 0.
    let max = out
        .iter()
        .flat_map(|r| r.iter())
        .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    for row in &mut out {
        for v in row.iter_mut() {
            *v = v.max(max - top_db);
        }
    }
    out
}

/// `librosa.util.sync(..., aggregate=np.median)` over the frequency axis:
/// the median of each column.
fn median_of_columns(s: &[Vec<f64>]) -> Vec<f64> {
    let n_frames = s.first().map_or(0, Vec::len);
    let mut out = vec![0.0; n_frames];
    let mut column: Vec<f64> = Vec::with_capacity(s.len());
    for (t, o) in out.iter_mut().enumerate() {
        column.clear();
        column.extend(s.iter().map(|row| row[t]));
        column.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = column.len();
        *o = if n == 0 {
            0.0
        } else if n.is_multiple_of(2) {
            (column[n / 2 - 1] + column[n / 2]) / 2.0
        } else {
            column[n / 2]
        };
    }
    out
}

/// `librosa.onset.onset_strength(y, sr, hop_length=512,
/// aggregate=np.median)`.
pub fn onset_envelope(y: &[f32], sr: u32, hop: usize, n_fft: usize) -> Vec<f64> {
    let mel = melspectrogram(y, sr, n_fft, hop, 128);
    let db = power_to_db(&mel, 80.0);
    let n_frames = db.first().map_or(0, Vec::len);
    // `spectral_flux(lag=1, max_size=1)`: S[:, lag:] - S[:, :-lag].
    let mut flux = vec![vec![0.0; n_frames.saturating_sub(1)]; db.len()];
    for (m, row) in db.iter().enumerate() {
        for t in 1..n_frames {
            flux[m][t - 1] = (row[t] - row[t - 1]).max(0.0);
        }
    }
    let onset = median_of_columns(&flux);
    // Pad by `lag + n_fft // (2 * hop_length)` frames and trim to n_frames.
    let pad = 1 + n_fft / (2 * hop);
    let mut out = vec![0.0; pad];
    out.extend(onset);
    out.truncate(n_frames);
    out
}

// ------------------------------------------------------------------- tempo

/// `librosa.feature.tempogram(onset_envelope, win_length, hop_length=1)`:
/// the normalized local autocorrelation (one column per frame).
pub fn tempogram(onset: &[f64], win_length: usize) -> Vec<Vec<f64>> {
    let n = onset.len();
    let half = win_length / 2;
    // `np.pad(mode="linear_ramp", end_values=[0, 0])`.
    // `np.pad(mode="linear_ramp", end_values=[0, 0])`: a linear ramp from
    // 0 at the outer edge to the neighbouring sample.
    let mut padded = vec![0.0; n + 2 * half];
    for (i, v) in padded.iter_mut().enumerate() {
        if i < half {
            *v = onset[0] * (i as f64 / half as f64);
        } else if i >= half + n {
            let from_edge = (n + 2 * half - 1 - i) as f64 / half as f64;
            *v = onset[n - 1] * from_edge;
        } else {
            *v = onset[i - half];
        }
    }
    let window = hann_periodic(win_length);
    let n_pad = next_fast_len(2 * win_length - 1);
    let mut out = vec![vec![0.0; n]; win_length];
    let mut segment = vec![0.0; win_length];
    for t in 0..n.min(padded.len().saturating_sub(win_length)) {
        for i in 0..win_length {
            segment[i] = padded[t + i] * window[i];
        }
        let spec = rfft(&segment, n_pad);
        let power: Vec<Complex<f64>> = spec.iter().map(|c| Complex::new(c.norm_sqr(), 0.0)).collect();
        let ac = irfft(&power, n_pad);
        let max = ac[..win_length].iter().fold(0.0f64, |a, &b| a.max(b.abs()));
        for lag in 0..win_length {
            out[lag][t] = if max > f64::MIN_POSITIVE {
                ac[lag] / max
            } else {
                ac[lag]
            };
        }
    }
    out
}

/// `librosa.feature.tempo` (as `beat_track` calls it: `start_bpm=120`,
/// `std_bpm=1`, `ac_size=8`, `max_tempo=320`, `aggregate=np.mean`).
pub fn tempo(onset: &[f64], sr: u32, hop: usize) -> f64 {
    // `time_to_frames(8.0)` (n_fft=None: offset 0).
    let win_length = ((8.0 * f64::from(sr)) as usize) / hop;
    let win_length = win_length.max(1);
    let tg = tempogram(onset, win_length);
    let frames = tg.first().map_or(0, Vec::len);
    if frames == 0 {
        return 0.0;
    }
    // Aggregate: the mean over frames.
    let mean: Vec<f64> = tg
        .iter()
        .map(|row| row.iter().sum::<f64>() / frames as f64)
        .collect();
    // `tempo_frequencies`: bpms[0] = inf.
    let bpms: Vec<f64> = (0..win_length)
        .map(|lag| {
            if lag == 0 {
                f64::INFINITY
            } else {
                60.0 * f64::from(sr) / (hop as f64 * lag as f64)
            }
        })
        .collect();
    let logprior = |bpm: f64| -> f64 {
        let ratio = bpm.log2() - 120.0f64.log2();
        -0.5 * (ratio / 1.0).powi(2)
    };
    // Kill everything above max_tempo = 320.
    let max_idx = bpms.iter().position(|&b| b < 320.0).unwrap_or(0);
    let mut best = (f64::NEG_INFINITY, 0usize);
    for (i, bpm) in bpms.iter().enumerate() {
        if i < max_idx {
            continue;
        }
        let prior = logprior(*bpm);
        if !prior.is_finite() {
            continue;
        }
        // `np.log1p(1e6 * tg)`, with numpy's NaN semantics (a NaN wins).
        let value = 1e6 * mean[i];
        let score = if value <= -1.0 {
            f64::NAN
        } else {
            value.ln_1p() + prior
        };
        match score.partial_cmp(&best.0) {
            Some(std::cmp::Ordering::Greater) => best = (score, i),
            None => {
                // NaN: numpy's argmax picks the first one.
                if best.0.is_nan() {
                    continue;
                }
                if score.is_nan() {
                    best = (score, i);
                }
            }
            _ => {}
        }
    }
    bpms[best.1]
}

/// `librosa.beat.beat_track(y=y, sr=sr)`'s tempo (0 for no onsets).
pub fn beat_track_tempo(y: &[f32], sr: u32) -> f64 {
    let onset = onset_envelope(y, sr, HOP_LENGTH, N_FFT);
    if !onset.iter().any(|&v| v != 0.0) {
        return 0.0;
    }
    tempo(&onset, sr, HOP_LENGTH)
}
