//! Window-local timeline geometry and placement values, never a wire protocol.
//! Fixed point is for display only; durable native positions remain exact frames.
use omniusong_core::DocumentClipAddress;

pub(super) const TICK_SCALE: i128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TickFp(pub i128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Viewport {
    pub origin: TickFp,
    pub span: TickFp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlacementValue {
    LegacyTick(u64),
    NativeFrame(i64),
}
impl PlacementValue {
    pub fn validate_for(self, address: &DocumentClipAddress) -> Result<(), String> {
        match (self, address) {
            (Self::LegacyTick(value), DocumentClipAddress::LegacyAudio { .. })
                if value <= 1_000_000_000 =>
            {
                Ok(())
            }
            (Self::NativeFrame(_), DocumentClipAddress::NativeAudio { .. }) => Ok(()),
            _ => Err("Arrangement placement has the wrong unit or exceeds the clip limit".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Snap {
    #[default]
    Off,
    Beat,
    SixteenthNote,
}
impl Snap {
    pub fn ticks(self) -> Option<u64> {
        match self {
            Self::Off => None,
            Self::Beat => Some(960),
            Self::SixteenthNote => Some(240),
        }
    }
}
