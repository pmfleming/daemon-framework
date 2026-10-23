//! Bounded presentation rounding compatible with the former ECMAScript toFixed
//! labels. Rust's formatter rounds exact ties to even; presentation uses ties
//! away from zero. Detect ties in the exact binary rational, not by multiplying
//! f64s (which would incorrectly round e.g. 2.55 to 2.6).
pub fn fixed_decimal(value: f64, decimals: u8) -> Option<String> {
    if !value.is_finite() || decimals > 9 {
        return None;
    }
    let magnitude = value.abs();
    let bits = magnitude.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa =
        u128::from(bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u128 << 52 };
    let binary_exponent = if exponent == 0 {
        -1074
    } else {
        exponent - 1023 - 52
    };
    let numerator = mantissa * 5_u128.pow(u32::from(decimals));
    let shift = -(binary_exponent + i32::from(decimals));
    let tie =
        (1..128).contains(&shift) && numerator & ((1_u128 << shift) - 1) == 1_u128 << (shift - 1);
    let rounded = if tie {
        // A tie is positive and finite; advance one representable magnitude
        // without requiring f64::next_up (the framework supports Rust 1.85).
        f64::from_bits(magnitude.to_bits() + 1).copysign(value)
    } else {
        value + 0.0
    };
    Some(format!(
        "{rounded:.precision$}",
        precision = usize::from(decimals)
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_rounding_matches_existing_decimal_labels() {
        for (value, digits, expected) in [
            (1.25, 1, "1.3"),
            (-1.25, 1, "-1.3"),
            (2.55, 1, "2.5"),
            (1.005, 2, "1.00"),
            (100.5, 0, "101"),
            (0.0, 1, "0.0"),
            (-0.0, 1, "0.0"),
            (999.0, 0, "999"),
            (1.25, 2, "1.25"),
        ] {
            assert_eq!(fixed_decimal(value, digits).as_deref(), Some(expected));
        }
        assert_eq!(fixed_decimal(f64::NAN, 1), None);
        assert_eq!(fixed_decimal(f64::INFINITY, 1), None);
        assert_eq!(fixed_decimal(1.0, 255), None);
        assert!(fixed_decimal(f64::MIN_POSITIVE, 9).is_some());
        assert!(fixed_decimal(f64::MAX, 2).is_some());
    }
}
