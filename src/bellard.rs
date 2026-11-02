//! Second, independent spigot formula for π's hexadecimal digits used exclusively for verification.
//!
//! Bellard's series uses different terms and denominators from BBP so that a bug in the BBP
//! series evaluation cannot reproduce the same wrong digit here:
//!
//! π = 2^-6 Σ (-1)^k 2^-10k ( -2^5/(4k+1) - 1/(4k+3) + 2^8/(10k+1) - 2^6/(10k+3)
//!                            - 2^2/(10k+5) - 2^2/(10k+7) + 1/(10k+9) )
//!
//! It shares the interval type, `mod_pow`, MPFR, and digit-extraction logic with BBP;
//! a defect in those shared primitives would not be caught by comparing the two formulas.
use crate::math::{
    certify, mod_pow, sum_in_parallel, Cancel, Interval, MathError, TermSlice, CANCEL_CHECK_MASK,
};
use rug::{
    float::Round,
    ops::{AddAssignRound, AssignRound, DivAssignRound, SubAssignRound},
    Float,
};

struct Series {
    numerator_log2: i128,
    multiplier: u64,
    offset: u64,
    positive: bool,
}

const fn series(numerator_log2: i128, multiplier: u64, offset: u64, positive: bool) -> Series {
    Series {
        numerator_log2,
        multiplier,
        offset,
        positive,
    }
}

const SERIES: [Series; 7] = [
    series(5, 4, 1, false),
    series(0, 4, 3, false),
    series(8, 10, 1, true),
    series(6, 10, 3, false),
    series(2, 10, 5, false),
    series(2, 10, 7, false),
    series(0, 10, 9, true),
];

/// Partial sum of `16^n · π` modulo integers over the given `k` slice.
fn partial_sum(
    n: u64,
    terms: TermSlice,
    precision: u32,
    cancel: &Cancel<'_>,
) -> Result<Interval, MathError> {
    let mut sum = Interval::zero(precision);
    let mut residue = Float::new(precision);
    let mut denominator = Float::new(precision);
    let mut term = Interval::zero(precision);
    for (step, k) in terms.enumerate() {
        if step & CANCEL_CHECK_MASK == 0 && cancel() {
            return Err(MathError::Cancelled {
                index: n,
                precision,
            });
        }
        for series in &SERIES {
            let d = series
                .multiplier
                .checked_mul(k)
                .and_then(|v| v.checked_add(series.offset))
                .ok_or(overflow_of(n))?;
            let exponent = 4 * i128::from(n) - 6 - 10 * i128::from(k) + series.numerator_log2;
            denominator.assign_round(d, Round::Down);
            let reduced = if exponent >= 0 {
                let exponent = u64::try_from(exponent).map_err(|_| overflow_of(n))?;
                mod_pow(2, exponent, d).ok_or(overflow_of(n))?
            } else {
                1
            };
            residue.assign_round(reduced, Round::Down);
            term.lo.assign_round(&residue, Round::Down);
            term.lo.div_assign_round(&denominator, Round::Down);
            term.hi.assign_round(&residue, Round::Up);
            term.hi.div_assign_round(&denominator, Round::Up);
            if exponent < 0 {
                let shift = u32::try_from(-exponent).map_err(|_| overflow_of(n))?;
                term.lo >>= shift;
                term.hi >>= shift;
            }
            if series.positive == (k % 2 == 0) {
                sum.add(&term);
            } else {
                sum.sub(&term);
            }
        }
    }
    Ok(sum)
}

fn overflow_of(index: u64) -> MathError {
    MathError::IndexOverflow { index }
}

/// Returns an interval enclosing a real value congruent to `16^n · π` modulo 1.
///
/// The numerator magnitudes in each term sum to less than `2^9` and every denominator
/// is at least 1; since terms decay by `2^-10`, the sum of terms past `k = K` is bounded
/// by `2^(4n - 6 - 10K + 10)`. `K` is chosen so this bound is `<= 2^-precision`.
fn pi_interval(n: u64, precision: u32, cancel: &Cancel<'_>) -> Result<Interval, MathError> {
    let bits = n
        .checked_mul(4)
        .and_then(|v| v.checked_add(u64::from(precision) + 4))
        .ok_or(overflow_of(n))?;
    let terms = bits / 10 + 1;
    let mut parts = sum_in_parallel(terms, |range| {
        Ok(vec![partial_sum(n, range, precision, cancel)?])
    })?;
    let mut sum = parts.pop().expect("single sum");
    let remainder = Float::with_val(precision, 1) >> precision;
    sum.lo.sub_assign_round(&remainder, Round::Down);
    sum.hi.add_assign_round(&remainder, Round::Up);
    Ok(sum)
}

/// Computes hexadecimal digits of the fractional part of π starting at index `n`
/// using Bellard's formula under the same certification rule as the BBP engine.
pub fn bellard_hex_digits(
    n: u64,
    max_digits: usize,
    cancel: &Cancel<'_>,
) -> Result<Vec<u8>, MathError> {
    certify(n, max_digits, cancel, |precision| {
        pi_interval(n, precision, cancel)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{certified_hex_digits, tests::reference_digits, MAX_BATCH_DIGITS};

    #[test]
    fn first_digits_are_correct() {
        let digits = bellard_hex_digits(0, 8, &|| false).unwrap();
        assert_eq!(digits, [2, 4, 3, 15, 6, 10, 8, 8]);
    }

    #[test]
    fn every_start_index_matches_mpfr_pi() {
        let reference = reference_digits(400);
        for n in 0..380usize {
            let digits = bellard_hex_digits(n as u64, MAX_BATCH_DIGITS, &|| false).unwrap();
            assert_eq!(digits, reference[n..n + digits.len()], "start {n}");
        }
    }

    #[test]
    fn threaded_evaluation_agrees_with_bbp_at_depth() {
        let n = 50_000;
        let ours = bellard_hex_digits(n, MAX_BATCH_DIGITS, &|| false).unwrap();
        let bbp = certified_hex_digits(n, MAX_BATCH_DIGITS, &|| false).unwrap();
        let shared = ours.len().min(bbp.len());
        assert!(shared > 0);
        assert_eq!(ours[..shared], bbp[..shared]);
    }

    #[test]
    fn cancellation_is_reported() {
        let error = bellard_hex_digits(100_000, 1, &|| true).unwrap_err();
        assert!(matches!(error, MathError::Cancelled { .. }));

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let error = bellard_hex_digits(100_000, 1, &|| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 5
        })
        .unwrap_err();
        assert!(matches!(error, MathError::Cancelled { .. }));
    }
}
