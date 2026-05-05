//! Deterministic `getSqrtRatioAtTick` from Uniswap V3 TickMath, in Rust.
//!
//! Source of truth: <https://github.com/Uniswap/v3-core/blob/main/contracts/libraries/TickMath.sol>.
//! All arithmetic is U256 with 128-bit-shift-after-multiply pattern, identical to the
//! Solidity reference. Floats are forbidden here (SPEC §1.4 byte-determinism).

use alloy::primitives::U256;
use anyhow::{anyhow, Result};

pub const MIN_TICK: i32 = -887_272;
pub const MAX_TICK: i32 = 887_272;

/// Equivalent to `TickMath.getSqrtRatioAtTick(tick)` returning a uint160 sqrtPriceX96.
/// We store it as `U256` because `(sqrtPriceX96)^2 / 2^192` later needs more than 160
/// bits during squaring; truncation to uint160 is the Solidity quirk we don't need.
pub fn get_sqrt_ratio_at_tick(tick: i32) -> Result<U256> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return Err(anyhow!(
            "tick {} out of range [{}, {}]",
            tick,
            MIN_TICK,
            MAX_TICK
        ));
    }
    let abs_tick: u32 = tick.unsigned_abs();

    // Initialize ratio per the Solidity reference. The "every 1 set bit picks up a
    // multiplier" pattern below leaves us with ratio ≈ (1.0001)^(-absTick) * 2^128.
    let mut ratio: U256 = if abs_tick & 0x1 != 0 {
        u256_hex("fffcb933bd6fad37aa2d162d1a594001")
    } else {
        u256_hex("100000000000000000000000000000000")
    };

    macro_rules! step {
        ($mask:expr, $mul:expr) => {
            if abs_tick & $mask != 0 {
                ratio = mul_shr_128(ratio, u256_hex($mul));
            }
        };
    }
    step!(0x2, "fff97272373d413259a46990580e213a");
    step!(0x4, "fff2e50f5f656932ef12357cf3c7fdcc");
    step!(0x8, "ffe5caca7e10e4e61c3624eaa0941cd0");
    step!(0x10, "ffcb9843d60f6159c9db58835c926644");
    step!(0x20, "ff973b41fa98c081472e6896dfb254c0");
    step!(0x40, "ff2ea16466c96a3843ec78b326b52861");
    step!(0x80, "fe5dee046a99a2a811c461f1969c3053");
    step!(0x100, "fcbe86c7900a88aedcffc83b479aa3a4");
    step!(0x200, "f987a7253ac413176f2b074cf7815e54");
    step!(0x400, "f3392b0822b70005940c7a398e4b70f3");
    step!(0x800, "e7159475a2c29b7443b29c7fa6e889d9");
    step!(0x1000, "d097f3bdfd2022b8845ad8f792aa5825");
    step!(0x2000, "a9f746462d870fdf8a65dc1f90e061e5");
    step!(0x4000, "70d869a156d2a1b890bb3df62baf32f7");
    step!(0x8000, "31be135f97d08fd981231505542fcfa6");
    step!(0x10000, "9aa508b5b7a84e1c677de54f3e99bc9");
    step!(0x20000, "5d6af8dedb81196699c329225ee604");
    step!(0x40000, "2216e584f5fa1ea926041bedfe98");
    step!(0x80000, "48a170391f7dc42444e8fa2");

    // For positive tick we invert: ratio_pos = U256::MAX / ratio.
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }

    // sqrtPriceX96 = uint160((ratio >> 32) + (ratio % (1 << 32) == 0 ? 0 : 1))
    let two_pow_32 = U256::from(1u128 << 32);
    let shifted = ratio >> 32;
    let remainder = ratio % two_pow_32;
    let sqrt_price_x96 = if remainder.is_zero() {
        shifted
    } else {
        shifted + U256::from(1u64)
    };
    Ok(sqrt_price_x96)
}

/// `(a * b) >> 128`, computed in U512 to avoid uint256 overflow during the multiply.
fn mul_shr_128(a: U256, b: U256) -> U256 {
    // alloy::U256 doesn't expose widening mul directly; do the multiply in U512 via
    // a manual high/low split — a × b = (a.hi × b + a.lo × b * 2^128) — too involved.
    // Simpler: every Uniswap step has both factors < 2^128, so a × b < 2^256 fits in
    // U256 without overflow. We rely on that invariant. (The init `ratio` is < 2^128
    // and every constant is < 2^128, so after each step the result is again < 2^128.)
    let prod = a
        .checked_mul(b)
        .expect("TickMath multiply overflow — invariant violated");
    prod >> 128
}

fn u256_hex(s: &str) -> U256 {
    U256::from_str_radix(s, 16).expect("static hex constant")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_ratio_at_tick_zero_is_q96_one() {
        // 1.0001^0 = 1; sqrtPriceX96 = 1 * 2^96 = 79228162514264337593543950336
        let want = U256::from(1u128) << 96;
        let got = get_sqrt_ratio_at_tick(0).unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn sqrt_ratio_at_tick_min_max_in_range() {
        // Just verifying that the extreme valid ticks don't error; their exact values
        // are pinned by Uniswap's own tests in TickMath.t.sol.
        get_sqrt_ratio_at_tick(MIN_TICK).unwrap();
        get_sqrt_ratio_at_tick(MAX_TICK).unwrap();
    }

    #[test]
    fn sqrt_ratio_out_of_range_errors() {
        assert!(get_sqrt_ratio_at_tick(MIN_TICK - 1).is_err());
        assert!(get_sqrt_ratio_at_tick(MAX_TICK + 1).is_err());
    }

    #[test]
    fn sqrt_ratio_negative_tick_smaller_than_one() {
        // For negative tick, sqrtPriceX96 < 2^96.
        let q96 = U256::from(1u128) << 96;
        let r = get_sqrt_ratio_at_tick(-1000).unwrap();
        assert!(r < q96);
    }

    #[test]
    fn sqrt_ratio_positive_tick_greater_than_one() {
        let q96 = U256::from(1u128) << 96;
        let r = get_sqrt_ratio_at_tick(1000).unwrap();
        assert!(r > q96);
    }

    #[test]
    fn sqrt_ratio_known_negative_tick() {
        // tick = -69894 corresponds to LPT/WETH ≈ 0.000916 (real on-chain reading).
        // sqrt(0.000916) ≈ 0.030266 → sqrtPriceX96 ≈ 0.030266 * 2^96 ≈ 2.398e27.
        let r = get_sqrt_ratio_at_tick(-69894).unwrap();
        let s = r.to_string();
        // Length of the decimal string for ~2.398e27 is 28 digits.
        assert!(
            s.len() >= 27 && s.len() <= 29,
            "unexpected magnitude: {}",
            s
        );
    }
}
