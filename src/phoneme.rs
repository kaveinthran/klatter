//! Phoneme inventory (ARPAbet) with features, Klatt (1979) inherent and minimum
//! durations, and synthesis targets based on Klatt (1980), Table III. Formants,
//! and consonant bandwidths, follow the table; vowel bandwidths follow Hawks &
//! Miller (1995). Consonant amplitudes are set for Klatter's peak-normalized
//! parallel branch, so they are not the table's values.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ph {
    IY,
    IH,
    EY,
    EH,
    AE,
    AA,
    AO,
    OW,
    UH,
    UW,
    AH,
    AX,
    IX,
    ER,
    AY,
    AW,
    OY,
    P,
    B,
    T,
    D,
    K,
    G,
    CH,
    JH,
    F,
    V,
    TH,
    DH,
    S,
    Z,
    SH,
    ZH,
    HH,
    M,
    N,
    NG,
    L,
    R,
    W,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Vowel,
    Stop,
    Affricate,
    Fricative,
    Aspirate,
    Nasal,
    Liquid,
    Glide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Labial,
    Dental,
    Alveolar,
    PostAlveolar,
    Velar,
    Other,
}

/// Synthesis targets. For stops and affricates, the source amplitudes describe
/// the release burst or frication; closures are derived from `voiced`.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub f: [f64; 3],
    pub b: [f64; 3],
    /// Offglide target for diphthongs.
    pub f_end: Option<[f64; 3]>,
    pub av: f64,
    pub af: f64,
    pub ab: f64,
    /// Parallel amplitudes A2..A6.
    pub a: [f64; 5],
}

const fn sp(f: [f64; 3], b: [f64; 3], av: f64) -> Spec {
    Spec {
        f,
        b,
        f_end: None,
        av,
        af: 0.0,
        ab: 0.0,
        a: [0.0; 5],
    }
}

/// Every vowel is voiced at the same AV.
pub(crate) const VOWEL_AV: f64 = 60.0;

/// Hawks & Miller's (1995) regression of formant bandwidth on formant
/// frequency, fitted to bandwidths measured in natural vowels: one fifth-order
/// polynomial below 500 Hz and one above, scaled for the talker's F0
/// (S = 1 at 132 Hz). Klatter's baseline F0 sets S. Coefficients as
/// reproduced by VoiceSauce (Shue et al. 2011).
pub fn vowel_bandwidth(f: f64) -> f64 {
    const BELOW_500: [f64; 6] = [
        165.327516,
        -6.73636734e-1,
        1.80874446e-3,
        -4.52201682e-6,
        7.49514000e-9,
        -4.70219241e-12,
    ];
    const ABOVE_500: [f64; 6] = [
        15.8146139,
        8.10159009e-2,
        -9.79728215e-5,
        5.28725064e-8,
        -1.07099364e-11,
        7.91528509e-16,
    ];
    let s = 1.0 + 0.25 * (crate::prosody::BASE_F0 - 132.0) / 88.0;
    let c = if f < 500.0 { &BELOW_500 } else { &ABOVE_500 };
    s * c.iter().rev().fold(0.0, |acc, &k| acc * f + k)
}

pub fn vowel_bandwidths(f: [f64; 3]) -> [f64; 3] {
    f.map(vowel_bandwidth)
}

fn vowel(f: [f64; 3]) -> Spec {
    sp(f, vowel_bandwidths(f), VOWEL_AV)
}

/// A diphthong's bandwidths follow its formants through the glide.
fn diphthong(f: [f64; 3], f_end: [f64; 3]) -> Spec {
    Spec {
        f_end: Some(f_end),
        ..vowel(f)
    }
}

const fn fr(f: [f64; 3], b: [f64; 3], av: f64, af: f64, ab: f64, a: [f64; 5]) -> Spec {
    Spec {
        f,
        b,
        f_end: None,
        av,
        af,
        ab,
        a,
    }
}

impl Ph {
    pub const ALL: [Ph; 41] = [
        Ph::IY,
        Ph::IH,
        Ph::EY,
        Ph::EH,
        Ph::AE,
        Ph::AA,
        Ph::AO,
        Ph::OW,
        Ph::UH,
        Ph::UW,
        Ph::AH,
        Ph::AX,
        Ph::IX,
        Ph::ER,
        Ph::AY,
        Ph::AW,
        Ph::OY,
        Ph::P,
        Ph::B,
        Ph::T,
        Ph::D,
        Ph::K,
        Ph::G,
        Ph::CH,
        Ph::JH,
        Ph::F,
        Ph::V,
        Ph::TH,
        Ph::DH,
        Ph::S,
        Ph::Z,
        Ph::SH,
        Ph::ZH,
        Ph::HH,
        Ph::M,
        Ph::N,
        Ph::NG,
        Ph::L,
        Ph::R,
        Ph::W,
        Ph::Y,
    ];

    pub fn parse(s: &str) -> Option<Ph> {
        Ph::ALL.iter().copied().find(|p| p.name() == s)
    }

    pub fn name(self) -> &'static str {
        use Ph::*;
        match self {
            IY => "IY",
            IH => "IH",
            EY => "EY",
            EH => "EH",
            AE => "AE",
            AA => "AA",
            AO => "AO",
            OW => "OW",
            UH => "UH",
            UW => "UW",
            AH => "AH",
            AX => "AX",
            IX => "IX",
            ER => "ER",
            AY => "AY",
            AW => "AW",
            OY => "OY",
            P => "P",
            B => "B",
            T => "T",
            D => "D",
            K => "K",
            G => "G",
            CH => "CH",
            JH => "JH",
            F => "F",
            V => "V",
            TH => "TH",
            DH => "DH",
            S => "S",
            Z => "Z",
            SH => "SH",
            ZH => "ZH",
            HH => "HH",
            M => "M",
            N => "N",
            NG => "NG",
            L => "L",
            R => "R",
            W => "W",
            Y => "Y",
        }
    }

    pub fn class(self) -> Class {
        use Ph::*;
        match self {
            IY | IH | EY | EH | AE | AA | AO | OW | UH | UW | AH | AX | IX | ER | AY | AW | OY => {
                Class::Vowel
            }
            P | B | T | D | K | G => Class::Stop,
            CH | JH => Class::Affricate,
            F | V | TH | DH | S | Z | SH | ZH => Class::Fricative,
            HH => Class::Aspirate,
            M | N | NG => Class::Nasal,
            L | R => Class::Liquid,
            W | Y => Class::Glide,
        }
    }

    pub fn place(self) -> Place {
        use Ph::*;
        match self {
            P | B | F | V | M | W => Place::Labial,
            TH | DH => Place::Dental,
            T | D | S | Z | N | L => Place::Alveolar,
            CH | JH | SH | ZH | R | Y => Place::PostAlveolar,
            K | G | NG => Place::Velar,
            _ => Place::Other,
        }
    }

    pub fn is_vowel(self) -> bool {
        self.class() == Class::Vowel
    }

    pub fn is_sonorant(self) -> bool {
        matches!(
            self.class(),
            Class::Vowel | Class::Nasal | Class::Liquid | Class::Glide
        )
    }

    pub fn is_voiced(self) -> bool {
        use Ph::*;
        !matches!(self, P | T | K | CH | F | TH | S | SH | HH)
    }

    pub fn is_plosive(self) -> bool {
        self.class() == Class::Stop
    }

    /// Klatt (1979) inherent and minimum durations in ms.
    pub fn durations(self) -> (f64, f64) {
        use Ph::*;
        match self {
            AA => (240.0, 100.0),
            AE => (230.0, 80.0),
            AH => (140.0, 60.0),
            AO => (240.0, 100.0),
            AW => (260.0, 100.0),
            AX => (120.0, 60.0),
            AY => (250.0, 150.0),
            EH => (150.0, 70.0),
            ER => (180.0, 80.0),
            EY => (190.0, 70.0),
            IH => (135.0, 40.0),
            IX => (110.0, 40.0),
            IY => (155.0, 55.0),
            OW => (220.0, 80.0),
            OY => (280.0, 150.0),
            UH => (160.0, 60.0),
            UW => (210.0, 70.0),
            B => (85.0, 60.0),
            CH => (70.0, 50.0),
            D => (75.0, 50.0),
            DH => (50.0, 30.0),
            F => (100.0, 80.0),
            G => (80.0, 60.0),
            HH => (80.0, 20.0),
            JH => (70.0, 50.0),
            K => (80.0, 60.0),
            L => (80.0, 40.0),
            M => (70.0, 60.0),
            N => (60.0, 50.0),
            NG => (95.0, 60.0),
            P => (90.0, 50.0),
            R => (80.0, 30.0),
            S => (105.0, 60.0),
            SH => (105.0, 80.0),
            T => (75.0, 50.0),
            TH => (90.0, 60.0),
            V => (60.0, 40.0),
            W => (80.0, 60.0),
            Y => (80.0, 40.0),
            Z => (75.0, 40.0),
            ZH => (70.0, 40.0),
        }
    }

    pub fn spec(self) -> Spec {
        use Ph::*;
        match self {
            // Every vowel has the same AV, so the cascade gives open vowels
            // their greater intrinsic intensity, as in natural speech
            // (Lehiste & Peterson 1959). Formants are Klatt's (1980)
            // citation-form targets, with EY, OW and UW as diphthongs.
            // Citation targets keep the vowel space wide: formants measured in
            // running speech are already undershot, and the transitions
            // undershoot them again, which merges vowel pairs such as EH and IH.
            IY => vowel([310.0, 2020.0, 2960.0]),
            IH => vowel([400.0, 1800.0, 2570.0]),
            EY => diphthong([480.0, 1720.0, 2520.0], [330.0, 2020.0, 2600.0]),
            EH => vowel([530.0, 1680.0, 2500.0]),
            AE => vowel([620.0, 1660.0, 2430.0]),
            AA => vowel([700.0, 1220.0, 2600.0]),
            AO => vowel([600.0, 990.0, 2570.0]),
            OW => diphthong([540.0, 1100.0, 2300.0], [450.0, 900.0, 2300.0]),
            UH => vowel([450.0, 1100.0, 2350.0]),
            UW => diphthong([350.0, 1250.0, 2200.0], [320.0, 900.0, 2200.0]),
            AH => vowel([620.0, 1220.0, 2550.0]),
            // Reduced vowels: Klatt's schwa [ə] (AX), and the higher, more
            // central [ɨ] (IX) that Flemming & Johnson (2007) find between
            // consonants.
            AX => vowel([500.0, 1400.0, 2300.0]),
            IX => vowel([420.0, 1650.0, 2400.0]),
            ER => vowel([470.0, 1270.0, 1540.0]),
            AY => diphthong([660.0, 1200.0, 2550.0], [400.0, 1880.0, 2500.0]),
            AW => diphthong([640.0, 1230.0, 2550.0], [420.0, 940.0, 2350.0]),
            OY => diphthong([550.0, 960.0, 2400.0], [360.0, 1820.0, 2450.0]),

            W => sp([290.0, 610.0, 2150.0], [50.0, 80.0, 60.0], 57.0),
            Y => sp([260.0, 2070.0, 3020.0], [40.0, 250.0, 500.0], 57.0),
            // R's low F3 carries American r-colouring.
            R => sp([310.0, 1060.0, 1380.0], [70.0, 100.0, 120.0], 57.0),
            L => sp([310.0, 1050.0, 2880.0], [50.0, 100.0, 280.0], 57.0),
            M => sp([480.0, 1270.0, 2130.0], [40.0, 200.0, 200.0], 57.0),
            N => sp([480.0, 1340.0, 2470.0], [40.0, 300.0, 300.0], 57.0),
            NG => sp([480.0, 2000.0, 2900.0], [160.0, 150.0, 400.0], 57.0),

            F => fr(
                [340.0, 1100.0, 2080.0],
                [200.0, 120.0, 150.0],
                0.0,
                60.0,
                46.0,
                [0.0; 5],
            ),
            V => fr(
                [220.0, 1100.0, 2080.0],
                [60.0, 90.0, 120.0],
                47.0,
                55.0,
                40.0,
                [0.0; 5],
            ),
            TH => fr(
                [320.0, 1290.0, 2540.0],
                [200.0, 90.0, 200.0],
                0.0,
                60.0,
                48.0,
                [0.0, 0.0, 0.0, 0.0, 28.0],
            ),
            DH => fr(
                [270.0, 1290.0, 2540.0],
                [60.0, 80.0, 170.0],
                47.0,
                55.0,
                38.0,
                [0.0, 0.0, 0.0, 0.0, 28.0],
            ),
            S => fr(
                [320.0, 1390.0, 2530.0],
                [200.0, 80.0, 200.0],
                0.0,
                60.0,
                0.0,
                [0.0, 0.0, 0.0, 52.0, 60.0],
            ),
            Z => fr(
                [240.0, 1390.0, 2530.0],
                [70.0, 60.0, 180.0],
                47.0,
                55.0,
                0.0,
                [0.0, 0.0, 0.0, 47.0, 55.0],
            ),
            SH => fr(
                [300.0, 1840.0, 2750.0],
                [200.0, 100.0, 300.0],
                0.0,
                60.0,
                0.0,
                [0.0, 63.0, 55.0, 53.0, 50.0],
            ),
            ZH => fr(
                [300.0, 1840.0, 2750.0],
                [60.0, 100.0, 300.0],
                47.0,
                55.0,
                0.0,
                [0.0, 58.0, 50.0, 48.0, 45.0],
            ),
            HH => sp([500.0, 1500.0, 2500.0], [300.0, 150.0, 200.0], 0.0),

            // A listener heard P's release pop at the AF of 60 dB that Klatt
            // (1980) gives voiceless obstruents, so it is 6 dB weaker.
            P => fr(
                [400.0, 1100.0, 2150.0],
                [300.0, 150.0, 220.0],
                0.0,
                54.0,
                55.0,
                [0.0; 5],
            ),
            B => fr(
                [200.0, 1100.0, 2150.0],
                [60.0, 110.0, 130.0],
                0.0,
                57.0,
                50.0,
                [0.0; 5],
            ),
            T => fr(
                [400.0, 1600.0, 2600.0],
                [300.0, 120.0, 250.0],
                0.0,
                60.0,
                0.0,
                [0.0, 25.0, 40.0, 47.0, 55.0],
            ),
            D => fr(
                [200.0, 1600.0, 2600.0],
                [60.0, 100.0, 170.0],
                0.0,
                57.0,
                0.0,
                [0.0, 20.0, 35.0, 42.0, 50.0],
            ),
            K => fr(
                [300.0, 1990.0, 2850.0],
                [250.0, 160.0, 330.0],
                0.0,
                60.0,
                0.0,
                [47.0, 44.0, 35.0, 35.0, 0.0],
            ),
            G => fr(
                [200.0, 1990.0, 2850.0],
                [60.0, 150.0, 280.0],
                0.0,
                57.0,
                0.0,
                [42.0, 39.0, 30.0, 30.0, 0.0],
            ),
            CH => fr(
                [350.0, 1800.0, 2820.0],
                [200.0, 90.0, 300.0],
                0.0,
                60.0,
                0.0,
                [0.0, 63.0, 55.0, 53.0, 50.0],
            ),
            JH => fr(
                [260.0, 1800.0, 2820.0],
                [60.0, 80.0, 270.0],
                47.0,
                55.0,
                0.0,
                [0.0, 58.0, 50.0, 48.0, 45.0],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for p in Ph::ALL {
            assert_eq!(Ph::parse(p.name()), Some(p));
        }
    }

    #[test]
    fn minimum_never_exceeds_inherent() {
        for p in Ph::ALL {
            let (inh, min) = p.durations();
            assert!(min <= inh, "{p:?}");
        }
    }
}
