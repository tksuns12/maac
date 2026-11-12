//! The pitches a chord plays (§8.2): where they come from, and which of
//! them the chord keeps.
//!
//! A chord names its pitches with `pitches` or with `voicing`, or leaves
//! them to the nearest use or placement that supplies a voicing. `top`,
//! `bottom` or `pick` then keeps some of them, in list order.

use num_traits::ToPrimitive;

use super::{ReferenceTarget, Validator};
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Field, Object, Value, ValueKind};

/// Where a chord's pitches come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The chord's own `pitches`.
    Own,
    /// The voicing with this ID, which the chord names.
    Voicing(String),
    /// The voicing that the nearest use or placement supplies.
    Supplied,
}

/// Which of its pitches a chord keeps. Counts and positions are at least 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Every one.
    All,
    /// The last so many, by `top`.
    Top(usize),
    /// The first so many, by `bottom`.
    Bottom(usize),
    /// Those at these positions, by `pick`: counted from 1 for the first
    /// pitch, and strictly increasing.
    Pick(Vec<usize>),
}

impl Keep {
    /// The pitches kept of `pitches`, in list order, unless this reaches
    /// past them.
    pub(crate) fn of<'p, T>(&self, pitches: &'p [T]) -> Result<Vec<&'p T>, Misfit> {
        let past = |field| Misfit::Keep {
            field,
            pitches: pitches.len(),
        };
        match self {
            Keep::All => Ok(pitches.iter().collect()),
            Keep::Top(count) => {
                let first = pitches.len().checked_sub(*count).ok_or(past("top"))?;
                Ok(pitches[first..].iter().collect())
            }
            Keep::Bottom(count) => Ok(pitches
                .get(..*count)
                .ok_or(past("bottom"))?
                .iter()
                .collect()),
            Keep::Pick(positions) => positions
                .iter()
                .map(|position| pitches.get(position - 1).ok_or(past("pick")))
                .collect(),
        }
    }
}

/// The chord fields that hold one value for every member, or a list with
/// one entry per played pitch.
pub(crate) const PER_MEMBER: [&str; 4] = [
    "velocity",
    "release_velocity",
    "onset_offset",
    "release_offset",
];

/// The pitches a chord plays, as its fields declare them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Played {
    pub(crate) source: Source,
    pub(crate) keep: Keep,
    /// Each per-member field the chord writes as a list, with the length
    /// of the list.
    pub(crate) lists: Vec<(&'static str, usize)>,
}

impl Played {
    /// The pitches the chord plays of `pitches`, those of its source, in
    /// list order, unless the chord does not fit them.
    pub(crate) fn of<'p, T>(&self, pitches: &'p [T]) -> Result<Vec<&'p T>, Misfit> {
        let played = self.keep.of(pitches)?;
        match self
            .lists
            .iter()
            .find(|(_, length)| *length != played.len())
        {
            Some((field, _)) => Err(Misfit::List {
                field,
                played: played.len(),
            }),
            None => Ok(played),
        }
    }
}

/// How a chord does not fit the pitches of its source. A chord with its
/// own pitches or a named voicing is checked against them as it is read; a
/// supplied voicing can differ at each occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Misfit {
    /// `field`, a `top`, `bottom` or `pick`, reaches past the `pitches`
    /// there are.
    Keep { field: &'static str, pitches: usize },
    /// The list in per-member `field` has another length than the `played`
    /// pitches.
    List { field: &'static str, played: usize },
}

impl Misfit {
    /// The chord field that does not fit.
    pub(crate) fn field(&self) -> &'static str {
        match self {
            Misfit::Keep { field, .. } | Misfit::List { field, .. } => field,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Misfit::Keep { field, pitches } => {
                let rule = match *field {
                    "pick" => "pick positions must be integers",
                    "top" => "top must be an integer",
                    _ => "bottom must be an integer",
                };
                format!("chord.{rule} from 1 through {pitches}, the number of pitches")
            }
            Misfit::List { field, played } => {
                format!("chord.{field} must have one entry per pitch played ({played})")
            }
        }
    }
}

impl Validator<'_> {
    /// Check the `pitches`, `voicing`, `top`, `bottom` and `pick` of chord
    /// `object` and keep what it plays under its path. Returns the number of
    /// members, when the fields alone give it.
    pub(super) fn validate_played(&mut self, object: &Object, path: &[String]) -> Option<usize> {
        let reported = self.reported();
        // The source, and how many pitches it has when that is known here.
        let (source, pitches) = match (object.field("pitches"), object.field("voicing")) {
            (Some(field), voicing) => {
                if let Some(voicing) = voicing {
                    self.push(
                        DiagnosticCode::Range,
                        "a chord names its pitches with `pitches` or `voicing`, not both",
                        Some(voicing.value.span),
                        path.to_vec(),
                        vec!["voicing".into()],
                    );
                }
                (Some(Source::Own), self.validate_pitch_list(field, path))
            }
            (None, Some(field)) => {
                match self.expect_object_ref(field, "voicing", &["voicing"], path) {
                    Some(ReferenceTarget::Object { id, .. }) => {
                        let pitches = self
                            .document
                            .object(&id)
                            .and_then(|voicing| voicing.field("pitches"))
                            .and_then(|pitches| match &pitches.value.kind {
                                ValueKind::List(items) if !items.is_empty() => Some(items.len()),
                                _ => None,
                            });
                        (Some(Source::Voicing(id)), pitches)
                    }
                    _ => (None, None),
                }
            }
            (None, None) => (Some(Source::Supplied), None),
        };
        let (keep, members) = match self.read_keep(object, path, pitches) {
            Some((keep, members)) => (Some(keep), members),
            None => (None, None),
        };
        if let (true, Some(source), Some(keep)) = (self.reported() == reported, source, keep) {
            let lists = PER_MEMBER
                .into_iter()
                .filter_map(|name| match &object.field(name)?.value.kind {
                    ValueKind::List(items) => Some((name, items.len())),
                    _ => None,
                })
                .collect();
            self.models.played.insert(
                path.to_vec(),
                Played {
                    source,
                    keep,
                    lists,
                },
            );
        }
        members
    }

    /// A chord's `top`, `bottom` or `pick`, each from 1 through the
    /// `pitches` it keeps some of when their number is known. Returns it
    /// with the number of members when that is known.
    fn read_keep(
        &mut self,
        object: &Object,
        path: &[String],
        pitches: Option<usize>,
    ) -> Option<(Keep, Option<usize>)> {
        let written: Vec<&Field> = ["top", "bottom", "pick"]
            .into_iter()
            .filter_map(|name| object.field(name))
            .collect();
        if let [_, field, ..] = written.as_slice() {
            self.push(
                DiagnosticCode::Range,
                "a chord takes at most one of `top`, `bottom` and `pick`",
                Some(field.value.span),
                path.to_vec(),
                vec![field.name.clone()],
            );
            return None;
        }
        let Some(field) = written.first().copied() else {
            return Some((Keep::All, pitches));
        };
        if field.name == "pick" {
            let positions = self.read_pick(field, path, pitches)?;
            let members = positions.len();
            return Some((Keep::Pick(positions), Some(members)));
        }
        let name = field.name.as_str();
        let count = self.integer(field, path, name)?;
        let count = count.to_integer().to_usize().filter(|count| *count >= 1);
        match (count, pitches) {
            (Some(count), Some(pitches)) if count <= pitches => {
                Some((Self::keep_ends(name, count), Some(count)))
            }
            (Some(count), None) => Some((Self::keep_ends(name, count), None)),
            _ => {
                self.push(
                    DiagnosticCode::Range,
                    match pitches {
                        Some(pitches) => format!(
                            "chord.{name} must be from 1 through {pitches}, the number of pitches"
                        ),
                        None => format!("chord.{name} must be positive"),
                    },
                    Some(field.value.span),
                    path.to_vec(),
                    vec![name.to_owned()],
                );
                None
            }
        }
    }

    fn keep_ends(name: &str, count: usize) -> Keep {
        if name == "top" {
            Keep::Top(count)
        } else {
            Keep::Bottom(count)
        }
    }

    /// A chord's `pick`: one position or a strictly increasing list, each
    /// from 1 through the `pitches` when their number is known. Their count
    /// is the number of members under any voicing.
    fn read_pick(
        &mut self,
        field: &Field,
        path: &[String],
        pitches: Option<usize>,
    ) -> Option<Vec<usize>> {
        let values: Vec<&Value> = match &field.value.kind {
            ValueKind::List(items) => items.iter().collect(),
            _ => vec![&field.value],
        };
        let mut problem = values.is_empty().then(|| {
            (
                "chord.pick must name at least one position".to_owned(),
                field.value.span,
            )
        });
        let mut positions = Vec::with_capacity(values.len());
        let mut previous = 0;
        for value in &values {
            let position = self
                .integer_at(value, path, &["pick".into()], "chord.pick")?
                .to_integer()
                .to_usize()
                .filter(|position| *position >= 1 && pitches.is_none_or(|n| *position <= n));
            let Some(position) = position else {
                problem.get_or_insert_with(|| {
                    let message = match pitches {
                        Some(n) => format!(
                            "chord.pick positions must be from 1 through {n}, the number of pitches"
                        ),
                        None => "chord.pick positions must be positive".to_owned(),
                    };
                    (message, value.span)
                });
                continue;
            };
            if position <= previous {
                problem.get_or_insert_with(|| {
                    (
                        "chord.pick positions must strictly increase".to_owned(),
                        value.span,
                    )
                });
            }
            previous = position;
            positions.push(position);
        }
        if let Some((message, span)) = problem {
            self.push(
                DiagnosticCode::Range,
                message,
                Some(span),
                path.to_vec(),
                vec!["pick".into()],
            );
            return None;
        }
        Some(positions)
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// What `chord c { ...fields }` plays, if that reads.
    fn played(fields: &str) -> Option<Played> {
        let document = crate::parse(&format!(
            "maac 1; voicing g {{ pitches = [G2, B2, D3, G3]; }} \
             pattern bar {{ length = 4q; chord c {{ at = 0q; dur = 1q; {fields} }} }}"
        ))
        .unwrap();
        models_of(&document)
            .played
            .remove(["bar".to_owned(), "c".to_owned()].as_slice())
    }

    #[test]
    fn a_chord_reads_where_its_pitches_come_from() {
        for (fields, source) in [
            ("pitches = [C4, E4];", Source::Own),
            ("voicing = &g;", Source::Voicing("g".into())),
            ("", Source::Supplied),
        ] {
            assert_eq!(
                played(fields),
                Some(Played {
                    source,
                    keep: Keep::All,
                    lists: Vec::new(),
                }),
                "{fields}"
            );
        }
    }

    /// Editing counts the members of a chord in a tree whose imported
    /// tunings are not resolved.
    #[test]
    fn a_wrong_pitch_does_not_hide_what_a_chord_plays() {
        for fields in [
            "pitches = [C4, 1q]; top = 1;",
            "pitches = [degree(1, &elsewhere.just), C4]; top = 1;",
        ] {
            assert_eq!(
                played(fields),
                Some(Played {
                    source: Source::Own,
                    keep: Keep::Top(1),
                    lists: Vec::new(),
                }),
                "{fields}"
            );
        }
    }

    #[test]
    fn a_chord_reads_which_pitches_it_keeps() {
        for (fields, keep) in [
            ("top = 3;", Keep::Top(3)),
            ("bottom = 1;", Keep::Bottom(1)),
            ("pick = 2;", Keep::Pick(vec![2])),
            ("pick = [1, 3, 4];", Keep::Pick(vec![1, 3, 4])),
        ] {
            for (source_fields, source) in [
                ("voicing = &g; ", Source::Voicing("g".into())),
                // A supplied voicing may have any number of pitches.
                ("", Source::Supplied),
            ] {
                assert_eq!(
                    played(&format!("{source_fields}{fields}")),
                    Some(Played {
                        source,
                        keep: keep.clone(),
                        lists: Vec::new(),
                    }),
                    "{source_fields}{fields}"
                );
            }
        }
        assert_eq!(
            played("top = 9;").map(|played| played.keep),
            Some(Keep::Top(9))
        );
    }

    #[test]
    fn a_chord_with_a_wrong_field_plays_nothing() {
        for fields in [
            "pitches = [C4]; voicing = &g;",
            "pitches = [];",
            "voicing = &nowhere;",
            "voicing = &bar;",
            "voicing = &g; top = 5;",
            "voicing = &g; bottom = 0;",
            "voicing = &g; top = 1; pick = 1;",
            "voicing = &g; pick = [];",
            "voicing = &g; pick = [2, 1];",
            "voicing = &g; pick = [1, 5];",
            "pick = 3/2;",
            "top = 0;",
        ] {
            assert_eq!(played(fields), None, "{fields}");
        }
    }

    #[test]
    fn a_chord_reads_its_lists_and_fits_them_to_the_pitches_it_plays() {
        let chord = played(
            "top = 2; velocity = [1/2, 1/4]; onset_offset = [0ms, 5ms]; release_velocity = 1/2;",
        )
        .unwrap();
        assert_eq!(chord.lists, [("velocity", 2), ("onset_offset", 2)]);
        let pitches = ["G2", "B2", "D3"];
        assert_eq!(chord.of(&pitches), Ok(vec![&"B2", &"D3"]));
        // One pitch is fewer than `top` keeps.
        let misfit = chord.of(&pitches[..1]).unwrap_err();
        assert_eq!(
            misfit,
            Misfit::Keep {
                field: "top",
                pitches: 1
            }
        );
        assert_eq!(
            misfit.message(),
            "chord.top must be an integer from 1 through 1, the number of pitches"
        );
        // Every pitch played needs its entry.
        let all = played("velocity = [1/2, 1/4];").unwrap();
        let misfit = all.of(&pitches).unwrap_err();
        assert_eq!(
            misfit,
            Misfit::List {
                field: "velocity",
                played: 3
            }
        );
        assert_eq!(
            misfit.message(),
            "chord.velocity must have one entry per pitch played (3)"
        );
    }

    #[test]
    fn kept_pitches_stay_in_list_order() {
        let pitches = ["G2", "B2", "D3", "G3"];
        for (keep, kept) in [
            (Keep::All, Some(vec!["G2", "B2", "D3", "G3"])),
            (Keep::Top(3), Some(vec!["B2", "D3", "G3"])),
            (Keep::Top(4), Some(vec!["G2", "B2", "D3", "G3"])),
            (Keep::Top(5), None),
            (Keep::Bottom(1), Some(vec!["G2"])),
            (Keep::Bottom(5), None),
            (Keep::Pick(vec![2]), Some(vec!["B2"])),
            (Keep::Pick(vec![1, 4]), Some(vec!["G2", "G3"])),
            (Keep::Pick(vec![1, 5]), None),
        ] {
            let found = keep
                .of(&pitches)
                .ok()
                .map(|kept| kept.into_iter().copied().collect::<Vec<_>>());
            assert_eq!(found, kept, "{keep:?}");
        }
    }
}
