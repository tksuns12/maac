//! A vary (§9.4): a declared, seeded departure from the score that a
//! placement names.
//!
//! Each event's velocity and timing move by a draw from the seed and the
//! event's address, within ±`velocity` and ±`timing` seconds.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::fields::{default_of, late, required, DefaultValue, SourceField};
use super::Validator;
use crate::syntax::{Object, Unit, ValueKind};

pub(crate) const FIELDS: &[SourceField] = &[
    required("seed"),
    late("timing", DefaultValue::Quantity(0, Unit::S)),
    late("velocity", DefaultValue::Number(0)),
];

/// A vary as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vary {
    pub(crate) seed: u32,
    /// The furthest a velocity moves, either way.
    pub(crate) velocity: BigRational,
    /// The furthest an onset or a gate end moves, either way, in seconds.
    pub(crate) timing: BigRational,
}

impl Vary {
    /// The offset for `field` of the event at `address`: `amount × (u1 + u2
    /// − 1)`, where `u1` and `u2` are the top 53 bits of the first two
    /// SplitMix64 outputs from the seed mixed with `address/field`, over
    /// 2^53. Exact, triangular within ±`amount`.
    pub(crate) fn offset(&self, address: &str, field: &str, amount: &BigRational) -> BigRational {
        let mut rng = crate::seeded::Rng::new(crate::seeded::mix(
            u64::from(self.seed),
            format!("{address}/{field}").as_bytes(),
        ));
        let (u1, u2) = (rng.next() >> 11, rng.next() >> 11);
        let scale = BigInt::from(1u64 << 53);
        amount * BigRational::new(BigInt::from(u1) + BigInt::from(u2) - &scale, scale)
    }
}

impl Validator<'_> {
    /// Check a `vary` object and keep its model.
    pub(super) fn validate_vary(&mut self, object: &Object, path: &[String]) {
        if let Some(vary) = self.read_vary(object, path) {
            self.models.varies.insert(object.id.clone(), vary);
        }
    }

    fn read_vary(&mut self, object: &Object, path: &[String]) -> Option<Vary> {
        self.check_declared(object, path, FIELDS);
        let seed = object.field("seed").and_then(|field| {
            let seed = match &field.value.kind {
                ValueKind::Number(seed) if seed.is_integer() && *seed >= BigRational::one() => {
                    seed.to_integer().to_u32()
                }
                _ => None,
            };
            self.within(
                seed.is_some(),
                "vary.seed must be an integer from 1 to 4294967295",
                field,
                path,
            );
            seed
        });
        let velocity = match object.field("velocity") {
            None => Some(default_of(FIELDS, "velocity")),
            Some(field) => self
                .number_value(&field.value, path, "velocity")
                .and_then(|velocity| {
                    self.within(
                        !velocity.is_negative() && velocity <= BigRational::new(1.into(), 2.into()),
                        "vary.velocity must be from 0 to 1/2",
                        field,
                        path,
                    )
                    .then_some(velocity)
                }),
        };
        let timing = match object.field("timing") {
            None => Some(default_of(FIELDS, "timing")),
            Some(field) => self
                .quantity(field, &[Unit::S, Unit::Ms], path, "timing")
                .and_then(|timing| {
                    self.within(
                        timing >= BigRational::zero()
                            && timing <= BigRational::new(1.into(), 20.into()),
                        "vary.timing must be from 0ms to 50ms",
                        field,
                        path,
                    )
                    .then_some(timing)
                }),
        };
        Some(Vary {
            seed: seed?,
            velocity: velocity?,
            timing: timing?,
        })
    }
}
