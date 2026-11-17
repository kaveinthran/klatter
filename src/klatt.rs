//! The KLSYN88 cascade/parallel formant synthesizer (Klatt & Klatt, 1990),
//! which extends Klatt (1980) with the KLGLOTT88 voicing source.
//!
//! The voicing source generates one polynomial glottal flow pulse per period
//! (open quotient OQ, double pulsing DI) at four times the output rate, then
//! low-pass filters and downsamples it. Radiation is folded into each source
//! as a first difference, as in the paper. A low-pass resonator adds the spectral
//! tilt TL and aspiration noise AH is added. This excites the cascade vocal
//! tract: nasal and tracheal pole-zero pairs, then F1 (which can change while
//! the glottis is open, via DF1 and DB1) through F3 and fixed poles above.
//! Aspiration's own path swaps those fixed poles for F4/F5. Frication excites the
//! parallel branch, whose resonators are normalized so that A2..A6 give the
//! spectral peak level in dB relative to the flat bypass path AB. The 1980
//! impulse-driven voice bar (AVS) is kept for stop closures.

use std::f64::consts::PI;

const FRAME_RATE: u32 = 200;
pub const FRAME_MS: f64 = 1000.0 / FRAME_RATE as f64;
const REFERENCE_SAMPLE_RATE: f64 = 16_000.0;

/// Constants at their KLSYN88 defaults (Klatt & Klatt 1990, Table XII).
const F4: f64 = 3250.0;
const F5: f64 = 3700.0;
const F6: f64 = 4990.0;
const FNP: f64 = 280.0;
const BNP: f64 = 90.0;
const BNZ: f64 = 90.0;
const FT: f64 = 2150.0;
const BT: f64 = 180.0;
/// Frication-excited parallel bandwidths B2F..B6F.
const PARALLEL_BW: [f64; 5] = [250.0, 320.0, 350.0, 500.0, 1500.0];
/// Cascade formants above F3: KLSYN88's default F4, F5 and F6 with their
/// default bandwidths (Klatt & Klatt 1990, Table XII), which give the energy
/// concentration around 3-3.5 kHz and the falloff above about 4 kHz typical of
/// male talkers (Klatt 1980). KLSYN88 ran at 10 kHz, so F6 sat at the top of its
/// band; at higher output rates one more pole, at the seventh resonance of a
/// uniform 17.5 cm male vocal tract (Fant 1960), stands in for the resonances
/// above F6 so vowels keep their 5-7 kHz energy.
const CASCADE_HIGH: [(f64, f64); 4] = [(F4, 200.0), (F5, 200.0), (F6, 500.0), (6500.0, 1000.0)];
/// Aspiration keeps KLSYN88's F4/F5 instead: through CASCADE_HIGH it hisses.
/// Zeros cancel CASCADE_HIGH on the
/// noise path because those poles must come last: ahead of the nasal and F1-F3
/// filters, their boosted highs make the nasal zero click when it switches.
const ASPIRATION_HIGH: [(f64, f64); 2] = [(F4, 200.0), (F5, 200.0)];

/// Period lengths are quantized to a quarter sample to avoid audible
/// "staircase" steps in slowly changing F0 (1990 paper, note 17).
const OVERSAMPLE: usize = 4;
const DECIMATION_TAPS: usize = 64;

const CASCADE_GAIN: f64 = 1.0;
/// Peak glottal flow at AV = 60 dB and F0 = 100 Hz. The peak is scaled by the
/// period (AV x T0), a design choice rather than the paper's (which defines
/// AV as the pulse's peak amplitude), so the closure excitation is
/// independent of F0.
const GLOTTAL_PEAK: f64 = 0.75;
const REFERENCE_PERIOD_S: f64 = 0.01;
const PARALLEL_GAIN: f64 = 0.035;
/// Aspiration at the AV level matches the high harmonics of default voicing,
/// the KLSYN88 calibration, with the voicing's presence boost included.
const ASPIRATION_GAIN: f64 = 0.0192;
const VOICE_BAR_GAIN: f64 = 1.0;

/// Parameters for one 5 ms frame. Amplitudes are in dB; 0 means silent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub f0: f64,
    pub av: f64,
    /// Open quotient: percent of each glottal period that the glottis is open.
    pub oq: f64,
    /// Extra spectral tilt of the voicing source, in dB down at 3 kHz.
    pub tl: f64,
    /// Diplophonic double pulsing, in percent of maximum.
    pub di: f64,
    pub avs: f64,
    pub ah: f64,
    pub af: f64,
    pub ab: f64,
    /// F1..F3 in Hz.
    pub f: [f64; 3],
    /// B1..B3 in Hz.
    pub b: [f64; 3],
    /// Increases to F1 and B1 while the glottis is open, in Hz.
    pub df1: f64,
    pub db1: f64,
    pub fnz: f64,
    /// Tracheal pole and zero frequencies and bandwidths, in Hz.
    pub ftp: f64,
    pub btp: f64,
    pub ftz: f64,
    pub btz: f64,
    /// Parallel amplitudes A2..A6.
    pub a: [f64; 5],
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            f0: 100.0,
            av: 0.0,
            oq: 50.0,
            tl: 0.0,
            di: 0.0,
            avs: 0.0,
            ah: 0.0,
            af: 0.0,
            ab: 0.0,
            f: [500.0, 1500.0, 2500.0],
            b: [60.0, 90.0, 150.0],
            df1: 0.0,
            db1: 0.0,
            fnz: FNP,
            ftp: FT,
            btp: BT,
            ftz: FT,
            btz: BT,
            a: [0.0; 5],
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct Resonator {
    a: f64,
    b: f64,
    c: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn set(&mut self, f: f64, bw: f64, sr: f64) {
        if f >= 0.49 * sr {
            // Above Nyquist the resonator would alias, so pass the signal through.
            self.bypass();
            return;
        }
        let t = 1.0 / sr;
        let r = (-PI * bw * t).exp();
        self.c = -(r * r);
        self.b = 2.0 * r * (2.0 * PI * f * t).cos();
        self.a = 1.0 - self.b - self.c;
    }

    fn bypass(&mut self) {
        self.a = 1.0;
        self.b = 0.0;
        self.c = 0.0;
    }

    fn copy_coefficients(&mut self, other: &Resonator) {
        self.a = other.a;
        self.b = other.b;
        self.c = other.c;
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn gain_at(&self, f: f64, sr: f64) -> f64 {
        let w = 2.0 * PI * f / sr;
        let re = 1.0 - self.b * w.cos() - self.c * (2.0 * w).cos();
        let im = self.b * w.sin() + self.c * (2.0 * w).sin();
        self.a.abs() / (re * re + im * im).sqrt()
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct AntiResonator {
    a: f64,
    b: f64,
    c: f64,
    x1: f64,
    x2: f64,
}

impl AntiResonator {
    fn set(&mut self, f: f64, bw: f64, sr: f64) {
        let mut r = Resonator::default();
        r.set(f, bw, sr);
        self.a = 1.0 / r.a;
        self.b = -r.b / r.a;
        self.c = -r.c / r.a;
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.a * x + self.b * self.x1 + self.c * self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        y
    }
}

fn db_to_lin(db: f64) -> f64 {
    if db <= 0.0 {
        0.0
    } else {
        10f64.powf((db - 60.0) / 20.0)
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct Peaking {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Peaking {
    /// Octave bandwidth compensates for bilinear warping across sample rates.
    fn new(f: f64, octaves: f64, gain_db: f64, sr: f64) -> Self {
        let amp = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / sr;
        let alpha = w.sin() * (2f64.ln() / 2.0 * octaves * w / w.sin()).sinh();
        let a0 = 1.0 + alpha / amp;
        Peaking {
            b: [
                (1.0 + alpha * amp) / a0,
                -2.0 * w.cos() / a0,
                (1.0 - alpha * amp) / a0,
            ],
            a: [-2.0 * w.cos() / a0, (1.0 - alpha / amp) / a0],
            ..Peaking::default()
        }
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }

    #[cfg(test)]
    fn gain_at(&self, f: f64, sr: f64) -> f64 {
        let w = 2.0 * PI * f / sr;
        let z = |c: [f64; 3]| {
            let re = c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos();
            let im = -(c[1] * w.sin() + c[2] * (2.0 * w).sin());
            (re * re + im * im).sqrt()
        };
        z(self.b) / z([1.0, self.a[0], self.a[1]])
    }
}

/// Only voicing needs presence; aspiration and frication have their own shaping.
/// KLSYN88 has no such filter; a listener preferred voicing with this midrange
/// presence in blind listening.
const PRESENCE_HZ: f64 = 2200.0;
const PRESENCE_OCTAVES: f64 = 1.7;
const PRESENCE_DB: f64 = 5.0;

/// Bandwidth of the 0 Hz low-pass resonator that attenuates by `tl` dB at
/// 3 kHz; the paper defines TL by that attenuation.
fn tilt_bandwidth(tl: f64, sr: f64) -> f64 {
    let attenuation = |bw: f64| {
        let mut r = Resonator::default();
        r.set(0.0, bw, sr);
        -20.0 * r.gain_at(3000.0, sr).log10()
    };
    let (mut lo, mut hi) = (1.0f64, 1.0e6f64);
    for _ in 0..60 {
        let mid = (lo * hi).sqrt();
        if attenuation(mid) > tl {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * hi).sqrt()
}

/// One KLGLOTT88 glottal period, in oversampled samples: the glottis opens
/// at `start`, the flow follows U(t) = a t^2 - b t^3 and returns to zero at
/// closure, and the glottis stays closed for the rest of the period.
#[derive(Clone, Copy, Default, Debug)]
struct GlottalPulse {
    start: usize,
    open_len: f64,
    a: f64,
}

impl GlottalPulse {
    fn open_samples(period_len: usize, oq: f64) -> f64 {
        (period_len as f64 * oq.clamp(10.0, 99.0) / 100.0)
            .round()
            .clamp(2.0, period_len as f64)
    }

    fn new(period_len: usize, oq: f64, peak: f64, delay: usize) -> Self {
        let open = Self::open_samples(period_len, oq);
        // U peaks at 2T/3 with height 4aT^2/27.
        GlottalPulse {
            start: delay,
            open_len: open,
            a: peak * 27.0 / (4.0 * open * open),
        }
    }

    fn is_open(&self, pos: usize) -> bool {
        pos >= self.start && ((pos - self.start) as f64) < self.open_len
    }

    fn flow(&self, pos: usize) -> f64 {
        if !self.is_open(pos) {
            return 0.0;
        }
        let t = (pos - self.start) as f64;
        self.a * t * t * (1.0 - t / self.open_len)
    }
}

/// The KLGLOTT88 pulse train, generated at `OVERSAMPLE` times the output rate
/// and low-pass filtered so the downsampled waveform is free of aliasing.
struct GlottalSource {
    period_len: usize,
    pos: usize,
    pulse: GlottalPulse,
    modify_pulse: bool,
    taps: [f64; DECIMATION_TAPS],
    history: [f64; DECIMATION_TAPS],
    open_history: [bool; DECIMATION_TAPS],
    head: usize,
}

impl GlottalSource {
    fn new() -> Self {
        // Blackman-windowed sinc with its cutoff just below the output Nyquist frequency.
        let fc = 0.45 / OVERSAMPLE as f64;
        let m = (DECIMATION_TAPS - 1) as f64;
        let mut taps = [0.0; DECIMATION_TAPS];
        for (n, tap) in taps.iter_mut().enumerate() {
            let x = n as f64 - m / 2.0;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * x).sin() / (PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * PI * n as f64 / m).cos()
                + 0.08 * (4.0 * PI * n as f64 / m).cos();
            *tap = sinc * w;
        }
        let sum: f64 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= sum);
        GlottalSource {
            period_len: 0,
            pos: 0,
            pulse: GlottalPulse::default(),
            modify_pulse: false,
            taps,
            history: [0.0; DECIMATION_TAPS],
            open_history: [false; DECIMATION_TAPS],
            head: 0,
        }
    }

    /// F0, AV, OQ and DI take effect only at the start of a period so that
    /// changes never cut into a pulse (1990 paper, note 18).
    fn start_period(&mut self, fr: &Frame, av: f64, sr: f64) {
        let f0 = if fr.f0 > 20.0 { fr.f0 } else { 100.0 };
        let rate = OVERSAMPLE as f64 * sr;
        self.period_len = ((rate / f0).round() as usize).max(2 * OVERSAMPLE);
        self.pos = 0;
        let mut peak = av * GLOTTAL_PEAK * (self.period_len as f64 / rate) / REFERENCE_PERIOD_S;
        let mut delay = 0;
        // Double pulsing alters alternate pulses: delayed by up to the time
        // that makes their closure meet the next opening, and attenuated
        // linearly to zero at DI = 100%.
        self.modify_pulse = !self.modify_pulse;
        if fr.di > 0.0 && self.modify_pulse {
            let d = fr.di.min(100.0) / 100.0;
            let closed =
                self.period_len as f64 - GlottalPulse::open_samples(self.period_len, fr.oq);
            delay = (d * closed).round() as usize;
            peak *= 1.0 - d;
        }
        self.pulse = GlottalPulse::new(self.period_len, fr.oq, peak, delay);
    }

    /// Advances one output sample, returning the flow, whether the glottis is
    /// open, and the length in output samples of a period that began.
    fn tick(&mut self, fr: &Frame, av: f64, sr: f64) -> (f64, bool, Option<f64>) {
        let mut started = None;
        for _ in 0..OVERSAMPLE {
            if self.pos >= self.period_len {
                self.start_period(fr, av, sr);
                started = Some(self.period_len as f64 / OVERSAMPLE as f64);
            }
            self.history[self.head] = self.pulse.flow(self.pos);
            self.open_history[self.head] = self.pulse.is_open(self.pos);
            self.head = (self.head + 1) % DECIMATION_TAPS;
            self.pos += 1;
        }
        // The linear-phase decimation filter delays the flow by half its
        // length, so the open phase reported with it must be delayed alike.
        let open = self.open_history[(self.head + DECIMATION_TAPS / 2) % DECIMATION_TAPS];
        let flow = (0..DECIMATION_TAPS)
            .map(|k| self.taps[k] * self.history[(self.head + k) % DECIMATION_TAPS])
            .sum();
        (flow, open, started)
    }
}

/// Gaussian noise through a one-pole low-pass, which the radiation first
/// difference follows. Klatt's (1980) low-pass is an integrator that the
/// radiation exactly cancels, leaving flat noise; this pole of 0.75 leaves a
/// high-pass shelf instead (about 8 dB down at 300 Hz relative to 4 kHz),
/// with which Klatter's consonant and aspiration levels are set.
struct NoiseSource {
    rng: u32,
    lp: f64,
    pole: f64,
    gain: f64,
}

impl NoiseSource {
    fn new(sr: f64) -> Self {
        let pole = 0.75f64.powf(REFERENCE_SAMPLE_RATE / sr);
        // Keep the noise's spectral density and low-pass corner in physical units.
        let gain = (sr / REFERENCE_SAMPLE_RATE).sqrt() * (1.0 - pole) / 0.25;
        NoiseSource {
            rng: 0x2545_F491,
            lp: 0.0,
            pole,
            gain,
        }
    }

    fn uniform(&mut self) -> f64 {
        // xorshift32 keeps output deterministic without external crates.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f64 / u32::MAX as f64) - 0.5
    }

    fn tick(&mut self) -> f64 {
        let r = (0..16).map(|_| self.uniform()).sum::<f64>() * 0.5;
        self.lp = r * self.gain + self.pole * self.lp;
        self.lp
    }
}

/// First difference: the radiation characteristic. It is applied to each
/// source so that the time-varying tract filters are excited by U'g, not Ug.
#[derive(Clone, Copy, Default)]
struct Radiation {
    prev: f64,
}

impl Radiation {
    fn tick(&mut self, x: f64) -> f64 {
        let y = x - self.prev;
        self.prev = x;
        y
    }
}

pub struct Synth {
    sr: f64,
    radiation_gain: f64,
    sample_remainder: u64,
    source: GlottalSource,
    noise: NoiseSource,
    tilt: Resonator,
    tilt_tl: f64,
    rgp_bar: Resonator,
    rgs: Resonator,
    rnp: Resonator,
    rnz: AntiResonator,
    rtp: Resonator,
    rtz: AntiResonator,
    f1_closed: Resonator,
    f1_open: Resonator,
    cascade: [Resonator; 7],
    aspiration_high: [Resonator; 2],
    aspiration_cancel: [AntiResonator; 4],
    parallel: [Resonator; 5],
    parallel_norm: [f64; 5],
    voice_rad: Radiation,
    presence: Peaking,
    bar_rad: Radiation,
    noise_rad: Radiation,
    last_frame: Option<Frame>,
}

impl Synth {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f64;
        let mut s = Synth {
            sr,
            radiation_gain: sr / REFERENCE_SAMPLE_RATE,
            sample_remainder: 0,
            source: GlottalSource::new(),
            noise: NoiseSource::new(sr),
            tilt: Resonator::default(),
            tilt_tl: 0.0,
            rgp_bar: Resonator::default(),
            rgs: Resonator::default(),
            rnp: Resonator::default(),
            rnz: AntiResonator::default(),
            rtp: Resonator::default(),
            rtz: AntiResonator::default(),
            f1_closed: Resonator::default(),
            f1_open: Resonator::default(),
            cascade: [Resonator::default(); 7],
            aspiration_high: [Resonator::default(); 2],
            aspiration_cancel: [AntiResonator::default(); 4],
            parallel: [Resonator::default(); 5],
            parallel_norm: [0.0; 5],
            voice_rad: Radiation::default(),
            presence: Peaking::new(PRESENCE_HZ, PRESENCE_OCTAVES, PRESENCE_DB, sr),
            bar_rad: Radiation::default(),
            noise_rad: Radiation::default(),
            last_frame: None,
        };
        s.tilt.bypass();
        s.rgp_bar.set(0.0, 100.0, sr);
        s.rgs.set(0.0, 200.0, sr);
        s.rnp.set(FNP, BNP, sr);
        s
    }

    fn configure(&mut self, fr: &Frame) {
        let sr = self.sr;
        self.rnz.set(fr.fnz, BNZ, sr);
        self.rtp.set(fr.ftp, fr.btp, sr);
        self.rtz.set(fr.ftz, fr.btz, sr);
        self.f1_closed.set(fr.f[0], fr.b[0], sr);
        self.f1_open.set(fr.f[0] + fr.df1, fr.b[0] + fr.db1, sr);
        self.cascade[1].set(fr.f[1], fr.b[1], sr);
        self.cascade[2].set(fr.f[2], fr.b[2], sr);
        for (i, &(f, b)) in CASCADE_HIGH.iter().enumerate() {
            if f < 0.45 * sr {
                self.cascade[i + 3].set(f, b, sr);
                self.aspiration_cancel[i].set(f, b, sr);
            } else {
                self.cascade[i + 3].bypass();
                self.aspiration_cancel[i] = AntiResonator {
                    a: 1.0,
                    ..AntiResonator::default()
                };
            }
        }
        for (r, &(f, b)) in self.aspiration_high.iter_mut().zip(&ASPIRATION_HIGH) {
            if f < 0.45 * sr {
                r.set(f, b, sr);
            } else {
                r.bypass();
            }
        }
        if fr.tl != self.tilt_tl {
            self.tilt_tl = fr.tl;
            // The tilt resonator keeps running when TL is zero so that its
            // state is current when tilt returns.
            if fr.tl > 0.0 {
                self.tilt.set(0.0, tilt_bandwidth(fr.tl, sr), sr);
            } else {
                self.tilt.bypass();
            }
        }
        let par_freqs = [fr.f[1], fr.f[2], F4, F5, F6];
        for i in 0..5 {
            self.parallel[i].set(par_freqs[i], PARALLEL_BW[i], sr);
            let g = self.parallel[i].gain_at(par_freqs[i], sr);
            self.parallel_norm[i] = if g > 0.0 { 1.0 / g } else { 0.0 };
        }
    }

    fn glide_formants(&mut self, previous: &Frame, fr: &Frame, t: f64) {
        let at = |a: f64, b: f64| a + (b - a) * t;
        let f1 = at(previous.f[0], fr.f[0]);
        let b1 = at(previous.b[0], fr.b[0]);
        self.f1_closed.set(f1, b1, self.sr);
        self.f1_open.set(
            f1 + at(previous.df1, fr.df1),
            b1 + at(previous.db1, fr.db1),
            self.sr,
        );
        for k in 1..3 {
            self.cascade[k].set(
                at(previous.f[k], fr.f[k]),
                at(previous.b[k], fr.b[k]),
                self.sr,
            );
        }
    }

    /// One output sample of the radiated sources: (tilted voicing plus voice
    /// bar, noise, glottis open).
    fn excite(&mut self, fr: &Frame, av: f64, avs: f64, voiced: bool) -> (f64, f64, bool) {
        let (flow, open, started) = self.source.tick(fr, av, self.sr);
        let voice = self
            .presence
            .tick(self.tilt.tick(self.voice_rad.tick(flow)));
        let impulse = started.unwrap_or(0.0);
        let voice_bar = self
            .bar_rad
            .tick(self.rgs.tick(self.rgp_bar.tick(impulse * avs)));

        let mut noise = self.noise_rad.tick(self.noise.tick());
        // Noise is amplitude modulated by the glottal cycle while voicing,
        // as in the 1980 synthesizer.
        if voiced && !open {
            noise *= 0.5;
        }
        // A first difference measures change per sample; radiation is per unit time.
        (
            (voice + voice_bar) * self.radiation_gain,
            noise * self.radiation_gain,
            open,
        )
    }

    /// The index of the first sample `render` produces for `frame`.
    pub fn frame_start(frame: usize, sample_rate: u32) -> usize {
        (frame as u64 * sample_rate as u64 / FRAME_RATE as u64) as usize
    }

    /// Renders frames to floating-point samples (unnormalized), preserving
    /// source, filter and fractional-sample timing state across calls.
    pub fn render(&mut self, frames: &[Frame]) -> Vec<f64> {
        let spf = self.sr * FRAME_MS / 1000.0;
        let mut out = Vec::with_capacity((frames.len() as f64 * spf) as usize + 1);
        for fr in frames {
            let previous = self.last_frame.unwrap_or(*fr);
            let moving = previous.f != fr.f
                || previous.b != fr.b
                || previous.df1 != fr.df1
                || previous.db1 != fr.db1;
            self.configure(fr);
            // Integer accounting avoids drift at rates with fractional samples per frame.
            self.sample_remainder += self.sr as u64;
            let n = (self.sample_remainder / FRAME_RATE as u64) as usize;
            self.sample_remainder %= FRAME_RATE as u64;
            let av = db_to_lin(fr.av);
            let avs = db_to_lin(fr.avs) * VOICE_BAR_GAIN;
            let ah = db_to_lin(fr.ah) * ASPIRATION_GAIN;
            let af = db_to_lin(fr.af);
            let ab = db_to_lin(fr.ab);
            let pa: [f64; 5] = std::array::from_fn(|k| db_to_lin(fr.a[k]) * self.parallel_norm[k]);
            let voiced = av > 0.0 || avs > 0.0;
            for i in 0..n {
                // Stepping a ringing resonator's coefficients injects a transient.
                if moving {
                    self.glide_formants(&previous, fr, (i + 1) as f64 / n as f64);
                }
                let (voice, noise, open) = self.excite(fr, av, avs, voiced);

                let mut breath = noise * ah;
                for z in &mut self.aspiration_cancel {
                    breath = z.tick(breath);
                }
                for r in &mut self.aspiration_high {
                    breath = r.tick(breath);
                }
                let mut x = voice + breath;
                x = self.rnp.tick(x);
                x = self.rnz.tick(x);
                x = self.rtp.tick(x);
                x = self.rtz.tick(x);
                let f1 = if open && av > 0.0 {
                    self.f1_open
                } else {
                    self.f1_closed
                };
                self.cascade[0].copy_coefficients(&f1);
                for k in 0..self.cascade.len() {
                    x = self.cascade[k].tick(x);
                }
                let cascade = x * CASCADE_GAIN;

                let fric = noise * af;
                let mut parallel = fric * ab;
                for k in 0..5 {
                    let sign = if k % 2 == 0 { -1.0 } else { 1.0 };
                    parallel += sign * self.parallel[k].tick(fric * pa[k]);
                }
                out.push(cascade + parallel * PARALLEL_GAIN);
            }
            self.last_frame = Some(*fr);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resonator_has_unity_dc_gain() {
        let mut r = Resonator::default();
        r.set(500.0, 60.0, 16000.0);
        assert!((r.gain_at(0.0, 16000.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn antiresonator_cancels_matching_resonator() {
        let mut r = Resonator::default();
        let mut z = AntiResonator::default();
        r.set(270.0, 100.0, 16000.0);
        z.set(270.0, 100.0, 16000.0);
        let input: Vec<f64> = (0..200).map(|i| ((i * 7919) % 13) as f64 - 6.0).collect();
        for &x in &input {
            let y = z.tick(r.tick(x));
            assert!((y - x).abs() < 1e-6);
        }
    }

    fn dft_mag(signal: &[f64], f: f64, sr: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &x) in signal.iter().enumerate() {
            let w = 2.0 * PI * f * n as f64 / sr;
            re += x * w.cos();
            im -= x * w.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn vowel_spectrum_peaks_near_formants() {
        let sr = 16000.0;
        let frame = Frame {
            f0: 100.0,
            av: 60.0,
            f: [700.0, 1200.0, 2600.0],
            b: [80.0, 90.0, 150.0],
            ..Frame::default()
        };
        let mut synth = Synth::new(sr as u32);
        let out = synth.render(&vec![frame; 100]);
        let tail = &out[out.len() / 2..];
        // Harmonics of 100 Hz: the harmonic at F1 must beat ones well away from it.
        let at_f1 = dft_mag(tail, 700.0, sr);
        let away = dft_mag(tail, 1800.0, sr);
        assert!(at_f1 > away * 3.0, "F1 {at_f1} vs trough {away}");
        let at_f2 = dft_mag(tail, 1200.0, sr);
        assert!(at_f2 > away, "F2 {at_f2} vs trough {away}");
    }

    #[test]
    fn formant_changes_do_not_step_the_first_sample() {
        let from = Frame {
            f0: 100.0,
            av: 60.0,
            f: [350.0, 1100.0, 2600.0],
            ..Frame::default()
        };
        let to = Frame {
            f: [700.0, 2200.0, 2800.0],
            b: [200.0, 150.0, 250.0],
            ..from
        };
        for sr in [16000, 22050, 48000] {
            let mut synth = Synth::new(sr);
            let before = synth.render(&[from; 37]);
            let peak = before.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            let changed = synth.render(&[to]);
            let mut control = Synth::new(sr);
            control.render(&[from; 37]);
            let unchanged = control.render(&[from]);
            let step = (changed[0] - unchanged[0]).abs();
            assert!(
                step < 0.05 * peak,
                "{sr} Hz: first-sample step {step:.6}, steady peak {peak:.6}"
            );
        }
    }

    #[test]
    fn tilt_resonator_attenuates_by_tl_at_3khz() {
        let sr = 16000.0;
        for tl in [6.0, 20.0, 41.0] {
            let mut r = Resonator::default();
            r.set(0.0, tilt_bandwidth(tl, sr), sr);
            let db = 20.0 * r.gain_at(3000.0, sr).log10();
            assert!((db + tl).abs() < 0.01, "TL {tl}: {db} dB");
        }
    }

    #[test]
    fn presence_boosts_midrange_without_boosting_spectral_extremes() {
        for sr in [8000.0, 16000.0, 16001.0, 22050.0, 44100.0, 48000.0] {
            let p = Peaking::new(PRESENCE_HZ, PRESENCE_OCTAVES, PRESENCE_DB, sr);
            let db = |f: f64| 20.0 * p.gain_at(f, sr).log10();
            assert!((db(2200.0) - PRESENCE_DB).abs() < 0.01, "{sr}");
            assert!(db(50.0).abs() < 0.1 && db(0.49 * sr).abs() < 0.5, "{sr}");
        }
    }

    #[test]
    fn presence_does_not_change_noise_or_voice_bar() {
        for source in [
            Frame {
                ah: 55.0,
                ..Frame::default()
            },
            Frame {
                af: 60.0,
                ab: 60.0,
                ..Frame::default()
            },
            Frame {
                avs: 47.0,
                ..Frame::default()
            },
        ] {
            let mut synth = Synth::new(16000);
            let boosted = synth.render(&[source; 40]);
            let mut control = Synth::new(16000);
            control.presence = Peaking::new(PRESENCE_HZ, PRESENCE_OCTAVES, 0.0, 16000.0);
            assert_eq!(boosted, control.render(&[source; 40]));
        }
    }

    #[test]
    fn glottal_pulse_opens_then_closes_for_rest_of_period() {
        let p = GlottalPulse::new(640, 50.0, 1.0, 0);
        assert!(p.is_open(0) && p.is_open(319) && !p.is_open(320));
        let peak = (0..320).map(|i| p.flow(i)).fold(0.0, f64::max);
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
        assert_eq!(p.flow(400), 0.0);
    }

    #[test]
    fn double_pulsing_delays_and_attenuates_alternate_pulses() {
        // The paper's example: OQ 50% and DI 50% delay the first pulse of each
        // pair by a quarter period and halve it.
        let fr = Frame {
            f0: 100.0,
            di: 50.0,
            ..Frame::default()
        };
        let mut src = GlottalSource::new();
        let peaks: Vec<(usize, f64)> = (0..2)
            .map(|_| {
                src.start_period(&fr, 1.0, 16000.0);
                let peak = (0..src.period_len)
                    .map(|i| src.pulse.flow(i))
                    .fold(0.0, f64::max);
                (src.pulse.start, peak)
            })
            .collect();
        assert_eq!(peaks[0].0, src.period_len / 4);
        assert!((peaks[0].1 / peaks[1].1 - 0.5).abs() < 0.01);
        assert_eq!(peaks[1].0, 0);
    }

    #[test]
    fn aspiration_at_av_level_matches_high_harmonics_of_default_voicing() {
        let sr = 16000.0;
        let win = (0.0256 * sr) as usize;
        let hamming: Vec<f64> = (0..win)
            .map(|n| 0.54 - 0.46 * (2.0 * PI * n as f64 / (win - 1) as f64).cos())
            .collect();
        let windowed = |x: &[f64]| {
            x.iter()
                .zip(&hamming)
                .map(|(a, w)| a * w)
                .collect::<Vec<_>>()
        };
        let freqs: Vec<f64> = (20..=40).map(|h| h as f64 * 100.0).collect();
        let level = |x: &[f64]| {
            let p: f64 = freqs.iter().map(|&f| dft_mag(x, f, sr).powi(2)).sum();
            10.0 * (p / freqs.len() as f64).log10()
        };

        // Through the production source path, so the noise is gated by the
        // glottal cycle exactly as in a voiced frame.
        let fr = Frame {
            f0: 100.0,
            av: 60.0,
            ..Frame::default()
        };
        let mut synth = Synth::new(sr as u32);
        synth.configure(&fr);
        let (voice, noise): (Vec<f64>, Vec<f64>) = (0..4000 + 40 * win)
            .map(|_| synth.excite(&fr, db_to_lin(60.0), 0.0, true))
            .map(|(v, n, _)| (v, n))
            .unzip();
        let voice_db = level(&windowed(&voice[4000..4000 + win]));

        let gain = db_to_lin(60.0) * ASPIRATION_GAIN;
        let noise_power: f64 = noise[4000..]
            .chunks_exact(win)
            .map(|c| {
                let scaled: Vec<f64> = c.iter().map(|x| x * gain).collect();
                10f64.powf(level(&windowed(&scaled)) / 10.0)
            })
            .sum::<f64>()
            / 40.0;
        let noise_db = 10.0 * noise_power.log10();
        assert!(
            (noise_db - voice_db).abs() < 1.0,
            "noise {noise_db:.1} dB vs voicing {voice_db:.1} dB"
        );
    }

    #[test]
    fn chunked_rendering_matches_whole_rendering() {
        let frames: Vec<Frame> = (0..200)
            .map(|i| Frame {
                f0: 100.0 + i as f64 / 4.0,
                av: if i % 9 < 6 { 60.0 } else { 0.0 },
                ah: 40.0,
                tl: (i % 20) as f64,
                af: if i % 9 >= 6 { 55.0 } else { 0.0 },
                ab: 50.0,
                f: [
                    400.0 + (i % 17) as f64 * 10.0,
                    1200.0 + (i % 13) as f64 * 25.0,
                    2500.0,
                ],
                b: [70.0 + (i % 11) as f64 * 3.0, 90.0, 150.0],
                df1: if i % 7 < 3 { 30.0 } else { 0.0 },
                ..Frame::default()
            })
            .collect();
        for sr in [8000, 16000, 16001, 22050, 44100, 48000] {
            let whole = Synth::new(sr).render(&frames);
            assert_eq!(whole.len(), sr as usize);
            assert!(whole.iter().all(|s| s.is_finite()));
            for chunk_size in [1, 7, 37] {
                let mut synth = Synth::new(sr);
                let mut chunked = Vec::new();
                for chunk in frames.chunks(chunk_size) {
                    assert!(synth.render(&[]).is_empty());
                    chunked.extend(synth.render(chunk));
                }
                assert_eq!(chunked.len(), whole.len(), "{sr} Hz, {chunk_size} frames");
                assert!(
                    chunked == whole,
                    "{sr} Hz, {chunk_size} frames: waveform mismatch"
                );
            }
        }
    }

    #[test]
    fn source_levels_are_consistent_across_sample_rates() {
        let sources = [
            (
                "voicing",
                Frame {
                    av: 60.0,
                    ..Frame::default()
                },
            ),
            (
                "voice bar",
                Frame {
                    avs: 47.0,
                    ..Frame::default()
                },
            ),
            (
                "aspiration",
                Frame {
                    ah: 55.0,
                    ..Frame::default()
                },
            ),
            (
                "frication",
                Frame {
                    af: 60.0,
                    a: [0.0, 55.0, 50.0, 45.0, 40.0],
                    ..Frame::default()
                },
            ),
        ];
        let rms = |frame: Frame, sr| {
            let audio = Synth::new(sr).render(&vec![frame; 400]);
            let tail = &audio[audio.len() / 2..];
            (tail.iter().map(|x| x * x).sum::<f64>() / tail.len() as f64).sqrt()
        };
        for (name, frame) in sources {
            let reference = rms(frame, 16000);
            for sr in [22050, 44100, 48000] {
                let db = 20.0 * (rms(frame, sr) / reference).log10();
                assert!(db.abs() < 1.5, "{name} at {sr} Hz: {db:.2} dB");
            }
        }
    }

    #[test]
    fn open_phase_flag_tracks_filtered_glottal_flow() {
        // The flag that gates noise and DF1 must mark when the decimated flow
        // passes, whose t^2(1-t) pulse has its centroid at 0.6 of the open phase.
        for sr in [16000.0, 48000.0] {
            for f0 in [100.0, 220.0] {
                let fr = Frame {
                    f0,
                    av: 60.0,
                    ..Frame::default()
                };
                let mut src = GlottalSource::new();
                let ticks: Vec<_> = (0..sr as usize / 2)
                    .map(|_| src.tick(&fr, 1.0, sr))
                    .collect();
                let starts: Vec<usize> =
                    (0..ticks.len()).filter(|&i| ticks[i].2.is_some()).collect();
                let offsets: Vec<f64> = starts[2..]
                    .windows(2)
                    .map(|w| {
                        let period = &ticks[w[0]..w[1]];
                        let open: Vec<f64> = (0..period.len())
                            .filter(|&i| period[i].1)
                            .map(|i| i as f64)
                            .collect();
                        let window_centre = open.iter().sum::<f64>() / open.len() as f64;
                        let mass: f64 = period.iter().map(|t| t.0).sum();
                        let centroid = period
                            .iter()
                            .enumerate()
                            .map(|(i, t)| i as f64 * t.0)
                            .sum::<f64>()
                            / mass;
                        (centroid - window_centre) / open.len() as f64
                    })
                    .collect();
                let offset = offsets.iter().sum::<f64>() / offsets.len() as f64;
                assert!(
                    (offset - 0.1).abs() < 0.03,
                    "{sr} Hz, F0 {f0}: flow centroid at {offset:.3} of open phase"
                );
            }
        }
    }

    #[test]
    fn silent_frames_render_silence() {
        let mut synth = Synth::new(16000);
        let out = synth.render(&vec![Frame::default(); 10]);
        assert_eq!(out.len(), 800);
        assert!(out.iter().all(|&x| x.abs() < 1e-12));
    }
}
