//! A groove (§9.2): a named swing map that a placement applies to its
//! events.
//!
//! Counting pairs of `grid` steps from each bar line, the first step of a
//! pair takes `ratio` of the pair. Every onset and gate end is then `delay`
//! seconds later.

use num_rational::BigRational;
use num_traits::{Signed, Zero};

use super::fields::{default_of, late, required, DefaultValue, SourceField};
use super::Validator;
use crate::syntax::{Object, Unit};

pub(crate) const FIELDS: &[SourceField] = &[
    late("delay", DefaultValue::Quantity(0, Unit::S)),
    required("grid"),
    required("ratio"),
];

/// A groove as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Groove {
    /// The length of one step, in quarter notes.
    pub(crate) grid: BigRational,
    /// The share of each pair of steps that the first step takes.
    pub(crate) ratio: BigRational,
    /// Seconds by which every event sits behind the beat.
    pub(crate) delay: BigRational,
}

impl Validator<'_> {
    /// Check a `groove` object and keep its model.
    pub(super) fn validate_groove(&mut self, object: &Object, path: &[String]) {
        if let Some(groove) = self.read_groove(object, path) {
            self.models.grooves.insert(object.id.clone(), groove);
        }
    }

    fn read_groove(&mut self, object: &Object, path: &[String]) -> Option<Groove> {
        self.check_declared(object, path, FIELDS);
        let limit = BigRational::new(1.into(), 20.into());
        let delay = match object.field("delay") {
            None => Some(default_of(FIELDS, "delay")),
            Some(field) => self
                .quantity(field, &[Unit::S, Unit::Ms], path, "delay")
                .and_then(|delay| {
                    self.within(
                        delay.abs() <= limit,
                        "groove.delay must be from -50ms to 50ms",
                        field,
                        path,
                    )
                    .then_some(delay)
                }),
        };
        let grid = object.field("grid").and_then(|field| {
            let grid = self.quantity(field, &[Unit::Q], path, "grid")?;
            self.within(
                grid > BigRational::zero(),
                "groove.grid must be positive",
                field,
                path,
            )
            .then_some(grid)
        });
        let ratio = object.field("ratio").and_then(|field| {
            let ratio = self.number_value(&field.value, path, "ratio")?;
            self.within(
                ratio >= BigRational::new(1.into(), 2.into())
                    && ratio <= BigRational::new(3.into(), 4.into()),
                "groove.ratio must be from 1/2 through 3/4",
                field,
                path,
            )
            .then_some(ratio)
        });
        Some(Groove {
            grid: grid?,
            ratio: ratio?,
            delay: delay?,
        })
    }
}
