//! Dumps the segment timeline the engine builds for a text, as TSV on stdout.
//!
//! Columns: ph, stress, fn (function word), final (phrase-final), start_ms,
//! dur_ms (frame-quantized, as rendered), f1-f3 (table targets), wi (word
//! initial), flap (1 for a /t/ or /d/ spoken as a flap), trans_in_ms and
//! trans_out_ms (the formant transitions from the previous phone and into
//! the next that lie inside this span; the rest is the phone's steady state),
//! word (the word spelled out on its first phone).
//!
//! Usage:
//!   cargo run --release --example segdump -- text.txt > segs.tsv
//!   cargo run --release --example segdump -- text.txt SCALE out.wav > segs.tsv
//!   cargo run --release --example segdump -- --frames text.txt [SCALE] > frames.tsv
//!
//! With SCALE, every duration is multiplied by it and the result is also
//! rendered to out.wav (16 kHz), e.g. to inspect timing at a faster rate.
//! With --frames, it prints one row per 5 ms frame instead: the segment, its
//! stress, position, word and timing, the voice-source parameters (AV, AH,
//! OQ, TL, B1, F0), the rendered level in dB, then the formants and the
//! remaining amplitudes (F1-F3, B2-B3, AVS, AF, AB, FNZ, A2-A6).
use klatter::*;
use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

fn parse_args(args: &[String]) -> Option<(&str, Option<(f64, &str)>)> {
    match args {
        [input] => Some((input, None)),
        [input, scale, output] => {
            let scale = scale
                .parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)?;
            Some((input, Some((scale, output))))
        }
        _ => None,
    }
}

fn run(input: &str, render: Option<(f64, &str)>) -> io::Result<()> {
    let text = std::fs::read_to_string(input)?;
    let tokens = text::normalize(&text);
    let words: Vec<&str> = tokens
        .iter()
        .filter_map(|t| match t {
            text::Token::Word(w) => Some(w.as_str()),
            _ => None,
        })
        .collect();
    let mut word_iter = words.iter();
    let mut segs = utterance::build(&tokens);
    prosody::assign_durations(&mut segs);

    if let Some((scale, _)) = render {
        prosody::scale_durations(&mut segs, scale);
    }
    let (starts, lens) = params::frame_layout(&segs);
    if let Some((scale, output)) = render {
        let voice = Voice {
            wpm: DEFAULT_WPM / scale,
            ..Voice::default()
        };
        let speech = speak(&[Item::Text(&text)], DEFAULT_SAMPLE_RATE, &voice);
        wav::write_wav(
            BufWriter::new(std::fs::File::create(output)?),
            &speech.samples,
            DEFAULT_SAMPLE_RATE,
        )?;
    }

    let mut out = BufWriter::new(io::stdout().lock());
    writeln!(
        out,
        "ph\tstress\tfn\tfinal\tstart_ms\tdur_ms\tf1\tf2\tf3\twi\tflap\ttrans_in_ms\ttrans_out_ms\tword"
    )?;
    for (i, seg) in segs.iter().enumerate() {
        let (name, f) = match seg.ph {
            Some(p) => (p.name(), p.spec().f),
            None => ("_", [0.0; 3]),
        };
        let word = if seg.word_initial && seg.ph.is_some() {
            word_iter.next().copied().unwrap_or("?")
        } else {
            ""
        };
        writeln!(
            out,
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{word}",
            seg.stress,
            seg.function_word as u8,
            seg.phrase_final as u8,
            starts[i] as f64 * klatt::FRAME_MS,
            lens[i] as f64 * klatt::FRAME_MS,
            f[0],
            f[1],
            f[2],
            seg.word_initial as u8,
            seg.flap as u8,
            seg.trans_in_ms,
            seg.trans_out_ms,
        )?;
    }
    out.flush()
}

/// Per-frame parameters and rendered level, at the natural rate or `scale`
/// times its durations, with the segment and word each frame belongs to.
fn run_frames(input: &str, scale: f64) -> io::Result<()> {
    let text = std::fs::read_to_string(input)?;
    let tokens = text::normalize(&text);
    let mut words = tokens.iter().filter_map(|t| match t {
        text::Token::Word(w) => Some(w.as_str()),
        _ => None,
    });
    let mut segs = utterance::build(&tokens);
    prosody::assign_durations(&mut segs);
    prosody::scale_durations(&mut segs, scale);
    let (starts, lens) = params::frame_layout(&segs);
    let frames = params::build_frames_with_time_scale(&segs, &starts, &lens, scale);
    let rate = DEFAULT_SAMPLE_RATE;
    let audio = klatt::Synth::new(rate).render(&frames);
    let mut out = BufWriter::new(io::stdout().lock());
    writeln!(
        out,
        "seg\tph\tstress\tfn\tfinal\tk\tlen\tav\tah\toq\ttl\tb1\tf0\tdb\t\
         word\twi\twf\tcoda\tsyl\tnsyl\tflap\tasp_ms\ttin_ms\ttout_ms\t\
         f1\tf2\tf3\tb2\tb3\tavs\taf\tab\tfnz\ta2\ta3\ta4\ta5\ta6"
    )?;
    let mut word = "";
    for (i, seg) in segs.iter().enumerate() {
        let name = seg.ph.map_or("_", |p| p.name());
        if seg.ph.is_none() {
            word = "";
        } else if seg.word_initial {
            word = words.next().unwrap_or("?");
        }
        for k in 0..lens[i] {
            let frame = starts[i] + k;
            let a = klatt::Synth::frame_start(frame, rate).min(audio.len());
            let b = klatt::Synth::frame_start(frame + 1, rate).min(audio.len());
            let power = audio[a..b].iter().map(|x| x * x).sum::<f64>() / (b - a).max(1) as f64;
            let fr = &frames[frame];
            writeln!(
                out,
                "{i}\t{name}\t{}\t{}\t{}\t{k}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.0}\t{:.1}\t{:.2}\t\
                 {word}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t\
                 {:.0}\t{:.0}\t{:.0}\t{:.0}\t{:.0}\t{:.1}\t{:.1}\t{:.1}\t{:.0}\t\
                 {:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}",
                seg.stress,
                seg.function_word as u8,
                seg.phrase_final as u8,
                lens[i],
                fr.av,
                fr.ah,
                fr.oq,
                fr.tl,
                fr.b[0],
                fr.f0,
                10.0 * (power + 1e-12).log10(),
                seg.word_initial as u8,
                seg.word_final as u8,
                seg.coda as u8,
                seg.syllable,
                seg.syllables_in_word,
                seg.flap as u8,
                seg.aspiration_ms,
                seg.trans_in_ms,
                seg.trans_out_ms,
                fr.f[0],
                fr.f[1],
                fr.f[2],
                fr.b[1],
                fr.b[2],
                fr.avs,
                fr.af,
                fr.ab,
                fr.fnz,
                fr.a[0],
                fr.a[1],
                fr.a[2],
                fr.a[3],
                fr.a[4],
            )?;
        }
    }
    out.flush()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--frames") {
        let scale = args.get(2).and_then(|s| s.parse::<f64>().ok());
        let (Some(input), Some(scale)) = (
            args.get(1),
            scale
                .filter(|s| s.is_finite() && *s > 0.0 && args.len() <= 3)
                .or((args.len() == 2).then_some(1.0)),
        ) else {
            eprintln!("usage: segdump --frames <text.txt> [SCALE]");
            return ExitCode::from(2);
        };
        return match run_frames(input, scale) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("segdump: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some((input, render)) = parse_args(&args) else {
        eprintln!("usage: segdump <text.txt> [SCALE <out.wav>]");
        return ExitCode::from(2);
    };
    match run(input, render) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("segdump: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_complete_arguments_and_positive_finite_scales() {
        for args in [
            vec![],
            vec!["text.txt", "2"],
            vec!["text.txt", "0", "out.wav"],
            vec!["text.txt", "-1", "out.wav"],
            vec!["text.txt", "nan", "out.wav"],
            vec!["text.txt", "inf", "out.wav"],
            vec!["text.txt", "fast", "out.wav"],
            vec!["text.txt", "2", "out.wav", "extra"],
        ] {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            assert!(parse_args(&args).is_none(), "{args:?}");
        }
        assert_eq!(parse_args(&["text.txt".into()]), Some(("text.txt", None)));
        assert_eq!(
            parse_args(&["text.txt".into(), "0.5".into(), "out.wav".into()]),
            Some(("text.txt", Some((0.5, "out.wav"))))
        );
    }
}
