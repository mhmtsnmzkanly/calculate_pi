//! Certified BBP π hexadecimal digit engine using directed-rounding MPFR interval arithmetic.
use rug::{
    float::Round,
    ops::{AddAssignRound, AssignRound, DivAssignRound, SubAssignRound},
    Float, Integer,
};
use std::{iter::StepBy, ops::Range, sync::OnceLock, thread};
use thiserror::Error;

/// Maximum number of hex digits extractable from a single BBP evaluation.
/// Leaves ~48 guard bits for accumulated rounding error at base precision.
pub const MAX_BATCH_DIGITS: usize = 20;

const BASE_PRECISION: u32 = 128;
const MAX_PRECISION: u32 = 4096;
const SERIES_DENOMINATOR_OFFSETS: [u64; 4] = [1, 4, 5, 6];
/// Below this term count per thread, thread spawning overhead outweighs parallelism gains.
const MIN_TERMS_PER_THREAD: u64 = 4_000;
/// Poll the cancellation callback once every 256 terms within each slice.
pub(crate) const CANCEL_CHECK_MASK: usize = 0xFF;

pub type Cancel<'a> = dyn Fn() -> bool + Sync + 'a;
pub(crate) type TermSlice = StepBy<Range<u64>>;

#[derive(Debug, Error)]
pub enum MathError {
    #[error("certification cancelled at index {index}, precision {precision} bits")]
    Cancelled { index: u64, precision: u32 },
    #[error("failed to certify index {index} up to precision {precision} bits")]
    CertificationFailed { index: u64, precision: u32 },
    #[error("arithmetic overflow at index {index}")]
    IndexOverflow { index: u64 },
    #[error("invalid certification interval at index {index}")]
    InvalidInterval { index: u64 },
}

pub fn mod_pow(base: u64, exp: u64, modulus: u64) -> Option<u64> {
    if modulus == 0 {
        return None;
    }
    if modulus == 1 {
        return Some(0);
    }
    if exp == 0 {
        return Some(1);
    }
    if modulus < (1u64 << 63) {
        if (modulus & 1) == 1 {
            return Some(mod_pow_montgomery(base, exp, modulus));
        }
        if base == 16 {
            let s = modulus.trailing_zeros();
            if let Some(bit_exp) = exp.checked_mul(4) {
                if bit_exp >= u64::from(s) {
                    let m_odd = modulus >> s;
                    if m_odd == 1 {
                        return Some(0);
                    }
                    return Some(mod_pow_montgomery(2, bit_exp - u64::from(s), m_odd) << s);
                }
            }
        }
    }
    if modulus <= u32::MAX as u64 {
        return Some(mod_pow_u64(base % modulus, exp, modulus));
    }
    let m = modulus as u128;
    let mut result: u128 = 1;
    let mut t = (base as u128) % m;
    let mut exp = exp;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result * t % m;
        }
        t = t * t % m;
        exp >>= 1;
    }
    Some(result as u64)
}

/// Computes `-m^-1 mod 2^64` for an odd modulus `m` using 5 Newton-Raphson iterations.
#[inline(always)]
fn mont_neg_inv(m: u64) -> u64 {
    let mut inv = m;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u64.wrapping_sub(m.wrapping_mul(inv)));
    }
    inv.wrapping_neg()
}

/// Montgomery reduction of `a * b * 2^-64 mod m` for odd `m < 2^63` and `a, b < m`.
#[inline(always)]
fn mont_mul(a: u64, b: u64, m: u64, neg_inv: u64) -> u64 {
    let t = (a as u128) * (b as u128);
    let k = (t as u64).wrapping_mul(neg_inv);
    let u = ((t + (k as u128) * (m as u128)) >> 64) as u64;
    if u >= m {
        u - m
    } else {
        u
    }
}

/// Division-free binary exponentiation in Montgomery space for odd `1 < m < 2^63` and `exp > 0`.
///
/// Keeps `result` in standard representation and `t` in Montgomery form (`base * 2^64 mod m`),
/// since `mont_mul(plain, mont) = plain * base * 2^64 * 2^-64 = plain * base mod m`.
fn mod_pow_montgomery(base: u64, mut exp: u64, m: u64) -> u64 {
    let neg_inv = mont_neg_inv(m);
    let mut result = 1u64;
    let mut t = ((((base % m) as u128) << 64) % (m as u128)) as u64;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mont_mul(result, t, m, neg_inv);
        }
        exp >>= 1;
        if exp > 0 {
            t = mont_mul(t, t, m, neg_inv);
        }
    }
    result
}

/// `base` and `modulus` must fit in 32 bits so products never overflow `u64`.
fn mod_pow_u64(base: u64, mut exp: u64, modulus: u64) -> u64 {
    let mut result = 1;
    let mut t = base;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result * t % modulus;
        }
        t = t * t % modulus;
        exp >>= 1;
    }
    result
}

/// Closed interval with outward-rounded bounds.
#[derive(Debug, Clone)]
pub(crate) struct Interval {
    pub(crate) lo: Float,
    pub(crate) hi: Float,
}

impl Interval {
    pub(crate) fn zero(precision: u32) -> Self {
        Self {
            lo: Float::with_val(precision, 0),
            hi: Float::with_val(precision, 0),
        }
    }

    pub(crate) fn add(&mut self, other: &Interval) {
        self.lo.add_assign_round(&other.lo, Round::Down);
        self.hi.add_assign_round(&other.hi, Round::Up);
    }

    pub(crate) fn sub(&mut self, other: &Interval) {
        self.lo.sub_assign_round(&other.hi, Round::Down);
        self.hi.sub_assign_round(&other.lo, Round::Up);
    }

    fn shifted_left(mut self, bits: u32) -> Self {
        self.lo <<= bits;
        self.hi <<= bits;
        self
    }
}

fn denominator_of(n: u64, k: u64, j: u64) -> Result<u64, MathError> {
    8u64.checked_mul(k)
        .and_then(|v| v.checked_add(j))
        .ok_or(MathError::IndexOverflow { index: n })
}

/// Evaluates all four BBP sub-series `Σ (16^(n-k) mod (8k + j)) / (8k + j)` for `j in [1, 4, 5, 6]`
/// in a single fused pass over `k` in `terms`.
fn sums_for_range(
    n: u64,
    terms: TermSlice,
    precision: u32,
    cancel: &Cancel<'_>,
) -> Result<Vec<Interval>, MathError> {
    let mut sums: Vec<Interval> = (0..SERIES_DENOMINATOR_OFFSETS.len())
        .map(|_| Interval::zero(precision))
        .collect();
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
        let base_den = 8u64
            .checked_mul(k)
            .ok_or(MathError::IndexOverflow { index: n })?;
        let exp = n - k;
        for (sum, j) in sums.iter_mut().zip(SERIES_DENOMINATOR_OFFSETS) {
            let den = base_den
                .checked_add(j)
                .ok_or(MathError::IndexOverflow { index: n })?;
            let reduced = mod_pow(16, exp, den).ok_or(MathError::IndexOverflow { index: n })?;
            // Both integers are < 2^64, so assignment at >= 128-bit precision is exact.
            residue.assign_round(reduced, Round::Down);
            denominator.assign_round(den, Round::Down);
            term.lo.assign_round(&residue, Round::Down);
            term.lo.div_assign_round(&denominator, Round::Down);
            term.hi.assign_round(&residue, Round::Up);
            term.hi.div_assign_round(&denominator, Round::Up);
            sum.add(&term);
        }
    }
    Ok(sums)
}

/// Number of explicit tail terms required so the analytic remainder bound is ~2^-precision.
fn tail_terms(precision: u32) -> u64 {
    u64::from(precision / 4)
}

/// Computes an interval enclosing the infinite tail `Σ[m=1..∞] 16^-m / (8(n+m) + j)`.
///
/// After `M` explicit terms, the remaining tail is non-negative and bounded above by
/// `16^-M / (15 · (8(n+M+1) + j))` since `Σ[m=M+1..∞] 16^-m = 16^-M / 15`.
fn tail(n: u64, j: u64, precision: u32) -> Result<Interval, MathError> {
    let explicit = tail_terms(precision);
    let mut sum = Interval::zero(precision);
    let mut denominator = Float::new(precision);
    let mut term = Interval::zero(precision);
    for m in 1..=explicit {
        let k = n
            .checked_add(m)
            .ok_or(MathError::IndexOverflow { index: n })?;
        denominator.assign_round(denominator_of(n, k, j)?, Round::Down);
        term.lo.assign_round(1, Round::Down);
        term.lo.div_assign_round(&denominator, Round::Down);
        term.hi.assign_round(1, Round::Up);
        term.hi.div_assign_round(&denominator, Round::Up);
        // Division/shifting by a power of two is exact.
        term.lo >>= (4 * m) as u32;
        term.hi >>= (4 * m) as u32;
        sum.add(&term);
    }

    let k = n
        .checked_add(explicit + 1)
        .ok_or(MathError::IndexOverflow { index: n })?;
    let den = denominator_of(n, k, j)?
        .checked_mul(15)
        .ok_or(MathError::IndexOverflow { index: n })?;
    denominator.assign_round(den, Round::Down);
    let mut remainder = Float::with_val(precision, 1);
    remainder.div_assign_round(&denominator, Round::Up);
    remainder >>= (4 * explicit) as u32;
    sum.hi.add_assign_round(&remainder, Round::Up);
    Ok(sum)
}

fn available_threads() -> u64 {
    static THREADS: OnceLock<u64> = OnceLock::new();
    *THREADS.get_or_init(|| {
        thread::available_parallelism()
            .map(|count| count.get() as u64)
            .unwrap_or(1)
    })
}

/// Partitions `0..terms` into `parts` interleaved slices so every thread receives
/// a balanced mix of large and small modular exponents.
fn split_terms(terms: u64, parts: u64) -> Vec<TermSlice> {
    let step = usize::try_from(parts).unwrap_or(usize::MAX).max(1);
    (0..parts).map(|part| (part..terms).step_by(step)).collect()
}

/// Evaluates `0..terms` in parallel across available cores and merges the partial sums
/// component-wise. Partitioning only affects rounding order of the outward bounds, never
/// which digits can be certified.
pub(crate) fn sum_in_parallel<F>(terms: u64, work: F) -> Result<Vec<Interval>, MathError>
where
    F: Fn(TermSlice) -> Result<Vec<Interval>, MathError> + Sync,
{
    let threads = (terms / MIN_TERMS_PER_THREAD).clamp(1, available_threads());
    if threads == 1 {
        return work((0..terms).step_by(1));
    }
    let work = &work;
    let results: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = split_terms(terms, threads)
            .into_iter()
            .map(|range| scope.spawn(move || work(range)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let mut results = results.into_iter();
    let mut merged = results.next().expect("at least one partition exists")?;
    for result in results {
        for (total, part) in merged.iter_mut().zip(result?) {
            total.add(&part);
        }
    }
    Ok(merged)
}

/// Computes finite sums for each of the four BBP sub-series.
fn finite_sums(n: u64, precision: u32, cancel: &Cancel<'_>) -> Result<Vec<Interval>, MathError> {
    let terms = n
        .checked_add(1)
        .ok_or(MathError::IndexOverflow { index: n })?;
    sum_in_parallel(terms, |range| sums_for_range(n, range, precision, cancel))
}

/// Returns an interval enclosing a real value congruent to `16^n · π` modulo 1.
fn pi_interval(n: u64, precision: u32, cancel: &Cancel<'_>) -> Result<Interval, MathError> {
    let mut series = finite_sums(n, precision, cancel)?;
    for (sum, j) in series.iter_mut().zip(SERIES_DENOMINATOR_OFFSETS) {
        sum.add(&tail(n, j, precision)?);
    }
    let mut series = series.into_iter();
    let mut next = || series.next().expect("one sum per series");
    let (s1, s4, s5, s6) = (next(), next(), next(), next());
    // 4·S1 - 2·S4 - S5 - S6; bit shifts are exact.
    let mut pi = s1.shifted_left(2);
    pi.sub(&s4.shifted_left(1));
    pi.sub(&s5);
    pi.sub(&s6);
    Ok(pi)
}

/// Up to `max_digits` leading hexadecimal digits on which both interval bounds agree.
fn common_digits(interval: Interval, max_digits: usize, index: u64) -> Result<Vec<u8>, MathError> {
    let invalid = || MathError::InvalidInterval { index };
    if interval.lo > interval.hi {
        return Err(invalid());
    }
    let scale = 4 * max_digits as u32;
    let floor_scaled = |mut bound: Float| {
        bound <<= scale;
        bound.floor_mut();
        bound.to_integer().ok_or_else(invalid)
    };
    let lower = floor_scaled(interval.lo)?;
    let upper = floor_scaled(interval.hi)?;
    let sixteen = Integer::from(16);
    let mut digits = Vec::with_capacity(max_digits);
    for position in 1..=max_digits {
        let drop_bits = 4 * (max_digits - position) as u32;
        let cell = Integer::from(&lower >> drop_bits);
        if cell != Integer::from(&upper >> drop_bits) {
            break;
        }
        digits.push(cell.modulo(&sixteen).to_u32_wrapping() as u8);
    }
    Ok(digits)
}

/// Doubles precision until `interval` yields at least one certified common digit.
pub(crate) fn certify<F>(
    n: u64,
    max_digits: usize,
    cancel: &Cancel<'_>,
    interval: F,
) -> Result<Vec<u8>, MathError>
where
    F: Fn(u32) -> Result<Interval, MathError>,
{
    let max_digits = max_digits.clamp(1, MAX_BATCH_DIGITS);
    let mut precision = BASE_PRECISION;
    while precision <= MAX_PRECISION {
        if cancel() {
            return Err(MathError::Cancelled {
                index: n,
                precision,
            });
        }
        let digits = common_digits(interval(precision)?, max_digits, n)?;
        if !digits.is_empty() {
            return Ok(digits);
        }
        precision *= 2;
    }
    Err(MathError::CertificationFailed {
        index: n,
        precision: MAX_PRECISION,
    })
}

/// Certified hexadecimal digits of the fractional part of π starting at index `n`.
///
/// Returns between 1 and `max_digits` digits (clamped to [`MAX_BATCH_DIGITS`]).
/// A digit is accepted only when the entire enclosing interval remains inside the
/// same integer cell for that digit and all preceding digits in the batch.
pub fn certified_hex_digits(
    n: u64,
    max_digits: usize,
    cancel: &Cancel<'_>,
) -> Result<Vec<u8>, MathError> {
    certify(n, max_digits, cancel, |precision| {
        pi_interval(n, precision, cancel)
    })
}

pub fn certified_hex_digit(n: u64) -> Result<u8, MathError> {
    Ok(certified_hex_digits(n, 1, &|| false)?[0])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rug::float::Constant;

    /// Reference fractional hex digits taken from MPFR's independent π constant.
    pub(crate) fn reference_digits(count: usize) -> Vec<u8> {
        let bits = (count as u32 + 32) * 4;
        let pi = Float::with_val(bits, Constant::Pi);
        let text = pi.to_string_radix(16, Some(count + 16));
        text.split('.')
            .nth(1)
            .unwrap()
            .bytes()
            .take(count)
            .map(|b| (b as char).to_digit(16).unwrap() as u8)
            .collect()
    }

    fn collect_batches(count: usize) -> (Vec<u8>, usize) {
        let mut digits = Vec::with_capacity(count);
        let mut batches = 0;
        while digits.len() < count {
            let n = digits.len() as u64;
            digits.extend(certified_hex_digits(n, MAX_BATCH_DIGITS, &|| false).unwrap());
            batches += 1;
        }
        digits.truncate(count);
        (digits, batches)
    }

    #[test]
    fn first_pi_hex_digits_are_deterministic() {
        let digits: String = (0..8)
            .map(|n| char::from_digit(certified_hex_digit(n).unwrap() as u32, 16).unwrap())
            .collect();
        assert_eq!(digits, "243f6a88");
    }

    #[test]
    fn known_hex_indices() {
        for (n, expected) in [(0, 2), (8, 8), (16, 1), (32, 10), (64, 4)] {
            assert_eq!(certified_hex_digit(n).unwrap(), expected);
        }
    }

    #[test]
    fn batches_match_independent_mpfr_pi() {
        let count = 6_000;
        let (digits, batches) = collect_batches(count);
        assert_eq!(digits, reference_digits(count));
        assert!(batches < count / 10, "batch extraction degraded: {batches}");
    }

    #[test]
    fn threaded_evaluation_matches_mpfr_pi_at_depth() {
        let start = 3 * MIN_TERMS_PER_THREAD as usize;
        let reference = reference_digits(start + MAX_BATCH_DIGITS);
        let digits = certified_hex_digits(start as u64, MAX_BATCH_DIGITS, &|| false).unwrap();
        assert!(!digits.is_empty());
        assert_eq!(digits, reference[start..start + digits.len()]);
    }

    #[test]
    fn every_start_index_agrees_with_single_digit_answers() {
        let reference = reference_digits(400);
        for n in 0..380usize {
            let digits = certified_hex_digits(n as u64, MAX_BATCH_DIGITS, &|| false).unwrap();
            assert_eq!(digits, reference[n..n + digits.len()], "start {n}");
        }
    }

    #[test]
    fn interval_encloses_reference_value() {
        for n in [0u64, 7, 500] {
            let interval = pi_interval(n, 256, &|| false).unwrap();
            let reference = reference_digits(n as usize + 40);
            let mut fraction = Float::with_val(256, 0);
            for digit in reference[n as usize..n as usize + 40].iter().rev() {
                fraction += *digit as u32;
                fraction /= 16u32;
            }
            // The interval is congruent to π modulo 1; compare fractional parts.
            let width = Float::with_val(256, &interval.hi - &interval.lo);
            let shift = Float::with_val(256, interval.lo.clone().floor());
            let lo = Float::with_val(256, &interval.lo - &shift);
            assert!(width < 1e-40);
            let hi = Float::with_val(256, &lo + &width);
            let tolerance = Float::with_val(256, 1) >> 152u32;
            assert!(fraction >= Float::with_val(256, &lo - &tolerance));
            assert!(fraction <= Float::with_val(256, &hi + &tolerance));
        }
    }

    #[test]
    fn tail_bound_covers_direct_tail() {
        let p = 256;
        let (n, j) = (3u64, 1u64);
        let enclosure = tail(n, j, p).unwrap();
        let mut direct = Float::with_val(2048, 0);
        for m in 1..400u32 {
            let den = 8 * (n + m as u64) + j;
            let mut term = Float::with_val(2048, 1);
            term /= den;
            term >>= 4 * m;
            direct += term;
        }
        assert!(enclosure.lo <= direct);
        assert!(enclosure.hi >= direct);
        let width = Float::with_val(p, &enclosure.hi - &enclosure.lo);
        assert!(width < (Float::with_val(p, 1) >> 240u32));
    }

    #[test]
    fn modular_power_handles_small_known_values() {
        assert_eq!(mod_pow(16, 5, 13), Some(9));
        assert_eq!(mod_pow(10, 0, 7), Some(1));
        assert_eq!(mod_pow(u64::MAX, 2, u64::MAX), Some(0));
        assert_eq!(mod_pow(1, 1, 0), None);
    }

    #[test]
    fn modular_power_paths_agree_at_the_u32_boundary() {
        let wide = |base: u64, exp: u64, m: u64| {
            let (mut r, mut t, mut e) = (1u128, base as u128 % m as u128, exp);
            while e > 0 {
                if e & 1 == 1 {
                    r = r * t % m as u128;
                }
                t = t * t % m as u128;
                e >>= 1;
            }
            r as u64
        };
        for m in [
            2u64,
            3,
            4,
            6,
            12,
            24,
            32,
            257,
            65_537,
            u32::MAX as u64 - 1,
            u32::MAX as u64,
            u32::MAX as u64 + 1,
            u32::MAX as u64 + 2,
            (1u64 << 62) - 57,
            (1u64 << 62) + 4,
            (1u64 << 63) - 1,
            (1u64 << 63) + 1,
        ] {
            for (base, exp) in [
                (2, 1),
                (2, 999_999),
                (16, 0),
                (16, 1),
                (16, 2),
                (16, 12345),
                (16, 1_000_000),
                (u64::MAX, 77),
                (m - 1, 3),
            ] {
                assert_eq!(
                    mod_pow(base, exp, m),
                    Some(wide(base, exp, m)),
                    "base={base}, exp={exp}, m={m}"
                );
            }
        }
    }

    #[test]
    fn cancellation_is_reported() {
        let err = certified_hex_digits(0, 1, &|| true).unwrap_err();
        assert!(matches!(err, MathError::Cancelled { .. }));

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let err = certified_hex_digits(3 * MIN_TERMS_PER_THREAD, 1, &|| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 5
        })
        .unwrap_err();
        assert!(matches!(err, MathError::Cancelled { .. }));
    }

    #[test]
    fn split_terms_cover_range_without_gaps() {
        let parts = split_terms(1003, 8);
        let counts: Vec<usize> = parts.iter().map(|part| part.clone().count()).collect();
        assert!(counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 1);
        let mut covered: Vec<u64> = parts.into_iter().flatten().collect();
        covered.sort_unstable();
        assert_eq!(covered, (0..1003).collect::<Vec<u64>>());
    }
}
