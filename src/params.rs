//! Synthesis by rule: converts timed phoneme segments into Klatt parameter
//! frames, with formant transitions, stop closures, bursts and aspiration.

use crate::klatt::{FRAME_MS, Frame};
use crate::phoneme::{Class, Ph, Place};
use crate::prosody;
use crate::utterance::Seg;

const ASPIRATION_DB: f64 = 55.0;
const VOICE_BAR_DB: f64 = 47.0;
/// Keeps short frication and release cues prominent relative to vowels.
const CONSONANT_NOISE_BOOST_DB: f64 = 6.0;
/// /h/ is voiced with noise on top rather than pure noise: between voiced
/// sounds English /h/ is usually breathy-voiced [ɦ] (Ladefoged & Johnson
/// 2011). Its voicing is 10 dB below a vowel's, Klatt's (1980) level for
/// sonorant consonants, under the usual aspiration.
const HH_AV_DB: f64 = crate::phoneme::VOWEL_AV - 10.0;
const HH_AH_DB: f64 = ASPIRATION_DB;
/// Voicing and frication change across a boundary rather than within one
/// 5 ms parameter update, which sounds like clips spliced together. The ramp
/// spans the nearest symmetric number of frames, 25 ms, to Hertz's (1991,
/// Fig. 7) 30 ms fricative-to-vowel transition, across which frication gives
/// way to voicing.
const CROSS_FADE_FRAMES: usize = 5;
/// Fades run from this inaudible level so they span only the audible range.
const FADE_FLOOR_DB: f64 = 30.0;
const NASAL_ZERO_HZ: f64 = 450.0;
/// Vowels are nasalized as the velum opens ahead of a nasal and closes after
/// it; that nasalization is the main cue to a nasal coda ("bank" vs "back").
const NASALIZE_BEFORE_MS: f64 = 30.0;
const NASALIZE_AFTER_MS: f64 = 30.0;
/// Affricates need most of their time as frication, or CH is heard as T.
const AFFRICATE_RELEASE_SHARE: f64 = 0.65;

/// KLGLOTT88 source settings for a male voice: modal phonation at the
/// synthesizer's default open quotient, then the fully breathy values (Klatt &
/// Klatt 1990, Table XII and Sec. III A).
const MODAL_OQ: f64 = 50.0;
const BREATHY_OQ: f64 = 80.0;
const BREATHY_TL: f64 = 20.0;
const BREATHY_AH: f64 = 52.0;
/// Table XIV stimulus 11: B1 200 to 400 Hz and B2 110 to 170 Hz.
const BREATHY_B1_HZ: f64 = 200.0;
const BREATHY_B2_HZ: f64 = 60.0;
/// Fig. 15: AV falls about 10 dB over a breathy offset into silence.
const OFFSET_AV_DROP_DB: f64 = 10.0;
/// Ramp lengths read from Figs. 15 and 16.
const OFFSET_RAMP_MS: f64 = 100.0;
const VOICELESS_RAMP_MS: f64 = 80.0;
const VOICELESS_PEAK: f64 = 0.8;
/// Klatt & Klatt (1990) found the first harmonic at mid-vowel no stronger
/// after [h] than after a glottal stop (Table II): even [h]'s wide glottal
/// opening does not reach the middle of the vowel. So the breathy margins of
/// voiceless consonants stay within this share of a sonorant at each end.
const VOICELESS_MARGIN_SHARE: f64 = 1.0 / 3.0;
/// Stressed syllables are spoken with more vocal effort and are more intense
/// (Fry 1955; Lehiste 1970). Klatt & Klatt's (1990) male talkers' stressed
/// syllables were 4.3 dB more intense at mid-vowel, partly from their higher
/// F0, which the pitch contour already supplies. Klatt (1987) notes that F0
/// alone raises the intensity of stressed vowels and that a specific rule to
/// increase their intensity made them sound artificially strong. Klatter
/// instead takes a modest part of the difference from unstressed vowels,
/// which a listener preferred in blind comparison.
const UNSTRESSED_AV_DROP_DB: f64 = 3.0;

fn ms(x: f64) -> usize {
    (x / FRAME_MS).round() as usize
}

/// AY, AW and OY are an onset vowel and an offglide joined by an interior
/// transition (Hertz, Younes & Zinovieva 1999), timed like a vowel-vowel
/// transition (Hertz 1991, Figs. 6-9) and stable before voiced and voiceless
/// consonants alike: the onset vowel absorbs the change (pp. 99-100). The
/// offglide holds for 10 ms before the next transition (Hertz & Huffman 1992,
/// p. 1171). When the vowel is too short, the onset keeps a third of it and
/// the glide is cut off short of the offglide, as in Hertz's fast [said],
/// whose [a] keeps 35 ms and whose interior transition stops after 70 of its
/// 100 ms (1991, Fig. 17, p. 106): the glide keeps its slope where the vowel
/// leaves room for 70% of it, and steepens where it does not, so it always
/// gets that far.
const DIPHTHONG_OFFGLIDE_MS: f64 = 10.0;
const MIN_ONSET: f64 = 1.0 / 3.0;
const MIN_GLIDE_REACHED: f64 = 0.7;
/// Velar closures are made wherever the tongue body sits for the vowel, so
/// the velar F2 locus follows the vowel's F2 instead of one fixed value: the
/// locus equation for /g/ onsets of Sussman, McCaffrey & Matthews' (1991,
/// Table I) male talkers, F2 onset = 0.66 F2 vowel + 807 Hz.
const VELAR_LOCUS_BASE_HZ: f64 = 807.0;
const VELAR_LOCUS_SLOPE: f64 = 0.66;

/// F2 locus of the velar stop at `i`, from the vowel it releases into, or
/// else the vowel before it.
fn velar_f2_locus(segs: &[Seg], i: usize) -> Option<f64> {
    let ph = segs[i].ph?;
    if ph.class() != Class::Stop || ph.place() != Place::Velar {
        return None;
    }
    let vowel = |j: usize| {
        segs.get(j)
            .and_then(|s| s.ph)
            .filter(|p| p.is_vowel())
            .map(Ph::spec)
    };
    let f2 = vowel(i + 1).map(|s| s.f[1]).or_else(|| {
        i.checked_sub(1)
            .and_then(vowel)
            .map(|s| s.f_end.unwrap_or(s.f)[1])
    })?;
    Some(VELAR_LOCUS_BASE_HZ + VELAR_LOCUS_SLOPE * f2)
}

/// Klatt's (1980) labial and alveolar stop F2 and F3 targets are virtual
/// loci: at the release the formants lie between the locus and the vowel.
/// A fixed onset (a "hub") makes listeners hear the wrong consonant before
/// some vowels (Delattre et al. 1955, in Klatt 1987, Fig. 16), so the stop
/// takes the point `k` of the way from its locus to the vowel it releases
/// into, or else the vowel before it, with Delattre's and Holmes's k of 0.5
/// (Klatt 1987, note 4).
const LOCUS_K: f64 = 0.5;

/// F2 and F3 at the labial or alveolar stop at `i`, from its virtual loci.
fn stop_f2_f3(segs: &[Seg], i: usize) -> Option<[f64; 2]> {
    let ph = segs[i].ph?;
    if ph.class() != Class::Stop || !matches!(ph.place(), Place::Labial | Place::Alveolar) {
        return None;
    }
    let vowel = |j: usize| {
        segs.get(j)
            .and_then(|s| s.ph)
            .filter(|p| p.is_vowel())
            .map(Ph::spec)
    };
    let v = vowel(i + 1).map(|s| s.f).or_else(|| {
        i.checked_sub(1)
            .and_then(vowel)
            .map(|s| s.f_end.unwrap_or(s.f))
    })?;
    let locus = ph.spec().f;
    Some([1, 2].map(|j| lerp(locus[j], v[j], LOCUS_K)))
}

/// (frames before, frames after) each boundary: the transition between the
/// two phones, timed in `timing`, in whole frames.
fn transition_windows(segs: &[Seg], lens: &[usize]) -> Vec<(usize, usize)> {
    let frames = |x: f64| (x / FRAME_MS).round() as usize;
    let mut windows: Vec<(usize, usize)> = (0..segs.len().saturating_sub(1))
        .map(|i| {
            (
                frames(segs[i].trans_out_ms),
                frames(segs[i + 1].trans_in_ms),
            )
        })
        .collect();
    // Frame rounding can still crowd a short phone; keep one target frame.
    for i in 0..segs.len() {
        let incoming = if i > 0 { windows[i - 1].1 } else { 0 };
        let outgoing = windows.get(i).map_or(0, |w| w.0);
        let available = lens[i].saturating_sub(1);
        if incoming + outgoing > available {
            // Share only contested time, keeping one target frame between transitions.
            let aspiration = if i > 0 {
                aspiration_frames(segs, i - 1, lens[i])
            } else {
                0
            };
            let incoming = incoming
                .min(available / 2)
                .max(available.saturating_sub(outgoing))
                .max(aspiration);
            if i > 0 {
                windows[i - 1].1 = incoming;
            }
            if let Some(w) = windows.get_mut(i) {
                w.0 = outgoing.min(available - incoming);
            }
        }
    }
    windows
}

#[derive(Clone, Copy)]
struct Formants {
    f: [f64; 3],
    b: [f64; 3],
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Per-frame formant targets of a segment, before coarticulation. `incoming`
/// frames at its start are covered by the transition from the segment
/// before, and a diphthong glides for `glide_ms`.
fn targets(ph: Ph, len: usize, incoming: usize, glide_ms: f64) -> Vec<Formants> {
    let spec = ph.spec();
    let frames = |ms: f64| ms / FRAME_MS;
    // AY, AW and OY hold the onset vowel from the end of the transition into
    // it and glide just before the next consonant, so the open onset of
    // "time" or "down" is heard. EY and OW glide throughout.
    let settled = incoming.min(len / 2) as f64;
    let span = len as f64 - settled;
    let (glide_start, glide_len) = match ph {
        Ph::AY | Ph::AW | Ph::OY => {
            let earliest = settled + MIN_ONSET * span;
            let glide = frames(glide_ms)
                .min((len as f64 - 1.0 - earliest) / MIN_GLIDE_REACHED)
                .max(1.0);
            let start = len as f64 - frames(DIPHTHONG_OFFGLIDE_MS) - glide;
            (start.max(earliest), glide)
        }
        _ => (0.25 * len as f64, 0.6 * len as f64),
    };
    (0..len)
        .map(|k| match spec.f_end {
            Some(end) => {
                let t = ((k as f64 - glide_start) / glide_len.max(1.0)).clamp(0.0, 1.0);
                let f = std::array::from_fn(|j| lerp(spec.f[j], end[j], t));
                Formants {
                    f,
                    b: crate::phoneme::vowel_bandwidths(f),
                }
            }
            None => Formants {
                f: spec.f,
                b: spec.b,
            },
        })
        .collect()
}

pub fn frame_layout(segs: &[Seg]) -> (Vec<usize>, Vec<usize>) {
    let mut starts = Vec::with_capacity(segs.len());
    let mut lens = Vec::with_capacity(segs.len());
    let mut elapsed_ms = 0.0;
    let mut frame = 0usize;
    for s in segs {
        elapsed_ms += s.dur_ms;
        // Cumulative rounding keeps total timing exact despite 5 ms frames.
        let end = ((elapsed_ms / FRAME_MS).round() as usize).max(frame + 1);
        starts.push(frame);
        lens.push(end - frame);
        frame = end;
    }
    (starts, lens)
}

fn formant_tracks(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<Formants>> {
    let n = segs.len();
    let windows = transition_windows(segs, lens);
    // Pauses and /h/ borrow formants from a neighbour so nothing jumps oddly.
    let resolved: Vec<Option<Ph>> = (0..n)
        .map(|i| match segs[i].ph {
            Some(Ph::HH) | None => (i + 1..n)
                .chain((0..i).rev())
                .find_map(|j| segs[j].ph.filter(|p| *p != Ph::HH)),
            // Both flaps are voiced taps with /d/'s low F1.
            Some(Ph::T) if segs[i].flap => Some(Ph::D),
            p => p,
        })
        .collect();
    let mut track: Vec<Formants> = Vec::new();
    for i in 0..n {
        if cancelled() {
            return None;
        }
        let ph = resolved[i].unwrap_or(Ph::AX);
        // The offglide must finish before the outgoing transition can leave its target.
        let outgoing = if ph.spec().f_end.is_some() {
            windows.get(i).map_or(0, |w| w.0)
        } else {
            0
        };
        let incoming = if i > 0 { windows[i - 1].1 } else { 0 };
        let mut t = targets(ph, lens[i] - outgoing, incoming, segs[i].glide_ms);
        t.resize(lens[i], *t.last().unwrap());
        if let Some(f2) = velar_f2_locus(segs, i) {
            for f in &mut t {
                f.f[1] = f2;
            }
        }
        if let Some([f2, f3]) = stop_f2_f3(segs, i) {
            for f in &mut t {
                f.f[1] = f2;
                f.f[2] = f3;
            }
        }
        if segs[i].ph == Some(Ph::HH) {
            for f in &mut t {
                f.b = [300.0, 150.0, 200.0];
            }
        }
        track.extend(t);
    }
    let target = track.clone();

    for (i, &(left_frames, right_frames)) in windows.iter().enumerate() {
        if cancelled() {
            return None;
        }
        let boundary = starts[i + 1];
        let a = boundary - left_frames;
        let b = boundary + right_frames;
        if a == 0 || b >= target.len() || a == b {
            continue;
        }
        let from = target[a - 1];
        let to = target[b];
        let span = (b - a + 1) as f64;
        for k in a..b {
            let t = (k - a + 1) as f64 / span;
            track[k] = Formants {
                f: std::array::from_fn(|j| lerp(from.f[j], to.f[j], t)),
                b: std::array::from_fn(|j| lerp(from.b[j], to.b[j], t)),
            };
        }
    }
    Some(track)
}

fn release_frames(ph: Ph) -> usize {
    if ph.place() == Place::Velar { 3 } else { 2 }
}

fn noise_level(af: f64) -> f64 {
    if af > 0.0 {
        af + CONSONANT_NOISE_BOOST_DB
    } else {
        0.0
    }
}

fn aspiration_frames(segs: &[Seg], i: usize, next_len: usize) -> usize {
    ms(segs[i + 1].aspiration_ms).min(next_len.saturating_sub(1))
}

fn is_voiceless(s: Option<&Seg>) -> bool {
    s.is_some_and(|s| s.ph.is_some_and(|p| !p.is_voiced()) && !s.flap)
}

/// Per-frame (breathiness, offset) for a sonorant: breathiness 0 is modal and
/// 1 fully breathy; offset rises from 0 to 1 over the end of voicing before
/// silence.
/// Klatt & Klatt (1990) found breathiness rises toward vowel offsets into
/// silence and near voiceless consonants, and is greater in sentence-final
/// syllables. Their male talkers' first harmonic at mid-vowel was no
/// stronger in unstressed syllables than in stressed ones (Table II), and
/// their vowels' noise ratings were low, if somewhat higher in unstressed
/// syllables (Tables V and VI), so for
/// this male voice unstressed syllables keep modal phonation; Klatt's (1987)
/// rules likewise kept breathiness for utterance offsets.
fn breathiness(segs: &[Seg], lens: &[usize], i: usize, time_scale: f64) -> Vec<(f64, f64)> {
    let s = &segs[i];
    let len = lens[i];
    let prev = i.checked_sub(1).map(|j| &segs[j]);
    let next = segs.get(i + 1);
    // Klatt & Klatt's (1990, Fig. 15) offset ramp spans the last stretch of
    // voicing, so it starts in the vowel before a short final sonorant such
    // as the /l/ of "table" instead of starting halfway at the /l/.
    let run_end = (i + 1..segs.len())
        .find(|&j| !segs[j].ph.is_some_and(Ph::is_sonorant))
        .unwrap_or(segs.len());
    let before_silence = segs.get(run_end).is_none_or(|n| n.ph.is_none());
    let tail: usize = lens[i + 1..run_end].iter().sum();
    let frames = |ms: f64| (ms * time_scale / FRAME_MS).max(1.0);
    // A breathy onset after a voiceless plosive starts where voicing does,
    // after the aspiration.
    let voicing = i
        .checked_sub(1)
        .map_or(0, |j| aspiration_frames(segs, j, len));
    let voiced_len = (len - voicing) as f64;
    let margin = frames(VOICELESS_RAMP_MS).min((voiced_len * VOICELESS_MARGIN_SHARE).max(1.0));
    let floor: f64 = if s.phrase_final { 0.25 } else { 0.0 };
    (0..len)
        .map(|k| {
            let from_end = (len - k) as f64;
            let mut b = floor;
            let mut offset = 0.0;
            if before_silence {
                let to_silence = from_end + tail as f64;
                offset = (1.0 - to_silence / frames(OFFSET_RAMP_MS)).max(0.0);
                b = b.max(offset);
            } else if is_voiceless(next) {
                b = b.max(VOICELESS_PEAK * (1.0 - from_end / margin));
            }
            if is_voiceless(prev) {
                let from_onset = (k + 1).saturating_sub(voicing) as f64;
                b = b.max(VOICELESS_PEAK * (1.0 - from_onset / margin));
            }
            (b.clamp(0.0, 1.0), offset)
        })
        .collect()
}

/// Applies breathy phonation (Klatt & Klatt 1990, Sec. III A steps 4 to 6):
/// a higher open quotient strengthens the first harmonic, spectral tilt
/// weakens the upper harmonics, aspiration noise replaces them, and glottal
/// losses widen B1 and B2.
fn apply_voice_quality(fr: &mut Frame, b: f64, offset: f64) {
    fr.oq = lerp(MODAL_OQ, BREATHY_OQ, b);
    fr.tl = BREATHY_TL * b;
    fr.b[0] += BREATHY_B1_HZ * b;
    fr.b[1] += BREATHY_B2_HZ * b;
    fr.av -= OFFSET_AV_DROP_DB * offset;
    if b > 0.0 {
        // Noise amplitude grows in proportion to breathiness, so it fades in
        // from silence rather than switching on.
        fr.ah = fr.ah.max(BREATHY_AH + 20.0 * b.log10());
    }
}

pub fn build_frames(segs: &[Seg]) -> Vec<Frame> {
    let (starts, lens) = frame_layout(segs);
    build_frames_with_layout(segs, &starts, &lens)
}

/// Builds frames using the layout returned by [`frame_layout`] for these segments.
pub fn build_frames_with_layout(segs: &[Seg], starts: &[usize], lens: &[usize]) -> Vec<Frame> {
    build_frames_with_time_scale(segs, starts, lens, 1.0)
}

/// Pitch accents and breathiness ramps must follow the speaking-rate scale.
pub fn build_frames_with_time_scale(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    time_scale: f64,
) -> Vec<Frame> {
    build_frames_cancellable(segs, starts, lens, time_scale, &|| false)
        .expect("uncancelled frame planning")
}

pub(crate) fn build_frames_cancellable(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    time_scale: f64,
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<Frame>> {
    assert_eq!(segs.len(), starts.len());
    assert_eq!(segs.len(), lens.len());
    let formants = formant_tracks(segs, starts, lens, cancelled)?;
    let f0 = prosody::f0_contour_cancellable(segs, starts, lens, FRAME_MS, time_scale, cancelled)?;
    let mut frames: Vec<Frame> = (0..formants.len())
        .map(|k| Frame {
            f0: f0[k],
            f: formants[k].f,
            b: formants[k].b,
            ..Frame::default()
        })
        .collect();

    for i in 0..segs.len() {
        if cancelled() {
            return None;
        }
        let Some(ph) = segs[i].ph else { continue };
        let spec = ph.spec();
        let range = starts[i]..starts[i] + lens[i];
        let len = lens[i];
        match ph.class() {
            Class::Vowel | Class::Liquid | Class::Glide | Class::Nasal => {
                let unstressed = ph.is_vowel() && (segs[i].stress == 0 || segs[i].function_word);
                let av = spec.av
                    - if unstressed {
                        UNSTRESSED_AV_DROP_DB
                    } else {
                        0.0
                    };
                for fr in &mut frames[range] {
                    fr.av = av;
                    if ph.class() == Class::Nasal {
                        fr.fnz = NASAL_ZERO_HZ;
                    }
                }
            }
            Class::Fricative => {
                for fr in &mut frames[range] {
                    fr.av = spec.av;
                    fr.af = noise_level(spec.af);
                    fr.ab = spec.ab;
                    fr.a = spec.a;
                }
            }
            Class::Aspirate => {
                for fr in &mut frames[range] {
                    fr.av = HH_AV_DB;
                    fr.ah = HH_AH_DB;
                }
            }
            Class::Stop if segs[i].flap => {
                // A tap is too brief to stop voicing and has no release
                // burst; voicing only drops to the level of a voiced closure.
                for fr in &mut frames[range] {
                    fr.av = VOICE_BAR_DB;
                }
            }
            Class::Stop | Class::Affricate => {
                let release = if ph.class() == Class::Affricate {
                    (len as f64 * AFFRICATE_RELEASE_SHARE).round() as usize
                } else {
                    release_frames(ph).min(len.saturating_sub(1)).max(1)
                };
                let closure = len - release;
                for (k, fr) in frames[range].iter_mut().enumerate() {
                    if k < closure {
                        // Only voiced closures carry a voice bar (Klatt
                        // 1980); voicing stops for a voiceless one (Hertz
                        // 1991).
                        if ph.is_voiced() {
                            fr.avs = VOICE_BAR_DB;
                        }
                    } else {
                        fr.av = spec.av;
                        fr.af = noise_level(spec.af);
                        fr.ab = spec.ab;
                        fr.a = spec.a;
                        if ph.class() == Class::Stop && ph.is_voiced() {
                            fr.avs = VOICE_BAR_DB;
                        }
                    }
                }
            }
        }
    }

    cross_fade(segs, starts, lens, &mut frames);
    // Voicing stops for a voiceless plosive or affricate (Hertz 1991, Figs.
    // 14-15, turns it off before the closure) and starts again only after the
    // burst and any aspiration, which is what voice-onset time measures
    // (Klatt 1987), so the cross-fade must not carry the vowels' voicing into
    // its closure or release: voicing fades within the vowel before it.
    let half = CROSS_FADE_FRAMES / 2;
    for i in 0..segs.len() {
        let voiceless_stop = segs[i]
            .ph
            .is_some_and(|p| matches!(p.class(), Class::Stop | Class::Affricate) && !p.is_voiced());
        if segs[i].flap || !voiceless_stop {
            continue;
        }
        for fr in &mut frames[starts[i]..starts[i] + lens[i]] {
            fr.av = 0.0;
        }
        let Some(from) = starts[i].checked_sub(half + 1) else {
            continue;
        };
        let level = frames[from].av;
        if level > 0.0 {
            for d in 1..=half {
                let fade = lerp(FADE_FLOOR_DB, level, d as f64 / (half + 1) as f64);
                let fr = &mut frames[starts[i] - d];
                fr.av = fr.av.min(fade);
            }
        }
    }
    // A flap is a brief tap of the tongue tip (Zue & Laferriere 1979), heard
    // as a short dip in voicing. Averaged with the vowels around it, a one-
    // or two-frame tap at fast rates lost all but 1-2 dB of its dip, so its
    // own frames keep the voiced-closure level after the cross-fade.
    for i in 0..segs.len() {
        if segs[i].flap {
            for fr in &mut frames[starts[i]..starts[i] + lens[i]] {
                fr.av = fr.av.min(VOICE_BAR_DB);
            }
        }
    }
    nasalize_vowels(segs, starts, lens, &mut frames);
    if cancelled() {
        return None;
    }

    // Aspiration replaces voicing at the start of the sonorant after a
    // voiceless plosive, which is how voice-onset time is realized.
    for i in 0..segs.len().saturating_sub(1) {
        if cancelled() {
            return None;
        }
        let n = aspiration_frames(segs, i, lens[i + 1]);
        for fr in &mut frames[starts[i + 1]..starts[i + 1] + n] {
            fr.av = 0.0;
            fr.ah = ASPIRATION_DB;
            fr.b[0] += 150.0;
        }
        if n == 0 {
            continue;
        }
        // The folds adduct gradually after aspiration (Lofqvist & McGowan
        // 1992), so voicing grows over the same cross-fade as other onsets
        // rather than switching on at full strength, continuing into the
        // next phone when aspiration leaves a short sonorant little voicing.
        let onset = starts[i + 1] + n;
        for (d, fr) in frames[onset..].iter_mut().take(half).enumerate() {
            if fr.av > 0.0 {
                let voiced = (half + 1 + d) as f64 / CROSS_FADE_FRAMES as f64;
                fr.av = lerp(FADE_FLOOR_DB, fr.av, voiced);
            }
        }
    }

    for i in 0..segs.len() {
        if cancelled() {
            return None;
        }
        if !segs[i].ph.is_some_and(Ph::is_sonorant) {
            continue;
        }
        let b = breathiness(segs, lens, i, time_scale);
        for (fr, b) in frames[starts[i]..starts[i] + lens[i]].iter_mut().zip(b) {
            if fr.av > 0.0 {
                apply_voice_quality(fr, b.0, b.1);
            }
        }
    }
    for i in 0..segs.len() {
        if cancelled() {
            return None;
        }
        if segs[i].ph.is_some() {
            continue;
        }
        let start = starts[i];
        // A new pulse in a pause misses the sonorant's breathy offset.
        for fr in &mut frames[start..start + lens[i]] {
            fr.av = 0.0;
        }
        if i == 0 {
            continue;
        }
        // Retuning the tract or tilt while the last pulse decays can click.
        // Reserve only the next sound's cross-fade for its own settings.
        let last = frames[start - 1];
        let hold = if i + 1 == segs.len() {
            lens[i]
        } else {
            lens[i].saturating_sub(half)
        };
        for fr in &mut frames[start..start + hold] {
            fr.f = last.f;
            fr.b = last.b;
            fr.tl = last.tl;
            fr.oq = last.oq;
            fr.fnz = last.fnz;
        }
    }
    if cancelled() { None } else { Some(frames) }
}

/// Separates the nasal pole-zero pair over the end of a vowel before a nasal
/// and the start of one after it.
fn nasalize_vowels(segs: &[Seg], starts: &[usize], lens: &[usize], frames: &mut [Frame]) {
    let is_nasal = |j: usize| {
        segs.get(j)
            .and_then(|s| s.ph)
            .is_some_and(|p| p.class() == Class::Nasal)
    };
    for i in 0..segs.len() {
        if !segs[i].ph.is_some_and(Ph::is_vowel) {
            continue;
        }
        let vowel = &mut frames[starts[i]..starts[i] + lens[i]];
        let len = vowel.len();
        if is_nasal(i + 1) {
            let n = ms(NASALIZE_BEFORE_MS).min(len);
            for k in 0..n {
                let t = (k + 1) as f64 / (n + 1) as f64;
                vowel[len - n + k].fnz = lerp(vowel[len - n + k].fnz, NASAL_ZERO_HZ, t);
            }
        }
        if i > 0 && is_nasal(i - 1) {
            let n = ms(NASALIZE_AFTER_MS).min(len);
            for k in 0..n {
                let t = 1.0 - (k + 1) as f64 / (n + 1) as f64;
                vowel[k].fnz = vowel[k]
                    .fnz
                    .max(lerp(Frame::default().fnz, NASAL_ZERO_HZ, t));
            }
        }
    }
}

/// Smooths voicing and fricative noise across segment boundaries. Stop and
/// affricate bursts stay abrupt because their sharpness is the cue.
fn cross_fade(segs: &[Seg], starts: &[usize], lens: &[usize], frames: &mut [Frame]) {
    let mut fricative = vec![false; frames.len()];
    for i in 0..segs.len() {
        if segs[i].ph.is_some_and(|p| p.class() == Class::Fricative) {
            fricative[starts[i]..starts[i] + lens[i]].fill(true);
        }
    }
    let half = CROSS_FADE_FRAMES / 2;
    let smooth = |track: Vec<f64>| -> Vec<f64> {
        (0..track.len())
            .map(|k| {
                let window = &track[k.saturating_sub(half)..(k + half + 1).min(track.len())];
                let mean =
                    window.iter().map(|v| v.max(FADE_FLOOR_DB)).sum::<f64>() / window.len() as f64;
                if mean > FADE_FLOOR_DB + 0.5 {
                    mean
                } else {
                    0.0
                }
            })
            .collect()
    };
    let av = smooth(frames.iter().map(|f| f.av).collect());
    let noise = smooth(
        frames
            .iter()
            .zip(&fricative)
            .map(|(f, &m)| if m { f.af } else { 0.0 })
            .collect(),
    );
    for k in 0..frames.len() {
        frames[k].av = av[k];
        if fricative[k] {
            frames[k].af = noise[k];
        } else if noise[k] > 0.0 && frames[k].af == 0.0 {
            // The fading tail keeps the spectrum of the fricative it belongs to.
            let source = (1..=half)
                .flat_map(|d| [k.checked_sub(d), Some(k + d)])
                .flatten()
                .find(|&j| j < frames.len() && fricative[j])
                .unwrap();
            frames[k].af = noise[k];
            frames[k].ab = frames[source].ab;
            frames[k].a = frames[source].a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prosody::assign_durations;
    use crate::text::normalize;
    use crate::utterance::build;

    fn frames_for(text: &str) -> (Vec<Seg>, Vec<usize>, Vec<usize>, Vec<Frame>) {
        let mut segs = build(&normalize(text));
        assign_durations(&mut segs);
        let (starts, lens) = frame_layout(&segs);
        let frames = build_frames(&segs);
        (segs, starts, lens, frames)
    }

    #[test]
    fn voicing_fades_into_stop_closure_but_burst_stays_sharp() {
        let (segs, starts, _, frames) = frames_for("at");
        let t = segs.iter().position(|s| s.ph == Some(Ph::T)).unwrap();
        let closure = starts[t];
        for k in closure - 2..closure + 3 {
            let step = frames[k - 1].av.max(FADE_FLOOR_DB) - frames[k].av.max(FADE_FLOOR_DB);
            assert!(
                step < 15.0,
                "frame {k}: {} -> {}",
                frames[k - 1].av,
                frames[k].av
            );
        }
        let burst = frames[closure..].iter().position(|f| f.af > 0.0).unwrap() + closure;
        assert_eq!(frames[burst - 1].af, 0.0);
    }

    #[test]
    fn consonant_noise_boost_preserves_stop_closure_and_release_spectra() {
        for (text, ph) in [("at", Ph::T), ("pitch", Ph::CH)] {
            let (segs, starts, lens, frames) = frames_for(text);
            let i = segs.iter().position(|s| s.ph == Some(ph)).unwrap();
            let phone = &frames[starts[i]..starts[i] + lens[i]];
            let release = phone.iter().position(|fr| fr.af > 0.0).unwrap();
            assert!(release > 0, "{text}");
            assert!(phone[..release].iter().all(|fr| fr.af == 0.0));
            let spec = ph.spec();
            assert!(
                phone[release..]
                    .iter()
                    .all(|fr| { fr.af == spec.af + 6.0 && fr.ab == spec.ab && fr.a == spec.a }),
                "{text}"
            );
        }
    }

    #[test]
    fn sustained_fricative_noise_is_stronger_without_changing_voicing() {
        let (segs, starts, lens, frames) = frames_for("zoo");
        let i = segs.iter().position(|s| s.ph == Some(Ph::Z)).unwrap();
        let fr = frames[starts[i] + lens[i] / 2];
        let spec = Ph::Z.spec();
        assert_eq!(fr.af, spec.af + 6.0);
        assert_eq!(fr.av, spec.av);
        assert_eq!((fr.ab, fr.a), (spec.ab, spec.a));
    }

    #[test]
    fn pauses_do_not_start_voicing_or_retune_the_ringing_tract() {
        for text in ["Or Australia? And she tried.", "man. see."] {
            for scale in [1.0, 0.5, 1.0 / 3.0] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                for i in 0..segs.len() {
                    if segs[i].ph.is_some() {
                        continue;
                    }
                    let pause = &frames[starts[i]..starts[i] + lens[i]];
                    assert!(
                        pause.iter().all(|fr| fr.av == 0.0),
                        "{text}, scale {scale}: pause {i} voices"
                    );
                    if i == 0 {
                        continue;
                    }
                    let last = frames[starts[i] - 1];
                    let half = CROSS_FADE_FRAMES / 2;
                    let hold = if i + 1 == segs.len() {
                        lens[i]
                    } else {
                        lens[i].saturating_sub(half)
                    };
                    for fr in &pause[..hold] {
                        assert_eq!(
                            (fr.f, fr.b, fr.tl, fr.oq, fr.fnz),
                            (last.f, last.b, last.tl, last.oq, last.fnz),
                            "{text}, scale {scale}: pause {i} retunes the ringing tract"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn layout_covers_all_frames() {
        let (_, starts, lens, frames) = frames_for("Hello there, how are you?");
        assert_eq!(starts.last().unwrap() + lens.last().unwrap(), frames.len());
    }

    #[test]
    fn voiceless_stop_aspirates_following_vowel() {
        let (segs, starts, _, frames) = frames_for("tea");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::IY)).unwrap();
        let first = frames[starts[vowel]];
        assert_eq!(first.av, 0.0);
        assert!(first.ah > 0.0);
        let stop = segs.iter().position(|s| s.ph == Some(Ph::T)).unwrap();
        assert_eq!(frames[starts[stop]].av, 0.0);
        assert_eq!(frames[starts[stop]].af, 0.0);
    }

    #[test]
    fn voiceless_plosive_bursts_are_unvoiced_at_every_rate() {
        for text in ["a tea", "a stay", "pick it up", "to cut"] {
            for scale in [1.0, 0.5, 0.38] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                for i in 0..segs.len() {
                    if segs[i].flap || !segs[i].ph.is_some_and(|p| p.is_plosive() && !p.is_voiced())
                    {
                        continue;
                    }
                    let burst = frames[starts[i]..starts[i] + lens[i]]
                        .iter()
                        .filter(|f| f.af > 0.0);
                    assert!(
                        burst.clone().count() > 0 && burst.clone().all(|f| f.av == 0.0),
                        "{text}, scale {scale}: {:?}",
                        segs[i].ph
                    );
                }
            }
        }
    }

    #[test]
    fn flaps_dip_in_voicing_at_every_rate() {
        for text in ["water", "a city", "get up"] {
            for scale in [1.0, 0.5, 0.38] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                let flap = segs.iter().position(|s| s.flap).unwrap();
                let peak = |i: usize| {
                    frames[starts[i]..starts[i] + lens[i]]
                        .iter()
                        .map(|f| f.av)
                        .fold(0.0, f64::max)
                };
                let tap = &frames[starts[flap]..starts[flap] + lens[flap]];
                let dip = peak(flap - 1).min(peak(flap + 1))
                    - tap.iter().map(|f| f.av).fold(f64::MAX, f64::min);
                assert!(dip >= 6.0, "{text}, scale {scale}: dip {dip}");
                assert!(tap.iter().all(|f| f.av > 0.0));
            }
        }
    }

    #[test]
    fn flap_stays_voiced_without_burst_and_is_shorter_than_a_stop() {
        let (segs, starts, lens, frames) = frames_for("water");
        let flap = segs.iter().position(|s| s.flap).unwrap();
        let tap = &frames[starts[flap]..starts[flap] + lens[flap]];
        assert!(tap.iter().all(|f| f.av > 0.0 && f.af == 0.0 && f.ab == 0.0));
        let vowel = &frames[starts[flap + 1]];
        assert!(vowel.av > 0.0, "no aspiration after a flap");
        let (stop_segs, _, stop_lens, _) = frames_for("button");
        let stop = stop_segs.iter().position(|s| s.ph == Some(Ph::T)).unwrap();
        assert!(lens[flap] * 2 <= stop_lens[stop]);
    }

    #[test]
    fn aspiration_matches_the_prosodic_budget() {
        for text in ["tea", "tree", "try", "quick", "to be", "key", "stay", "dee"] {
            let (segs, starts, lens, frames) = frames_for(text);
            for i in 0..segs.len() - 1 {
                if !segs[i].ph.is_some_and(Ph::is_plosive) {
                    continue;
                }
                let next = &frames[starts[i + 1]..starts[i + 1] + lens[i + 1]];
                let actual = next
                    .iter()
                    .take_while(|f| f.av == 0.0 && f.ah > 0.0)
                    .count();
                let expected = ms(prosody::aspiration_ms(&segs, i));
                assert_eq!(actual, expected, "{text}: {:?}", segs[i].ph);
            }
        }
    }

    #[test]
    fn rate_scales_aspiration_and_preserves_voicing() {
        for text in ["try", "quick", "cat cat", "tea", "to be", "stay"] {
            for scale in [2.0, 1.0, 0.5, 1.0 / 3.0] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                let budgets: Vec<f64> = segs.iter().map(|s| s.aspiration_ms).collect();
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                for i in 1..segs.len() {
                    assert_eq!(segs[i].aspiration_ms, budgets[i] * scale);
                    if budgets[i] == 0.0 {
                        continue;
                    }
                    let part = &frames[starts[i]..starts[i] + lens[i]];
                    let expected = ms(budgets[i] * scale).min(lens[i] - 1);
                    let actual = part
                        .iter()
                        .take_while(|f| f.av == 0.0 && f.ah > 0.0)
                        .count();
                    assert_eq!(actual, expected, "{text}, scale {scale}, segment {i}");
                    assert!(
                        part[expected..].iter().any(|f| f.av > 0.0),
                        "{text}, scale {scale}, segment {i} lost voicing"
                    );
                }
            }
        }
    }

    #[test]
    fn faster_speech_does_not_spread_breathiness_across_the_vowel() {
        let mut segs = build(&normalize("bee"));
        assign_durations(&mut segs);
        for s in &mut segs {
            s.dur_ms = 180.0;
        }
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::IY)).unwrap();
        let natural = breathiness(&segs, &vec![36; segs.len()], vowel, 1.0);
        let fast = breathiness(&segs, &vec![12; segs.len()], vowel, 1.0 / 3.0);
        for (k, &(b, offset)) in fast.iter().enumerate() {
            let corresponding = natural[k * 3];
            assert!((b - corresponding.0).abs() < 1e-12, "frame {k}: {b}");
            assert!(
                (offset - corresponding.1).abs() < 1e-12,
                "frame {k}: {offset}"
            );
        }
    }

    #[test]
    fn aspirated_release_transition_spans_the_aspiration() {
        for text in ["tea", "to be", "key"] {
            let (segs, starts, _, frames) = frames_for(text);
            let i = segs.iter().position(|s| s.aspiration_ms > 0.0).unwrap();
            let start = starts[i];
            let aspiration = ms(segs[i].aspiration_ms);
            let target = segs[i].ph.unwrap().spec().f;
            assert!(
                frames[start..start + aspiration]
                    .iter()
                    .all(|f| f.av == 0.0 && f.f != target),
                "{text}"
            );
            assert_eq!(frames[start + aspiration].f, target, "{text}");
        }
    }

    #[test]
    fn short_sonorants_keep_aspiration_and_a_voiced_target() {
        let (mut segs, _, _, _) = frames_for("try");
        for s in &mut segs {
            s.stress = 0;
            s.function_word = true;
        }
        assign_durations(&mut segs);
        let (starts, lens) = frame_layout(&segs);
        let windows = transition_windows(&segs, &lens);
        let frames = build_frames(&segs);
        let r = segs.iter().position(|s| s.ph == Some(Ph::R)).unwrap();
        let aspiration = ms(prosody::aspiration_ms(&segs, r - 1));
        assert_eq!(aspiration, 5);
        assert!(windows[r - 1].1 >= aspiration);
        assert!(windows[r - 1].1 + windows[r].0 < lens[r]);
        let segment = &frames[starts[r]..starts[r] + lens[r]];
        assert!(
            segment[..aspiration]
                .iter()
                .all(|f| f.av == 0.0 && f.ah > 0.0)
        );
        assert!(segment[aspiration..].iter().all(|f| f.av > 0.0));
        assert_eq!(segment[windows[r - 1].1].f, Ph::R.spec().f);
    }

    #[test]
    fn short_transition_windows_do_not_overlap_or_escape_segments() {
        let (mut segs, _, _, _) = frames_for("quick five loud voice");
        for duration in [5.0, 10.0, 25.0, 60.0, 120.0] {
            for s in &mut segs {
                s.dur_ms = duration;
            }
            let (_, lens) = frame_layout(&segs);
            let windows = transition_windows(&segs, &lens);
            for i in 0..segs.len() {
                let incoming = if i > 0 { windows[i - 1].1 } else { 0 };
                let outgoing = windows.get(i).map_or(0, |w| w.0);
                assert!(incoming + outgoing < lens[i], "{duration} ms: segment {i}");
            }
            let frames = build_frames(&segs);
            assert_eq!(frames.len(), lens.iter().sum::<usize>());
            assert!(
                frames
                    .iter()
                    .all(|f| f.f0.is_finite() && f.f.iter().all(|v| v.is_finite()))
            );
        }
    }

    #[test]
    fn diphthongs_hold_their_onset_then_glide_just_before_closure() {
        for (text, ph) in [("five", Ph::AY), ("cows", Ph::AW), ("noise", Ph::OY)] {
            let (segs, starts, lens, frames) = frames_for(text);
            let vowel = segs.iter().position(|s| s.ph == Some(ph)).unwrap();
            let windows = transition_windows(&segs, &lens);
            let closure = starts[vowel] + lens[vowel] - windows[vowel].0;
            let offglide = closure - ms(DIPHTHONG_OFFGLIDE_MS);
            let glide = offglide - ms(segs[vowel].glide_ms);
            let spec = ph.spec();
            let target = spec.f_end.unwrap();
            // The onset is held from the end of the incoming transition.
            let settled = starts[vowel] + windows[vowel - 1].1;
            assert!(glide > settled + (closure - settled) / 4, "{text}");
            for fr in &frames[settled..=glide] {
                assert_eq!(fr.f, spec.f, "{text}");
            }
            for (k, fr) in frames[glide..offglide].iter().enumerate() {
                let expected: [f64; 3] = std::array::from_fn(|j| {
                    lerp(
                        spec.f[j],
                        target[j],
                        k as f64 / ms(segs[vowel].glide_ms) as f64,
                    )
                });
                assert_eq!(fr.f, expected, "{text}: glide frame {k}");
            }
            for fr in &frames[offglide..closure] {
                assert_eq!(fr.f, target, "{text}");
            }
            assert_ne!(frames[closure].f, target, "{text}");
        }
    }

    #[test]
    fn diphthongs_reach_most_of_their_offglide_in_fast_speech() {
        for (text, ph) in [("five", Ph::AY), ("boy", Ph::OY), ("go down", Ph::AW)] {
            for scale in [1.0, 0.45] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                crate::prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames(&segs);
                let v = segs.iter().position(|s| s.ph == Some(ph)).unwrap();
                let (from, to) = (ph.spec().f[1], ph.spec().f_end.unwrap()[1]);
                let reached = frames[starts[v]..starts[v] + lens[v]]
                    .iter()
                    .map(|f| (f.f[1] - from) / (to - from))
                    .fold(f64::MIN, f64::max);
                assert!(reached > 0.69, "{text} at {scale}: {reached}");
            }
        }
    }

    #[test]
    fn diphthong_onsets_absorb_clipping_before_voiceless_obstruents() {
        let onset_share = |text: &str| {
            let (segs, starts, lens, frames) = frames_for(text);
            let vowel = segs.iter().position(|s| s.ph == Some(Ph::AY)).unwrap();
            let onset = frames[starts[vowel]..starts[vowel] + lens[vowel]]
                .iter()
                .filter(|f| f.f == Ph::AY.spec().f)
                .count();
            onset as f64 / lens[vowel] as f64
        };
        assert!(onset_share("rice") < onset_share("rise"));
    }

    #[test]
    fn breathy_offset_spans_a_short_final_sonorant_and_its_vowel() {
        for text in ["table.", "the weather is terrible.", "again."] {
            for scale in [1.0, 0.5] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                let last = segs.iter().rposition(|s| s.ph.is_some()).unwrap();
                assert!(
                    !segs[last].is_vowel() && segs[last - 1].is_vowel(),
                    "{text}"
                );
                let (a, b) = (frames[starts[last] - 1], frames[starts[last]]);
                assert!(a.oq > MODAL_OQ + 6.5, "{text}, scale {scale}: {}", a.oq);
                assert!(
                    (a.av - b.av).abs() < 3.0 && (b.oq - a.oq).abs() < 4.0,
                    "{text}, scale {scale}: av {} -> {}, oq {} -> {}",
                    a.av,
                    b.av,
                    a.oq,
                    b.oq
                );
            }
        }
    }

    #[test]
    fn vowel_becomes_breathy_before_silence() {
        let (segs, starts, lens, frames) = frames_for("saw");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::AO)).unwrap();
        let mid = frames[starts[vowel] + lens[vowel] / 2];
        let last = frames[starts[vowel] + lens[vowel] - 1];
        assert!(last.oq > mid.oq && last.tl > mid.tl && last.ah > mid.ah);
        assert!(last.av > 0.0);
    }

    #[test]
    fn voicing_keeps_growing_after_aspiration_fills_a_short_sonorant() {
        for text in ["likely", "country", "quick", "try"] {
            for scale in [1.0, 0.5, 0.38] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                prosody::scale_durations(&mut segs, scale);
                let (starts, lens) = frame_layout(&segs);
                let frames = build_frames_with_time_scale(&segs, &starts, &lens, scale);
                for i in 1..segs.len() {
                    if segs[i].aspiration_ms == 0.0 {
                        continue;
                    }
                    let onset = starts[i] + ms(segs[i].aspiration_ms).min(lens[i] - 1);
                    let av: Vec<f64> = frames[onset..(onset + 4).min(frames.len())]
                        .iter()
                        .map(|f| f.av)
                        .collect();
                    for w in av.windows(2) {
                        assert!(w[1] - w[0] < 7.0, "{text}, scale {scale}: {av:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn voicing_grows_breathily_after_aspiration() {
        let (segs, starts, lens, frames) = frames_for("take a cup");
        let i = segs.iter().position(|s| s.ph == Some(Ph::EY)).unwrap();
        let vowel = &frames[starts[i]..starts[i] + lens[i]];
        let onset = vowel.iter().position(|f| f.av > 0.0).unwrap();
        assert!(onset > 0);
        let av: Vec<f64> = vowel[onset..onset + 3].iter().map(|f| f.av).collect();
        assert!(av[0] < av[1] && av[1] < av[2], "{av:?}");
        assert_eq!(av[2], Ph::EY.spec().av);
        assert!(vowel[onset].oq > vowel[onset + 3].oq && vowel[onset].ah > 0.0);
    }

    #[test]
    fn breathy_margins_of_voiceless_consonants_leave_a_modal_middle() {
        let (segs, starts, lens, frames) = frames_for("sit down");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::IH)).unwrap();
        let vowel = &frames[starts[vowel]..starts[vowel] + lens[vowel]];
        let modal = vowel.iter().filter(|f| f.oq == MODAL_OQ && f.ah == 0.0);
        assert!(modal.count() * 3 >= vowel.len() - 1, "{}", vowel.len());
        assert!(vowel[0].oq > MODAL_OQ && vowel[vowel.len() - 1].oq > MODAL_OQ);
    }

    #[test]
    fn unstressed_vowel_between_voiced_consonants_stays_modal() {
        let (segs, starts, lens, frames) = frames_for("banana bread");
        let vowel = segs
            .iter()
            .position(|s| s.ph.is_some_and(Ph::is_vowel))
            .unwrap();
        assert_eq!(segs[vowel].stress, 0);
        let mid = frames[starts[vowel] + lens[vowel] / 2];
        assert_eq!((mid.oq, mid.tl, mid.ah), (MODAL_OQ, 0.0, 0.0));
    }

    #[test]
    fn unstressed_vowels_are_voiced_more_weakly_than_stressed_ones() {
        let (segs, starts, lens, frames) = frames_for("banana bread");
        let mid_av = |i: usize| frames[starts[i] + lens[i] / 2].av;
        let vowels: Vec<usize> = (0..segs.len())
            .filter(|&i| segs[i].ph.is_some_and(Ph::is_vowel))
            .collect();
        let (weak, strong) = (vowels[0], vowels[1]);
        assert_eq!(segs[strong].stress, 1);
        let spec = |i: usize| segs[i].ph.unwrap().spec().av;
        assert_eq!(mid_av(weak), spec(weak) - UNSTRESSED_AV_DROP_DB);
        assert_eq!(mid_av(strong), spec(strong));
    }

    #[test]
    fn vowel_is_nasalized_toward_a_following_nasal() {
        let (segs, starts, lens, frames) = frames_for("ban");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::AE)).unwrap();
        let first = frames[starts[vowel]].fnz;
        let last = frames[starts[vowel] + lens[vowel] - 1].fnz;
        assert_eq!(first, Frame::default().fnz);
        assert!(last > first && last < NASAL_ZERO_HZ, "{first} -> {last}");
    }

    #[test]
    fn vowel_before_a_stop_is_not_nasalized() {
        let (segs, starts, lens, frames) = frames_for("bat");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::AE)).unwrap();
        assert!(
            frames[starts[vowel]..starts[vowel] + lens[vowel]]
                .iter()
                .all(|f| f.fnz == Frame::default().fnz)
        );
    }

    #[test]
    fn labial_and_alveolar_stops_start_between_locus_and_vowel() {
        let onset = |text: &str, ph: Ph| {
            let (segs, starts, lens, frames) = frames_for(text);
            let i = segs.iter().position(|s| s.ph == Some(ph)).unwrap();
            frames[starts[i] + lens[i] - 1].f[1]
        };
        for ph in [Ph::D, Ph::B] {
            let name = ph.name().to_lowercase();
            let (front, back) = (
                onset(&format!("a {name}ee"), ph),
                onset(&format!("a {name}oo"), ph),
            );
            assert!(front > back + 300.0, "{ph:?}: {front} {back}");
        }
    }

    #[test]
    fn velar_locus_follows_the_vowel() {
        let locus = |text: &str| {
            let (segs, starts, lens, frames) = frames_for(text);
            let g = segs.iter().position(|s| s.ph == Some(Ph::G)).unwrap();
            frames[starts[g] + lens[g] / 2].f[1]
        };
        assert!(
            locus("goo") + 300.0 < locus("geese"),
            "{} vs {}",
            locus("goo"),
            locus("geese")
        );
        assert!(
            locus("bog") < locus("beg"),
            "{} vs {}",
            locus("bog"),
            locus("beg")
        );
    }

    #[test]
    fn formants_move_smoothly_into_vowel() {
        let (segs, starts, lens, frames) = frames_for("bee");
        let vowel = segs.iter().position(|s| s.ph == Some(Ph::IY)).unwrap();
        let f2: Vec<f64> = (starts[vowel]..starts[vowel] + lens[vowel])
            .map(|k| frames[k].f[1])
            .collect();
        assert!(f2[0] < f2[f2.len() - 1]);
        for w in f2.windows(2) {
            assert!((w[1] - w[0]).abs() < 300.0, "{f2:?}");
        }
    }

    #[test]
    fn diphthong_bandwidths_follow_the_glide() {
        let t = targets(Ph::AY, 40, 0, 100.0);
        let (first, last) = (t[0], t[t.len() - 1]);
        assert!(last.f[1] > first.f[1] + 500.0);
        assert!(last.b[1] > first.b[1]);
        assert_eq!(last.b, crate::phoneme::vowel_bandwidths(last.f));
    }
}
