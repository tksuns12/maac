//! The fields a language object declares, written once for each kind, or
//! part of a kind, that has a module beside this one: which fields it
//! requires, and the default of each it may leave out.
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
    /// A numerator over a denominator, of a unit.
    Fraction(i64, i64, Unit),
    /// A symbol.
    Symbol(&'static str),
}

impl DefaultValue {
    /// The exact value of a number or quantity, in the default's own unit.
    pub(crate) fn rational(self) -> Option<BigRational> {
        match self {
            Self::Number(value) | Self::Quantity(value, _) => {
                Some(BigRational::from_integer(value.into()))
            }
            Self::Fraction(numerator, denominator, _) => {
                Some(BigRational::new(numerator.into(), denominator.into()))
            }
            Self::Symbol(_) => None,
        }
    }
}

/// Whether a field must be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presence {
    Required,
    /// The field may be left out, and then holds this value.
    Default(DefaultValue),
    /// The field may be left out, and then holds this value. The language
    /// gained the field after the execution hash was defined: the hash
    /// leaves the field out at its default, so that earlier documents keep
    /// theirs.
    Late(DefaultValue),
    /// The field may be left out, and then says nothing.
    Optional,
    /// The field belongs beside the named field of its object. Left out
    /// beside it, the field holds this value; without it, the field says
    /// nothing.
    Beside(&'static str, DefaultValue),
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
            Presence::Required | Presence::Optional => None,
            Presence::Default(value) | Presence::Late(value) | Presence::Beside(_, value) => {
                Some(value)
            }
        }
    }

    /// The value N(A) writes for the field in an object that leaves it out
    /// (§20.2), where `written` tells which fields the object writes.
    pub(crate) fn expanded(&self, written: impl Fn(&str) -> bool) -> Option<DefaultValue> {
        match self.presence {
            Presence::Default(value) | Presence::Late(value) => Some(value),
            Presence::Beside(owner, value) if written(owner) => Some(value),
            _ => None,
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

pub(super) const fn defaulted(name: &'static str, default: DefaultValue) -> SourceField {
    SourceField {
        name,
        presence: Presence::Default(default),
    }
}

pub(super) const fn late(name: &'static str, default: DefaultValue) -> SourceField {
    SourceField {
        name,
        presence: Presence::Late(default),
    }
}

pub(super) const fn optional(name: &'static str) -> SourceField {
    SourceField {
        name,
        presence: Presence::Optional,
    }
}

pub(super) const fn beside(
    name: &'static str,
    owner: &'static str,
    default: DefaultValue,
) -> SourceField {
    SourceField {
        name,
        presence: Presence::Beside(owner, default),
    }
}

/// The number or quantity field `name` of `fields` holds when it is left
/// out.
pub(super) fn default_of(fields: &[SourceField], name: &str) -> BigRational {
    fields
        .iter()
        .find(|field| field.name == name)
        .and_then(SourceField::default)
        .and_then(DefaultValue::rational)
        .unwrap_or_else(|| unreachable!("field `{name}` declares no default number"))
}

/// The symbol field `name` of `fields` holds when it is left out.
pub(super) fn default_symbol_of(fields: &[SourceField], name: &str) -> &'static str {
    match fields
        .iter()
        .find(|field| field.name == name)
        .and_then(SourceField::default)
    {
        Some(DefaultValue::Symbol(symbol)) => symbol,
        _ => unreachable!("field `{name}` declares no default symbol"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{declared_fields, models_of, Models, DECLARED, PARTS};
    use super::*;

    /// A default as source text.
    fn written(default: DefaultValue) -> String {
        match default {
            DefaultValue::Number(value) => value.to_string(),
            DefaultValue::Quantity(value, unit) => format!("{value}{}", unit.as_str()),
            DefaultValue::Fraction(numerator, denominator, unit) => {
                format!("{numerator}/{denominator}{}", unit.as_str())
            }
            DefaultValue::Symbol(symbol) => symbol.to_owned(),
        }
    }

    /// One object of each kind that declares its fields, with only its
    /// required fields written.
    const BARE: [(&str, &str); 4] = [
        ("groove", "grid = 1/2q; ratio = 2/3;"),
        ("phrase", "at = 0q; length = 4q; dynamics = &arc;"),
        ("vary", "seed = 7;"),
        ("vibrato", "rate = 5Hz; depth = 40ct;"),
    ];

    fn document(kind: &str, fields: &str) -> crate::syntax::Document {
        let object = format!("{kind} one {{ {fields} }}");
        // A phrase is a child of a pattern.
        let object = match kind {
            "phrase" => format!("pattern bar {{ length = 4q; {object} }}"),
            _ => object,
        };
        crate::parse(&format!(
            "maac 1; curve arc {{ clock = score; points = [(0q, 1, step)]; }} {object}"
        ))
        .unwrap()
    }

    #[test]
    fn every_kind_that_declares_its_fields_is_covered() {
        let declared: Vec<&str> = DECLARED.iter().map(|(kind, _)| *kind).collect();
        let covered: Vec<&str> = BARE.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(declared, covered);
    }

    #[test]
    fn the_declared_fields_of_a_kind_differ() {
        for (kind, fields) in DECLARED.iter().chain(PARTS) {
            let mut names: Vec<&str> = fields.iter().map(|field| field.name).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), fields.len(), "{kind}");
        }
    }

    /// A field that belongs beside another names a field of the same list.
    #[test]
    fn a_field_beside_another_names_a_declared_field() {
        for (kind, fields) in DECLARED.iter().chain(PARTS) {
            for field in *fields {
                if let Presence::Beside(owner, _) = field.presence {
                    assert!(
                        fields.iter().any(|other| other.name == owner),
                        "{kind}.{} beside {owner}",
                        field.name
                    );
                }
            }
        }
    }

    /// The model of an object that leaves a field out is the model of the
    /// object that writes the declared default.
    #[test]
    fn a_field_left_out_reads_as_its_declared_default() {
        for (kind, bare) in BARE {
            let left_out = models_of(&document(kind, bare));
            assert_eq!(
                left_out.grooves.len()
                    + left_out.phrases.len()
                    + left_out.varies.len()
                    + left_out.vibratos.len(),
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
