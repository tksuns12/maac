//! A pitch (§3.2, §7): a spelled pitch, twelve-tone key index, positive
//! frequency, ratio against a frequency, or degree in a tuning.
//!
//! Notes, chords, voicings and pitch overrides all write pitches in the same
//! five forms. Semantic analysis reads and checks each pitch once; the
//! compiler lowers these models (resolving `degree` references against its
//! tunings) and editing reads them for chord members.

use std::collections::HashMap;
use std::sync::Arc;

use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};

use super::{is_integer, Validator};
use crate::diagnostic::{DiagnosticCode, Span};
use crate::exact::MAX_RATIONAL_BITS;
use crate::music::{Pitch as MusicPitch, Spelling, Tuning};
use crate::syntax::{Field, Object, Unit, Value, ValueKind};

/// A pitch as semantic analysis reads it from source, before tuning lookup
/// and receiver-specific resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pitch {
    /// A spelled pitch (`C4`, `F#3`, `Bb2`).
    Spelled(Spelling),
    /// A twelve-tone key index (`key(60)`).
    Key(i64),
    /// A positive frequency in Hz (`440Hz`; `kHz` is scaled to Hz).
    Frequency(BigRational),
    /// A positive ratio times a positive base frequency in Hz
    /// (`ratio(3/2, 440Hz)`).
    Ratio {
        ratio: BigRational,
        frequency_hz: BigRational,
    },
    /// A step index into a named tuning (`degree(3, &just)`, or an imported
    /// `&module.just` in an unlinked authored tree).
    Degree { index: i64, tuning: Vec<String> },
}

impl Pitch {
    /// Lower this semantic pitch to a [`MusicPitch`], resolving any `degree`
    /// tuning reference through `tunings`. Returns `None` if a `degree`
    /// tuning reference does not name a tuning in `tunings`.
    pub(crate) fn resolve(&self, tunings: &HashMap<String, Arc<Tuning>>) -> Option<MusicPitch> {
        Some(match self {
            Pitch::Spelled(spelling) => MusicPitch::Spelled(spelling.clone()),
            Pitch::Key(key) => MusicPitch::Key(*key),
            Pitch::Frequency(hz) => MusicPitch::Frequency(hz.clone()),
            Pitch::Ratio {
                ratio,
                frequency_hz,
            } => MusicPitch::Ratio {
                ratio: ratio.clone(),
                frequency_hz: frequency_hz.clone(),
            },
            Pitch::Degree { index, tuning } => {
                let [id] = tuning.as_slice() else {
                    return None;
                };
                MusicPitch::degree(*index, tunings.get(id)?.clone())
            }
        })
    }
}

fn rational_within_bits(value: &BigRational) -> bool {
    value.numer().magnitude().bits() <= MAX_RATIONAL_BITS
        && value.denom().magnitude().bits() <= MAX_RATIONAL_BITS
}

impl Validator<'_> {
    /// Check the `pitch` of a `note` object and keep its model under `path`.
    pub(super) fn validate_note_pitch(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("pitch") {
            if let Some(pitch) = self.read_pitch(&field.value, path, "pitch") {
                self.models.pitches.insert(path.to_vec(), pitch);
            }
        }
    }

    /// Check the `pitch` replacement of an `override`'s `set` record and keep
    /// its model under the override's `object_path`. Diagnostics report at
    /// `set_path` + `"pitch"`.
    pub(super) fn validate_override_pitch(
        &mut self,
        value: &Value,
        object_path: &[String],
        set_path: &[String],
    ) {
        if let Some(pitch) = self.read_pitch(value, set_path, "pitch") {
            self.models.pitches.insert(object_path.to_vec(), pitch);
        }
    }

    /// Read a nonempty list of pitches, as a chord or voicing `pitches`.
    /// Returns the parsed pitches when every entry reads, and the list length
    /// whenever the field is a nonempty list (so a chord's `top`, `bottom`,
    /// `pick` and per-member lists still check against the count even when one
    /// entry fails).
    pub(super) fn read_pitch_list(
        &mut self,
        field: &Field,
        path: &[String],
    ) -> (Option<Vec<Pitch>>, Option<usize>) {
        match &field.value.kind {
            ValueKind::List(items) if !items.is_empty() => {
                let mut pitches = Some(Vec::with_capacity(items.len()));
                for item in items {
                    match self.read_pitch(item, path, "pitches") {
                        Some(pitch) => {
                            if let Some(list) = &mut pitches {
                                list.push(pitch);
                            }
                        }
                        None => pitches = None,
                    }
                }
                (pitches, Some(items.len()))
            }
            _ => {
                self.push(
                    DiagnosticCode::Range,
                    "pitches must be a nonempty list of pitches",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["pitches".into()],
                );
                (None, None)
            }
        }
    }

    /// Check and read one pitch value at field `name` of `path`.
    pub(super) fn read_pitch(
        &mut self,
        value: &Value,
        path: &[String],
        name: &str,
    ) -> Option<Pitch> {
        match &value.kind {
            ValueKind::Symbol(symbol) => match MusicPitch::parse_spelled(symbol) {
                Ok(MusicPitch::Spelled(spelling)) => Some(Pitch::Spelled(spelling)),
                _ => {
                    self.push(
                        DiagnosticCode::Range,
                        "pitch symbol must be a valid spelled pitch",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    None
                }
            },
            ValueKind::Quantity {
                unit: unit @ (Unit::Hz | Unit::KHz),
                value: frequency,
            } => {
                if frequency <= &BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "pitch frequency must be positive",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                let hz = self.scale_pitch_hz(*unit, frequency, value.span, path, name)?;
                Some(Pitch::Frequency(hz))
            }
            ValueKind::Call { function, args } if function == "key" => {
                // §3.2: an integer twelve-tone key index.
                match args.as_slice() {
                    [Value {
                        kind: ValueKind::Number(key),
                        span,
                    }] if is_integer(key) => {
                        let key = self.pitch_i64(key, *span, path, name)?;
                        Some(Pitch::Key(key))
                    }
                    _ => {
                        self.push(
                            DiagnosticCode::Range,
                            "key() requires one integer argument",
                            Some(value.span),
                            path.to_vec(),
                            vec![name.into()],
                        );
                        None
                    }
                }
            }
            ValueKind::Call { function, args } if function == "ratio" => {
                if args.len() != 2
                    || !matches!(args[0].kind, ValueKind::Number(_))
                    || !matches!(
                        args[1].kind,
                        ValueKind::Quantity {
                            unit: Unit::Hz | Unit::KHz,
                            ..
                        }
                    )
                {
                    self.push(
                        DiagnosticCode::Unit,
                        "ratio() requires a dimensionless ratio and Hz base",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                let [Value {
                    kind: ValueKind::Number(ratio),
                    span: ratio_span,
                }, Value {
                    kind:
                        ValueKind::Quantity {
                            unit: base_unit,
                            value: base,
                        },
                    span: base_span,
                }] = args.as_slice()
                else {
                    return None;
                };
                // §3.2: a positive ratio times a positive frequency.
                if ratio <= &BigRational::zero() || base <= &BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "ratio() requires a positive ratio and positive frequency",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                if !rational_within_bits(ratio) {
                    self.push(
                        DiagnosticCode::ResourceLimit,
                        format!("rational exceeds {MAX_RATIONAL_BITS}-bit bound"),
                        Some(*ratio_span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                let frequency_hz = self.scale_pitch_hz(*base_unit, base, *base_span, path, name)?;
                if let Err(error) = MusicPitch::ratio(ratio.clone(), frequency_hz.clone()) {
                    self.push(
                        DiagnosticCode::ResourceLimit,
                        error.message,
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                Some(Pitch::Ratio {
                    ratio: ratio.clone(),
                    frequency_hz,
                })
            }
            ValueKind::Call { function, args } if function == "degree" => {
                if args.len() != 2
                    || !matches!(args[0].kind, ValueKind::Number(_))
                    || args[1].reference().is_none()
                {
                    self.push(
                        DiagnosticCode::Reference,
                        "degree() requires an integer index and tuning reference",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    return None;
                }
                let index = match &args[0].kind {
                    ValueKind::Number(index) if !index.denom().is_one() => {
                        self.push(
                            DiagnosticCode::Range,
                            "degree() index must be an integer",
                            Some(args[0].span),
                            path.to_vec(),
                            vec![name.into()],
                        );
                        None
                    }
                    ValueKind::Number(index) => self.pitch_i64(index, args[0].span, path, name),
                    _ => None,
                };
                let reference = args[1].reference().expect("checked above");
                let tuning_resolves = reference.port.is_none()
                    && reference.path.len() == 1
                    && self
                        .document
                        .object(&reference.path[0])
                        .is_some_and(|object| object.kind == "tuning");
                // Editing walks single-file authored trees whose imported
                // tunings (such as `&elsewhere.just`) are not linked yet;
                // keep the model while still reporting the unresolved ref to
                // full validation.
                let imported = reference.port.is_none() && reference.path.len() > 1;
                if !tuning_resolves {
                    self.push(
                        DiagnosticCode::Reference,
                        "degree() tuning reference must resolve to a tuning",
                        Some(args[1].span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                    if imported {
                        self.outside_models += 1;
                    }
                }
                index
                    .filter(|_| tuning_resolves || imported)
                    .map(|index| Pitch::Degree {
                        index,
                        tuning: reference.path.clone(),
                    })
            }
            _ => {
                self.push(
                    DiagnosticCode::Unit,
                    "pitch must be a spelled pitch, key(), degree(), ratio(), or positive Hz quantity",
                    Some(value.span),
                    path.to_vec(),
                    vec![name.into()],
                );
                None
            }
        }
    }

    fn scale_pitch_hz(
        &mut self,
        unit: Unit,
        value: &BigRational,
        span: Span,
        path: &[String],
        name: &str,
    ) -> Option<BigRational> {
        if !rational_within_bits(value) {
            self.push(
                DiagnosticCode::ResourceLimit,
                format!("rational exceeds {MAX_RATIONAL_BITS}-bit bound"),
                Some(span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        if unit == Unit::KHz {
            let thousand = BigRational::from_integer(1000.into());
            if value.numer().magnitude().bits() + thousand.numer().magnitude().bits()
                > MAX_RATIONAL_BITS
            {
                self.push(
                    DiagnosticCode::ResourceLimit,
                    "rational multiplication intermediate exceeds the bit bound",
                    Some(span),
                    path.to_vec(),
                    vec![name.into()],
                );
                return None;
            }
            let scaled = value * thousand;
            if !rational_within_bits(&scaled) {
                self.push(
                    DiagnosticCode::ResourceLimit,
                    format!("rational exceeds {MAX_RATIONAL_BITS}-bit bound"),
                    Some(span),
                    path.to_vec(),
                    vec![name.into()],
                );
                return None;
            }
            Some(scaled)
        } else {
            Some(value.clone())
        }
    }

    fn pitch_i64(
        &mut self,
        value: &BigRational,
        span: Span,
        path: &[String],
        name: &str,
    ) -> Option<i64> {
        if !rational_within_bits(value) {
            self.push(
                DiagnosticCode::ResourceLimit,
                format!("rational exceeds {MAX_RATIONAL_BITS}-bit bound"),
                Some(span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        match value.to_integer().to_i64() {
            Some(integer) => Some(integer),
            None => {
                self.push(
                    DiagnosticCode::ResourceLimit,
                    "integer exceeds the signed 64-bit bound",
                    Some(span),
                    path.to_vec(),
                    vec![name.into()],
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    fn note_pitch(expr: &str) -> Option<Pitch> {
        let source = format!(
            "maac 1; \
             tuning just {{ period = 1200ct; steps = [0ct, 702ct]; reference_index = 0; reference_frequency = 440Hz; }} \
             pattern bar {{ length = 4q; note n {{ at = 0q; dur = 1q; pitch = {expr}; }} }}"
        );
        let document = crate::parse(&source).unwrap();
        models_of(&document)
            .pitches
            .remove(["bar".to_owned(), "n".to_owned()].as_slice())
    }

    #[test]
    fn reads_all_five_pitch_forms() {
        assert_eq!(
            note_pitch("F#4"),
            Some(Pitch::Spelled(Spelling::new('F', 1, 4).unwrap()))
        );
        assert_eq!(note_pitch("key(60)"), Some(Pitch::Key(60)));
        assert_eq!(note_pitch("key(-3)"), Some(Pitch::Key(-3)));
        assert_eq!(
            note_pitch("440Hz"),
            Some(Pitch::Frequency(BigRational::from_integer(440.into())))
        );
        assert_eq!(
            note_pitch("1kHz"),
            Some(Pitch::Frequency(BigRational::from_integer(1000.into())))
        );
        assert_eq!(
            note_pitch("ratio(3/2, 1kHz)"),
            Some(Pitch::Ratio {
                ratio: BigRational::new(3.into(), 2.into()),
                frequency_hz: BigRational::from_integer(1000.into()),
            })
        );
        assert_eq!(
            note_pitch("degree(3, &just)"),
            Some(Pitch::Degree {
                index: 3,
                tuning: vec!["just".into()],
            })
        );
    }

    #[test]
    fn reads_imported_degree_tuning_for_editing_models() {
        assert_eq!(
            note_pitch("degree(-1, &elsewhere.just)"),
            Some(Pitch::Degree {
                index: -1,
                tuning: vec!["elsewhere".into(), "just".into()],
            })
        );
    }

    #[test]
    fn reads_insert_and_override_pitches() {
        let document = crate::parse(
            r#"maac 1;
            pattern p { length = 4q; note n { at = 0q; dur = 1q; pitch = C4; } }
            place main { pattern = &p; track = &t; at = 0q;
                override edit { event = "0/n"; set = { pitch = G4; }; }
                insert add { note extra { at = 1q; dur = 1q; pitch = key(67); } }
            }"#,
        )
        .unwrap();
        let models = models_of(&document);
        assert_eq!(
            models
                .pitches
                .get(["p".to_owned(), "n".to_owned()].as_slice()),
            Some(&Pitch::Spelled(Spelling::new('C', 0, 4).unwrap()))
        );
        assert_eq!(
            models
                .pitches
                .get(["main".to_owned(), "edit".to_owned()].as_slice()),
            Some(&Pitch::Spelled(Spelling::new('G', 0, 4).unwrap()))
        );
        assert_eq!(
            models
                .pitches
                .get(["main".to_owned(), "add".to_owned(), "extra".to_owned()].as_slice()),
            Some(&Pitch::Key(67))
        );
    }

    #[test]
    fn rejects_invalid_and_out_of_bound_pitches() {
        for bad in [
            "c4",
            "\"C4\"",
            "0Hz",
            "-440Hz",
            "1q",
            "key(1/2)",
            "key(999999999999999999999999999999999)",
            "ratio(0, 440Hz)",
            "ratio(3/2, 0Hz)",
            "ratio(3/2, 440)",
            "degree(1/2, &just)",
            "degree(999999999999999999999999999999999, &just)",
            "degree(1, &nowhere)",
        ] {
            assert_eq!(note_pitch(bad), None, "{bad}");
        }
    }

    #[test]
    fn resolves_semantic_pitch_against_tunings() {
        let tuning = Arc::new(
            Tuning::new(
                BigRational::from_integer(1200.into()),
                vec![BigRational::zero(), BigRational::from_integer(702.into())],
                0,
                BigRational::from_integer(440.into()),
            )
            .unwrap(),
        );
        let mut tunings = HashMap::new();
        tunings.insert("just".to_owned(), tuning.clone());
        let degree = Pitch::Degree {
            index: 1,
            tuning: vec!["just".into()],
        };
        assert_eq!(
            degree.resolve(&tunings),
            Some(MusicPitch::degree(1, tuning))
        );
        let imported = Pitch::Degree {
            index: 1,
            tuning: vec!["elsewhere".into(), "just".into()],
        };
        assert_eq!(imported.resolve(&tunings), None);
    }
}
