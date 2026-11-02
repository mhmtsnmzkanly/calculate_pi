//! Streaming base-16 to base-10 converter backed by GMP big-integer invariants.
use rug::Integer;

/// `DecimalConverter`: converts accumulated hexadecimal digits into finalized
/// decimal digits with exact rounding and stability guarantees.
#[derive(Clone)]
pub struct DecimalConverter {
    pub y: Integer,      // x·10^k mod 2^bits  (invariant: 0 <= y < 2^bits)
    pub bits: u32,       // 4·N, denominator 16^N = 2^bits
    pub pow10k: Integer, // 10^k
}

impl Default for DecimalConverter {
    fn default() -> Self {
        DecimalConverter {
            y: Integer::new(),
            bits: 0,
            pow10k: Integer::from(1),
        }
    }
}

impl DecimalConverter {
    /// Appends a new hexadecimal digit and returns any finalized decimal digits (`0..=9`).
    /// On average, `~log10(16)` decimal digits are finalized per call.
    pub fn add_hex(&mut self, h: u8) -> Result<Vec<u8>, String> {
        if h > 0x0f {
            return Err(format!("hex digit out of range: {h}"));
        }
        let bits = self
            .bits
            .checked_add(4)
            .ok_or("converter digit length overflow")?;
        self.bits = bits;

        self.y <<= 4;
        self.y += u32::from(h) * &self.pow10k;
        self.y.keep_bits_mut(bits);

        // Because the denominator is 2^bits, division and remainder reduce to bit shifts and masks.
        let mut finalized: Vec<u8> = Vec::new();
        loop {
            let mut t = Integer::from(&self.y * 10u32);
            let d = Integer::from(&t >> bits).to_u8().unwrap_or(0);
            t.keep_bits_mut(bits);

            let next_pow10k = Integer::from(&self.pow10k * 10u32);
            if (Integer::from(&t + &next_pow10k) - 1u32).significant_bits() <= bits {
                finalized.push(d);
                self.y = t;
                self.pow10k = next_pow10k;
            } else {
                break;
            }
        }
        Ok(finalized)
    }

    /// Returns the remainder `y` as an uppercase hexadecimal string for JSON persistence.
    pub fn y_hex_str(&self) -> String {
        format!("{:X}", self.y)
    }

    /// Restores the converter from persisted `y_hex`, `N` (hex digit count), and `k` (decimal digit count).
    pub fn from_hex_str(y_hex: &str, hex_n: usize, decimal_k: usize) -> Result<Self, String> {
        if !y_hex.is_empty() && !y_hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("converter y_hex field contains non-hex characters".into());
        }
        let bits = u32::try_from(hex_n)
            .ok()
            .and_then(|n| n.checked_mul(4))
            .ok_or("converter digit length overflow")?;
        let y = if y_hex.is_empty() {
            Integer::new()
        } else {
            Integer::parse_radix(y_hex, 16)
                .map(Integer::from)
                .map_err(|_| "invalid converter integer")?
        };
        if y.significant_bits() > bits {
            return Err("converter remainder out of denominator range".into());
        }
        let pow10k = rug_pow10(decimal_k)?;
        Ok(DecimalConverter { y, bits, pow10k })
    }

    /// Verifies that the current converter state matches `hex` and `decimal` strings
    /// in a single algebraic check without replaying digits one by one.
    ///
    /// Letting `H` be the integer value of the `N` hex digits and `D` be the integer
    /// value of the `k` decimal digits, the state must satisfy `H·10^k = D·16^N + y`.
    /// Furthermore, the `k`-th digit must be finalized (`y + 10^k - 1 < 16^N`) while
    /// the `(k+1)`-th digit must not yet be finalized.
    pub fn matches_digits(&self, hex: &str, decimal: &str) -> Result<bool, String> {
        let parse = |digits: &str, radix| {
            if digits.is_empty() {
                return Ok(Integer::new());
            }
            Integer::parse_radix(digits, radix)
                .map(Integer::from)
                .map_err(|e| format!("invalid digit string: {e}"))
        };
        let high = parse(hex, 16)?;
        let low = parse(decimal, 10)?;
        let bits = u32::try_from(hex.len())
            .ok()
            .and_then(|n| n.checked_mul(4))
            .ok_or("converter digit length overflow")?;
        if self.y < 0 || self.y.significant_bits() > bits {
            return Ok(false);
        }
        let pow10 = rug_pow10(decimal.len())?;

        let scaled = Integer::from(&high * &pow10);
        if scaled != (low << bits) + &self.y {
            return Ok(false);
        }
        let current_is_final = (Integer::from(&self.y + &pow10) - 1u32).significant_bits() <= bits;
        let next_remainder = Integer::from(&self.y * 10u32).keep_bits(bits);
        let next_is_final = (next_remainder + pow10 * 10u32 - 1u32).significant_bits() <= bits;
        Ok(current_is_final && !next_is_final)
    }

    /// Verifies and restores a `DecimalConverter` directly from `hex` and `decimal` digit strings
    /// in `O(1)` big-integer operations, optionally cross-checking an explicit `y_hex` remainder if present.
    pub fn from_digits(hex: &str, decimal: &str, y_hex: &str) -> Result<Self, String> {
        let parse = |digits: &str, radix| {
            if digits.is_empty() {
                return Ok(Integer::new());
            }
            Integer::parse_radix(digits, radix)
                .map(Integer::from)
                .map_err(|e| format!("invalid digit string: {e}"))
        };
        let high = parse(hex, 16)?;
        let low = parse(decimal, 10)?;
        let bits = u32::try_from(hex.len())
            .ok()
            .and_then(|n| n.checked_mul(4))
            .ok_or("converter digit length overflow")?;
        let pow10k = rug_pow10(decimal.len())?;

        let scaled = Integer::from(&high * &pow10k);
        let y = scaled - (low << bits);
        if y < 0 || y.significant_bits() > bits {
            return Err("converter/decimal data does not match hexadecimal digits".into());
        }
        if !y_hex.is_empty() {
            if !y_hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("converter y_hex field contains non-hex characters".into());
            }
            let saved_y = Integer::parse_radix(y_hex, 16)
                .map(Integer::from)
                .map_err(|_| "invalid converter integer")?;
            if saved_y != y {
                return Err("converter/decimal data does not match hexadecimal digits".into());
            }
        }
        let current_is_final = (Integer::from(&y + &pow10k) - 1u32).significant_bits() <= bits;
        let next_remainder = Integer::from(&y * 10u32).keep_bits(bits);
        let next_is_final =
            (next_remainder + Integer::from(&pow10k * 10u32) - 1u32).significant_bits() <= bits;
        if !current_is_final || next_is_final {
            return Err("converter/decimal data does not match hexadecimal digits".into());
        }
        Ok(DecimalConverter { y, bits, pow10k })
    }

    /// Recomputes state by feeding each digit sequentially (reference for `matches_digits`).
    #[cfg(test)]
    pub fn recompute_from_hex(hex_str: &str) -> Result<(Self, String), String> {
        let mut converter = DecimalConverter::default();
        let mut decimal = String::new();
        for c in hex_str.chars() {
            let h = c
                .to_digit(16)
                .ok_or_else(|| format!("invalid hex character: {c}"))?;
            for d in converter.add_hex(h as u8)? {
                decimal.push(char::from_digit(d as u32, 10).unwrap_or('?'));
            }
        }
        Ok((converter, decimal))
    }
}

fn rug_pow10(k: usize) -> Result<Integer, String> {
    let k = u32::try_from(k).map_err(|_| "converter digit length overflow")?;
    Ok(Integer::from(Integer::u_pow_u(10, k)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PI_HEX: &str = "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89";

    /// Deterministic hex strings: digits of π and repeating sequences that delay finalization.
    fn samples() -> Vec<String> {
        let mut state = 0x0123_4567_89AB_CDEF_u64;
        let mut samples = vec![PI_HEX.to_string(), "F".repeat(90), "0".repeat(90)];
        samples.push(format!("7FFFFFFF{}", "F".repeat(60)));
        samples.push(format!("8000000{}", "0".repeat(60)));
        for _ in 0..40 {
            let text: String = (0..80)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    char::from_digit((state >> 60) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase()
                })
                .collect();
            samples.push(text);
        }
        samples
    }

    fn state_of(hex: &str) -> (DecimalConverter, String) {
        let (converter, decimal) = DecimalConverter::recompute_from_hex(hex).unwrap();
        let restored =
            DecimalConverter::from_hex_str(&converter.y_hex_str(), hex.len(), decimal.len())
                .unwrap();
        (restored, decimal)
    }

    #[test]
    fn every_replayed_prefix_matches() {
        for sample in samples() {
            for n in 0..=sample.len() {
                let (converter, decimal) = state_of(&sample[..n]);
                assert!(
                    converter.matches_digits(&sample[..n], &decimal).unwrap(),
                    "prefix of length {n} of {sample}"
                );
            }
        }
    }

    #[test]
    fn restored_state_continues_like_the_original() {
        let (mut original, _) = DecimalConverter::recompute_from_hex(&PI_HEX[..40]).unwrap();
        let (mut restored, _) = state_of(&PI_HEX[..40]);
        for digit in [3, 1, 15, 0, 9, 2, 6] {
            assert_eq!(
                original.add_hex(digit).unwrap(),
                restored.add_hex(digit).unwrap()
            );
        }
        assert_eq!(original.y, restored.y);
        assert_eq!(original.pow10k, restored.pow10k);
    }

    #[test]
    fn wrong_decimal_digits_are_rejected() {
        let (converter, decimal) = state_of(PI_HEX);
        let flipped = if decimal.starts_with('1') { "2" } else { "1" };
        let wrong = format!("{flipped}{}", &decimal[1..]);
        assert!(!converter.matches_digits(PI_HEX, &wrong).unwrap());
    }

    #[test]
    fn wrong_hex_digits_are_rejected() {
        let (converter, decimal) = state_of(PI_HEX);
        let wrong = format!("3{}", &PI_HEX[1..]);
        assert!(!converter.matches_digits(&wrong, &decimal).unwrap());
    }

    #[test]
    fn wrong_remainder_is_rejected() {
        let (mut converter, decimal) = state_of(PI_HEX);
        converter.y += 1u32;
        assert!(!converter.matches_digits(PI_HEX, &decimal).unwrap());
    }

    /// A state that stops finalization early or advances it one digit too far must be rejected.
    #[test]
    fn a_consistent_state_with_the_wrong_digit_count_is_rejected() {
        for sample in samples() {
            let (exact, decimal) = state_of(&sample);
            let (bits, k) = (sample.len() * 4, decimal.len());
            let high = Integer::from_str_radix(&sample, 16).unwrap();
            let denom = Integer::from(1) << bits as u32;
            for other in [k.saturating_sub(1), k + 1] {
                if other == k {
                    continue;
                }
                let scaled = high.clone() * rug_pow10(other).unwrap();
                let (quotient, y) = scaled.div_rem_euc(denom.clone());
                let text = format!("{:0>other$}", quotient.to_string_radix(10));
                let wrong =
                    DecimalConverter::from_hex_str(&y.to_string_radix(16), sample.len(), other)
                        .unwrap();
                assert!(
                    !wrong.matches_digits(&sample, &text).unwrap(),
                    "count {other} instead of {k} for {sample}"
                );
            }
            assert!(exact.matches_digits(&sample, &decimal).unwrap());
        }
    }

    #[test]
    fn hex_fraction_converts_to_known_decimal_prefix() {
        let (converter, decimal) = DecimalConverter::recompute_from_hex("243F6A8885A3").unwrap();
        assert_eq!(&decimal[..8], "14159265");
        assert!(converter.bits > 0);
    }

    #[test]
    fn converter_round_trip_preserves_state() {
        let mut original = DecimalConverter::default();
        for digit in [2, 4, 3, 15, 6, 10] {
            original.add_hex(digit).unwrap();
        }
        let restored = DecimalConverter::from_hex_str(&original.y_hex_str(), 6, 0).unwrap();
        assert_eq!(restored.y, original.y);
        assert_eq!(restored.bits, original.bits);
    }
}
