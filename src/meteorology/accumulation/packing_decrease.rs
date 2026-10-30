//! #216 `ADMIT_BOUNDED` classification of same-run accumulation decreases (#219).
//!
//! Normative decision: `fixtures/accumulation/grib-packing-decrease-decision-v1.json`
//! (`decision`), `docs/research/grib-packing-accumulation-216.md` and the reference
//! predicate `scripts/precipitation/grib_packing_decision.py::classify`, whose
//! rejection order [`classify_packing_decrease`] keeps check for check. Nothing here
//! widens the decision: no tolerance, no epsilon, no GRIB2 or `D != 0` rule.
//!
//! For consecutive declared observations `(i-1, i)` of one cell with canonical
//! `f32` values `c_prev`, `c_cur`:
//!
//! ```text
//! d   = c_cur - c_prev                                  (exact)
//! r_k = ulp_f32(c_k)/2 + 2^-50 (|c_k| + ulp_f32(c_k))
//! B   = 1000 (2^E_prev + 2^E_cur) + r_prev + r_cur      [kg/m2]
//! admit iff d < 0, (R, E, D)_prev != (R, E, D)_cur, and -d < B (strict)
//! ```
//!
//! `d` and `B` are evaluated exactly on dyadic rationals, as in the Python
//! (`fractions.Fraction`) and Rust (`tests/grib_packing_decision.rs`) reference
//! implementations, so the strict comparison has no rounding ambiguity.
//! `ulp_f32(c)` is the spacing of `f32` values in the binade of `c`
//! (`2^-149` for zero and subnormals). It equals the reference's
//! "next larger `f32` minus `c`" for every finite value except `f32::MAX`, where
//! the reference is undefined (the next value is infinite).

use std::cmp::Ordering;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::meteorology::{
    FieldId, GribPackingProvenance, SourceQuantization, Unit,
    CANONICAL_KG_PER_SQUARE_METER_PER_METRE, CONVECTIVE_PRECIPITATION_PARAM_ID,
    F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST, GRIB1_GRID_SIMPLE_ENCODING,
    LARGE_SCALE_PRECIPITATION_PARAM_ID, MARS_CLASS_OPERATIONAL, SOURCE_UNIT_METRE,
};

/// Schema of the frozen #216 decision this classifier implements.
pub const GRIB_PACKING_DECISION_SCHEMA: &str = "flexpart-gpu/grib-packing-decrease-decision/v1";

/// `2^-50` relative term of the canonical f64 decode/unit-conversion path.
const F64_PATH_RELATIVE_SHIFT: i64 = 50;
/// Smallest dyadic scale that holds `2^-50 * 2^-149` exactly.
const MINIMUM_DYADIC_SCALE: i64 = 200;
/// `f32` subnormal exponent: value = mantissa * 2^-149.
const F32_SUBNORMAL_EXPONENT: i64 = -149;
/// `f32` exponent bias plus mantissa width: value = mantissa * 2^(biased - 150).
const F32_EXPONENT_OFFSET: i64 = 150;

/// #216 verdict of one pair (frozen artifact tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PackingDecreaseVerdict {
    /// Quantization-compatible decrease; projected to `+0.0`.
    AdmitBounded,
    /// Inconsistent or out-of-bound pair.
    Reject,
    /// Outside the supported source/packing/conversion class.
    Unresolved,
    /// Not a same-run decrease; ordinary #75 rules apply.
    OutsideRule,
}

/// #216 classification code (frozen artifact tokens).
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackingDecreaseCode {
    QuantizationCompatibleDecrease,
    DecreaseExceedsQuantizationBound,
    IdenticalPackingParameters,
    MissingPackingProvenance,
    UnsupportedSourceClass,
    UnsupportedPacking,
    UnsupportedConversion,
    PackingProvenanceMismatch,
    RunIdentityMismatch,
    UnitMismatch,
    IdentityMismatch,
    MissingValue,
    NonFiniteValue,
    NegativeAccumulatedAmount,
    MissingTimeMetadata,
    NonIncreasingValidTime,
    ResetMismatch,
    UnsupportedField,
    DeclaredContiguousReset,
    OrdinarySameRunInterval,
}

/// One observation of a #216 pair. `None` marks absent metadata or values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PackingObservation<'a> {
    /// Canonical accumulated value exactly as uploaded, kg/m2.
    pub canonical_value: Option<f32>,
    /// Observation valid time.
    pub valid_time_epoch_seconds: Option<i64>,
    /// Declared accumulation reset.
    pub reset_epoch_seconds: Option<i64>,
    /// Horizontal grid identity.
    pub grid_id: &'a str,
    /// Provider source identity of the observation.
    pub source_id: Option<&'a str>,
    /// SHA-256 of the message that produced the observation.
    pub source_message_sha256: Option<&'a str>,
    /// Packing keys claimed for the observation.
    pub packing: Option<&'a GribPackingProvenance>,
}

impl<'a> PackingObservation<'a> {
    /// Observation of a canonical field, with its optional source record.
    #[must_use]
    pub fn canonical(
        canonical_value: f32,
        valid_time_epoch_seconds: i64,
        reset_epoch_seconds: i64,
        grid_id: &'a str,
        source: Option<&'a SourceQuantization>,
    ) -> Self {
        Self {
            canonical_value: Some(canonical_value),
            valid_time_epoch_seconds: Some(valid_time_epoch_seconds),
            reset_epoch_seconds: Some(reset_epoch_seconds),
            grid_id,
            source_id: source.map(|s| s.source_id.as_str()),
            source_message_sha256: source.map(|s| s.source_message_sha256.as_str()),
            packing: source.map(|s| &s.packing),
        }
    }
}

/// A declared consecutive pair `(previous, current)` of one cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PackingDecreasePair<'a> {
    /// Canonical field of both observations.
    pub field_id: FieldId,
    /// Canonical unit of both values.
    pub canonical_unit: Unit,
    /// Observation `i-1`.
    pub previous: PackingObservation<'a>,
    /// Observation `i`.
    pub current: PackingObservation<'a>,
}

/// Exact `d` and `B` of an admitted pair.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmittedPackingBound {
    /// `d` rounded to the nearest `f64` (diagnostic float).
    pub signed_difference_kg_per_square_meter: f64,
    /// Exact `d` as a reduced fraction, e.g. `-1/4194304`.
    pub signed_difference_exact: String,
    /// `B` rounded to the nearest `f64` (diagnostic float).
    pub bound_kg_per_square_meter: f64,
    /// Exact `B` as a reduced fraction, the value the decision compares against.
    pub bound_exact: String,
}

/// Verdict, code and, for an admission, the exact bound.
#[derive(Debug, Clone, PartialEq)]
pub struct PackingClassification {
    /// #216 verdict.
    pub verdict: PackingDecreaseVerdict,
    /// #216 code.
    pub code: PackingDecreaseCode,
    /// Present exactly when the verdict is `ADMIT_BOUNDED`.
    pub admitted: Option<AdmittedPackingBound>,
}

impl PackingClassification {
    const fn decided(verdict: PackingDecreaseVerdict, code: PackingDecreaseCode) -> Self {
        Self {
            verdict,
            code,
            admitted: None,
        }
    }
}

/// Typed #216 admission of one (interval, cell); never an amount or a rate.
///
/// The canonical #75/#90 products of this interval are `+0.0`; the signed
/// difference survives only here. The run total telescopes as
/// `sum(interval amounts) = final total + sum(-signed_difference)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingDecreaseAdmission {
    /// Decision identity, [`GRIB_PACKING_DECISION_SCHEMA`].
    pub decision: String,
    /// Always `ADMIT_BOUNDED`.
    pub verdict: PackingDecreaseVerdict,
    /// Always `quantization_compatible_decrease`.
    pub code: PackingDecreaseCode,
    /// Canonical field.
    pub field_id: FieldId,
    /// Interval (= current observation) index.
    pub interval_index: usize,
    /// Horizontal cell index, x-fastest.
    pub cell_index: usize,
    /// Interval start (previous valid time).
    pub interval_start_epoch_seconds: i64,
    /// Interval end (current valid time).
    pub interval_end_epoch_seconds: i64,
    /// Uploaded canonical `c_prev`, kg/m2.
    pub previous_canonical_kg_per_square_meter: f32,
    /// Uploaded canonical `c_cur`, kg/m2.
    pub current_canonical_kg_per_square_meter: f32,
    /// Signed canonical difference `d < 0` (diagnostic only), kg/m2.
    pub signed_difference_kg_per_square_meter: f64,
    /// Exact `d`.
    pub signed_difference_exact: String,
    /// Approved bound `B` (diagnostic float), kg/m2.
    pub bound_kg_per_square_meter: f64,
    /// Exact `B`.
    pub bound_exact: String,
    /// Source record of observation `i-1`.
    pub previous_source: SourceQuantization,
    /// Source record of observation `i`.
    pub current_source: SourceQuantization,
}

impl PackingDecreaseAdmission {
    /// Message hashes `(previous, current)` of the admitted pair.
    #[must_use]
    pub fn message_sha256(&self) -> (&str, &str) {
        (
            &self.previous_source.packing.message_sha256,
            &self.current_source.packing.message_sha256,
        )
    }
}

fn expected_param_id(field_id: FieldId) -> Option<u32> {
    match field_id {
        FieldId::LargeScalePrecipitation => Some(LARGE_SCALE_PRECIPITATION_PARAM_ID),
        FieldId::ConvectivePrecipitation => Some(CONVECTIVE_PRECIPITATION_PARAM_ID),
        _ => None,
    }
}

/// Classify one pair with the frozen #216 predicate and rejection order.
#[must_use]
#[allow(clippy::too_many_lines)] // the frozen rejection order is kept in one sequence
pub fn classify_packing_decrease(pair: &PackingDecreasePair<'_>) -> PackingClassification {
    use PackingDecreaseCode as Code;
    use PackingDecreaseVerdict as Verdict;
    let (prev, cur) = (&pair.previous, &pair.current);
    let decision = PackingClassification::decided;
    let Some(param_id) = expected_param_id(pair.field_id) else {
        return decision(Verdict::Reject, Code::UnsupportedField);
    };
    if pair.canonical_unit != Unit::KilogramPerSquareMeter {
        return decision(Verdict::Reject, Code::UnitMismatch);
    }
    if prev.grid_id != cur.grid_id || prev.source_id != cur.source_id {
        return decision(Verdict::Reject, Code::IdentityMismatch);
    }
    let mut values = [0.0_f32; 2];
    for (slot, side) in values.iter_mut().zip([prev, cur]) {
        let Some(value) = side.canonical_value else {
            return decision(Verdict::Reject, Code::MissingValue);
        };
        if !value.is_finite() {
            return decision(Verdict::Reject, Code::NonFiniteValue);
        }
        if value < 0.0 {
            return decision(Verdict::Reject, Code::NegativeAccumulatedAmount);
        }
        *slot = value;
    }
    let (Some(prev_valid), Some(cur_valid)) =
        (prev.valid_time_epoch_seconds, cur.valid_time_epoch_seconds)
    else {
        return decision(Verdict::Reject, Code::MissingTimeMetadata);
    };
    if cur_valid <= prev_valid {
        return decision(Verdict::Reject, Code::NonIncreasingValidTime);
    }
    let (Some(prev_reset), Some(cur_reset)) = (prev.reset_epoch_seconds, cur.reset_epoch_seconds)
    else {
        return decision(Verdict::Reject, Code::MissingTimeMetadata);
    };
    if cur_reset == prev_valid && cur_reset > prev_reset {
        return decision(Verdict::OutsideRule, Code::DeclaredContiguousReset);
    }
    if cur_reset != prev_reset || cur_reset >= prev_valid {
        return decision(Verdict::Reject, Code::ResetMismatch);
    }
    let [c_prev, c_cur] = values;
    if c_cur >= c_prev {
        return decision(Verdict::OutsideRule, Code::OrdinarySameRunInterval);
    }
    let mut packings = Vec::with_capacity(2);
    for side in [prev, cur] {
        let Some(packing) = side.packing else {
            return decision(Verdict::Unresolved, Code::MissingPackingProvenance);
        };
        if packing.mars_class.as_deref() != Some(MARS_CLASS_OPERATIONAL) {
            return decision(Verdict::Unresolved, Code::UnsupportedSourceClass);
        }
        if packing.encoding != GRIB1_GRID_SIMPLE_ENCODING
            || packing.bits_per_value < 1
            || packing.bitmap_present
            || packing.decimal_scale_factor != 0
        {
            return decision(Verdict::Unresolved, Code::UnsupportedPacking);
        }
        if packing.canonical_conversion != F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST {
            return decision(Verdict::Unresolved, Code::UnsupportedConversion);
        }
        if side.source_message_sha256 != Some(packing.message_sha256.as_str())
            || packing.param_id != param_id
        {
            return decision(Verdict::Reject, Code::PackingProvenanceMismatch);
        }
        let reference = packing.forecast_reference_epoch_seconds;
        if side.reset_epoch_seconds != Some(reference)
            || reference.checked_add(packing.step_seconds) != side.valid_time_epoch_seconds
        {
            return decision(Verdict::Reject, Code::RunIdentityMismatch);
        }
        if packing.source_unit != SOURCE_UNIT_METRE
            || packing.canonical_per_source_unit != CANONICAL_KG_PER_SQUARE_METER_PER_METRE
        {
            return decision(Verdict::Reject, Code::UnitMismatch);
        }
        packings.push(packing);
    }
    let (p_prev, p_cur) = (packings[0], packings[1]);
    #[allow(clippy::float_cmp)] // exact identity of the recorded GRIB reference values
    let identical = p_prev.reference_value == p_cur.reference_value
        && p_prev.binary_scale_factor == p_cur.binary_scale_factor
        && p_prev.decimal_scale_factor == p_cur.decimal_scale_factor;
    if identical {
        return decision(Verdict::Reject, Code::IdenticalPackingParameters);
    }
    let bound = ExactBound::new(
        [c_prev, c_cur],
        [p_prev.binary_scale_factor, p_cur.binary_scale_factor],
    );
    if bound.admits() {
        PackingClassification {
            verdict: Verdict::AdmitBounded,
            code: Code::QuantizationCompatibleDecrease,
            admitted: Some(bound.admitted()),
        }
    } else {
        decision(Verdict::Reject, Code::DecreaseExceedsQuantizationBound)
    }
}

/// `-d` and `B` as integers in units of `2^-scale`.
struct ExactBound {
    scale: i64,
    decrease: Natural,
    bound: Natural,
}

impl ExactBound {
    /// Preconditions (checked by the caller): finite non-negative values,
    /// `c_cur < c_prev`, `D = 0`, scale 1000 per source unit.
    fn new(values: [f32; 2], binary_scale_factors: [i32; 2]) -> Self {
        let scale = binary_scale_factors
            .iter()
            .map(|e| -i64::from(*e))
            .fold(MINIMUM_DYADIC_SCALE, i64::max);
        let mut bound = Natural::zero();
        for (value, exponent) in values.iter().zip(binary_scale_factors) {
            let (mantissa, value_exponent) = f32_dyadic(*value);
            let ulp_exponent = f32_ulp_exponent(*value);
            let quantum = Natural::shifted(
                u64::from(CANONICAL_KG_PER_SQUARE_METER_PER_METRE),
                i64::from(exponent) + scale,
            );
            let half_ulp = Natural::shifted(1, ulp_exponent - 1 + scale);
            let path =
                Natural::shifted(mantissa, value_exponent + scale - F64_PATH_RELATIVE_SHIFT).add(
                    &Natural::shifted(1, ulp_exponent + scale - F64_PATH_RELATIVE_SHIFT),
                );
            bound = bound.add(&quantum).add(&half_ulp).add(&path);
        }
        let [(m_prev, e_prev), (m_cur, e_cur)] = values.map(f32_dyadic);
        let decrease =
            Natural::shifted(m_prev, e_prev + scale).sub(&Natural::shifted(m_cur, e_cur + scale));
        Self {
            scale,
            decrease,
            bound,
        }
    }

    /// The frozen strict inequality `-d < B`; equality is rejected.
    fn admits(&self) -> bool {
        self.decrease < self.bound
    }

    fn admitted(&self) -> AdmittedPackingBound {
        AdmittedPackingBound {
            signed_difference_kg_per_square_meter: -self.decrease.to_f64_scaled(self.scale),
            signed_difference_exact: format!("-{}", fraction(&self.decrease, self.scale)),
            bound_kg_per_square_meter: self.bound.to_f64_scaled(self.scale),
            bound_exact: fraction(&self.bound, self.scale),
        }
    }
}

/// `value = mantissa * 2^exponent` for a finite non-negative `f32`.
fn f32_dyadic(value: f32) -> (u64, i64) {
    let bits = value.to_bits();
    let biased = i64::from((bits >> 23) & 0xFF);
    let fraction = u64::from(bits & 0x007F_FFFF);
    if biased == 0 {
        (fraction, F32_SUBNORMAL_EXPONENT)
    } else {
        (fraction | (1 << 23), biased - F32_EXPONENT_OFFSET)
    }
}

/// `log2(ulp_f32(value))`: spacing of `f32` values in the binade of `value`.
fn f32_ulp_exponent(value: f32) -> i64 {
    let biased = i64::from((value.to_bits() >> 23) & 0xFF);
    if biased == 0 {
        F32_SUBNORMAL_EXPONENT
    } else {
        biased - F32_EXPONENT_OFFSET
    }
}

/// Reduced fraction `n / 2^scale` in the format of the #216 artifact.
fn fraction(numerator: &Natural, scale: i64) -> String {
    let reduce = i64::try_from(numerator.trailing_zeros())
        .unwrap_or(i64::MAX)
        .min(scale);
    let numerator = numerator.shr(reduce);
    let denominator_exponent = scale - reduce;
    if denominator_exponent == 0 {
        numerator.to_decimal()
    } else {
        format!(
            "{}/{}",
            numerator.to_decimal(),
            Natural::shifted(1, denominator_exponent).to_decimal()
        )
    }
}

/// Minimal arbitrary-precision natural number (little-endian `u64` limbs).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Natural(Vec<u64>);

impl Natural {
    const fn zero() -> Self {
        Self(Vec::new())
    }

    fn normalized(mut limbs: Vec<u64>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self(limbs)
    }

    /// `value * 2^shift`; `shift` is non-negative by construction of the scale.
    fn shifted(value: u64, shift: i64) -> Self {
        let shift = u64::try_from(shift).expect("dyadic scale covers every term");
        let limb_shift = usize::try_from(shift / 64).expect("bounded GRIB scale factor");
        let bit_shift = shift % 64;
        let mut limbs = vec![0; limb_shift];
        let wide = u128::from(value) << bit_shift;
        #[allow(clippy::cast_possible_truncation)]
        limbs.extend([wide as u64, (wide >> 64) as u64]);
        Self::normalized(limbs)
    }

    fn add(&self, other: &Self) -> Self {
        let mut limbs = Vec::with_capacity(self.0.len().max(other.0.len()) + 1);
        let mut carry = 0_u128;
        for index in 0..self.0.len().max(other.0.len()) {
            let sum = u128::from(self.0.get(index).copied().unwrap_or(0))
                + u128::from(other.0.get(index).copied().unwrap_or(0))
                + carry;
            #[allow(clippy::cast_possible_truncation)]
            limbs.push(sum as u64);
            carry = sum >> 64;
        }
        #[allow(clippy::cast_possible_truncation)]
        limbs.push(carry as u64);
        Self::normalized(limbs)
    }

    /// `self - other`; requires `self >= other`.
    fn sub(&self, other: &Self) -> Self {
        assert!(
            self.cmp(other) != Ordering::Less,
            "natural subtraction underflow"
        );
        let mut limbs = Vec::with_capacity(self.0.len());
        let mut borrow = false;
        for (index, limb) in self.0.iter().enumerate() {
            let (partial, first) = limb.overflowing_sub(other.0.get(index).copied().unwrap_or(0));
            let (difference, second) = partial.overflowing_sub(u64::from(borrow));
            limbs.push(difference);
            borrow = first || second;
        }
        Self::normalized(limbs)
    }

    fn bit_length(&self) -> u64 {
        self.0.last().map_or(0, |top| {
            64 * (self.0.len() as u64 - 1) + u64::from(64 - top.leading_zeros())
        })
    }

    /// `self * 2^-scale` rounded to the nearest `f64` (ties to even; normal range).
    fn to_f64_scaled(&self, scale: i64) -> f64 {
        const KEPT_BITS: u64 = 54;
        let length = self.bit_length();
        if length == 0 {
            return 0.0;
        }
        let drop = length.saturating_sub(KEPT_BITS);
        let shift = i64::try_from(drop).expect("bounded bit length");
        let kept = self.shr(shift);
        let sticky = self.trailing_zeros() < drop;
        let mut mantissa = kept.0.first().copied().unwrap_or(0);
        let mut exponent = shift - scale;
        if length >= KEPT_BITS {
            let round = mantissa & 1 == 1;
            mantissa >>= 1;
            exponent += 1;
            if round && (sticky || mantissa & 1 == 1) {
                mantissa += 1;
            }
        }
        #[allow(clippy::cast_precision_loss)] // mantissa <= 2^53 is exact
        let mut value = mantissa as f64;
        while exponent != 0 {
            let step = exponent.clamp(-1_000, 1_000);
            value *= 2_f64.powi(i32::try_from(step).expect("clamped step"));
            exponent -= step;
        }
        value
    }

    fn trailing_zeros(&self) -> u64 {
        let mut zeros = 0;
        for limb in &self.0 {
            if *limb != 0 {
                return zeros + u64::from(limb.trailing_zeros());
            }
            zeros += 64;
        }
        zeros
    }

    fn shr(&self, shift: i64) -> Self {
        let shift = u64::try_from(shift).expect("non-negative shift");
        let limb_shift = usize::try_from(shift / 64).expect("bounded shift");
        let bit_shift = u32::try_from(shift % 64).expect("bit shift");
        let limbs = self
            .0
            .iter()
            .enumerate()
            .skip(limb_shift)
            .map(|(index, limb)| {
                let high = self.0.get(index + 1).copied().unwrap_or(0);
                if bit_shift == 0 {
                    *limb
                } else {
                    (limb >> bit_shift) | (high << (64 - bit_shift))
                }
            })
            .collect();
        Self::normalized(limbs)
    }

    fn to_decimal(&self) -> String {
        const CHUNK: u64 = 10_000_000_000_000_000_000;
        if self.0.is_empty() {
            return "0".to_string();
        }
        let mut limbs = self.0.clone();
        let mut chunks = Vec::new();
        while !limbs.is_empty() {
            let mut remainder = 0_u128;
            for limb in limbs.iter_mut().rev() {
                let current = (remainder << 64) | u128::from(*limb);
                #[allow(clippy::cast_possible_truncation)]
                {
                    *limb = (current / u128::from(CHUNK)) as u64;
                    remainder = current % u128::from(CHUNK);
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            chunks.push(remainder as u64);
            limbs = Self::normalized(limbs).0;
        }
        let mut text = chunks.pop().map(|c| c.to_string()).unwrap_or_default();
        for chunk in chunks.iter().rev() {
            write!(text, "{chunk:019}").expect("writing to a String cannot fail");
        }
        text
    }
}

impl PartialOrd for Natural {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Natural {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // exact dyadic results
    fn test_packing_natural_arithmetic_is_exact() {
        let a = Natural::shifted(3, 130);
        let b = Natural::shifted(1, 129);
        assert_eq!(a.sub(&b), Natural::shifted(5, 129));
        assert_eq!(a.add(&b).shr(129), Natural::shifted(7, 0));
        assert_eq!(
            Natural::shifted(1, 73).to_decimal(),
            "9444732965739290427392"
        );
        assert_eq!(Natural::shifted(1, 0).to_decimal(), "1");
        assert_eq!(Natural::zero().to_decimal(), "0");
        assert_eq!(Natural::shifted(u64::MAX, 64).trailing_zeros(), 64);
        assert_eq!(fraction(&Natural::shifted(3, 198), 200), "3/4");
        assert_eq!(fraction(&Natural::shifted(4, 200), 200), "4");
        assert_eq!(Natural::shifted(3, 0).to_f64_scaled(2), 0.75);
        // 2^54 + 3 rounds up to 2^54 + 4; 2^54 + 2 ties to even 2^54.
        let big = Natural::shifted(1, 54);
        assert_eq!(
            big.add(&Natural::shifted(3, 0)).to_f64_scaled(0),
            2_f64.powi(54) + 4.0
        );
        assert_eq!(
            big.add(&Natural::shifted(2, 0)).to_f64_scaled(0),
            2_f64.powi(54)
        );
    }

    #[test]
    fn test_packing_bound_comparison_is_strict() {
        let at = |decrease: u64, bound: u64| ExactBound {
            scale: 0,
            decrease: Natural::shifted(decrease, 0),
            bound: Natural::shifted(bound, 0),
        };
        assert!(at(12, 13).admits());
        assert!(!at(13, 13).admits(), "-d == B is rejected");
        assert!(!at(14, 13).admits());
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact powers of two
    fn test_packing_f32_dyadic_and_ulp() {
        assert_eq!(f32_dyadic(1.0), (1 << 23, -23));
        assert_eq!(f32_ulp_exponent(1.0), -23);
        assert_eq!(f32_ulp_exponent(0.0), -149);
        assert_eq!(f32_ulp_exponent(f32::from_bits(1)), -149);
        let below_two = f32::from_bits(2.0_f32.to_bits() - 1);
        assert_eq!(
            f64::from(f32::from_bits(below_two.to_bits() + 1)) - f64::from(below_two),
            2_f64.powi(i32::try_from(f32_ulp_exponent(below_two)).expect("exp"))
        );
        assert_eq!(f32_ulp_exponent(f32::MAX), 104);
    }
}
