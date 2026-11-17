//! Prosody: Klatt's (1979) segmental duration rules, whose form Klatt (1987)
//! summarizes without the numbers, with rate-normalized accent peaks and
//! declination.

use crate::phoneme::{Class, Ph, Place};
use crate::text::Break;
use crate::timing;
use crate::utterance::Seg;

/// Klatt's (1979) pause rule: 200 ms at a comma or clause boundary. His rules
/// time one sentence at a time and give no pause between sentences; Klatter
/// pauses twice as long there, the nearest choice that keeps sentence ends
/// distinct from commas.
const CLAUSE_PAUSE_MS: f64 = 200.0;
const SENTENCE_PAUSE_MS: f64 = 2.0 * CLAUSE_PAUSE_MS;

pub fn pause_ms(brk: Option<Break>, utterance_edge: bool) -> f64 {
    match brk {
        _ if utterance_edge => 150.0,
        Some(Break::Comma) => CLAUSE_PAUSE_MS,
        Some(_) => SENTENCE_PAUSE_MS,
        None => 150.0,
    }
}

fn neighbor(segs: &[Seg], i: usize, offset: isize) -> Option<Ph> {
    let j = i as isize + offset;
    if j < 0 {
        return None;
    }
    segs.get(j as usize).and_then(|s| s.ph)
}

/// Rule 9 (postvocalic context): effect of the consonant after a vowel.
fn postvocalic_factor(segs: &[Seg], i: usize) -> f64 {
    let s = &segs[i];
    if s.word_final {
        return 1.2;
    }
    // Sonorant codas pass the effect on from the obstruent after them.
    let mut j = i + 1;
    while j < segs.len() && !segs[j - 1].word_final {
        let Some(p) = segs[j].ph else { break };
        if p.is_vowel() {
            return 1.0;
        }
        if matches!(p.class(), Class::Liquid | Class::Glide) && !segs[j].word_final {
            j += 1;
            continue;
        }
        return match (p.class(), p.is_voiced()) {
            (Class::Fricative, true) => 1.6,
            (Class::Stop | Class::Affricate, true) => 1.2,
            (Class::Nasal, _) => 0.85,
            (Class::Stop | Class::Affricate, false) => 0.7,
            _ => 1.0,
        };
    }
    1.0
}

/// Exclusive end of the acoustic nucleus starting at vowel `i`: the vowel plus
/// following liquids and glides in the same syllable (Hertz & Huffman 1992).
pub fn nucleus_end(segs: &[Seg], i: usize) -> usize {
    let mut j = i + 1;
    while j < segs.len()
        && !segs[j - 1].word_final
        && segs[j].coda
        && segs[j]
            .ph
            .is_some_and(|p| matches!(p.class(), Class::Liquid | Class::Glide))
    {
        j += 1;
    }
    j
}

/// Aspiration after the plosive at `i`, or 0 if it isn't aspirated.
pub fn aspiration_ms(segs: &[Seg], i: usize) -> f64 {
    let s = &segs[i];
    let Some(ph) = s.ph else { return 0.0 };
    let after_s = i > 0 && segs[i - 1].ph == Some(Ph::S) && !s.word_initial;
    let Some(next) = segs
        .get(i + 1)
        .filter(|n| n.ph.is_some_and(Ph::is_sonorant))
    else {
        return 0.0;
    };
    if !ph.is_plosive() || ph.is_voiced() || after_s || s.flap {
        return 0.0;
    }
    // Aspiration belongs to a plosive released into its own syllable: a coda
    // plosive before the next word ("night rate" vs "nitrate", Klatt 1987)
    // or syllable is weakly aspirated, and a plosive before a nasal is
    // released through the nose rather than aspirated.
    if s.coda || next.ph.is_some_and(|p| p.class() == Class::Nasal) {
        return 0.0;
    }
    let ms = if next.stress > 0 && !next.function_word {
        50.0
    } else {
        25.0
    };
    if ph.place() == Place::Velar {
        ms + 10.0
    } else {
        ms
    }
}

/// Hertz & Huffman (1992): the voiced vowel after aspiration is about 20 ms
/// shorter than elsewhere, but the aspiration itself adds to the syllable.
const VOWEL_SHORTENING_AFTER_ASPIRATION_MS: f64 = 20.0;

/// Syllables are kept evenly timed, for the predictable rhythm and delivery
/// a listener chose in blind comparisons over the more sing-song timing of
/// Klatt's rules alone: stressed vowels are shorter than the rules give,
/// which avoids a drawl on content words, while unstressed syllables of
/// content words stay audible instead of being swallowed.
const STRESSED_VOWEL_FACTOR: f64 = 0.65;
const UNSTRESSED_CONTENT_VOWEL_FACTOR: f64 = 1.8;

/// Part of the same even rhythm: closures stay short, while fricatives keep
/// most of their time to carry their noise cue.
const STOP_FACTOR: f64 = 0.8;
const FRICATIVE_FACTOR: f64 = 0.95;
/// Affricates keep the stop's closure and add a fricative's worth of noise.
const AFFRICATE_FACTOR: f64 = 1.2;

/// Klatt's value is 1.4, but final vowels already escape non-final shortening
/// and get the full postvocalic effect; a listener heard drawled phrase ends
/// and chose this trimmed value for an even delivery.
const PHRASE_FINAL_LENGTHENING: f64 = 0.8;

/// Inherent and minimum durations (ms) of a flapped /t/ or /d/: a single tap,
/// much shorter than a stop closure (Zue & Laferriere 1979).
const FLAP_DURATIONS: (f64, f64) = (25.0, 15.0);

pub fn assign_durations(segs: &mut [Seg]) {
    let last = segs.len().saturating_sub(1);
    // The vowel's (percent, minimum), kept so its nucleus can be timed as a whole.
    let mut vowel_scaling = vec![(1.0, 0.0); segs.len()];
    for i in 0..segs.len() {
        segs[i].aspiration_ms = 0.0;
        let Some(ph) = segs[i].ph else {
            segs[i].dur_ms = pause_ms(segs[i].brk, i == 0 || i == last);
            continue;
        };
        let s = &segs[i];
        let (inh, mut min) = if s.flap {
            FLAP_DURATIONS
        } else {
            ph.durations()
        };
        let mut prcnt = 1.0;
        let vowel = ph.is_vowel();
        let stressed = s.stress > 0 && !s.function_word;

        // Rule 2: clause-final lengthening of the final syllable's nucleus and coda.
        if s.phrase_final && (vowel || s.coda) {
            prcnt *= PHRASE_FINAL_LENGTHENING;
        }
        // Rule 3: non-phrase-final shortening.
        if vowel && !s.phrase_final {
            prcnt *= 0.6;
        }
        // Rule 4: non-word-final shortening.
        if vowel && s.syllable + 1 < s.syllables_in_word {
            prcnt *= 0.85;
        }
        // Rule 5: polysyllabic shortening.
        if vowel && s.syllables_in_word > 1 {
            prcnt *= 0.8;
        }
        // Rule 6: non-initial consonant shortening.
        if !vowel && !s.word_initial {
            prcnt *= 0.85;
        }
        // Rule 7: unstressed shortening.
        let prev = neighbor(segs, i, -1);
        let next = neighbor(segs, i, 1);
        if !stressed {
            min /= 2.0;
            let medial = s.syllable > 0 && s.syllable + 1 < s.syllables_in_word;
            let prevocalic_approximant = matches!(ph.class(), Class::Liquid | Class::Glide)
                && !s.coda
                && next.is_some_and(Ph::is_vowel);
            prcnt *= if vowel && medial {
                0.5
            } else if prevocalic_approximant {
                0.1
            } else {
                0.7
            };
            // Phrase-final syllables are already lengthened, so they need no
            // extra audibility.
            if vowel && !s.function_word && !s.phrase_final {
                prcnt *= UNSTRESSED_CONTENT_VOWEL_FACTOR;
            }
        }
        if vowel && stressed {
            prcnt *= STRESSED_VOWEL_FACTOR;
        }
        // Rule 9: postvocalic context, strongest at phrase ends.
        if vowel {
            let f = postvocalic_factor(segs, i);
            prcnt *= if s.phrase_final { f } else { 0.7 + 0.3 * f };
        }
        // Rule 10: shortening in clusters, except phrase-final codas.
        let is_cons = |p: Option<Ph>| p.is_some_and(|p| !p.is_vowel());
        if vowel {
            if next.is_some_and(Ph::is_vowel) {
                prcnt *= 1.2;
            }
            if prev.is_some_and(Ph::is_vowel) {
                prcnt *= 0.7;
            }
        } else if s.phrase_final && s.coda {
            // Phrase-final codas keep their final lengthening.
        } else if is_cons(prev) && is_cons(next) {
            prcnt *= 0.5;
        } else if is_cons(next) || is_cons(prev) {
            prcnt *= 0.7;
        }

        vowel_scaling[i] = (prcnt, min);
        segs[i].dur_ms = (inh - min) * prcnt + min;
        if ph.class() == Class::Fricative {
            segs[i].dur_ms *= FRICATIVE_FACTOR;
        } else if ph.class() == Class::Affricate {
            segs[i].dur_ms *= AFFRICATE_FACTOR;
        } else if ph.class() == Class::Stop {
            segs[i].dur_ms *= STOP_FACTOR;
        }
    }

    // The vowel's context scales its whole nucleus, coda liquids and glides
    // included: Hertz's (1991, Fig. 12) [l] is as long as the vowel before a
    // voiced stop and shortens with it before a voiceless one, and Hertz &
    // Huffman (1992) lengthen the non-vowel phones of a nucleus first. The
    // vowel takes the remainder, so vowel and coda trade time.
    for i in 0..segs.len() {
        if !segs[i].is_vowel() {
            continue;
        }
        let end = nucleus_end(segs, i);
        if end > i + 1 {
            let (prcnt, vowel_min) = vowel_scaling[i];
            let stressed = segs[i].stress > 0 && !segs[i].function_word;
            let scaled = |s: &Seg| {
                let (h, m) = s.ph.map(Ph::durations).unwrap_or_default();
                let m = if stressed { m } else { m / 2.0 };
                (h, m, (h - m) * prcnt + m)
            };
            let (mut inh, mut min, mut coda) = (0.0, 0.0, 0.0);
            for s in &segs[i..end] {
                let (h, m, _) = scaled(s);
                inh += h;
                min += m;
            }
            for j in i + 1..end {
                segs[j].dur_ms = scaled(&segs[j]).2;
                coda += segs[j].dur_ms;
            }
            // The codas also keep the transitions that are theirs, so the
            // vowel gives up that time too.
            let mut moved = 0.0;
            for j in i + 1..end {
                let ms = timing::coda_transition_ms(segs, j);
                segs[j].dur_ms += ms;
                moved += ms;
            }
            segs[i].dur_ms = ((inh - min) * prcnt + min - coda - moved).max(vowel_min);
        }
    }
    for i in 1..segs.len() {
        let asp = aspiration_ms(segs, i - 1);
        if asp > 0.0 {
            let floor = segs[i].ph.map_or(0.0, |p| p.durations().1 / 2.0);
            segs[i].dur_ms =
                (segs[i].dur_ms - VOWEL_SHORTENING_AFTER_ASPIRATION_MS).max(floor) + asp;
            segs[i].aspiration_ms = asp;
        }
    }
    timing::assign(segs, 1.0);
}

/// Speech and onset budgets scale together, pauses follow `pause_scale`, and
/// transitions follow their own rate law (see `timing`).
pub fn scale_durations(segs: &mut [Seg], scale: f64) {
    for seg in segs.iter_mut() {
        seg.dur_ms *= if seg.ph.is_none() {
            pause_scale(scale)
        } else {
            scale
        };
        seg.aspiration_ms *= scale;
    }
    timing::assign(segs, scale);
}

/// Readers speed up mostly by pausing less, and articulate only a little
/// faster: as reading rate rose 2.7-fold, the number of pauses fell 10.3-fold
/// and their durations 1.6-fold, while articulation rate rose only 2.1-fold
/// (Lane & Grosjean 1973, reported by Grosjean & Lane 1974, p. 893; "increase
/// of speed in talking is due largely to the closing of gaps", Goldman-Eisler
/// 1968, p. 26). Klatter pauses only at punctuation and drops none, so above
/// the natural rate its pauses shorten instead, keeping the readers' total:
/// pause time falls 10.3 x 1.6 = 16.5-fold while phone time falls 2.1-fold,
/// a power of ln 16.5 / ln 2.1 = 3.8 of the phones' duration scale. Slower
/// than the natural rate, pauses lengthen with the phones.
const PAUSE_RATE_EXPONENT: f64 = 3.78;

/// The duration scale of pauses when phones scale by `scale`.
pub fn pause_scale(scale: f64) -> f64 {
    if scale < 1.0 {
        scale.powf(PAUSE_RATE_EXPONENT)
    } else {
        scale
    }
}

/// The pitch contour is a declining baseline with a separate rise-fall for
/// each accent, which F0 sags back toward between accents rather than holding
/// a hat (Pierrehumbert 1981, Figs. 4 and 6). Its range is narrower than in
/// Klatt's (1987, Fig. 26) example, and its accents, boundary tones and
/// declination are the values a listener chose in blind comparisons for a
/// predictable, not sing-song, delivery.
const START_F0: f64 = 110.0;
pub const BASE_F0: f64 = 100.0;
const FLOOR_F0: f64 = 74.0;
const FIRST_ACCENT: f64 = 18.0;
const ACCENT: f64 = 12.0;
const DECLINATION_PER_SEC: f64 = 8.0;
const ACCENT_RISE_MS: f64 = 200.0;
const ACCENT_FALL_MS: f64 = 150.0;

/// Builds an F0 track (Hz per frame) given each segment's first frame and
/// frame count.
pub fn f0_contour(segs: &[Seg], starts: &[usize], lens: &[usize], frame_ms: f64) -> Vec<f64> {
    f0_contour_with_time_scale(segs, starts, lens, frame_ms, 1.0)
}

pub(crate) fn f0_contour_with_time_scale(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    frame_ms: f64,
    time_scale: f64,
) -> Vec<f64> {
    f0_contour_cancellable(segs, starts, lens, frame_ms, time_scale, &|| false)
        .expect("uncancelled pitch planning")
}

pub(crate) fn f0_contour_cancellable(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    frame_ms: f64,
    time_scale: f64,
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<f64>> {
    let total: usize = lens.iter().sum();
    let mut f0 = vec![BASE_F0; total];
    let mut i = 0;
    let mut sentence_start = 0usize;
    while i < segs.len() {
        if cancelled() {
            return None;
        }
        if segs[i].ph.is_none() {
            if matches!(
                segs[i].brk,
                Some(Break::Period | Break::Question | Break::Exclamation)
            ) {
                sentence_start = starts[i] + lens[i];
            }
            for k in starts[i]..starts[i] + lens[i] {
                f0[k] = BASE_F0;
            }
            i += 1;
            continue;
        }
        let clause_begin = i;
        while i < segs.len() && segs[i].ph.is_some() {
            i += 1;
        }
        let ending = segs.get(i).and_then(|s| s.brk).unwrap_or(Break::Period);
        clause_contour(
            &segs[clause_begin..i],
            &starts[clause_begin..i],
            &lens[clause_begin..i],
            ending,
            sentence_start,
            frame_ms / time_scale,
            &mut f0,
            cancelled,
        )?;
        // Silent-frame pitch must not flatten the clause's boundary tone.
        let begin = starts[clause_begin];
        let end = starts[i - 1] + lens[i - 1];
        smooth(&mut f0[begin..end], 9);
    }
    add_microprosody(segs, starts, lens, frame_ms, &mut f0);
    add_flutter(&mut f0, frame_ms);
    if cancelled() { None } else { Some(f0) }
}

/// Consonants perturb the F0 of the sonorant after them: a voiceless one
/// raises its onset and a voiced one lowers it. Listeners use this as a
/// voicing cue, and its absence is part of what makes synthetic pitch sound
/// drawn with a ruler. Vocal-fold vibration that continues through a voiced
/// consonant, a flap or this voice's voiced /h/ cannot change rate in one
/// pitch period, so there the perturbation builds up across the consonant;
/// and it decays over the same time whether or not the sonorant lasts that
/// long, rather than snapping back at the next phone.
fn add_microprosody(segs: &[Seg], starts: &[usize], lens: &[usize], frame_ms: f64, f0: &mut [f64]) {
    const RAISE: f64 = 0.08;
    const LOWER: f64 = 0.04;
    const DECAY_MS: f64 = 50.0;
    let decay = (DECAY_MS / frame_ms) as usize;
    for i in 1..segs.len() {
        let (Some(p), Some(v)) = (segs[i - 1].ph, segs[i].ph) else {
            continue;
        };
        if !v.is_sonorant() || p.is_sonorant() {
            continue;
        }
        let shift = if p.is_voiced() || segs[i - 1].flap {
            -LOWER
        } else {
            RAISE
        };
        if p.is_voiced() || segs[i - 1].flap || p == Ph::HH {
            let n = lens[i - 1];
            for k in 0..n {
                f0[starts[i - 1] + k] *= 1.0 + shift * (k + 1) as f64 / (n + 1) as f64;
            }
        }
        let onset = starts[i];
        for (k, f) in f0[onset..].iter_mut().take(decay).enumerate() {
            *f *= 1.0 + shift * (1.0 - k as f64 / decay as f64);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clause_contour(
    segs: &[Seg],
    starts: &[usize],
    lens: &[usize],
    ending: Break,
    sentence_start: usize,
    natural_frame_ms: f64,
    f0: &mut [f64],
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    let begin = starts[0];
    let end = starts[segs.len() - 1] + lens[segs.len() - 1];
    let accents: Vec<usize> = (0..segs.len())
        .filter(|&k| segs[k].is_vowel() && segs[k].stress == 1 && !segs[k].function_word)
        .collect();
    let baseline = |frame: usize| {
        let secs = (frame - sentence_start) as f64 * natural_frame_ms / 1000.0;
        (START_F0 - DECLINATION_PER_SEC * secs).max(BASE_F0)
    };
    // Hertz, Younes & Zinovieva (1999, p. 2285) put the accent's high target
    // a set percentage of the way through the acoustic nucleus without giving
    // the number; their Fig. 5 example peaks roughly three quarters to four
    // fifths of the way through.
    let peak_of = |k: usize| {
        let e = nucleus_end(segs, k);
        starts[k] + (starts[e - 1] + lens[e - 1] - starts[k]) * 3 / 4
    };
    let rise = ACCENT_RISE_MS / natural_frame_ms;
    let fall = ACCENT_FALL_MS / natural_frame_ms;
    let ease = |x: f64| 0.5 - 0.5 * (std::f64::consts::PI * x.clamp(0.0, 1.0)).cos();
    for frame in begin..end {
        if cancelled() {
            return None;
        }
        // Taking the maximum prevents adjacent accents from forming a plateau.
        let bump = accents.iter().enumerate().fold(0.0f64, |m, (n, &k)| {
            let peak = peak_of(k) as f64;
            let t = frame as f64;
            let shape = if t <= peak {
                ease((t - (peak - rise)) / rise)
            } else {
                ease(1.0 - (t - peak) / fall)
            };
            m.max(if n == 0 { FIRST_ACCENT } else { ACCENT } * shape)
        });
        f0[frame] = baseline(frame) + bump;
    }
    // Boundary tones also apply to clauses with no pitch accents.
    let last_accent = accents.last().copied();
    let tail_start = last_accent.map(peak_of).unwrap_or_else(|| {
        segs.iter()
            .rposition(Seg::is_vowel)
            .map_or(begin, |k| starts[k])
    });
    let tail_len = (end - tail_start).max(1) as f64;
    let top = if tail_start > begin {
        f0[tail_start - 1]
    } else {
        baseline(begin)
    };
    for frame in tail_start..end {
        let t = (frame - tail_start) as f64 / tail_len;
        f0[frame] = match ending {
            Break::Period | Break::Exclamation => top + (FLOOR_F0 - top) * t.sqrt(),
            Break::Comma => {
                let dip = baseline(frame) - 10.0;
                if t < 0.6 {
                    top + (dip - top) * (t / 0.6)
                } else {
                    dip + 8.0 * (t - 0.6) / 0.4
                }
            }
            Break::Question => {
                let low = baseline(frame) - 5.0;
                if t < 0.4 {
                    top + (low - top) * (t / 0.4)
                } else {
                    low + 35.0 * ((t - 0.4) / 0.6).powf(1.5)
                }
            }
        };
    }
    Some(())
}

fn smooth(f0: &mut [f64], width: usize) {
    let src = f0.to_vec();
    let half = width / 2;
    for i in 0..f0.len() {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(src.len());
        f0[i] = src[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
    }
}

/// Slow quasi-random F0 drift (KLSYN88 "flutter") so steady vowels don't sound
/// mechanical. Klatt & Klatt (1990) found FL = 25% sounds realistic.
fn add_flutter(f0: &mut [f64], frame_ms: f64) {
    use std::f64::consts::TAU;
    const FL: f64 = 25.0;
    for (i, v) in f0.iter_mut().enumerate() {
        let t = i as f64 * frame_ms / 1000.0;
        let wobble = (TAU * 12.7 * t).sin() + (TAU * 7.1 * t).sin() + (TAU * 4.7 * t).sin();
        *v += (FL / 50.0) * (*v / 100.0) * wobble;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::normalize;
    use crate::utterance::build;

    #[test]
    fn coda_liquids_scale_with_their_nucleus() {
        let liquid = |text: &str| {
            let mut segs = build(&normalize(text));
            assign_durations(&mut segs);
            let l = segs.iter().position(|s| s.ph == Some(Ph::L)).unwrap();
            segs[l].dur_ms
        };
        assert!(liquid("they held it") > liquid("they felt it") + 1.0);
        assert!(liquid("it was cold.") > liquid("it was colder") + 5.0);
    }

    #[test]
    fn only_plosives_released_into_their_own_syllable_aspirate() {
        let aspirated = |text: &str, ph: Ph| {
            let mut segs = build(&normalize(text));
            assign_durations(&mut segs);
            let i = segs.iter().position(|s| s.ph == Some(ph)).unwrap();
            aspiration_ms(&segs, i) > 0.0
        };
        for (text, ph) in [("a cap", Ph::K), ("a plan", Ph::P), ("atop", Ph::T)] {
            assert!(aspirated(text, ph), "{text}");
        }
        for (text, ph) in [
            ("kept moving", Ph::T),
            ("help me", Ph::P),
            ("cat lay", Ph::T),
            ("back up", Ph::K),
            ("Whitney", Ph::T),
        ] {
            assert!(!aspirated(text, ph), "{text}");
        }
    }

    #[test]
    fn pauses_shrink_faster_than_speech_and_lengthen_with_it() {
        let mut segs = build(&normalize("The quick brown fox, and the lazy dog."));
        assign_durations(&mut segs);
        for scale in [0.5, 2.0] {
            let mut scaled = segs.clone();
            scale_durations(&mut scaled, scale);
            for (natural, scaled) in segs.iter().zip(&scaled) {
                if natural.ph.is_none() && scale < 1.0 {
                    assert!(scaled.dur_ms < natural.dur_ms * scale * scale);
                } else {
                    assert_eq!(scaled.dur_ms, natural.dur_ms * scale);
                }
            }
        }
    }

    #[test]
    fn microprosody_does_not_step_inside_continuous_voicing() {
        for text in ["her hat", "the dog", "from there", "a city", "very good"] {
            for scale in [1.0, 0.5, 0.38] {
                let mut segs = build(&normalize(text));
                assign_durations(&mut segs);
                scale_durations(&mut segs, scale);
                let (starts, lens) = crate::params::frame_layout(&segs);
                let mut f0 = vec![BASE_F0; lens.iter().sum()];
                add_microprosody(&segs, &starts, &lens, 5.0, &mut f0);
                // Voicing runs through sonorants, voiced consonants, flaps
                // and this voice's /h/.
                let voiced: Vec<bool> = (0..segs.len())
                    .flat_map(|i| {
                        let s = &segs[i];
                        let v = s.flap || s.ph.is_some_and(|p| p.is_voiced() || p == Ph::HH);
                        std::iter::repeat_n(v, lens[i])
                    })
                    .collect();
                for k in 1..f0.len() {
                    if voiced[k - 1] && voiced[k] {
                        let step = (f0[k] / f0[k - 1] - 1.0).abs();
                        assert!(step < 0.03, "{text}, scale {scale}, frame {k}: {f0:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn pitch_planning_can_stop_inside_a_single_clause() {
        let mut segs = build(&normalize(&"Alice reads books ".repeat(30)));
        assign_durations(&mut segs);
        let (starts, lens) = crate::params::frame_layout(&segs);
        let polls = std::cell::Cell::new(0);
        let result = f0_contour_cancellable(&segs, &starts, &lens, 5.0, 1.0, &|| {
            polls.set(polls.get() + 1);
            polls.get() == 10
        });
        assert!(result.is_none());
        assert_eq!(polls.get(), 10);
    }

    fn durations(text: &str) -> Vec<(Ph, f64)> {
        let mut segs = build(&normalize(text));
        assign_durations(&mut segs);
        segs.iter()
            .filter_map(|s| s.ph.map(|p| (p, s.dur_ms)))
            .collect()
    }

    #[test]
    fn phrase_final_unstressed_syllable_is_not_lengthened_twice() {
        let d = durations("roses");
        let vowels: Vec<f64> = d
            .iter()
            .filter(|(p, _)| p.is_vowel())
            .map(|x| x.1)
            .collect();
        assert!(vowels[1] < vowels[0], "{vowels:?}");
    }

    #[test]
    fn phrase_final_vowel_is_not_shortened() {
        let d = durations("cat cat");
        let vowels: Vec<f64> = d
            .iter()
            .filter(|(p, _)| p.is_vowel())
            .map(|x| x.1)
            .collect();
        assert!(vowels[1] >= vowels[0], "{vowels:?}");
    }

    #[test]
    fn vowel_before_voiced_consonant_is_longer() {
        let bead = durations("bead")[1].1;
        let beat = durations("beat")[1].1;
        assert!(bead > beat, "bead {bead} beat {beat}");
    }

    #[test]
    fn coda_liquid_takes_time_from_vowel() {
        let bee = durations("bee");
        let beer = durations("beer");
        let vowel = |d: &[(Ph, f64)]| d.iter().find(|(p, _)| p.is_vowel()).unwrap().1;
        let nucleus: f64 = beer[1..].iter().map(|x| x.1).sum();
        assert!(vowel(&beer) < vowel(&bee), "{beer:?} {bee:?}");
        assert!(nucleus > vowel(&bee), "{beer:?} {bee:?}");
    }

    #[test]
    fn aspiration_adds_to_syllable_but_shortens_voicing() {
        let tea = durations("tea")[1].1;
        let dee = durations("dee")[1].1;
        assert!(tea > dee && tea - 50.0 < dee, "tea {tea} dee {dee}");
    }

    #[test]
    fn aspiration_does_not_consume_the_voiced_minimum() {
        let mut segs = build(&normalize("try"));
        for s in &mut segs {
            s.stress = 0;
            s.function_word = true;
        }
        assign_durations(&mut segs);
        let r = segs.iter().position(|s| s.ph == Some(Ph::R)).unwrap();
        let asp = aspiration_ms(&segs, r - 1);
        assert_eq!(asp, 25.0);
        assert_eq!(segs[r].dur_ms, asp + Ph::R.durations().1 / 2.0);
    }

    #[test]
    fn durations_respect_minimums() {
        for (p, d) in durations("the quick brown fox jumps over the lazy dog") {
            assert!(d >= p.durations().1 / 2.0 - 1e-9, "{p:?} {d}");
        }
    }

    #[test]
    fn consonant_between_consonants_gets_cluster_shortening() {
        let d = durations("extreme cat");
        let s = d.iter().find(|(p, _)| *p == Ph::S).unwrap().1;
        let (inh, min) = Ph::S.durations();
        let expected = ((inh - min) * 0.85 * 0.5 + min) * FRICATIVE_FACTOR;
        assert!((s - expected).abs() < 1e-9, "{s} vs {expected}");
    }

    #[test]
    fn cluster_shortening_respects_pauses_and_final_codas() {
        let last_t = |text: &str| {
            durations(text)
                .into_iter()
                .rev()
                .find(|(p, _)| *p == Ph::T)
                .unwrap()
                .1
        };
        let (inh, min) = Ph::T.durations();
        let final_t = ((inh - min) * PHRASE_FINAL_LENGTHENING * 0.85 + min) * STOP_FACTOR;
        assert!((last_t("act") - final_t).abs() < 1e-9);
        assert!((last_t("act. cat") - final_t).abs() < 1e-9);
        let first_t = |text: &str| {
            durations(text)
                .into_iter()
                .find(|(p, _)| *p == Ph::T)
                .unwrap()
                .1
        };
        assert_eq!(first_t("tea"), first_t("bus. tea"));
        assert!(first_t("bus tea") < first_t("bus. tea"));
    }

    #[test]
    fn sentence_breaks_are_longer_than_commas() {
        let comma = pause_ms(Some(Break::Comma), false);
        for ending in [Break::Period, Break::Question, Break::Exclamation] {
            let sentence = pause_ms(Some(ending), false);
            assert!(0.0 < comma && comma < sentence);
        }
    }

    #[test]
    fn accents_return_to_baseline_and_keep_their_shape_at_faster_rates() {
        let mut segs = build(&normalize("bee a bee a bee"));
        segs.retain(|s| s.ph.is_some());
        let lens = vec![40; segs.len()];
        let starts: Vec<usize> = (0..segs.len()).map(|i| i * 40).collect();
        let mut natural = vec![0.0; lens.iter().sum()];
        clause_contour(
            &segs,
            &starts,
            &lens,
            Break::Period,
            0,
            5.0,
            &mut natural,
            &|| false,
        )
        .unwrap();
        let accents: Vec<usize> = (0..segs.len())
            .filter(|&i| segs[i].is_vowel() && segs[i].stress == 1 && !segs[i].function_word)
            .collect();
        assert_eq!(accents.len(), 3);
        let peaks: Vec<usize> = accents
            .iter()
            .map(|&i| starts[i] + lens[i] * 3 / 4)
            .collect();
        let between = (peaks[0] + peaks[1]) / 2;
        let baseline = (START_F0 - DECLINATION_PER_SEC * between as f64 * 0.005).max(BASE_F0);
        assert!((natural[between] - baseline).abs() < 0.01);
        assert!(natural[peaks[0]] > natural[between] + 10.0);
        let fast_starts: Vec<usize> = starts.iter().map(|s| s / 2).collect();
        let fast_lens: Vec<usize> = lens.iter().map(|s| s / 2).collect();
        let mut fast = vec![0.0; fast_lens.iter().sum()];
        clause_contour(
            &segs,
            &fast_starts,
            &fast_lens,
            Break::Period,
            0,
            10.0,
            &mut fast,
            &|| false,
        )
        .unwrap();
        // Boundary tones inherit the preceding sample; compare the accent portion.
        for (i, &f0) in fast.iter().take(peaks[2] / 2).enumerate() {
            assert!((f0 - natural[2 * i]).abs() < 1e-9, "frame {i}");
        }
    }

    #[test]
    fn question_rises_and_statement_falls() {
        for (text, rises) in [
            ("is it raining?", true),
            ("it is raining.", false),
            ("Are you?", true),
            ("Are you.", false),
            ("Who?", true),
            ("Who.", false),
        ] {
            let mut segs = build(&normalize(text));
            assign_durations(&mut segs);
            let lens: Vec<usize> = segs
                .iter()
                .map(|s| (s.dur_ms / 5.0).round().max(1.0) as usize)
                .collect();
            let mut starts = vec![0; lens.len()];
            for k in 1..lens.len() {
                starts[k] = starts[k - 1] + lens[k - 1];
            }
            let f0 = f0_contour(&segs, &starts, &lens, 5.0);
            let last = segs.len() - 2;
            let end = starts[last] + lens[last] - 3;
            let mid = starts[last] + lens[last] / 4;
            assert_eq!(
                f0[end] > f0[mid],
                rises,
                "{text}: {} -> {}",
                f0[mid],
                f0[end]
            );
        }
    }

    #[test]
    fn voiceless_onset_raises_and_voiced_onset_lowers_vowel_pitch() {
        let mut segs = build(&normalize("a pa ba"));
        assign_durations(&mut segs);
        let lens: Vec<usize> = segs
            .iter()
            .map(|s| (s.dur_ms / 5.0).round().max(1.0) as usize)
            .collect();
        let mut starts = vec![0; lens.len()];
        for k in 1..lens.len() {
            starts[k] = starts[k - 1] + lens[k - 1];
        }
        let base = f0_contour(&segs, &starts, &lens, 5.0);
        let mut f0 = base.clone();
        add_microprosody(&segs, &starts, &lens, 5.0, &mut f0);
        let onset = |ph: Ph| starts[segs.iter().position(|s| s.ph == Some(ph)).unwrap() + 1];
        let (p, b) = (onset(Ph::P), onset(Ph::B));
        assert!(f0[p] > base[p] && f0[b] < base[b]);
        assert_eq!(f0[p + 20], base[p + 20]);
    }
}
