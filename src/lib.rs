//! Klatter: a text-to-speech engine based on Klatt (1980, 1987).
//!
//! Pipeline: text normalization -> pronunciation (dictionary, then
//! letter-to-sound rules) -> segmental durations -> parameter frames with F0
//! -> cascade/parallel formant synthesis.

// Signal-processing loops index several parallel per-frame arrays at once,
// which reads more clearly than zipped iterators.
#![allow(clippy::needless_range_loop)]

#[cfg(test)]
mod cmudict;
pub mod klatt;
pub mod lexicon;
pub mod lts;
pub mod morph;
pub mod params;
pub mod phoneme;
pub mod prosody;
pub mod stream;
pub mod text;
pub mod timing;
pub mod utterance;
pub mod wav;

pub use lexicon::{PronunciationError, Pronunciations};
pub use stream::{CancelHandle, Rendered, Utterance, WordEvent};

pub const DEFAULT_SAMPLE_RATE: u32 = 16_000;

/// Leaves PCM headroom for the voicing-presence and consonant-noise boosts.
const GAIN: f64 = 96_000.0;
/// Keeps extreme pitch and inflection settings above the range where pulses become clicks.
const MIN_F0: f64 = 30.0;

/// Klatter's natural speaking rate in long readings, including pauses,
/// calibrated on *Alice's Adventures in Wonderland* so that a rate setting
/// gives that many words per minute.
pub const DEFAULT_WPM: f64 = 219.0;

/// The share of that natural-rate reading spent in pauses, calibrated on the
/// same text so that faster settings also give their words per minute
/// although pauses shrink faster than speech (see `prosody::pause_scale`).
const NATURAL_PAUSE_SHARE: f64 = 0.135;

/// The phones' duration scale that gives `wpm` words per minute in running
/// text. Slower than the natural rate, everything lengthens together; faster,
/// the phones take the scale whose shortened phones and pauses together fit
/// the time the rate allows.
fn duration_scale(wpm: f64) -> f64 {
    let time = DEFAULT_WPM / wpm;
    if time >= 1.0 {
        return time;
    }
    let reading_time = |scale: f64| {
        (1.0 - NATURAL_PAUSE_SHARE) * scale + NATURAL_PAUSE_SHARE * prosody::pause_scale(scale)
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if reading_time(mid) < time {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Speaking settings in physical units, so each host maps its own scales onto them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    /// Words per minute; must be positive and finite.
    pub wpm: f64,
    /// The pitch baseline in Hz; the whole contour moves with it.
    pub pitch_hz: f64,
    /// Pitch-range multiplier: 0 is monotone and 1 is natural.
    pub inflection: f64,
    /// Linear amplitude multiplier: 1 is full level.
    pub volume: f64,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            wpm: DEFAULT_WPM,
            pitch_hz: prosody::BASE_F0,
            inflection: 1.0,
            volume: 1.0,
        }
    }
}

/// Input to [`speak`]: text fragments separated at word boundaries, with marks
/// the caller wants located in the audio. Punctuation may occupy its own fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item<'a> {
    Text(&'a str),
    Mark(u32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Speech {
    pub samples: Vec<i16>,
    /// Each mark with the index of the sample it precedes, in input order.
    pub marks: Vec<(u32, usize)>,
}

pub fn synthesize_frames(input: &str) -> Vec<klatt::Frame> {
    frames_with_marks(&[Item::Text(input)], &Voice::default()).0
}

pub fn synthesize(input: &str, sample_rate: u32) -> Vec<i16> {
    speak(&[Item::Text(input)], sample_rate, &Voice::default()).samples
}

/// Speaks all items as one utterance, so text on either side of a mark keeps
/// its phrasing.
pub fn speak(items: &[Item], sample_rate: u32, voice: &Voice) -> Speech {
    speak_with_pronunciations(items, sample_rate, voice, &Pronunciations::default())
}

pub fn speak_with_pronunciations(
    items: &[Item],
    sample_rate: u32,
    voice: &Voice,
    pronunciations: &Pronunciations,
) -> Speech {
    let mut speech = Speech::default();
    speak_stream_with_pronunciations(
        items,
        sample_rate,
        voice,
        pronunciations,
        |samples, marks| {
            let start = speech.samples.len();
            speech
                .marks
                .extend(marks.iter().map(|&(id, offset)| (id, start + offset)));
            speech.samples.extend_from_slice(samples);
            true
        },
    );
    speech
}

/// Speaks one utterance in blocks of at most 100 ms without resetting synthesis
/// state. Mark offsets are relative to the current block, including its end.
///
/// Return false from `output` to cancel; this function returns false on cancellation.
/// The callback also receives empty blocks as cancellation checkpoints before
/// and after utterance planning. No callback is made after cancellation.
/// All callbacks run synchronously; slices are valid only for the callback.
pub fn speak_stream(
    items: &[Item],
    sample_rate: u32,
    voice: &Voice,
    output: impl FnMut(&[i16], &[(u32, usize)]) -> bool,
) -> bool {
    speak_stream_with_pronunciations(
        items,
        sample_rate,
        voice,
        &Pronunciations::default(),
        output,
    )
}

pub fn speak_stream_with_pronunciations(
    items: &[Item],
    sample_rate: u32,
    voice: &Voice,
    pronunciations: &Pronunciations,
    mut output: impl FnMut(&[i16], &[(u32, usize)]) -> bool,
) -> bool {
    if !output(&[], &[]) {
        return false;
    }
    let (frames, frame_marks) = frames_with_pronunciations(items, voice, pronunciations);
    if !output(&[], &[]) {
        return false;
    }
    let mut synth = klatt::Synth::new(sample_rate);
    let mut marks = frame_marks.into_iter().peekable();
    let mut sample_start = 0;
    let gain = GAIN * voice.volume;
    for (block, chunk) in frames.chunks(20).enumerate() {
        let raw = synth.render(chunk);
        let pcm: Vec<i16> = raw
            .iter()
            .map(|x| (x * gain).round().clamp(i16::MIN as f64, i16::MAX as f64) as i16)
            .collect();
        let frame_end = block * 20 + chunk.len();
        let mut block_marks = Vec::new();
        while marks.peek().is_some_and(|m| m.1 <= frame_end) {
            let (id, frame) = marks.next().unwrap();
            let offset = klatt::Synth::frame_start(frame, sample_rate) - sample_start;
            block_marks.push((id, offset));
        }
        if !output(&pcm, &block_marks) {
            return false;
        }
        sample_start += pcm.len();
    }
    let remaining: Vec<_> = marks.map(|(id, _)| (id, 0)).collect();
    remaining.is_empty() || output(&[], &remaining)
}

fn frames_with_marks(items: &[Item], voice: &Voice) -> (Vec<klatt::Frame>, Vec<(u32, usize)>) {
    frames_with_pronunciations(items, voice, &Pronunciations::default())
}

fn frames_with_pronunciations(
    items: &[Item],
    voice: &Voice,
    pronunciations: &Pronunciations,
) -> (Vec<klatt::Frame>, Vec<(u32, usize)>) {
    let mut tokens = Vec::new();
    for item in items {
        match *item {
            Item::Text(t) => assert!(text::append_with_spans_cancellable(
                t,
                &mut tokens,
                |_, _| {},
                pronunciations,
                &|| false,
            )),
            Item::Mark(id) => tokens.push(text::Token::Mark(id)),
        }
    }
    text::end_utterance(&mut tokens);
    let plan = plan_tokens(&tokens, voice, pronunciations);
    (plan.frames, plan.marks)
}

struct AcousticPlan {
    frames: Vec<klatt::Frame>,
    marks: Vec<(u32, usize)>,
    words: Vec<(usize, usize)>,
}

fn plan_tokens(
    tokens: &[text::Token],
    voice: &Voice,
    pronunciations: &Pronunciations,
) -> AcousticPlan {
    plan_tokens_cancellable(tokens, voice, pronunciations, &|| false)
        .expect("uncancelled acoustic planning")
}

fn plan_tokens_cancellable(
    tokens: &[text::Token],
    voice: &Voice,
    pronunciations: &Pronunciations,
    cancelled: &impl Fn() -> bool,
) -> Option<AcousticPlan> {
    let utterance::SegmentPlan {
        mut segs,
        marks,
        words,
    } = utterance::build_plan_cancellable(tokens, pronunciations, cancelled)?;
    if segs.iter().all(|s| s.ph.is_none()) {
        return Some(AcousticPlan {
            frames: Vec::new(),
            marks: marks.into_iter().map(|(id, _)| (id, 0)).collect(),
            words: Vec::new(),
        });
    }
    prosody::assign_durations(&mut segs);
    if cancelled() {
        return None;
    }
    let scale = duration_scale(voice.wpm);
    prosody::scale_durations(&mut segs, scale);
    if cancelled() {
        return None;
    }
    let (starts, lens) = params::frame_layout(&segs);
    let mut frames = params::build_frames_cancellable(&segs, &starts, &lens, scale, cancelled)?;
    let pitch_scale = voice.pitch_hz / prosody::BASE_F0;
    for f in &mut frames {
        if cancelled() {
            return None;
        }
        // Written so the natural settings leave F0 bit-for-bit unchanged.
        let shaped = f.f0 * voice.inflection + prosody::BASE_F0 * (1.0 - voice.inflection);
        f.f0 = (shaped * pitch_scale).max(MIN_F0);
    }
    let marks = marks
        .into_iter()
        .map(|(id, seg)| (id, starts.get(seg).copied().unwrap_or(frames.len())))
        .collect();
    let words = words
        .into_iter()
        .map(|(token, seg)| (token, starts[seg]))
        .collect();
    if cancelled() {
        None
    } else {
        Some(AcousticPlan {
            frames,
            marks,
            words,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_acoustic_planning_returns_no_partial_plan() {
        let tokens = text::normalize(&"Alice reads books ".repeat(30));
        for limit in [1, 20, 500] {
            let polls = std::cell::Cell::new(0);
            let plan = plan_tokens_cancellable(
                &tokens,
                &Voice::default(),
                &Pronunciations::default(),
                &|| {
                    polls.set(polls.get() + 1);
                    polls.get() == limit
                },
            );
            assert!(plan.is_none(), "cancellation at poll {limit}");
            assert_eq!(polls.get(), limit);
        }
    }

    #[test]
    fn caller_overrides_are_isolated_and_leave_default_speech_unchanged() {
        let items = [Item::Text("Read.")];
        let default = speak(&items, 16000, &Voice::default());
        let mut overrides = Pronunciations::default();
        assert_eq!(
            speak_with_pronunciations(&items, 16000, &Voice::default(), &overrides),
            default
        );
        overrides.insert("read", "R EH1 D").unwrap();
        assert_ne!(
            speak_with_pronunciations(&items, 16000, &Voice::default(), &overrides),
            default
        );
        assert_eq!(speak(&items, 16000, &Voice::default()), default);
        overrides.insert("glorp", "G L AO1 R P").unwrap();
        assert_eq!(
            utterance::pronounce_with("GLORPING", &overrides),
            vec![
                (phoneme::Ph::G, 0),
                (phoneme::Ph::L, 0),
                (phoneme::Ph::AO, 1),
                (phoneme::Ph::R, 0),
                (phoneme::Ph::P, 0),
                (phoneme::Ph::IH, 0),
                (phoneme::Ph::NG, 0)
            ]
        );
    }

    #[test]
    fn whole_identifier_overrides_precede_case_splitting_in_owned_and_pull_speech() {
        let items = [Item::Mark(1), Item::Text("getXMLValue"), Item::Mark(2)];
        let voice = Voice::default();
        let mut overrides = Pronunciations::default();
        overrides.insert("getXMLValue", "V AE1 L Y UW0").unwrap();
        let expected = speak_with_pronunciations(&items, 16000, &voice, &overrides);
        let equivalent = speak(
            &[Item::Mark(1), Item::Text("value"), Item::Mark(2)],
            16000,
            &voice,
        );
        assert_eq!(expected, equivalent);
        assert_ne!(expected.samples, speak(&items, 16000, &voice).samples);
        let mut stream = Utterance::new(&items, 16000, &voice, &overrides).unwrap();
        let mut actual = Speech::default();
        let mut words = Vec::new();
        while !stream.is_finished() {
            let mut buffer = [0; 137];
            let block = stream.render(&mut buffer);
            actual.samples.extend_from_slice(&buffer[..block.samples]);
            actual.marks.extend(block.marks);
            words.extend(block.words);
        }
        assert_eq!(actual, expected);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text_range, 0..11);
    }

    #[test]
    fn source_balance_retains_pcm_headroom_at_full_volume() {
        let text = "Tea, tree, try, quick, key, stay, dee, bee, beer. Cat cat. Or Australia? And she tried.";
        for sample_rate in [16000, 16001] {
            let speech = speak(
                &[Item::Text(text)],
                sample_rate,
                &Voice {
                    wpm: 110.0,
                    ..Voice::default()
                },
            );
            assert!(
                !speech
                    .samples
                    .iter()
                    .any(|&s| s == i16::MIN || s == i16::MAX),
                "{sample_rate} Hz clips at full volume"
            );
        }
    }

    #[test]
    fn produces_audible_speech_of_plausible_length() {
        let audio = synthesize(
            "The quick brown fox jumps over the lazy dog.",
            DEFAULT_SAMPLE_RATE,
        );
        let secs = audio.len() as f64 / DEFAULT_SAMPLE_RATE as f64;
        assert!((1.5..5.0).contains(&secs), "{secs} s");
        let rms =
            (audio.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / audio.len() as f64).sqrt();
        assert!(rms > 1000.0, "rms {rms}");
    }

    #[test]
    fn normalization_variants_produce_identical_frames() {
        for (input, equivalent) in [
            ("don\u{2019}t", "don't"),
            ("Klatt\u{2019}s", "Klatt's"),
            (
                "1,234 people",
                "one thousand two hundred thirty four people",
            ),
            ("-5", "minus five"),
        ] {
            assert!(
                synthesize_frames(input) == synthesize_frames(equivalent),
                "{input}"
            );
        }
    }

    #[test]
    fn unaccented_questions_have_distinct_pitch_tracks() {
        for word in ["Are you", "Who", "To whom"] {
            let question = synthesize_frames(&format!("{word}?"));
            let statement = synthesize_frames(&format!("{word}."));
            assert_eq!(question.len(), statement.len());
            assert!(
                question
                    .iter()
                    .zip(&statement)
                    .any(|(q, s)| q.av > 0.0 && q.f0 > s.f0 + 10.0)
            );
            for (q, s) in question.iter().zip(statement) {
                assert_eq!(klatt::Frame { f0: s.f0, ..*q }, s);
            }
        }
    }

    fn frames(text: &str, voice: Voice) -> Vec<klatt::Frame> {
        frames_with_marks(&[Item::Text(text)], &voice).0
    }

    const SENTENCE: &str =
        "Alice was beginning to get very tired of sitting by her sister on the bank.";

    #[test]
    fn rate_scales_duration() {
        let normal = synthesize_frames(SENTENCE).len() as f64;
        let voice = |factor: f64| Voice {
            wpm: DEFAULT_WPM * factor,
            ..Voice::default()
        };
        let slow = frames(SENTENCE, voice(0.5)).len() as f64;
        assert!(
            (slow * 0.5 / normal - 1.0).abs() < 0.1,
            "{slow} vs {normal}"
        );
        // Faster, the phones take what the shrinking pauses leave, so running
        // text keeps the rate while this sentence's phones follow their scale.
        for factor in [2.0, 3.0] {
            let scale = duration_scale(DEFAULT_WPM * factor);
            assert!(scale > 1.0 / factor);
            let reading = (1.0 - NATURAL_PAUSE_SHARE) * scale
                + NATURAL_PAUSE_SHARE * prosody::pause_scale(scale);
            assert!((reading * factor - 1.0).abs() < 1e-9);
            let len = frames(SENTENCE, voice(factor)).len() as f64;
            assert!(
                (len / normal / scale - 1.0).abs() < 0.1,
                "{factor}: {len} vs {normal}"
            );
        }
    }

    #[test]
    fn faster_voice_preserves_a_less_breathy_vowel_core() {
        let least_breathy = |wpm| {
            frames(
                "bee",
                Voice {
                    wpm,
                    ..Voice::default()
                },
            )
            .iter()
            .filter(|f| f.av > 50.0)
            .map(|f| f.oq)
            .reduce(f64::min)
            .expect("voiced vowel")
        };
        let natural = least_breathy(DEFAULT_WPM);
        for factor in [2.0, 3.0] {
            let fast = least_breathy(DEFAULT_WPM * factor);
            assert!(
                (fast - natural).abs() < 1.0,
                "{factor}x: open quotient {fast}, natural {natural}"
            );
        }
    }

    #[test]
    fn pitch_moves_and_inflection_stretches_the_contour() {
        let f0 = |voice| {
            frames(SENTENCE, voice)
                .iter()
                .map(|f| f.f0)
                .collect::<Vec<_>>()
        };
        let natural = f0(Voice::default());
        let higher = f0(Voice {
            pitch_hz: prosody::BASE_F0 * 1.5,
            ..Voice::default()
        });
        assert!(
            natural
                .iter()
                .zip(&higher)
                .all(|(a, b)| (b / a - 1.5).abs() < 1e-9)
        );
        let flat = f0(Voice {
            inflection: 0.0,
            ..Voice::default()
        });
        assert!(flat.iter().all(|&f| f == prosody::BASE_F0));
        let wide = f0(Voice {
            inflection: 2.0,
            ..Voice::default()
        });
        let range = |v: &[f64]| {
            v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!((range(&wide) / range(&natural) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn volume_scales_amplitude() {
        let full = speak(
            &[Item::Text(SENTENCE)],
            DEFAULT_SAMPLE_RATE,
            &Voice::default(),
        )
        .samples;
        let half = speak(
            &[Item::Text(SENTENCE)],
            DEFAULT_SAMPLE_RATE,
            &Voice {
                volume: 0.5,
                ..Voice::default()
            },
        )
        .samples;
        assert!(
            full.iter()
                .zip(&half)
                .all(|(&a, &b)| (a as f64 / 2.0 - b as f64).abs() <= 1.0)
        );
    }

    #[test]
    fn speech_level_is_consistent_across_sample_rates() {
        let rms = |sr| {
            let audio = synthesize(SENTENCE, sr);
            assert!(
                audio.iter().all(|&s| s != i16::MIN && s != i16::MAX),
                "{sr} Hz clips"
            );
            (audio.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / audio.len() as f64).sqrt()
        };
        let reference = rms(16000);
        for sr in [8000, 22050, 44100, 48000] {
            let db = 20.0 * (rms(sr) / reference).log10();
            assert!(db.abs() < 1.0, "{sr} Hz: {db:.2} dB");
        }
    }

    #[test]
    fn marks_do_not_change_the_audio_and_land_on_word_boundaries() {
        let plain = synthesize("Hello there. How are you?", DEFAULT_SAMPLE_RATE);
        let marked = speak(
            &[
                Item::Mark(1),
                Item::Text("Hello there."),
                Item::Mark(2),
                Item::Text("How are you?"),
                Item::Mark(3),
            ],
            DEFAULT_SAMPLE_RATE,
            &Voice::default(),
        );
        assert_eq!(marked.samples, plain);
        let ids: Vec<u32> = marked.marks.iter().map(|m| m.0).collect();
        assert_eq!(ids, [1, 2, 3]);
        let at = |id: u32| marked.marks.iter().find(|m| m.0 == id).unwrap().1;
        assert_eq!(at(1), 0);
        assert!(at(1) < at(2) && at(2) < at(3) && at(3) <= plain.len());
        let first = speak(
            &[Item::Text("Hello there.")],
            DEFAULT_SAMPLE_RATE,
            &Voice::default(),
        )
        .samples
        .len();
        let pause_gap =
            prosody::pause_ms(Some(text::Break::Period), false) - prosody::pause_ms(None, true);
        assert!(
            (at(2) as f64 - first as f64).abs()
                < (0.1 + pause_gap / 1000.0) * DEFAULT_SAMPLE_RATE as f64
        );
    }

    #[test]
    fn marks_without_speech_produce_no_audio() {
        let speech = speak(
            &[Item::Mark(4), Item::Text(" , "), Item::Mark(5)],
            DEFAULT_SAMPLE_RATE,
            &Voice::default(),
        );
        assert_eq!(
            speech,
            Speech {
                samples: Vec::new(),
                marks: vec![(4, 0), (5, 0)]
            }
        );
    }

    #[test]
    fn text_split_by_a_mark_keeps_its_phrasing() {
        let whole = synthesize_frames("one two three");
        let (split, marks) = frames_with_marks(
            &[Item::Text("one two"), Item::Mark(7), Item::Text("three")],
            &Voice::default(),
        );
        assert_eq!(split, whole);
        assert_eq!(marks.len(), 1);
    }

    #[test]
    fn punctuation_fragments_preserve_audio_and_mark_order() {
        for (left, right) in [
            ("hello", "?"),
            ("hello", ", world"),
            ("hello", ". world"),
            ("hello,", ". world"),
            ("hello.", ", world"),
            ("", "? hello"),
        ] {
            let whole = synthesize(&format!("{left}{right}"), DEFAULT_SAMPLE_RATE);
            let speech = speak(
                &[
                    Item::Mark(1),
                    Item::Text(left),
                    Item::Mark(2),
                    Item::Mark(3),
                    Item::Text(right),
                    Item::Mark(4),
                ],
                DEFAULT_SAMPLE_RATE,
                &Voice::default(),
            );
            assert_eq!(speech.samples, whole, "{left:?} + {right:?}");
            assert_eq!(
                speech.marks.iter().map(|m| m.0).collect::<Vec<_>>(),
                [1, 2, 3, 4]
            );
            assert!(speech.marks.windows(2).all(|m| m[0].1 <= m[1].1));
            assert!(speech.marks.iter().all(|m| m.1 <= speech.samples.len()));
        }
    }

    #[test]
    fn empty_input_is_silent() {
        assert!(synthesize("", DEFAULT_SAMPLE_RATE).iter().all(|&s| s == 0));
    }

    #[test]
    fn streaming_matches_monolithic_audio_and_absolute_marks() {
        let items = [
            Item::Mark(1),
            Item::Text("Hello"),
            Item::Mark(2),
            Item::Text(", world."),
            Item::Mark(3),
            Item::Mark(4),
            Item::Text("How are you?"),
            Item::Mark(5),
        ];
        for sample_rate in [8000, 16000, 16001, 22050, 44100, 48000] {
            let voice = Voice {
                wpm: 330.0,
                pitch_hz: 110.0,
                volume: 0.5,
                ..Voice::default()
            };
            let (frames, marks) = frames_with_marks(&items, &voice);
            let raw = klatt::Synth::new(sample_rate).render(&frames);
            let expected: Vec<i16> = raw
                .iter()
                .map(|x| {
                    (x * (GAIN * voice.volume))
                        .round()
                        .clamp(i16::MIN as f64, i16::MAX as f64) as i16
                })
                .collect();
            let expected_marks: Vec<_> = marks
                .iter()
                .map(|&(id, f)| (id, klatt::Synth::frame_start(f, sample_rate)))
                .collect();
            let mut actual = Speech::default();
            let mut checkpoints = 0;
            assert!(speak_stream(&items, sample_rate, &voice, |audio, marks| {
                if audio.is_empty() && marks.is_empty() {
                    assert!(actual.samples.is_empty());
                    checkpoints += 1;
                }
                assert!(audio.len() <= (sample_rate as usize).div_ceil(10));
                assert!(marks.iter().all(|m| m.1 <= audio.len()));
                actual.marks.extend(
                    marks
                        .iter()
                        .map(|&(id, at)| (id, actual.samples.len() + at)),
                );
                actual.samples.extend_from_slice(audio);
                true
            }));
            assert_eq!(checkpoints, 2);
            assert_eq!(actual.samples, expected, "{sample_rate} Hz");
            assert_eq!(actual.marks, expected_marks, "{sample_rate} Hz");
        }
    }

    #[test]
    fn streaming_can_cancel_before_or_after_planning_or_after_one_audio_block() {
        let text = SENTENCE.repeat(100);
        for cancel_on in [1, 2] {
            let mut calls = 0;
            assert!(!speak_stream(
                &[Item::Text(&text)],
                DEFAULT_SAMPLE_RATE,
                &Voice::default(),
                |audio, marks| {
                    assert!(audio.is_empty() && marks.is_empty());
                    calls += 1;
                    calls != cancel_on
                }
            ));
            assert_eq!(calls, cancel_on);
        }
        let mut audio_blocks = 0;
        assert!(!speak_stream(
            &[Item::Text(&text)],
            DEFAULT_SAMPLE_RATE,
            &Voice::default(),
            |audio, _| {
                assert_eq!(audio_blocks, 0, "callback after cancellation");
                if audio.is_empty() {
                    return true;
                }
                audio_blocks += 1;
                assert_eq!(audio.len(), 1600);
                false
            }
        ));
        assert_eq!(audio_blocks, 1);
    }

    #[test]
    fn streaming_preserves_empty_speech_marks() {
        let mut marks = Vec::new();
        assert!(speak_stream(
            &[Item::Mark(1), Item::Text(" , "), Item::Mark(2)],
            16000,
            &Voice::default(),
            |audio, block_marks| {
                assert!(audio.is_empty());
                marks.extend_from_slice(block_marks);
                true
            },
        ));
        assert_eq!(marks, [(1, 0), (2, 0)]);
    }
}
