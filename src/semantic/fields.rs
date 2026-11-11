//! The fields a language object declares, written once for each kind that
//! has a module beside this one: which fields it requires, and the default
//! of each it may leave out.
//!
//! Semantic analysis checks an object's fields against the list, the
//! kind's model takes its defaults from it, and semantic normalization
//! expands them into N(A) (§20.2).

use num_rational::BigRational;

use crate::syntax::Unit;

/// A default, as the specification writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefaultValue {
    /// A dimensionless whole number.
    Number(i64),
    /// A whole number of a unit.
    Quantity(i64, Unit),
}

impl DefaultValue {
    /// The exact value, in the default's own unit.
    pub(crate) fn rational(self) -> BigRational {
        let (Self::Number(value) | Self::Quantity(value, _)) = self;
        BigRational::from_integer(value.into())
    }
}

/// Whether a field must be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presence {
    Required,
    /// The field may be left out, and then holds this value. The language
    /// gained the field after the execution hash was defined: the hash
    /// leaves the field out at its default, so that earlier documents keep
    /// theirs.
    Late(DefaultValue),
}

/// One field of a language object, beside `label`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceField {
    pub(crate) name: &'static str,
    pub(crate) presence: Presence,
}

impl SourceField {
    /// The value the field holds when it is left out.
    pub(crate) const fn default(&self) -> Option<DefaultValue> {
        match self.presence {
            Presence::Required => None,
            Presence::Late(value) => Some(value),
        }
    }

    pub(crate) const fn is_required(&self) -> bool {
        matches!(self.presence, Presence::Required)
    }

    pub(crate) const fn is_late(&self) -> bool {
        matches!(self.presence, Presence::Late(_))
    }
}

pub(super) const fn required(name: &'static str) -> SourceField {
    SourceField {
        name,
        presence: Presence::Required,
    }
}

pub(super) const fn late(name: &'static str, default: DefaultValue) -> SourceField {
    SourceField {
        name,
        presence: Presence::Late(default),
    }
}

/// The value field `name` of `fields` holds when it is left out.
pub(super) fn default_of(fields: &[SourceField], name: &str) -> BigRational {
    fields
        .iter()
        .find(|field| field.name == name)
        .and_then(SourceField::default)
        .unwrap_or_else(|| unreachable!("field `{name}` declares no default"))
        .rational()
}

#[cfg(test)]
mod tests {
    use super::super::{declared_fields, models_of, Models, DECLARED};
    use super::*;

    /// A default as source text.
    fn written(default: DefaultValue) -> String {
        match default {
            DefaultValue::Number(value) => value.to_string(),
            DefaultValue::Quantity(value, unit) => format!("{value}{}", unit.as_str()),
        }
    }

    /// One object of each kind that declares its fields, with only its
    /// required fields written.
    const BARE: [(&str, &str); 2] = [
        ("groove", "grid = 1/2q; ratio = 2/3;"),
        ("vary", "seed = 7;"),
    ];

    fn document(kind: &str, fields: &str) -> crate::syntax::Document {
        crate::parse(&format!("maac 1; {kind} one {{ {fields} }}")).unwrap()
    }

    #[test]
    fn every_kind_that_declares_its_fields_is_covered() {
        let declared: Vec<&str> = DECLARED.iter().map(|(kind, _)| *kind).collect();
        let covered: Vec<&str> = BARE.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(declared, covered);
    }

    #[test]
    fn the_declared_fields_are_in_name_order_and_differ() {
        for (kind, _) in BARE {
            let names: Vec<&str> = declared_fields(kind)
                .unwrap()
                .iter()
                .map(|field| field.name)
                .collect();
            let mut sorted = names.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(names, sorted, "{kind}");
        }
    }

    /// The model of an object that leaves a field out is the model of the
    /// object that writes the declared default.
    #[test]
    fn a_field_left_out_reads_as_its_declared_default() {
        for (kind, bare) in BARE {
            let left_out = models_of(&document(kind, bare));
            assert_eq!(
                left_out.grooves.len() + left_out.varies.len(),
                1,
                "{kind} reads"
            );
            for field in declared_fields(kind).unwrap() {
                let Some(default) = field.default() else {
                    continue;
                };
                let source = format!("{bare} {} = {};", field.name, written(default));
                assert_eq!(
                    models_of(&document(kind, &source)),
                    left_out,
                    "{kind}.{}",
                    field.name
                );
            }
        }
    }

    /// An object without one of its required fields has no model.
    #[test]
    fn a_required_field_left_out_gives_no_model() {
        for (kind, bare) in BARE {
            for field in declared_fields(kind).unwrap() {
                if !field.is_required() {
                    continue;
                }
                let source: String = bare
                    .split_inclusive(';')
                    .filter(|part| part.trim().split(' ').next() != Some(field.name))
                    .collect();
                assert_ne!(source, bare, "{kind}.{} is written in BARE", field.name);
                assert_eq!(
                    models_of(&document(kind, &source)),
                    Models::default(),
                    "{kind} without {}",
                    field.name
                );
            }
        }
    }
}
