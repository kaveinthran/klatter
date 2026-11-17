//! Phone-and-transition timing (Hertz 1991, Journal of Phonetics 19:91-109).
//!
//! Formant transitions are timed units of their own between phones, rather
//! than parts of the phones (pp. 94-95). Each transition's duration depends on
//! the two phones it joins; phones are the steady states between transitions.
//! Klatt's duration rules give each segment its conventional span (Klatt's
//! segmentation puts the whole transition between a vowel and an obstruent,
//! and any aspiration, inside the vowel; Hertz 1991, pp. 94 and 97), so a
//! phone's steady state is its span minus the transition time inside it.
//! Within a syllable nucleus the vowel pays for the transitions, so vowel and
//! transitions trade time (Hertz & Huffman 1992, p. 1173). The spans stay the
//! frame of reference for voicing, frication, bursts and aspiration, which
//! Hertz, Younes & Zinovieva (1999, p. 2285) align with phone and transition
//! edges in the same way.

use crate::klatt::FRAME_MS;
use crate::phoneme::{Class, Ph, Place};
use crate::utterance::Seg;

/// Transitions between a stop and a sonorant, by the stop's place: the
/// transition durations Kewley-Port (1982, JASA 72:379-389) measured in
/// natural /b d g/ + vowel syllables, averaged across eight vowels: /b/ 18,
/// /d/ 27, /g/ 31 ms. These are her F1 transition durations; her F2 and F3
/// durations could not be checked, and Klatter moves all formants over one
/// transition, so the F1 figures time the whole transition. She measured
/// releases only; Klatter uses the same values for the mirror-image
/// transition into a stop closure, and for voiceless stops when unaspirated.
const LABIAL_STOP_MS: f64 = 18.0;
const ALVEOLAR_STOP_MS: f64 = 27.0;
const VELAR_STOP_MS: f64 = 31.0;
/// Transition durations (ms) read from Hertz's spectrogram-derived deltas.
/// Fricative release into a vowel: [said] s-a 30 ms (1991, Fig. 7, p. 98);
/// five f-a 30 ms (Hertz, Younes & Zinovieva 1999, Figs. 3-4, pp. 2285-2286).
const FRICATIVE_RELEASE_MS: f64 = 30.0;
/// Between two vowels: Said a-i 100 ms, the same as a diphthong's interior
/// transition (1991, Figs. 6-9, pp. 98-100).
const VOWEL_VOWEL_MS: f64 = 100.0;
/// The interior transition of /ay/: five a-y 75 ms (Hertz, Younes &
/// Zinovieva 1999, Figs. 3-4, pp. 2285-2286), whose nucleus is the phones a
/// and y.
const AY_GLIDE_MS: f64 = 75.0;
/// Vowel into a liquid in the same syllable's coda: [eyld] ey-l 80 ms (1991,
/// Figs. 12-13, pp. 102-103).
const VOWEL_LIQUID_MS: f64 = 80.0;
/// Coda liquid into an obstruent: [eyld] l-d 20 ms (1991, Fig. 13, p. 103).
const CODA_SONORANT_CLOSURE_MS: f64 = 20.0;
/// A diphthong's offglide into a consonant: tied y-d 15 ms (Hertz & Huffman
/// 1992, p. 1173).
const OFFGLIDE_CLOSURE_MS: f64 = 15.0;
/// A gliding vowel into a consonant: owed ow-d 40 ms (1991, Fig. 11, p. 102).
const GLIDING_VOWEL_CLOSURE_MS: f64 = 40.0;
/// Affricate release into a vowel: inches, whose affricate Hertz, Younes &
/// Zinovieva (1999, Fig. 5, p. 2286) build as t + sh, sh into the reduced
/// vowel 40 ms.
const AFFRICATE_RELEASE_MS: f64 = 40.0;
/// Vowel into a coda nasal: inches i-n 20 ms (Hertz, Younes & Zinovieva
/// 1999, Fig. 5, p. 2286).
const VOWEL_NASAL_MS: f64 = 20.0;
/// Vowel into a fricative: inches, the reduced vowel into z 18 ms (Hertz,
/// Younes & Zinovieva 1999, Fig. 5, p. 2286).
const VOWEL_FRICATIVE_MS: f64 = 18.0;
/// Between two consonants: inches n-t 1 ms (Hertz, Younes & Zinovieva 1999,
/// Fig. 5, p. 2286), an effectively instant change.
const CONSONANT_CONSONANT_MS: f64 = 1.0;
/// Hertz's sonorant values are all for a vowel moving into a liquid or nasal
/// of its own syllable's coda. The papers give no values for an onset nasal,
/// liquid or glide into its vowel (or a vowel into the next syllable's
/// onset), for a vowel into a coda glide, for a nasal next to another
/// sonorant, or for the joins around /h/; these are Klatter's own choices.
const SONORANT_MS: f64 = 40.0;
const ASPIRATE_MS: f64 = 60.0;
/// Hertz (1991, p. 106) expects fast speech to need "a second time stream to
/// shorten transitions", since transitions and steady states are not
/// shortened by the same proportion; her fast [said] keeps its transitions
/// while the [a] shrinks (Figs. 16-17). She gives the direction but no
/// figure. Above the natural rate, transitions shrink by the square root of
/// the phones' duration scale, less than the phones do: Klatter's own choice
/// of power, picked by speech-recognition intelligibility of Harvard
/// sentences at 450 words per minute, where fixed transitions left most
/// vowels only a 5 ms steady state and more words were misheard.
const TRANSITION_RATE_EXPONENT: f64 = 0.5;

fn is_obstruent(p: Ph) -> bool {
    matches!(p.class(), Class::Stop | Class::Affricate | Class::Fricative)
}

/// Whether the transition out of segment `i` starts from a diphthong's
/// offglide, a gliding vowel's end, or a coda liquid or glide.
fn closure_from_offglide(segs: &[Seg], i: usize) -> Option<f64> {
    let p = segs[i].ph?;
    match p {
        Ph::AY | Ph::AW | Ph::OY => Some(OFFGLIDE_CLOSURE_MS),
        _ if p.spec().f_end.is_some() => Some(GLIDING_VOWEL_CLOSURE_MS),
        _ if segs[i].coda && matches!(p.class(), Class::Liquid | Class::Glide) => {
            Some(CODA_SONORANT_CLOSURE_MS)
        }
        _ => None,
    }
}

/// The transition between a stop and a sonorant, by the stop's place.
fn stop_transition_ms(p: Ph) -> f64 {
    match p.place() {
        Place::Labial => LABIAL_STOP_MS,
        Place::Velar => VELAR_STOP_MS,
        _ => ALVEOLAR_STOP_MS,
    }
}

/// A diphthong's interior transition (`Seg::glide_ms`): /ay/'s own value,
/// and Hertz's vowel-vowel value for /aw/ and /oy/.
fn diphthong_glide_ms(p: Ph) -> f64 {
    if p == Ph::AY {
        AY_GLIDE_MS
    } else {
        VOWEL_VOWEL_MS
    }
}

/// Duration (ms, at the natural rate) of the transition between segments `i`
/// and `i + 1`, or 0 next to a pause.
fn published_ms(segs: &[Seg], i: usize) -> f64 {
    let (Some(l), Some(r)) = (segs[i].ph, segs[i + 1].ph) else {
        return 0.0;
    };
    let (lc, rc) = (l.class(), r.class());
    let ms = if lc == Class::Aspirate || rc == Class::Aspirate {
        ASPIRATE_MS
    } else if is_obstruent(l) && r.is_sonorant() {
        match lc {
            _ if rc == Class::Nasal => CONSONANT_CONSONANT_MS,
            Class::Fricative => FRICATIVE_RELEASE_MS,
            Class::Affricate => AFFRICATE_RELEASE_MS,
            _ => stop_transition_ms(l),
        }
    } else if l.is_sonorant() && is_obstruent(r) {
        match closure_from_offglide(segs, i) {
            Some(ms) => ms,
            None if lc == Class::Nasal => CONSONANT_CONSONANT_MS,
            None if rc == Class::Fricative => VOWEL_FRICATIVE_MS,
            None => stop_transition_ms(r),
        }
    } else if lc == Class::Vowel && rc == Class::Vowel {
        VOWEL_VOWEL_MS
    } else if lc == Class::Vowel || rc == Class::Vowel {
        // Hertz's values join a vowel to a consonant of its own syllable.
        let nucleus = lc == Class::Vowel && segs[i + 1].coda;
        match if lc == Class::Vowel { rc } else { lc } {
            Class::Liquid if nucleus => VOWEL_LIQUID_MS,
            Class::Nasal if nucleus => VOWEL_NASAL_MS,
            _ => SONORANT_MS,
        }
    } else if (lc == Class::Nasal) != (rc == Class::Nasal) && l.is_sonorant() && r.is_sonorant() {
        SONORANT_MS
    } else {
        CONSONANT_CONSONANT_MS
    };
    // A tap is a single ballistic flick of the tongue tip (Ladefoged &
    // Maddieson 1996, ch. 7), so the vowels around it move only for as long
    // as the tap itself lasts.
    match [i, i + 1].into_iter().find(|&j| segs[j].flap) {
        Some(j) => ms.min(segs[j].dur_ms),
        None => ms,
    }
}

/// Aspiration is superimposed on the release transition (Hertz 1991, pp.
/// 96-97; Hertz & Huffman 1992, p. 1172), which is generally longer than the
/// unaspirated one ([tad] 90 ms, [dad] 65 ms; Hertz 1991, p. 96, Fig. 4).
fn aspirated(segs: &[Seg], i: usize) -> bool {
    segs[i + 1].aspiration_ms > 0.0
}

/// Share of the transition between `i` and `i + 1` that lies inside segment
/// `i`'s span.
fn share_before(segs: &[Seg], i: usize) -> f64 {
    let (Some(l), Some(r)) = (segs[i].ph, segs[i + 1].ph) else {
        return 0.0;
    };
    match (l.is_vowel(), r.is_vowel()) {
        // /h/ aspirates the transition between the phones around it (Hertz
        // 1991, p. 97), so the transition is shared with it.
        _ if l.class() == Class::Aspirate || r.class() == Class::Aspirate => 0.5,
        // Conventional segmentation splits a vowel-vowel transition halfway
        // (Hertz 1991, p. 97).
        (true, true) => 0.5,
        // Voiced transitions into and out of the vowel belong to its nucleus
        // and take their time from the vowel (Hertz & Huffman 1992, p. 1173).
        (true, false) => 1.0,
        (false, true) => 0.0,
        _ if l.is_sonorant() && is_obstruent(r) => 1.0,
        _ if is_obstruent(l) && r.is_sonorant() => 0.0,
        _ => 0.5,
    }
}

/// How much transitions shrink at a duration `scale` (below 1 is faster than
/// the natural rate). They are stable when syllables are lengthened, so only
/// steady states stretch (Hertz 1991, p. 94).
fn rate_factor(scale: f64) -> f64 {
    scale.min(1.0).powf(TRANSITION_RATE_EXPONENT)
}

/// Transition duration (ms) between `i` and `i + 1` at a duration `scale`.
/// An aspirated release spans the aspiration, which shrinks with the phones,
/// and at least the unaspirated release.
fn transition_ms(segs: &[Seg], i: usize, scale: f64) -> f64 {
    let ms = published_ms(segs, i) * rate_factor(scale);
    if aspirated(segs, i) {
        ms.max(segs[i + 1].aspiration_ms)
    } else {
        ms
    }
}

/// The parts of the transitions on each side that lie inside each segment,
/// before any steady-state floor.
fn raw_parts(segs: &[Seg], scale: f64) -> Vec<(f64, f64)> {
    let mut parts = vec![(0.0, 0.0); segs.len()];
    for i in 0..segs.len().saturating_sub(1) {
        let t = transition_ms(segs, i, scale);
        let share = share_before(segs, i);
        parts[i].1 = t * share;
        parts[i + 1].0 = t * (1.0 - share);
    }
    parts
}

/// Shortens two transitions to fit `budget`, the longer one first, so a short
/// transition such as a stop release, whose duration cues the place, keeps
/// its time while a long sonorant join gives way.
fn squeeze(a: f64, b: f64, budget: f64) -> (f64, f64) {
    let half = budget / 2.0;
    if a + b <= budget {
        (a, b)
    } else if a <= half {
        (a, budget - a)
    } else if b <= half {
        (budget - b, b)
    } else {
        (half, half)
    }
}

/// Times the transitions for segments whose spans (`dur_ms`) are set, at a
/// duration `scale` (1 for the natural rate). Every phone keeps a steady
/// state of at least one frame, so phones never vanish in fast speech; where
/// the transitions would leave less, they are shortened, the longer first,
/// and still reach their targets.
pub fn assign(segs: &mut [Seg], scale: f64) {
    let parts = raw_parts(segs, scale);
    for (s, (into, out)) in segs.iter_mut().zip(parts) {
        s.glide_ms = s.ph.map_or(0.0, diphthong_glide_ms) * rate_factor(scale);
        let available = (s.dur_ms - FRAME_MS).max(0.0);
        // Aspiration keeps spanning the release transition, so a short
        // sonorant takes the time from its other transition.
        (s.trans_in_ms, s.trans_out_ms) = if s.aspiration_ms > 0.0 && into <= available {
            (into, out.min(available - into))
        } else {
            squeeze(into, out, available)
        };
    }
}

/// Natural-rate transition time inside the span of the coda liquid or glide
/// at `coda`. The nucleus moves it out of the vowel's span into the coda's,
/// because the non-vowel phones of a nucleus keep their assigned durations
/// and the vowel takes whatever is left (Hertz & Huffman 1992, p. 1173).
pub fn coda_transition_ms(segs: &[Seg], coda: usize) -> f64 {
    let parts = raw_parts(&segs[coda - 1..(coda + 2).min(segs.len())], 1.0);
    parts[1].0 + parts[1].1
}

/// The phone's steady state: its span less the transitions inside it.
pub fn steady_ms(seg: &Seg) -> f64 {
    seg.dur_ms - seg.trans_in_ms - seg.trans_out_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prosody::{assign_durations, scale_durations};
    use crate::text::normalize;
    use crate::utterance::build;

    fn timed(text: &str, scale: f64) -> Vec<Seg> {
        let mut segs = build(&normalize(text));
        assign_durations(&mut segs);
        scale_durations(&mut segs, scale);
        segs
    }

    fn find(segs: &[Seg], ph: Ph) -> usize {
        segs.iter().position(|s| s.ph == Some(ph)).unwrap()
    }

    #[test]
    fn lengthening_stretches_the_steady_state_not_the_transitions() {
        let (bead, beat) = (timed("bead", 2.0), timed("beat", 2.0));
        let (v, w) = (find(&bead, Ph::IY), find(&beat, Ph::IY));
        assert_eq!(bead[v].trans_in_ms, beat[w].trans_in_ms);
        assert_eq!(bead[v].trans_out_ms, beat[w].trans_out_ms);
        assert!(steady_ms(&bead[v]) > steady_ms(&beat[w]) + 20.0);
    }

    #[test]
    fn transition_durations_depend_on_the_phones_they_join() {
        let release = |text: &str, vowel: Ph| {
            let segs = timed(text, 1.0);
            segs[find(&segs, vowel)].trans_in_ms
        };
        let stop = release("a bee", Ph::IY);
        assert_eq!(stop, LABIAL_STOP_MS);
        assert_eq!(release("a dee", Ph::IY), ALVEOLAR_STOP_MS);
        assert_eq!(release("a ghee", Ph::IY), VELAR_STOP_MS);
        assert_eq!(release("a sea", Ph::IY), FRICATIVE_RELEASE_MS);
        assert_eq!(release("a lee", Ph::IY), SONORANT_MS);
        let tea = timed("tea", 1.0);
        let v = find(&tea, Ph::IY);
        assert_eq!(tea[v].trans_in_ms, tea[v].aspiration_ms);
        let ba = timed("bah", 1.0);
        let a = find(&ba, Ph::AA);
        assert_eq!(ba[a].trans_in_ms, stop);
    }

    #[test]
    fn transitions_shrink_less_than_phones_in_fast_speech_and_keep_their_durations_when_slow() {
        let natural = timed("a bee.", 1.0);
        let v = find(&natural, Ph::IY);
        let fast = timed("a bee.", 0.49);
        assert!((fast[v].dur_ms - natural[v].dur_ms * 0.49).abs() < 1e-9);
        assert!((fast[v].trans_in_ms - natural[v].trans_in_ms * 0.7).abs() < 1e-9);
        let slow = timed("a bee.", 2.0);
        assert_eq!(slow[v].trans_in_ms, natural[v].trans_in_ms);
        assert!(steady_ms(&slow[v]) > 2.0 * steady_ms(&natural[v]));
    }

    #[test]
    fn crowded_vowels_shorten_their_longer_transition_first() {
        let segs = timed("the ball", 0.45);
        let v = find(&segs, Ph::AO);
        assert!(segs[v].trans_out_ms < VOWEL_LIQUID_MS);
        assert_eq!(segs[v].trans_in_ms, LABIAL_STOP_MS * rate_factor(0.45));
    }

    #[test]
    fn aspirated_releases_are_never_shorter_than_unaspirated_ones() {
        for scale in [1.0, 0.45] {
            let (tea, dee) = (timed("a tea", scale), timed("a dee", scale));
            let (v, w) = (find(&tea, Ph::IY), find(&dee, Ph::IY));
            assert!(tea[v].trans_in_ms >= dee[w].trans_in_ms, "{scale}");
            assert!(tea[v].trans_in_ms >= tea[v].aspiration_ms, "{scale}");
        }
    }

    #[test]
    fn transitions_next_to_a_tap_last_no_longer_than_the_tap() {
        for scale in [1.0, 0.45] {
            let segs = timed("a city", scale);
            let tap = segs.iter().position(|s| s.flap).unwrap();
            assert!(segs[tap - 1].trans_out_ms <= segs[tap].dur_ms + 1e-9);
            assert!(segs[tap + 1].trans_in_ms <= segs[tap].dur_ms + 1e-9);
        }
    }

    #[test]
    fn the_vowel_pays_for_transitions_inside_its_nucleus() {
        let segs = timed("held", 2.0);
        let (v, l) = (find(&segs, Ph::EH), find(&segs, Ph::L));
        assert_eq!(segs[v].trans_out_ms, VOWEL_LIQUID_MS);
        assert_eq!(segs[l].trans_in_ms, 0.0);
        assert_eq!(segs[l].trans_out_ms, CODA_SONORANT_CLOSURE_MS);
    }

    #[test]
    fn every_phone_keeps_a_steady_frame_at_extreme_rates() {
        let text = "The quick brown fox, a strange idea, jumped over the lazy dogs.";
        for scale in [1.0, 0.3, 0.1] {
            for s in timed(text, scale).iter().filter(|s| s.ph.is_some()) {
                assert!(s.trans_in_ms >= 0.0 && s.trans_out_ms >= 0.0);
                assert!(
                    steady_ms(s) >= FRAME_MS.min(s.dur_ms) - 1e-9,
                    "{:?} at {scale}: {} ms span, {} ms steady",
                    s.ph,
                    s.dur_ms,
                    steady_ms(s)
                );
            }
        }
    }
}
