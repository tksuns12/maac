//! A vibrato (§8.1): a named periodic pitch shape that a pitch expression
//! adds to its note.
//!
//! One cycle repeats `rate` times a second from `delay`, scaled by `depth`
//! and grown over `rise`.

use num_rational::BigRational;
use num_traits::{Signed, Zero};

use super::fields::{
    default_of, default_symbol_of, defaulted, required, DefaultValue, SourceField,
};
use super::{ReferenceTarget, Validator};
use crate::syntax::{Field, Object, Unit};

pub(crate) const FIELDS: &[SourceField] = &[
    required("rate"),
    required("depth"),
    defaulted("delay", DefaultValue::Quantity(0, Unit::S)),
    defaulted("rise", DefaultValue::Quantity(0, Unit::S)),
    defaulted("shape", DefaultValue::Symbol("centered")),
];

/// The waveform of one cycle of a vibrato.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cycle {
    /// A triangle that rises from the pitch.
    Up,
    /// A triangle that falls from the pitch.
    Down,
    /// A triangle that swings half the depth either side of the pitch.
    Centered,
    /// The cycle curve with this ID.
    Curve(String),
}

impl Cycle {
    fn built_in(shape: &str) -> Option<Self> {
        match shape {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "centered" => Some(Self::Centered),
            _ => None,
        }
    }
}

/// A vibrato as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vibrato {
    /// Cycles a second.
    pub(crate) rate: BigRational,
    /// The swing of a whole cycle, in cents.
    pub(crate) depth: BigRational,
    /// Seconds from the note's onset to the first cycle.
    pub(crate) delay: BigRational,
    /// Seconds over which the swing grows to `depth`.
    pub(crate) rise: BigRational,
    pub(crate) cycle: Cycle,
}

impl Validator<'_> {
    /// Check a `vibrato` object and keep its model.
    pub(super) fn validate_vibrato(&mut self, object: &Object, path: &[String]) {
        if let Some(vibrato) = self.read_vibrato(object, path) {
            self.models.vibratos.insert(object.id.clone(), vibrato);
        }
    }

    fn read_vibrato(&mut self, object: &Object, path: &[String]) -> Option<Vibrato> {
        self.check_declared(object, path, FIELDS);
        let rate = object.field("rate").and_then(|field| {
            let rate = self.quantity(field, &[Unit::Hz, Unit::KHz], path, "rate")?;
            self.within(
                rate.is_positive(),
                "vibrato.rate must be positive",
                field,
                path,
            )
            .then_some(rate)
        });
        let depth = object.field("depth").and_then(|field| {
            let depth = self.quantity(field, &[Unit::Ct], path, "depth")?;
            self.within(
                depth.is_positive(),
                "vibrato.depth must be positive",
                field,
                path,
            )
            .then_some(depth)
        });
        let delay =
            self.vibrato_seconds(object, path, "delay", "vibrato.delay must be nonnegative");
        let rise = self.vibrato_seconds(object, path, "rise", "vibrato.rise must be nonnegative");
        let cycle = match object.field("shape") {
            None => Cycle::built_in(default_symbol_of(FIELDS, "shape")),
            Some(field) => self.read_cycle(field, path),
        };
        Some(Vibrato {
            rate: rate?,
            depth: depth?,
            delay: delay?,
            rise: rise?,
            cycle: cycle?,
        })
    }

    /// The nonnegative seconds of field `name`, or its declared default.
    fn vibrato_seconds(
        &mut self,
        object: &Object,
        path: &[String],
        name: &str,
        message: &str,
    ) -> Option<BigRational> {
        let Some(field) = object.field(name) else {
            return Some(default_of(FIELDS, name));
        };
        let seconds = self.quantity(field, &[Unit::S, Unit::Ms], path, name)?;
        self.within(seconds >= BigRational::zero(), message, field, path)
            .then_some(seconds)
    }

    /// A built-in shape, or a reference to a cycle curve.
    fn read_cycle(&mut self, field: &Field, path: &[String]) -> Option<Cycle> {
        if let Some(shape) = field.value.as_symbol() {
            let cycle = Cycle::built_in(shape);
            self.within(
                cycle.is_some(),
                "vibrato.shape must be up, down, centered or a cycle curve",
                field,
                path,
            );
            return cycle;
        }
        let ReferenceTarget::Object { id, .. } =
            self.expect_object_ref(field, "shape", &["curve"], path)?
        else {
            return None;
        };
        self.validate_cycle(&id, field, path)
            .then_some(Cycle::Curve(id))
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The model of `vibrato v { ...fields }`, if it reads.
    fn vibrato(fields: &str) -> Option<Vibrato> {
        let document = crate::parse(&format!(
            "maac 1; curve square {{ clock = normalized; points = [(0, 0, step), (1/2, 1, step), (1, 0, step)]; }} \
             curve open {{ clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }} \
             vibrato v {{ {fields} }}"
        ))
        .unwrap();
        models_of(&document).vibratos.remove("v")
    }

    fn ratio(numerator: i64, denominator: i64) -> BigRational {
        BigRational::new(numerator.into(), denominator.into())
    }

    #[test]
    fn a_vibrato_reads_its_fields_in_hertz_cents_and_seconds() {
        assert_eq!(
            vibrato("rate = 1/200kHz; depth = 38ct; delay = 300ms; rise = 1/2s; shape = up;"),
            Some(Vibrato {
                rate: ratio(5, 1),
                depth: ratio(38, 1),
                delay: ratio(3, 10),
                rise: ratio(1, 2),
                cycle: Cycle::Up,
            })
        );
    }

    #[test]
    fn a_shape_is_built_in_or_a_cycle_curve() {
        let cycle = |shape: &str| {
            vibrato(&format!("rate = 5Hz; depth = 40ct; shape = {shape};"))
                .map(|vibrato| vibrato.cycle)
        };
        assert_eq!(cycle("down"), Some(Cycle::Down));
        assert_eq!(cycle("centered"), Some(Cycle::Centered));
        assert_eq!(cycle("&square"), Some(Cycle::Curve("square".into())));
        assert_eq!(cycle("&open"), None);
        assert_eq!(cycle("&nothing"), None);
        assert_eq!(cycle("wobble"), None);
    }

    #[test]
    fn a_wrong_vibrato_has_no_model() {
        for fields in [
            "rate = 0Hz; depth = 40ct;",
            "rate = 5Hz; depth = 0ct;",
            "rate = 5Hz; depth = 40ct; delay = -1ms;",
            "rate = 5Hz; depth = 40ct; rise = 1q;",
            "depth = 40ct;",
        ] {
            assert_eq!(vibrato(fields), None, "{fields}");
        }
    }
}
