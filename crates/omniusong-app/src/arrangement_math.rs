//! Pure display/placement arithmetic. Geometry never becomes a saved position:
//! gestures quantize a delta in the target unit, add it to the exact baseline,
//! then optionally snap the resulting integer position to the absolute grid.
use super::arrangement_types::{PlacementValue, Snap, TICK_SCALE, TickFp, Viewport};

pub(super) const AXIS_LIMIT: TickFp = TickFp(6_000_000_000_000_000_000 * TICK_SCALE);
pub(super) const MAX_VIEWPORT_WIDTH: f32 = 1_000_000.0;
const MAX_LEGACY_TICK: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TempoRatio {
    mantissa: u128,
    denominator: u128,
}

impl TempoRatio {
    pub fn from_bpm(bpm: f64) -> Result<Self, String> {
        if !bpm.is_finite() || !(30.0..=300.0).contains(&bpm) {
            return Err("Arrangement tempo must be finite and within 30..300 BPM".into());
        }
        let bits = bpm.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
        let mantissa = u128::from((1u64 << 52) | (bits & ((1u64 << 52) - 1)));
        let denominator = 1u128
            .checked_shl((52 - exponent) as u32)
            .ok_or("Arrangement tempo denominator overflow")?;
        Ok(Self {
            mantissa,
            denominator,
        })
    }

    fn bpm(self) -> f64 {
        self.mantissa as f64 / self.denominator as f64
    }

    // Frames = ticks * A / B, with the original binary tempo kept exact.
    fn frame_ratio(self, rate: u32) -> Result<(u128, u128), String> {
        if !(8_000..=768_000).contains(&rate) {
            return Err("Arrangement sample rate is outside 8000..768000".into());
        }
        let a = u128::from(rate)
            .checked_mul(60)
            .and_then(|v| v.checked_mul(self.denominator))
            .ok_or("Arrangement frame denominator overflow")?;
        let b = self
            .mantissa
            .checked_mul(960)
            .ok_or("Arrangement tempo overflow")?;
        Ok((a, b))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ClipTiming {
    Legacy {
        start_tick: u64,
        duration_seconds: f64,
    },
    Native {
        position_frame: i64,
        frame_count: u64,
        sample_rate: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PlacementBasis {
    Legacy {
        start_tick: u64,
        duration_seconds: f64,
    },
    Native {
        position_frame: i64,
        frame_count: u64,
        sample_rate: u32,
    },
}

impl From<ClipTiming> for PlacementBasis {
    fn from(timing: ClipTiming) -> Self {
        match timing {
            ClipTiming::Legacy {
                start_tick,
                duration_seconds,
            } => Self::Legacy {
                start_tick,
                duration_seconds,
            },
            ClipTiming::Native {
                position_frame,
                frame_count,
                sample_rate,
            } => Self::Native {
                position_frame,
                frame_count,
                sample_rate,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AxisInterval {
    pub start: TickFp,
    pub end: TickFp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PixelInterval {
    pub start: f32,
    pub end: f32,
}

fn overflow() -> String {
    "Arrangement arithmetic exceeds its checked domain".into()
}

fn in_axis(value: i128) -> bool {
    value.unsigned_abs() <= AXIS_LIMIT.0 as u128
}

fn signed(magnitude: u128, negative: bool) -> Result<i128, String> {
    let value = i128::try_from(magnitude).map_err(|_| overflow())?;
    if negative {
        value.checked_neg().ok_or_else(overflow)
    } else {
        Ok(value)
    }
}

fn round_abs(numerator: u128, denominator: u128) -> Result<u128, String> {
    if denominator == 0 {
        return Err(overflow());
    }
    let whole = numerator / denominator;
    let rest = numerator % denominator;
    whole
        .checked_add(u128::from(rest != 0 && rest >= denominator - rest))
        .ok_or_else(overflow)
}

fn round_ratio(numerator: i128, denominator: u128) -> Result<i128, String> {
    signed(
        round_abs(numerator.unsigned_abs(), denominator)?,
        numerator < 0,
    )
}

fn floor_ratio(numerator: i128, denominator: u128) -> Result<i128, String> {
    let magnitude = numerator.unsigned_abs();
    let whole = magnitude / denominator;
    let rounded = whole
        .checked_add(u128::from(
            numerator < 0 && !magnitude.is_multiple_of(denominator),
        ))
        .ok_or_else(overflow)?;
    signed(rounded, numerator < 0)
}

fn ceil_ratio(numerator: i128, denominator: u128) -> Result<i128, String> {
    let magnitude = numerator.unsigned_abs();
    let whole = magnitude / denominator;
    let rounded = whole
        .checked_add(u128::from(
            numerator >= 0 && !magnitude.is_multiple_of(denominator),
        ))
        .ok_or_else(overflow)?;
    signed(rounded, numerator < 0)
}

// Avoid multiplying full-range fixed ticks by A. q is <2^64, ar <2^63;
// all products remain checked even when the frame delta itself exceeds i64.
fn ticks_to_frames(ticks: TickFp, a: u128, b: u128) -> Result<i128, String> {
    let magnitude = ticks.0.unsigned_abs();
    let scale = TICK_SCALE as u128;
    let q = magnitude / scale;
    let r = magnitude % scale;
    let x = q.checked_mul(a % b).ok_or_else(overflow)?;
    let base = q
        .checked_mul(a / b)
        .and_then(|v| v.checked_add(x / b))
        .ok_or_else(overflow)?;
    let z = (x % b)
        .checked_mul(scale)
        .and_then(|v| r.checked_mul(a).and_then(|r| v.checked_add(r)))
        .ok_or_else(overflow)?;
    let denominator = b.checked_mul(scale).ok_or_else(overflow)?;
    let rounded = base
        .checked_add(round_abs(z, denominator)?)
        .ok_or_else(overflow)?;
    signed(rounded, ticks.0 < 0)
}

fn whole_ticks_to_frames(ticks: i128, a: u128, b: u128) -> Result<i128, String> {
    let magnitude = ticks.unsigned_abs();
    let x = magnitude.checked_mul(a % b).ok_or_else(overflow)?;
    let base = magnitude.checked_mul(a / b).ok_or_else(overflow)?;
    let value = base.checked_add(round_abs(x, b)?).ok_or_else(overflow)?;
    signed(value, ticks < 0)
}

fn frame_to_fp(frame: i64, a: u128, b: u128, ceiling: bool) -> Result<TickFp, String> {
    let numerator = u128::from(frame.unsigned_abs())
        .checked_mul(b)
        .ok_or_else(overflow)?;
    let scaled_rest = (numerator % a)
        .checked_mul(TICK_SCALE as u128)
        .ok_or_else(overflow)?;
    let base = (numerator / a)
        .checked_mul(TICK_SCALE as u128)
        .and_then(|v| v.checked_add(scaled_rest / a))
        .ok_or_else(overflow)?;
    let away = scaled_rest % a != 0 && (ceiling != (frame < 0));
    let value = signed(
        base.checked_add(u128::from(away)).ok_or_else(overflow)?,
        frame < 0,
    )?;
    if !in_axis(value) {
        return Err(overflow());
    }
    Ok(TickFp(value))
}

// Exact ceil of the resulting binary f64 duration, rather than a second lossy
// multiplication/cast near a fixed-point integer boundary.
fn duration_fp_ceil(ticks: f64) -> Result<i128, String> {
    if !ticks.is_finite() || ticks <= 0.0 {
        return Err("Invalid legacy arrangement duration".into());
    }
    let bits = ticks.to_bits();
    let encoded = ((bits >> 52) & 0x7ff) as i32;
    let (mantissa, exponent) = if encoded == 0 {
        (u128::from(bits & ((1u64 << 52) - 1)), -1074)
    } else {
        (
            u128::from((1u64 << 52) | (bits & ((1u64 << 52) - 1))),
            encoded - 1023 - 52,
        )
    };
    let numerator = mantissa
        .checked_mul(TICK_SCALE as u128)
        .ok_or_else(overflow)?;
    let result = if exponent >= 0 {
        numerator
            .checked_mul(1u128.checked_shl(exponent as u32).ok_or_else(overflow)?)
            .ok_or_else(overflow)?
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            u128::from(numerator != 0)
        } else {
            let denominator = 1u128 << shift;
            (numerator / denominator)
                .checked_add(u128::from(numerator % denominator != 0))
                .ok_or_else(overflow)?
        }
    };
    i128::try_from(result).map_err(|_| overflow())
}

fn validate_basis(basis: PlacementBasis, tempo: &TempoRatio) -> Result<(), String> {
    match basis {
        PlacementBasis::Legacy {
            start_tick,
            duration_seconds,
        } => {
            if start_tick > MAX_LEGACY_TICK
                || !duration_seconds.is_finite()
                || !(0.0..=86_400.0).contains(&duration_seconds)
                || duration_seconds == 0.0
            {
                return Err("Invalid legacy arrangement placement or duration".into());
            }
        }
        PlacementBasis::Native {
            position_frame,
            frame_count,
            sample_rate,
        } => {
            tempo.frame_ratio(sample_rate)?;
            let count = i64::try_from(frame_count)
                .map_err(|_| "Native arrangement source count exceeds i64")?;
            if count == 0 || position_frame.checked_add(count).is_none() {
                return Err("Invalid native arrangement count or signed end".into());
            }
        }
    }
    Ok(())
}

pub(super) fn project_interval(
    timing: ClipTiming,
    tempo: &TempoRatio,
) -> Result<AxisInterval, String> {
    validate_basis(timing.into(), tempo)?;
    let interval = match timing {
        ClipTiming::Legacy {
            start_tick,
            duration_seconds,
        } => {
            let start = i128::from(start_tick)
                .checked_mul(TICK_SCALE)
                .ok_or_else(overflow)?;
            // Keep the existing legacy declared-duration evaluation order.
            let beats = duration_seconds * tempo.bpm();
            let ticks = beats * 960.0;
            let duration = duration_fp_ceil(ticks / 60.0)?;
            AxisInterval {
                start: TickFp(start),
                end: TickFp(start.checked_add(duration).ok_or_else(overflow)?),
            }
        }
        ClipTiming::Native {
            position_frame,
            frame_count,
            sample_rate,
        } => {
            let (a, b) = tempo.frame_ratio(sample_rate)?;
            let end = position_frame
                .checked_add(frame_count as i64)
                .ok_or_else(overflow)?;
            AxisInterval {
                start: frame_to_fp(position_frame, a, b, false)?,
                end: frame_to_fp(end, a, b, true)?,
            }
        }
    };
    validate_interval(interval)?;
    Ok(interval)
}

/// Durationless event marker; floor the exact musical position to one display
/// unit. This accepts i64::MAX without inventing an audio-frame count or end.
pub(super) fn project_point(
    frame: i64,
    sample_rate: u32,
    tempo: &TempoRatio,
) -> Result<TickFp, String> {
    let (a, b) = tempo.frame_ratio(sample_rate)?;
    frame_to_fp(frame, a, b, false)
}

fn validate_interval(interval: AxisInterval) -> Result<(), String> {
    if interval.start.0 > interval.end.0 || !in_axis(interval.start.0) || !in_axis(interval.end.0) {
        Err("Arrangement interval is inverted or exceeds the display axis".into())
    } else {
        Ok(())
    }
}

impl Viewport {
    pub fn new(origin: TickFp, span: TickFp) -> Result<Self, String> {
        let end = origin.0.checked_add(span.0).ok_or_else(overflow)?;
        if span.0 <= 0 || !in_axis(origin.0) || !in_axis(end) {
            return Err("Arrangement viewport must have positive bounded span".into());
        }
        Ok(Self { origin, span })
    }

    /// Clipped local pixels. A durationless marker may touch either edge;
    /// nonempty intervals touching only an edge have no visible extent.
    pub fn project_interval(
        self,
        interval: AxisInterval,
        width: f32,
    ) -> Result<Option<PixelInterval>, String> {
        Self::new(self.origin, self.span)?;
        validate_interval(interval)?;
        if !width.is_finite() || width <= 0.0 || width > MAX_VIEWPORT_WIDTH {
            return Err("Arrangement viewport width is outside its finite pixel budget".into());
        }
        let end = self
            .origin
            .0
            .checked_add(self.span.0)
            .ok_or_else(overflow)?;
        if interval.start == interval.end {
            if interval.start.0 < self.origin.0 || interval.start.0 > end {
                return Ok(None);
            }
        } else if interval.end.0 <= self.origin.0 || interval.start.0 >= end {
            return Ok(None);
        }
        let start = interval.start.0.max(self.origin.0);
        let finish = interval.end.0.min(end);
        let pixel = |value: i128| {
            ((value - self.origin.0) as f64 / self.span.0 as f64 * f64::from(width)) as f32
        };
        Ok(Some(PixelInterval {
            start: pixel(start),
            end: pixel(finish),
        }))
    }
}

fn finish_placement(basis: PlacementBasis, candidate: i128) -> Result<PlacementValue, String> {
    match basis {
        PlacementBasis::Legacy { .. } => {
            let value =
                u64::try_from(candidate).map_err(|_| "Legacy clip placement cannot be negative")?;
            if value > MAX_LEGACY_TICK {
                return Err("Legacy clip placement exceeds 1000000000 ticks".into());
            }
            Ok(PlacementValue::LegacyTick(value))
        }
        PlacementBasis::Native { frame_count, .. } => {
            let frame =
                i64::try_from(candidate).map_err(|_| "Native clip placement exceeds i64")?;
            if frame.checked_add(frame_count as i64).is_none() {
                return Err(
                    "Native clip placement plus its unchanged source count exceeds i64".into(),
                );
            }
            Ok(PlacementValue::NativeFrame(frame))
        }
    }
}

fn native_grid_position(frame: i128, a: u128, b: u128, grid: u64) -> Result<(i128, u128), String> {
    let numerator = frame
        .checked_mul(i128::try_from(b).map_err(|_| overflow())?)
        .ok_or_else(overflow)?;
    let denominator = a.checked_mul(u128::from(grid)).ok_or_else(overflow)?;
    Ok((numerator, denominator))
}

fn snap_position(
    basis: PlacementBasis,
    candidate: i128,
    tempo: &TempoRatio,
    snap: Snap,
) -> Result<i128, String> {
    let Some(grid) = snap.ticks() else {
        return Ok(candidate);
    };
    match basis {
        PlacementBasis::Legacy { .. } => round_ratio(candidate, u128::from(grid))?
            .checked_mul(i128::from(grid))
            .ok_or_else(overflow),
        PlacementBasis::Native { sample_rate, .. } => {
            let (a, b) = tempo.frame_ratio(sample_rate)?;
            let (numerator, denominator) = native_grid_position(candidate, a, b, grid)?;
            let ticks = round_ratio(numerator, denominator)?
                .checked_mul(i128::from(grid))
                .ok_or_else(overflow)?;
            whole_ticks_to_frames(ticks, a, b)
        }
    }
}

pub(super) fn drag_candidate(
    basis: PlacementBasis,
    delta: TickFp,
    tempo: &TempoRatio,
    snap: Snap,
) -> Result<PlacementValue, String> {
    validate_basis(basis, tempo)?;
    if delta.0.unsigned_abs() > (AXIS_LIMIT.0 as u128) * 2 {
        return Err("Arrangement drag exceeds the display delta domain".into());
    }
    let baseline = match basis {
        PlacementBasis::Legacy { start_tick, .. } => i128::from(start_tick),
        PlacementBasis::Native { position_frame, .. } => i128::from(position_frame),
    };
    if delta.0 == 0 {
        return finish_placement(basis, baseline);
    }
    let movement = match basis {
        PlacementBasis::Legacy { .. } => round_ratio(delta.0, TICK_SCALE as u128)?,
        PlacementBasis::Native { sample_rate, .. } => {
            let (a, b) = tempo.frame_ratio(sample_rate)?;
            ticks_to_frames(delta, a, b)?
        }
    };
    // Pointer jitter below one target-unit rounding threshold must not turn
    // an untouched off-grid placement into an explicit snapped edit.
    if movement == 0 {
        return finish_placement(basis, baseline);
    }
    let candidate = baseline.checked_add(movement).ok_or_else(overflow)?;
    finish_placement(basis, snap_position(basis, candidate, tempo, snap)?)
}

/// Off means one exact target unit. Snapped nudges choose the strictly earlier
/// or later absolute grid point, even when the current position is on the grid.
pub(super) fn nudge_candidate(
    basis: PlacementBasis,
    direction: i8,
    tempo: &TempoRatio,
    snap: Snap,
) -> Result<PlacementValue, String> {
    validate_basis(basis, tempo)?;
    if !matches!(direction, -1 | 1) {
        return Err("Arrangement nudge direction must be -1 or 1".into());
    }
    let baseline = match basis {
        PlacementBasis::Legacy { start_tick, .. } => i128::from(start_tick),
        PlacementBasis::Native { position_frame, .. } => i128::from(position_frame),
    };
    let candidate = if let Some(grid) = snap.ticks() {
        let (numerator, denominator) = match basis {
            PlacementBasis::Legacy { .. } => (baseline, u128::from(grid)),
            PlacementBasis::Native { sample_rate, .. } => {
                let (a, b) = tempo.frame_ratio(sample_rate)?;
                native_grid_position(baseline, a, b, grid)?
            }
        };
        let k = if direction > 0 {
            floor_ratio(numerator, denominator)?.checked_add(1)
        } else {
            ceil_ratio(numerator, denominator)?.checked_sub(1)
        }
        .ok_or_else(overflow)?;
        let at_grid = |k: i128| -> Result<i128, String> {
            let ticks = k.checked_mul(i128::from(grid)).ok_or_else(overflow)?;
            match basis {
                PlacementBasis::Legacy { .. } => Ok(ticks),
                PlacementBasis::Native { sample_rate, .. } => {
                    let (a, b) = tempo.frame_ratio(sample_rate)?;
                    whole_ticks_to_frames(ticks, a, b)
                }
            }
        };
        let mut candidate = at_grid(k)?;
        // A rounded grid frame can lie on either side of its exact grid point.
        // If that point maps back to the baseline, skip it once. Adjacent
        // sixteenth-note points are at least 400 frames apart in this domain.
        let strict = |value: i128| {
            if direction > 0 {
                value > baseline
            } else {
                value < baseline
            }
        };
        if !strict(candidate) {
            candidate = at_grid(k.checked_add(i128::from(direction)).ok_or_else(overflow)?)?;
        }
        if !strict(candidate) {
            return Err("Arrangement nudge has no strict neighboring placement".into());
        }
        candidate
    } else {
        baseline
            .checked_add(i128::from(direction))
            .ok_or_else(overflow)?
    };
    finish_placement(basis, candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempo(bpm: f64) -> TempoRatio {
        TempoRatio::from_bpm(bpm).unwrap()
    }
    fn native(frame: i64, count: u64, rate: u32) -> PlacementBasis {
        PlacementBasis::Native {
            position_frame: frame,
            frame_count: count,
            sample_rate: rate,
        }
    }
    fn legacy(tick: u64) -> PlacementBasis {
        PlacementBasis::Legacy {
            start_tick: tick,
            duration_seconds: 1.0,
        }
    }

    #[test]
    fn tempo_and_input_domains_fail_closed() {
        for invalid in [f64::NAN, f64::INFINITY, 0.0, 29.999, 300.001] {
            assert!(TempoRatio::from_bpm(invalid).is_err());
        }
        for bpm in [30.0, 119.25, 300.0, f64::from_bits(119.25f64.to_bits() + 1)] {
            assert_eq!(tempo(bpm).bpm().to_bits(), bpm.to_bits());
        }
        let t = tempo(120.0);
        for basis in [
            native(0, 0, 48_000),
            native(i64::MAX, 1, 48_000),
            native(0, 1, 7_999),
            native(0, u64::MAX, 48_000),
            legacy(1_000_000_001),
        ] {
            assert!(drag_candidate(basis, TickFp(0), &t, Snap::Off).is_err());
        }
        assert!(
            project_interval(
                ClipTiming::Legacy {
                    start_tick: 0,
                    duration_seconds: f64::NAN
                },
                &t
            )
            .is_err()
        );
    }

    #[test]
    fn native_projection_is_outward_and_keeps_negative_time() {
        let t = tempo(120.0);
        let interval = project_interval(
            ClipTiming::Native {
                position_frame: -1,
                frame_count: 2,
                sample_rate: 44_100,
            },
            &t,
        )
        .unwrap();
        assert_eq!(interval.start, TickFp(-43_537_415));
        assert_eq!(interval.end, TickFp(43_537_415));
        let exact = project_interval(
            ClipTiming::Native {
                position_frame: -24_000,
                frame_count: 48_000,
                sample_rate: 48_000,
            },
            &t,
        )
        .unwrap();
        assert_eq!(
            exact,
            AxisInterval {
                start: TickFp(-960 * TICK_SCALE),
                end: TickFp(960 * TICK_SCALE)
            }
        );
        let before = project_interval(
            ClipTiming::Native {
                position_frame: -48_000,
                frame_count: 24_000,
                sample_rate: 48_000,
            },
            &t,
        )
        .unwrap();
        assert!(before.end.0 < 0);
    }

    #[test]
    fn extreme_projection_and_local_pixels_preserve_adjacent_frames() {
        let t = tempo(300.0);
        for position in [i64::MIN, i64::MAX - 2, (1i64 << 53) + 1] {
            let interval = project_interval(
                ClipTiming::Native {
                    position_frame: position,
                    frame_count: 1,
                    sample_rate: 8_000,
                },
                &t,
            )
            .unwrap();
            assert_eq!(interval.end.0 - interval.start.0, 600_000_000);
            let view = Viewport::new(interval.start, TickFp(TICK_SCALE)).unwrap();
            let pixels = view.project_interval(interval, 100.0).unwrap().unwrap();
            assert_eq!(pixels.start, 0.0);
            assert_eq!(pixels.end, 60.0);
        }
        for frame in [i64::MIN, i64::MAX] {
            let point = project_point(frame, 8_000, &t).unwrap();
            assert!(in_axis(point.0));
            let view = Viewport::new(TickFp(point.0 - 1), TickFp(2)).unwrap();
            assert_eq!(
                view.project_interval(
                    AxisInterval {
                        start: point,
                        end: point
                    },
                    100.0
                )
                .unwrap(),
                Some(PixelInterval {
                    start: 50.0,
                    end: 50.0
                })
            );
        }
    }

    #[test]
    fn viewport_bounds_clipping_and_markers_are_checked() {
        let view = Viewport::new(TickFp(-10), TickFp(20)).unwrap();
        assert_eq!(
            view.project_interval(
                AxisInterval {
                    start: TickFp(-20),
                    end: TickFp(20)
                },
                100.0
            )
            .unwrap(),
            Some(PixelInterval {
                start: 0.0,
                end: 100.0
            })
        );
        assert_eq!(
            view.project_interval(
                AxisInterval {
                    start: TickFp(-20),
                    end: TickFp(-10)
                },
                100.0
            )
            .unwrap(),
            None
        );
        assert_eq!(
            view.project_interval(
                AxisInterval {
                    start: TickFp(10),
                    end: TickFp(10)
                },
                100.0
            )
            .unwrap(),
            Some(PixelInterval {
                start: 100.0,
                end: 100.0
            })
        );
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY, MAX_VIEWPORT_WIDTH + 1.0] {
            assert!(
                view.project_interval(
                    AxisInterval {
                        start: TickFp(0),
                        end: TickFp(1)
                    },
                    width
                )
                .is_err()
            );
        }
        assert!(Viewport::new(TickFp(0), TickFp(0)).is_err());
        assert!(Viewport::new(AXIS_LIMIT, TickFp(1)).is_err());
        assert!(
            Viewport {
                origin: TickFp(i128::MAX),
                span: TickFp(1)
            }
            .project_interval(
                AxisInterval {
                    start: TickFp(0),
                    end: TickFp(1)
                },
                10.0
            )
            .is_err()
        );
        assert!(
            view.project_interval(
                AxisInterval {
                    start: TickFp(1),
                    end: TickFp(-1)
                },
                10.0
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_start_is_exact_and_duration_uses_pinned_binary_estimate() {
        let t = tempo(119.25);
        let start_tick = 1_000_000_000;
        let result = project_interval(
            ClipTiming::Legacy {
                start_tick,
                duration_seconds: 0.123456789,
            },
            &t,
        )
        .unwrap();
        assert_eq!(result.start.0, i128::from(start_tick) * TICK_SCALE);
        let expected = duration_fp_ceil(((0.123456789 * t.bpm()) * 960.0) / 60.0).unwrap();
        assert_eq!(result.end.0 - result.start.0, expected);
        let exact = project_interval(
            ClipTiming::Legacy {
                start_tick: 3,
                duration_seconds: 0.5,
            },
            &tempo(120.0),
        )
        .unwrap();
        assert_eq!(exact.end, TickFp(963 * TICK_SCALE));
    }

    #[test]
    fn signed_deltas_larger_than_i64_reach_both_durable_ends() {
        // At 125 BPM/8000 Hz, one frame is exactly one quarter tick.
        let t = tempo(125.0);
        let low = i64::MIN;
        let high = i64::MAX - 1;
        let delta_frames = i128::from(high) - i128::from(low);
        let delta = TickFp(delta_frames * (TICK_SCALE / 4));
        assert!(delta_frames > i128::from(i64::MAX));
        assert_eq!(
            drag_candidate(native(low, 1, 8_000), delta, &t, Snap::Off).unwrap(),
            PlacementValue::NativeFrame(high)
        );
        assert_eq!(
            drag_candidate(native(high, 1, 8_000), TickFp(-delta.0), &t, Snap::Off).unwrap(),
            PlacementValue::NativeFrame(low)
        );
        assert!(
            drag_candidate(
                native(high, 1, 8_000),
                TickFp(TICK_SCALE / 4),
                &t,
                Snap::Off
            )
            .is_err()
        );
        // The widest legal frame move at 300 BPM exceeds the absolute axis
        // radius, but lies within its full signed span and remains admissible.
        let wide = TickFp(delta_frames * (TICK_SCALE * 3 / 5));
        assert!(wide.0 > AXIS_LIMIT.0);
        assert_eq!(
            drag_candidate(native(low, 1, 8_000), wide, &tempo(300.0), Snap::Off).unwrap(),
            PlacementValue::NativeFrame(high)
        );
        assert_eq!(
            drag_candidate(
                native(high, 1, 8_000),
                TickFp(-wide.0),
                &tempo(300.0),
                Snap::Off
            )
            .unwrap(),
            PlacementValue::NativeFrame(low)
        );
    }

    #[test]
    fn displacement_rounds_before_absolute_snap_with_signed_ties() {
        let t = tempo(125.0);
        // 480 ticks is exactly half a beat; first round a 0.5-frame move
        // away from zero, crossing that midpoint in the requested direction.
        assert_eq!(
            drag_candidate(
                native(1_919, 1, 8_000),
                TickFp(TICK_SCALE / 8),
                &t,
                Snap::Beat
            )
            .unwrap(),
            PlacementValue::NativeFrame(3_840)
        );
        assert_eq!(
            drag_candidate(
                native(-1_919, 1, 8_000),
                TickFp(-TICK_SCALE / 8),
                &t,
                Snap::Beat
            )
            .unwrap(),
            PlacementValue::NativeFrame(-3_840)
        );
        assert_eq!(
            drag_candidate(legacy(479), TickFp(TICK_SCALE / 2), &t, Snap::Beat).unwrap(),
            PlacementValue::LegacyTick(960)
        );
        assert_eq!(
            drag_candidate(native(0, 1, 8_000), TickFp(TICK_SCALE / 8), &t, Snap::Off).unwrap(),
            PlacementValue::NativeFrame(1)
        );
        assert_eq!(
            drag_candidate(native(0, 1, 8_000), TickFp(-TICK_SCALE / 8), &t, Snap::Off).unwrap(),
            PlacementValue::NativeFrame(-1)
        );
    }

    #[test]
    fn zero_and_subunit_moves_return_unchanged_typed_placement() {
        let t = tempo(119.25);
        assert_eq!(
            drag_candidate(native(12_345, 500, 48_000), TickFp(0), &t, Snap::Beat).unwrap(),
            PlacementValue::NativeFrame(12_345)
        );
        assert_eq!(
            drag_candidate(legacy(123), TickFp(0), &t, Snap::SixteenthNote).unwrap(),
            PlacementValue::LegacyTick(123)
        );
        assert_eq!(
            drag_candidate(legacy(123), TickFp(TICK_SCALE / 2 - 1), &t, Snap::Off).unwrap(),
            PlacementValue::LegacyTick(123)
        );
        for snap in [Snap::Beat, Snap::SixteenthNote] {
            for delta in [TickFp(1), TickFp(-1)] {
                assert_eq!(
                    drag_candidate(legacy(123), delta, &t, snap).unwrap(),
                    PlacementValue::LegacyTick(123)
                );
                assert_eq!(
                    drag_candidate(native(12_345, 500, 48_000), delta, &t, snap).unwrap(),
                    PlacementValue::NativeFrame(12_345)
                );
            }
        }
        assert!(drag_candidate(legacy(0), TickFp(-TICK_SCALE), &t, Snap::Off).is_err());
        assert!(drag_candidate(legacy(1_000_000_000), TickFp(TICK_SCALE), &t, Snap::Off).is_err());
        assert!(drag_candidate(legacy(0), TickFp(i128::MIN), &t, Snap::Off).is_err());
    }

    #[test]
    fn nudges_use_exact_units_or_strict_neighboring_grid() {
        let t = tempo(120.0);
        assert_eq!(
            nudge_candidate(native(-1, 100, 48_000), 1, &t, Snap::Off).unwrap(),
            PlacementValue::NativeFrame(0)
        );
        assert_eq!(
            nudge_candidate(legacy(3), -1, &t, Snap::Off).unwrap(),
            PlacementValue::LegacyTick(2)
        );
        assert_eq!(
            nudge_candidate(native(24_000, 1, 48_000), 1, &t, Snap::Beat).unwrap(),
            PlacementValue::NativeFrame(48_000)
        );
        assert_eq!(
            nudge_candidate(native(24_000, 1, 48_000), -1, &t, Snap::Beat).unwrap(),
            PlacementValue::NativeFrame(0)
        );
        assert_eq!(
            nudge_candidate(native(-24_001, 1, 48_000), 1, &t, Snap::Beat).unwrap(),
            PlacementValue::NativeFrame(-24_000)
        );
        assert_eq!(
            nudge_candidate(native(-24_000, 1, 48_000), -1, &t, Snap::Beat).unwrap(),
            PlacementValue::NativeFrame(-48_000)
        );
        assert_eq!(
            nudge_candidate(legacy(960), 1, &t, Snap::SixteenthNote).unwrap(),
            PlacementValue::LegacyTick(1_200)
        );
        assert!(nudge_candidate(legacy(0), -1, &t, Snap::Beat).is_err());
        assert!(nudge_candidate(native(i64::MAX - 1, 1, 48_000), 1, &t, Snap::Off).is_err());
        assert!(nudge_candidate(legacy(0), 0, &t, Snap::Off).is_err());
    }

    #[test]
    fn binary_fractional_tempos_and_rates_keep_snapping_deterministic() {
        for bpm in [30.0, 119.25, f64::from_bits(119.25f64.to_bits() + 1), 300.0] {
            let t = tempo(bpm);
            for rate in [8_000, 44_100, 48_000, 96_000, 768_000] {
                let (a, b) = t.frame_ratio(rate).unwrap();
                for tick in [-1_920i128, -960, -240, 240, 960, 1_920] {
                    let frame = whole_ticks_to_frames(tick, a, b).unwrap();
                    let frame = i64::try_from(frame).unwrap();
                    let value = snap_position(
                        native(frame, 1, rate),
                        i128::from(frame),
                        &t,
                        Snap::SixteenthNote,
                    )
                    .unwrap();
                    assert_eq!(value, i128::from(frame));
                }
                let one = i64::try_from(whole_ticks_to_frames(960, a, b).unwrap()).unwrap();
                let two = i64::try_from(whole_ticks_to_frames(1920, a, b).unwrap()).unwrap();
                for (position, direction, expected) in
                    [(one, -1, 0), (one, 1, two), (-one, -1, -two), (-one, 1, 0)]
                {
                    assert_eq!(
                        nudge_candidate(native(position, 1, rate), direction, &t, Snap::Beat)
                            .unwrap(),
                        PlacementValue::NativeFrame(expected)
                    );
                }
            }
        }
    }
}
