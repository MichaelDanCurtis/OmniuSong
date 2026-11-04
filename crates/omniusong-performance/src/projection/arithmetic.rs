use super::{invalid, require};
use omniusong_recording::Result;

/// Exact positive binary64 BPM ratio, with PPQ and seconds already factored in.
pub(super) struct Tempo {
    numerator: u128,
    denominator: u128,
}
impl Tempo {
    pub(super) fn new(bits: u64) -> Result<Self> {
        let value = f64::from_bits(bits);
        require(
            value.is_finite() && (30.0..=300.0).contains(&value),
            "Projection tempo must be finite and between 30 and 300 BPM",
        )?;
        let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
        let significand = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let shift = u32::try_from(52 - exponent).map_err(invalid)?;
        let denominator = 1u128
            .checked_shl(shift)
            .and_then(|n| n.checked_mul(60))
            .ok_or_else(|| invalid("Exact tempo denominator overflow"))?;
        let numerator = u128::from(significand)
            .checked_mul(u128::from(omniusong_core::PPQ))
            .ok_or_else(|| invalid("Exact tempo numerator overflow"))?;
        Ok(Self {
            numerator,
            denominator,
        })
    }
    pub(super) fn ticks(&self, frames: u64, rate: u32) -> Result<u64> {
        require(
            (8_000..=768_000).contains(&rate),
            "Projection source sample rate is unsupported",
        )?;
        let numerator = u128::from(frames)
            .checked_mul(self.numerator)
            .ok_or_else(|| invalid("Exact frame-to-tick numerator overflow"))?;
        let denominator = self
            .denominator
            .checked_mul(u128::from(rate))
            .ok_or_else(|| invalid("Exact frame-to-tick denominator overflow"))?;
        let quotient = numerator / denominator;
        let remainder = numerator % denominator;
        let rounded = quotient
            .checked_add(u128::from(remainder >= denominator.div_ceil(2)))
            .ok_or_else(|| invalid("Rounded tick overflow"))?;
        u64::try_from(rounded).map_err(invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_endpoints_round_independently_and_ties_are_up() {
        let tempo = Tempo::new(125.0f64.to_bits()).unwrap();
        assert_eq!(tempo.ticks(2, 8000).unwrap(), 1);
        assert_eq!(tempo.ticks(6, 8000).unwrap(), 2);
        // Duration is the difference of rounded endpoints, not rounded duration.
        assert_eq!(
            tempo.ticks(2, 8000).unwrap() - tempo.ticks(1, 8000).unwrap(),
            1
        );
        assert_eq!(tempo.ticks(1, 8000).unwrap(), 0);
    }
    #[test]
    fn large_integer_frames_do_not_pass_through_f64() {
        let tempo = Tempo::new(120.0f64.to_bits()).unwrap();
        let frame = (1u64 << 53) + 13;
        let numerator = u128::from(frame) * 1920;
        let expected = numerator / 48000 + u128::from(numerator % 48000 >= 24000);
        assert_eq!(tempo.ticks(frame, 48000).unwrap(), expected as u64);
        assert!(tempo.ticks(i64::MAX as u64, 8000).is_ok());
    }
    #[test]
    fn exact_nondecimal_tempo_bits_and_shape_edges_are_preserved() {
        for value in [30.0, 300.0, f64::from_bits(120.0f64.to_bits() + 1)] {
            let tempo = Tempo::new(value.to_bits()).unwrap();
            assert!(tempo.ticks(48000, 48000).unwrap() > 0);
        }
        for value in [f64::NAN, f64::INFINITY, -0.0, 0.0, 29.999, 300.001] {
            assert!(Tempo::new(value.to_bits()).is_err());
        }
    }
}
