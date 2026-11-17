//! Builds the segment sequence that the prosody and parameter stages work on:
//! words become phonemes annotated with syllable, stress and phrase position.

use crate::lexicon;
use crate::lts;
use crate::morph::{self, Suffix};
use crate::phoneme::Ph;
use crate::text::{Break, Token};

// Citation-form stress should not give every function word its own accent.
pub(crate) const FUNCTION_WORDS: &[&str] = &[
    "A", "AM", "AN", "AND", "ARE", "AS", "AT", "BE", "BEEN", "BUT", "BY", "CAN", "COULD", "DID",
    "DO", "DOES", "EACH", "FOR", "FROM", "HAD", "HAS", "HAVE", "HE", "HER", "HIM", "HIS", "HOW",
    "I", "I'D", "I'LL", "I'M", "I'VE", "IF", "IN", "INTO", "IS", "IT", "IT'S", "ITS", "MAY", "ME",
    "MUST", "MY", "NOR", "OF", "ON", "ONTO", "OR", "OUR", "SHALL", "SHE", "SHOULD", "SO", "SOME",
    "THAN", "THAT", "THAT'S", "THE", "THEIR", "THEM", "THEN", "THERE", "THESE", "THEY", "THEY'RE",
    "THIS", "THOSE", "THROUGH", "TO", "UP", "US", "WAS", "WE", "WE'RE", "WERE", "WHAT", "WHEN",
    "WHERE", "WHICH", "WHO", "WHOM", "WHOSE", "WHY", "WILL", "WITH", "WOULD", "YOU", "YOU'RE",
    "YOUR",
];

#[derive(Clone, Debug)]
pub struct Seg {
    /// `None` is a pause.
    pub ph: Option<Ph>,
    /// For pauses: the break that caused it (`None` for the utterance edges).
    pub brk: Option<Break>,
    /// Stress of the syllable this segment belongs to (0, 1 or 2).
    pub stress: u8,
    pub function_word: bool,
    pub word_initial: bool,
    pub word_final: bool,
    pub syllable: usize,
    pub syllables_in_word: usize,
    /// True for segments after the nucleus of their syllable.
    pub coda: bool,
    /// Syllable is the last one before a pause.
    pub phrase_final: bool,
    pub dur_ms: f64,
    /// Unvoiced onset budget within this segment, scaled with its duration.
    pub aspiration_ms: f64,
    /// Parts of the formant transitions from the previous segment and into
    /// the next one that fall inside this segment's span; the rest of the
    /// span is the phone's steady state (see `timing`).
    pub trans_in_ms: f64,
    pub trans_out_ms: f64,
    /// Duration of a diphthong's interior transition (see `timing`).
    pub glide_ms: f64,
    /// A /t/ or /d/ realized as an alveolar flap; `ph` keeps the phoneme so
    /// the vowel before it keeps its voicing-dependent length.
    pub flap: bool,
}

impl Seg {
    fn pause(brk: Option<Break>) -> Seg {
        Seg {
            ph: None,
            brk,
            stress: 0,
            function_word: false,
            word_initial: true,
            word_final: true,
            syllable: 0,
            syllables_in_word: 0,
            coda: false,
            phrase_final: false,
            dur_ms: 0.0,
            aspiration_ms: 0.0,
            trans_in_ms: 0.0,
            trans_out_ms: 0.0,
            glide_ms: 0.0,
            flap: false,
        }
    }

    pub fn is_vowel(&self) -> bool {
        self.ph.is_some_and(Ph::is_vowel)
    }
}

pub fn pronounce(word: &str) -> Vec<(Ph, u8)> {
    pronounce_with(word, &lexicon::Pronunciations::default())
}

pub fn pronounce_with(word: &str, overrides: &lexicon::Pronunciations) -> Vec<(Ph, u8)> {
    let mut phones = word_pronunciation(word, overrides);
    if FUNCTION_WORDS.contains(&word) {
        for (_, stress) in &mut phones {
            *stress = 0;
        }
    }
    phones
}

fn word_pronunciation(word: &str, overrides: &lexicon::Pronunciations) -> Vec<(Ph, u8)> {
    if let Some(entry) = lexicon::known_phones(word, overrides) {
        return entry;
    }
    if let Some(phones) = prefixed_known_word(word, overrides) {
        return phones;
    }
    let split = match word.strip_suffix("'S").filter(|b| !b.is_empty()) {
        Some(base) => Some((base.to_string(), Suffix::S)),
        None => morph::split(word),
    };
    if let Some((root, suffix)) = split {
        let mut phones = pronounce_with(&root, overrides);
        if !phones.is_empty() {
            // Roots like DO and BE are unstressed function words, but doing and being are not.
            if suffix != Suffix::S
                && phones.iter().all(|p| p.1 == 0)
                && let Some(v) = phones.iter_mut().find(|p| p.0.is_vowel())
            {
                v.1 = 1;
            }
            phones.extend(morph::suffix_phones(phones.last().map(|p| p.0), suffix));
            return phones;
        }
    }
    if let Some(phones) = lexicon::derive_known(word, overrides) {
        return phones;
    }
    let letters: String = word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let phones = lts::letters_to_phonemes(word);
    lts::assign_stress(&letters, &phones)
}

/// Productive unstressed prefixes. Words like "untitled" are missing from the
/// dictionary even when the word after the prefix is in it, and stripping a
/// suffix first would leave a non-word root ("untitl") for letter-to-sound.
const UNSTRESSED_WORD_PREFIXES: &[(&str, &[(Ph, u8)])] = {
    use Ph::*;
    &[
        ("UN", &[(AX, 0), (N, 0)]),
        ("RE", &[(R, 0), (IY, 0)]),
        ("DIS", &[(D, 0), (IH, 0), (S, 0)]),
        ("MIS", &[(M, 0), (IH, 0), (S, 0)]),
    ]
};

fn prefixed_known_word(word: &str, overrides: &lexicon::Pronunciations) -> Option<Vec<(Ph, u8)>> {
    UNSTRESSED_WORD_PREFIXES
        .iter()
        .find_map(|(prefix, phones)| {
            let rest = word.strip_prefix(prefix).filter(|r| r.len() >= 4)?;
            let rest_phones = lexicon::known_phones(rest, overrides)?;
            // A stressless remainder is a function word or a fragment, not a root.
            rest_phones.iter().any(|p| p.1 == 1).then(|| {
                let mut all = phones.to_vec();
                all.extend(rest_phones);
                all
            })
        })
}

/// American weak forms of function words (Ladefoged & Johnson 2011, ch. 5),
/// used when another word follows in the phrase. A function word keeps its
/// strong form when spoken alone and at the end of a phrase ("what is it
/// for?"), where it is not reduced.
const WEAK_FORMS: &[(&str, &[(Ph, u8)])] = {
    use Ph::*;
    &[
        ("AS", &[(AX, 0), (Z, 0)]),
        ("AT", &[(AX, 0), (T, 0)]),
        ("CAN", &[(K, 0), (AX, 0), (N, 0)]),
        ("FOR", &[(F, 0), (ER, 0)]),
        ("FROM", &[(F, 0), (R, 0), (AX, 0), (M, 0)]),
        ("NOR", &[(N, 0), (ER, 0)]),
        ("OR", &[(ER, 0)]),
        ("SHALL", &[(SH, 0), (AX, 0), (L, 0)]),
        ("SOME", &[(S, 0), (AX, 0), (M, 0)]),
        ("THAN", &[(DH, 0), (AX, 0), (N, 0)]),
        ("THEM", &[(DH, 0), (AX, 0), (M, 0)]),
        ("YOUR", &[(Y, 0), (ER, 0)]),
    ]
};

/// The form a word takes in connected speech, given the first phoneme of the
/// next word in its phrase (`None` before a pause).
fn connected_form(word: &str, mut phones: Vec<(Ph, u8)>, next: Option<Ph>) -> Vec<(Ph, u8)> {
    use Ph::*;
    if FUNCTION_WORDS.contains(&word) {
        let Some(next) = next else {
            return phones;
        };
        // Before a vowel, "the" is [ði], and "to" and "into" keep their full
        // vowel; before a consonant they take [ə].
        let weak: Option<&[(Ph, u8)]> = match word {
            "THE" if next.is_vowel() => Some(&[(DH, 0), (IY, 0)]),
            "TO" if !next.is_vowel() => Some(&[(T, 0), (AX, 0)]),
            "INTO" if !next.is_vowel() => Some(&[(IH, 0), (N, 0), (T, 0), (AX, 0)]),
            _ => WEAK_FORMS
                .iter()
                .find(|(w, _)| *w == word)
                .map(|(_, form)| *form),
        };
        return weak.map_or(phones, <[(Ph, u8)]>::to_vec);
    }
    let keep = stem_final_schwa(word, &phones);
    reduce_unstressed(&mut phones, keep);
    phones
}

/// A word-final unstressed vowel keeps its quality when a plural or
/// possessive suffix is added (Flemming & Johnson 2007), so "Rosa's" and
/// "sofas" keep the [ə] of "Rosa" and "sofa" while "roses" has [ɨ]. Returns
/// the index of such a stem-final schwa.
fn stem_final_schwa(word: &str, phones: &[(Ph, u8)]) -> Option<usize> {
    let stem = word.strip_suffix("'S").or_else(|| word.strip_suffix('S'))?;
    let n = phones.len();
    let schwa = n.checked_sub(2)?;
    let suffixed = matches!(phones[n - 1].0, Ph::S | Ph::Z) && phones[schwa] == (Ph::AX, 0);
    let stem_schwa = lexicon::cmudict_phones(stem).is_some_and(|p| p.last() == Some(&(Ph::AX, 0)));
    (suffixed && stem_schwa).then_some(schwa)
}

/// American English has two reduced vowels (Flemming & Johnson 2007): [ə]
/// (AX) word-finally, where a plural or possessive suffix leaves it intact,
/// and a high central [ɨ] (IX) elsewhere, as in "rabbit" and "roses". Klatter
/// keeps [ə] next to a vowel, at word edges and before /l/ ("about",
/// "little"), and reduces unstressed /ɪ/ to [ɨ] except in "-ing". `keep` is a
/// stem-final schwa to leave as it is.
fn reduce_unstressed(phones: &mut [(Ph, u8)], keep: Option<usize>) {
    let consonant = |j: Option<usize>| {
        j.and_then(|j| phones.get(j))
            .is_some_and(|p| !p.0.is_vowel())
    };
    let reduced: Vec<bool> = (0..phones.len())
        .map(|i| {
            let next = phones.get(i + 1).map(|p| p.0);
            phones[i].1 == 0
                && keep != Some(i)
                && match phones[i].0 {
                    Ph::IH => next != Some(Ph::NG),
                    Ph::AX => {
                        consonant(i.checked_sub(1)) && consonant(Some(i + 1)) && next != Some(Ph::L)
                    }
                    _ => false,
                }
        })
        .collect();
    for (p, reduced) in phones.iter_mut().zip(reduced) {
        if reduced {
            p.0 = Ph::IX;
        }
    }
}

/// American flapping (Ladefoged & Johnson 2011; Zue & Laferriere 1979): /t/
/// and /d/ after a vowel or /r/ become a brief voiced tap before an
/// unstressed vowel in the same word ("water", "garden"), or before any vowel
/// across a word boundary within a phrase ("get up"). Word-initial stops,
/// stops after /n/ or /l/, and /t/ before a syllabic /n/ ("button", which is
/// glottalized instead) keep their closure and release.
fn mark_flaps(segs: &mut [Seg]) {
    for i in 1..segs.len().saturating_sub(1) {
        let (prev, s, next) = (&segs[i - 1], &segs[i], &segs[i + 1]);
        if !matches!(s.ph, Some(Ph::T | Ph::D))
            || s.word_initial
            || !prev.ph.is_some_and(|p| p.is_vowel() || p == Ph::R)
            || !next.ph.is_some_and(Ph::is_vowel)
        {
            continue;
        }
        let syllabic_n = s.ph == Some(Ph::T)
            && !next.word_final
            && matches!(next.ph, Some(Ph::AX | Ph::IX))
            && segs.get(i + 2).is_some_and(|n| n.ph == Some(Ph::N));
        segs[i].flap = s.word_final || (next.stress == 0 && !syllabic_n);
    }
}

/// Index of the next spoken word in the same phrase as token `index`.
fn next_in_phrase(tokens: &[Token], lexical: &[Vec<(Ph, u8)>], index: usize) -> Option<usize> {
    for j in index + 1..tokens.len() {
        match &tokens[j] {
            Token::Break(_) => return None,
            Token::Word(_) if !lexical[j].is_empty() => return Some(j),
            _ => {}
        }
    }
    None
}

fn legal_onset(cluster: &[Ph]) -> bool {
    use Ph::*;
    match cluster {
        [] => true,
        [NG] | [ZH] => false,
        [_] => true,
        [P | B | K | G | F | TH | SH, R]
        | [T | D, R]
        | [P | B | K | G | F | S, L]
        | [S, P | T | K | M | N | W]
        | [T | D | K | G, W]
        | [P | B | K | F | M | V | HH, Y] => true,
        [S, P | T | K, R] | [S, P | K, L] | [S, K, W] | [S, P | K, Y] => true,
        _ => false,
    }
}

/// Returns, for each phone, its syllable index and whether it is a coda.
fn syllabify(phones: &[(Ph, u8)]) -> Vec<(usize, bool)> {
    let nuclei: Vec<usize> = (0..phones.len())
        .filter(|&i| phones[i].0.is_vowel())
        .collect();
    let mut result = vec![(0, false); phones.len()];
    if nuclei.is_empty() {
        return result;
    }
    for (s, &v) in nuclei.iter().enumerate() {
        result[v] = (s, false);
    }
    for i in 0..nuclei[0] {
        result[i] = (0, false);
    }
    let last = *nuclei.last().unwrap();
    for i in last + 1..phones.len() {
        result[i] = (nuclei.len() - 1, true);
    }
    for (s, pair) in nuclei.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let cluster: Vec<Ph> = phones[a + 1..b].iter().map(|p| p.0).collect();
        let onset_start = (0..=cluster.len())
            .find(|&k| legal_onset(&cluster[k..]))
            .unwrap_or(cluster.len());
        for (k, i) in (a + 1..b).enumerate() {
            result[i] = if k < onset_start {
                (s, true)
            } else {
                (s + 1, false)
            };
        }
    }
    result
}

pub fn build(tokens: &[Token]) -> Vec<Seg> {
    build_with_marks(tokens).0
}

/// Also returns each mark with the index of the segment that follows it; marks
/// before any speech precede the leading pause.
pub fn build_with_marks(tokens: &[Token]) -> (Vec<Seg>, Vec<(u32, usize)>) {
    build_with_pronunciations(tokens, &lexicon::Pronunciations::default())
}

pub fn build_with_pronunciations(
    tokens: &[Token],
    overrides: &lexicon::Pronunciations,
) -> (Vec<Seg>, Vec<(u32, usize)>) {
    let plan = build_plan(tokens, overrides);
    (plan.segs, plan.marks)
}

pub(crate) struct SegmentPlan {
    pub segs: Vec<Seg>,
    pub marks: Vec<(u32, usize)>,
    pub words: Vec<(usize, usize)>,
}

pub(crate) fn build_plan(tokens: &[Token], overrides: &lexicon::Pronunciations) -> SegmentPlan {
    build_plan_cancellable(tokens, overrides, &|| false).expect("uncancelled segment planning")
}

pub(crate) fn build_plan_cancellable(
    tokens: &[Token],
    overrides: &lexicon::Pronunciations,
    cancelled: &impl Fn() -> bool,
) -> Option<SegmentPlan> {
    let mut segs = vec![Seg::pause(None)];
    let mut marks = Vec::new();
    let mut words = Vec::new();
    let mut lexical = Vec::with_capacity(tokens.len());
    for token in tokens {
        if cancelled() {
            return None;
        }
        lexical.push(match token {
            Token::Word(w) => pronounce_with(w, overrides),
            _ => Vec::new(),
        });
    }
    for (index, token) in tokens.iter().enumerate() {
        if cancelled() {
            return None;
        }
        match token {
            Token::Mark(id) => {
                let speech_started = segs.iter().any(|s| s.ph.is_some());
                marks.push((*id, if speech_started { segs.len() } else { 0 }));
            }
            Token::Break(b) => {
                if segs.last().is_some_and(|s| s.ph.is_some()) {
                    segs.push(Seg::pause(Some(*b)));
                }
            }
            Token::Word(w) => {
                let mut phones = std::mem::take(&mut lexical[index]);
                if phones.is_empty() {
                    continue;
                }
                if overrides.get(w).is_none() {
                    let next = next_in_phrase(tokens, &lexical, index).map(|j| lexical[j][0].0);
                    phones = connected_form(w, phones, next);
                }
                words.push((index, segs.len()));
                let syl = syllabify(&phones);
                let n_syl = phones.iter().filter(|p| p.0.is_vowel()).count().max(1);
                let function_word = phones.iter().all(|p| p.1 == 0);
                let mut stress_of = vec![0u8; n_syl];
                for (i, p) in phones.iter().enumerate() {
                    if p.0.is_vowel() {
                        stress_of[syl[i].0] = p.1;
                    }
                }
                for (i, &(ph, _)) in phones.iter().enumerate() {
                    segs.push(Seg {
                        ph: Some(ph),
                        brk: None,
                        stress: stress_of[syl[i].0],
                        function_word,
                        word_initial: i == 0,
                        word_final: i + 1 == phones.len(),
                        syllable: syl[i].0,
                        syllables_in_word: n_syl,
                        coda: syl[i].1,
                        phrase_final: false,
                        dur_ms: 0.0,
                        aspiration_ms: 0.0,
                        trans_in_ms: 0.0,
                        trans_out_ms: 0.0,
                        glide_ms: 0.0,
                        flap: false,
                    });
                }
            }
        }
    }
    if segs.last().is_some_and(|s| s.ph.is_some()) {
        segs.push(Seg::pause(Some(Break::Period)));
    }
    mark_phrase_final(&mut segs);
    mark_flaps(&mut segs);
    if cancelled() {
        None
    } else {
        Some(SegmentPlan { segs, marks, words })
    }
}

fn mark_phrase_final(segs: &mut [Seg]) {
    let mut i = segs.len();
    while i > 0 {
        i -= 1;
        if segs[i].ph.is_some() {
            continue;
        }
        // Walk back to the start of the last word before this pause, then
        // mark the segments belonging to that word's final syllable.
        let mut j = i;
        while j > 0 && segs[j - 1].ph.is_some() {
            j -= 1;
            if segs[j].word_initial {
                break;
            }
        }
        let last_syllable = segs[j..i].iter().map(|s| s.syllable).max();
        if let Some(ls) = last_syllable {
            for s in &mut segs[j..i] {
                if s.syllable == ls {
                    s.phrase_final = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::normalize;

    #[test]
    fn new_derivations_feed_inflections_without_overriding_whole_words() {
        use Ph::*;
        assert_eq!(
            pronounce("BOOKDOCTORS"),
            vec![
                (B, 0),
                (UH, 1),
                (K, 0),
                (D, 0),
                (AA, 2),
                (K, 0),
                (T, 0),
                (ER, 0),
                (Z, 0)
            ]
        );
        assert_eq!(
            pronounce("MOONLESS"),
            vec![(M, 0), (UW, 1), (N, 0), (L, 0), (AX, 0), (S, 0)]
        );
        assert_eq!(
            pronounce("KINDNESS"),
            lexicon::cmudict_phones("KINDNESS").unwrap()
        );
        assert_eq!(
            pronounce("XYZZYFUL"),
            lts::assign_stress("XYZZYFUL", &lts::letters_to_phonemes("XYZZYFUL"))
        );
    }

    #[test]
    fn unstressed_prefix_keeps_the_known_word_after_it() {
        use Ph::*;
        let titled = lexicon::cmudict_phones("TITLED").unwrap();
        let mut untitled = vec![(AX, 0), (N, 0)];
        untitled.extend(titled.iter().copied());
        assert_eq!(pronounce("UNTITLED"), untitled);
        let mut retitled = vec![(R, 0), (IY, 0)];
        retitled.extend(titled);
        assert_eq!(pronounce("RETITLED"), retitled);
    }

    #[test]
    fn cmudict_supplies_irregular_words_through_normalization() {
        use Ph::*;
        for (text, expected) in [
            ("colonel", vec![(K, 0), (ER, 1), (N, 0), (AX, 0), (L, 0)]),
            ("yacht", vec![(Y, 0), (AA, 1), (T, 0)]),
            ("queue", vec![(K, 0), (Y, 0), (UW, 1)]),
            ("choir", vec![(K, 0), (W, 0), (AY, 1), (ER, 0)]),
        ] {
            let phones = pronounce(&text.to_uppercase());
            assert_eq!(phones, expected, "{text}");
            let segs = build(&normalize(text));
            let actual: Vec<Ph> = segs.iter().filter_map(|s| s.ph).collect();
            assert_eq!(
                actual,
                expected.iter().map(|p| p.0).collect::<Vec<_>>(),
                "{text}"
            );
            assert_eq!(
                segs.iter()
                    .filter(|s| s.is_vowel())
                    .map(|s| s.stress)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .filter(|p| p.0.is_vowel())
                    .map(|p| p.1)
                    .collect::<Vec<_>>(),
                "{text}"
            );
        }
    }

    #[test]
    fn pronunciation_overrides_and_function_word_stress_are_independent() {
        use Ph::*;
        assert_eq!(pronounce("READ"), vec![(R, 0), (IY, 1), (D, 0)]);
        assert_eq!(pronounce("WAS"), vec![(W, 0), (AX, 0), (Z, 0)]);
        assert_eq!(
            lexicon::lookup("TO").unwrap(),
            vec![("T".to_string(), 0), ("UW".to_string(), 1)]
        );
        assert_eq!(pronounce("TO"), vec![(T, 0), (UW, 0)]);
        for word in ["to", "do", "she", "into", "you're", "was", "the", "of"] {
            let segs = build(&normalize(word));
            assert!(
                segs.iter()
                    .filter(|s| s.ph.is_some())
                    .all(|s| s.function_word && s.stress == 0),
                "{word}"
            );
        }
    }

    #[test]
    fn derived_words_preserve_function_root_stress_rules() {
        use Ph::*;
        assert!(lexicon::cmudict_phones("COULDS").is_none());
        assert!(lexicon::cmudict_phones("DOED").is_none());
        assert_eq!(pronounce("COULDS"), vec![(K, 0), (UH, 0), (D, 0), (Z, 0)]);
        assert_eq!(pronounce("DOED"), vec![(D, 0), (UW, 1), (D, 0)]);
    }

    #[test]
    fn beautiful_has_a_reduced_ful_vowel() {
        use Ph::*;
        let segs = build(&normalize("beautiful"));
        let phones: Vec<_> = segs.iter().filter_map(|s| s.ph).collect();
        assert_eq!(phones, vec![B, Y, UW, T, IX, F, AX, L]);
        let vowels: Vec<_> = segs
            .iter()
            .filter(|s| s.is_vowel())
            .map(|s| (s.ph.unwrap(), s.stress))
            .collect();
        assert_eq!(vowels, vec![(UW, 1), (IX, 0), (AX, 0)]);
    }

    fn realized(text: &str) -> String {
        build(&normalize(text))
            .iter()
            .filter_map(|s| s.ph.map(|p| if s.flap { "DX" } else { p.name() }))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn alveolar_stops_flap_between_vowels_only_within_a_phrase() {
        assert_eq!(realized("water"), "W AO DX ER");
        assert_eq!(realized("garden"), "G AA R DX IX N");
        assert_eq!(realized("get up"), "G EH DX AH P");
        // Before a stressed vowel, after a nasal, word-initially, before a
        // syllabic /n/ and before a pause, the stop keeps its release.
        assert_eq!(realized("attack"), "AX T AE K");
        assert_eq!(realized("winter"), "W IH N T ER");
        assert_eq!(realized("a toy"), "AX T OY");
        assert_eq!(realized("button"), "B AH T IX N");
        assert_eq!(realized("get, up"), "G EH T AH P");
        let segs = build(&normalize("water"));
        let t = segs.iter().position(|s| s.flap).unwrap();
        assert_eq!(crate::prosody::aspiration_ms(&segs, t), 0.0);
    }

    #[test]
    fn unstressed_vowels_reduce_by_position() {
        assert_eq!(realized("rabbit"), "R AE B IX T");
        assert_eq!(realized("roses"), "R OW Z IX Z");
        assert_eq!(realized("Rosa's"), "R OW Z AX Z");
        assert_eq!(realized("sofas"), "S OW F AX Z");
        assert_eq!(realized("about"), "AX B AW T");
        assert_eq!(realized("little"), "L IH DX AX L");
        assert_eq!(realized("sitting"), "S IH DX IH NG");
        // Pronunciation lookups keep the dictionary's phonemes.
        assert_eq!(
            pronounce("RABBIT"),
            vec![(Ph::R, 0), (Ph::AE, 1), (Ph::B, 0), (Ph::AX, 0), (Ph::T, 0)]
        );
    }

    #[test]
    fn function_words_take_weak_forms_only_before_another_word() {
        assert_eq!(realized("to go"), "T AX G OW");
        assert_eq!(realized("to eat"), "T UW IY T");
        assert_eq!(realized("into the box"), "IH N T AX DH AX B AA K S");
        assert_eq!(realized("the end"), "DH IY EH N D");
        assert_eq!(realized("the dog"), "DH AX D AO G");
        assert_eq!(realized("wait for it"), "W EY T F ER IH T");
        assert_eq!(realized("what is it for?"), "W AH DX IH Z IH T F AO R");
        assert_eq!(realized("they had gone"), "DH EY HH AE D G AO N");
        assert_eq!(realized("to"), "T UW");
        let mut overrides = lexicon::Pronunciations::default();
        overrides.insert("for", "F AO1 R").unwrap();
        let segs = build_with_pronunciations(&normalize("for me"), &overrides).0;
        let phones: Vec<_> = segs.iter().filter_map(|s| s.ph).collect();
        assert_eq!(phones, vec![Ph::F, Ph::AO, Ph::R, Ph::M, Ph::IY]);
    }

    #[test]
    fn unknown_words_keep_rules_and_are_not_autocorrected() {
        for (typo, word) in [
            ("TEH", "THE"),
            ("RECIEVE", "RECEIVE"),
            ("WHCIH", "WHICH"),
            ("BECUASE", "BECAUSE"),
        ] {
            assert!(lexicon::cmudict_phones(typo).is_none());
            let phones = lts::letters_to_phonemes(typo);
            assert_eq!(pronounce(typo), lts::assign_stress(typo, &phones), "{typo}");
            assert_ne!(pronounce(typo), pronounce(word), "{typo}");
        }
    }

    #[test]
    fn unknown_inflections_keep_the_hand_lexicon_root() {
        use Ph::*;
        let root = vec![(K, 0), (L, 0), (AE, 1), (T, 0), (ER, 0)];
        for (word, suffix) in [
            ("KLATTER'S", vec![(Z, 0)]),
            ("KLATTERS", vec![(Z, 0)]),
            ("KLATTERED", vec![(D, 0)]),
            ("KLATTERING", vec![(IH, 0), (NG, 0)]),
        ] {
            assert!(lexicon::cmudict_phones(word).is_none(), "{word}");
            let mut expected = root.clone();
            expected.extend(suffix);
            assert_eq!(pronounce(word), expected, "{word}");
        }
    }

    #[test]
    fn unknown_inflections_can_use_a_cmudict_root() {
        use Ph::*;
        assert!(lexicon::lookup_phones("COLONEL").is_none());
        assert!(lexicon::cmudict_phones("COLONELING").is_none());
        assert_eq!(
            pronounce("COLONELING"),
            vec![(K, 0), (ER, 1), (N, 0), (AX, 0), (L, 0), (IH, 0), (NG, 0)]
        );
    }

    #[test]
    fn possessive_follows_voicing_of_base() {
        let names = |w: &str| pronounce(w).iter().map(|p| p.0).collect::<Vec<_>>();
        assert_eq!(names("KLATT'S"), vec![Ph::K, Ph::L, Ph::AE, Ph::T, Ph::S]);
        assert_eq!(*names("DOG'S").last().unwrap(), Ph::Z);
        assert_eq!(names("JUDGE'S")[3..], [Ph::IH, Ph::Z]);
    }

    #[test]
    fn dictionary_stress_survives_inflection() {
        use Ph::*;
        assert_eq!(pronounce("ALICE"), vec![(AE, 1), (L, 0), (IH, 0), (S, 0)]);
        let root = vec![(K, 0), (AX, 0), (N, 0), (S, 0), (IH, 1), (D, 0), (ER, 0)];
        assert_eq!(pronounce("CONSIDER"), root);
        for (word, suffix) in [
            ("CONSIDERS", vec![(Z, 0)]),
            ("CONSIDERED", vec![(D, 0)]),
            ("CONSIDERING", vec![(IH, 0), (NG, 0)]),
        ] {
            let mut expected = root.clone();
            expected.extend(suffix);
            assert_eq!(pronounce(word), expected, "{word}");
        }
    }

    #[test]
    fn down_and_inflections_keep_the_aw_vowel() {
        use Ph::*;
        let root = vec![(D, 0), (AW, 1), (N, 0)];
        assert_eq!(pronounce("DOWN"), root);
        for (word, suffix) in [
            ("DOWNS", vec![(Z, 0)]),
            ("DOWNED", vec![(D, 0)]),
            ("DOWNING", vec![(IH, 0), (NG, 0)]),
        ] {
            let mut expected = root.clone();
            expected.extend(suffix);
            assert_eq!(pronounce(word), expected, "{word}");
        }
        let segs = build(&normalize("down"));
        let phones: Vec<_> = segs.iter().filter_map(|s| s.ph).collect();
        assert_eq!(phones, vec![D, AW, N]);
        let vowel = segs.iter().find(|s| s.is_vowel()).unwrap();
        assert_eq!(vowel.stress, 1);
        assert!(!vowel.function_word);
    }

    #[test]
    fn ambiguous_plural_pronunciations() {
        use Ph::*;
        for (word, expected) in [
            ("BUSES", vec![B, AH, S, IH, Z]),
            ("GASES", vec![G, AE, S, AX, Z]),
            ("LOSES", vec![L, UW, Z, AX, Z]),
            ("USES", vec![Y, UW, Z, IH, Z]),
            ("ROSES", vec![R, OW, Z, IH, Z]),
            ("BOXES", vec![B, AA, K, S, AX, Z]),
            ("PLACES", vec![P, L, EY, S, AX, Z]),
            ("HOUSES", vec![HH, AW, Z, IH, Z]),
        ] {
            let actual: Vec<Ph> = pronounce(word).into_iter().map(|p| p.0).collect();
            assert_eq!(actual, expected, "{word}");
        }
    }

    #[test]
    fn dictionary_inflections_preserve_the_selected_reading_and_stress() {
        use Ph::*;
        for (word, expected) in [
            (
                "AMAZES",
                vec![(AX, 0), (M, 0), (EY, 1), (Z, 0), (IH, 0), (Z, 0)],
            ),
            (
                "AMUSES",
                vec![(AX, 0), (M, 0), (Y, 0), (UW, 1), (Z, 0), (IH, 0), (Z, 0)],
            ),
            (
                "ABUSES",
                vec![(AX, 0), (B, 0), (Y, 0), (UW, 1), (Z, 0), (IH, 0), (Z, 0)],
            ),
            ("ARISES", vec![(ER, 0), (AY, 1), (Z, 0), (AX, 0), (Z, 0)]),
            (
                "EXERCISES",
                vec![
                    (EH, 1),
                    (K, 0),
                    (S, 0),
                    (ER, 0),
                    (S, 0),
                    (AY, 2),
                    (Z, 0),
                    (AX, 0),
                    (Z, 0),
                ],
            ),
            (
                "RECOGNIZES",
                vec![
                    (R, 0),
                    (EH, 1),
                    (K, 0),
                    (AX, 0),
                    (G, 0),
                    (N, 0),
                    (AY, 2),
                    (Z, 0),
                    (AX, 0),
                    (Z, 0),
                ],
            ),
        ] {
            assert_eq!(pronounce(word), expected, "{word}");
        }
        for root in ["AMAZE", "AMUSE", "MOUSE"] {
            let mut expected = pronounce(root);
            expected.extend([(IH, 0), (Z, 0)]);
            assert_eq!(pronounce(&format!("{root}S")), expected, "{root} + S");
        }
    }

    #[test]
    fn syllabifies_with_maximal_onset() {
        let phones: Vec<(Ph, u8)> = [Ph::EH, Ph::K, Ph::S, Ph::T, Ph::R, Ph::AX]
            .iter()
            .map(|&p| (p, 0))
            .collect();
        let s = syllabify(&phones);
        assert_eq!(
            s,
            vec![
                (0, false),
                (0, true),
                (1, false),
                (1, false),
                (1, false),
                (1, false)
            ]
        );
    }

    #[test]
    fn marks_phrase_final_syllable() {
        let segs = build(&normalize("hello world"));
        let finals: Vec<Ph> = segs
            .iter()
            .filter(|s| s.phrase_final)
            .filter_map(|s| s.ph)
            .collect();
        assert_eq!(finals, vec![Ph::W, Ph::ER, Ph::L, Ph::D]);
        assert!(segs.first().unwrap().ph.is_none());
        assert!(segs.last().unwrap().ph.is_none());
    }
}
