//! Read-only source overview contracts, independent of native audio control IPC.
use omniusong_core::{
    AudioSourceWindow, DocumentClipAddress,
    contracts::{ProjectId, Validate},
    native::{AudioFades, TakeReference},
};
use serde::{Deserialize, Serialize};

pub const MAX_WAVEFORM_BINS: usize = 1024;
pub const MAX_WAVEFORM_CHANNELS: usize = 32;
pub const MAX_WAVEFORM_SAMPLES: u64 = 32 * 1024 * 1024;
pub const MAX_WAVEFORM_TAKE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_WAVEFORM_CHUNKS: u64 = 65_536;
pub const MAX_WAVEFORM_SUMMARY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaveformSourceKey {
    LegacyWav {
        asset_id: String,
        source_asset_id: Option<String>,
    },
    NativeAudio {
        source: TakeReference,
        sample_rate: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveformRequest {
    pub project_id: String,
    pub expected_revision: u64,
    pub address: DocumentClipAddress,
    pub source: WaveformSourceKey,
    pub bins: usize,
}
impl WaveformRequest {
    pub fn validate(&self) -> Result<(), String> {
        ProjectId(self.project_id.clone())
            .validate()
            .map_err(|e| e.to_string())?;
        self.address.validate().map_err(|e| e.to_string())?;
        if self.expected_revision > i64::MAX as u64 || !(1..=MAX_WAVEFORM_BINS).contains(&self.bins)
        {
            return Err("Invalid waveform revision or resolution".into());
        }
        match (&self.address, &self.source) {
            (
                DocumentClipAddress::LegacyAudio { .. },
                WaveformSourceKey::LegacyWav {
                    asset_id,
                    source_asset_id,
                },
            ) => {
                if !omniusong_core::valid_asset_id(asset_id)
                    || source_asset_id
                        .as_ref()
                        .is_some_and(|id| !omniusong_core::valid_asset_id(id))
                {
                    return Err("Invalid waveform asset identity".into());
                }
            }
            (
                DocumentClipAddress::NativeAudio { .. },
                WaveformSourceKey::NativeAudio {
                    source,
                    sample_rate,
                },
            ) => {
                source.validate().map_err(|e| e.to_string())?;
                if !(8000..=768000).contains(sample_rate) {
                    return Err("Invalid waveform source sample rate".into());
                }
            }
            _ => return Err("Waveform source and clip representation differ".into()),
        }
        Ok(())
    }
}

/// Baseline only. Editing preserves exact Number text in ClipDraft; these cached
/// floating-point legacy projections are explicitly renderer-rounded estimates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaveformSettings {
    Legacy {
        declared_duration_seconds: f64,
        start_tick: u64,
        trim_start_seconds: Option<f64>,
        trim_end_seconds: Option<f64>,
        fade_in_seconds: Option<f64>,
        fade_out_seconds: Option<f64>,
    },
    Native {
        window: AudioSourceWindow,
        position_frame: i64,
        fades: Option<AudioFades>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveformBin {
    pub start_frame: u64,
    pub frame_count: u64,
    pub content_frames: u64,
    pub gap_frames: u64,
    pub min: Option<f32>,
    pub max: Option<f32>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveformData {
    pub request: WaveformRequest,
    /// The physical owned object hash for native audio; asset hash stem for WAV.
    pub authenticated_sha256: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub source_start_frame: u64,
    pub frame_count: u64,
    pub unknown_tail: bool,
    pub channel_bins: Vec<Vec<WaveformBin>>,
    pub saved: WaveformSettings,
    pub original: Option<WaveformSettings>,
}
impl WaveformData {
    pub fn validate(&self) -> Result<(), String> {
        self.request.validate()?;
        if !(8000..=768000).contains(&self.sample_rate)
            || self.channels == 0
            || self.channels as usize > MAX_WAVEFORM_CHANNELS
            || self.channel_bins.len() != self.channels as usize
            || self.frame_count == 0
            || self
                .source_start_frame
                .checked_add(self.frame_count)
                .is_none()
        {
            return Err("Invalid waveform source extent/format".into());
        }
        if self.authenticated_sha256.len() != 64
            || !self
                .authenticated_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid authenticated waveform identity".into());
        }
        let source_end = self.source_start_frame + self.frame_count;
        match &self.request.source {
            WaveformSourceKey::LegacyWav { asset_id, .. } => {
                if self.authenticated_sha256 != asset_id[..64]
                    || self.source_start_frame != 0
                    || self.unknown_tail
                {
                    return Err(
                        "WAV waveform differs from its authenticated source identity/extent".into(),
                    );
                }
            }
            WaveformSourceKey::NativeAudio { sample_rate, .. } => {
                if self.sample_rate != *sample_rate
                    || self.channels > 16
                    || source_end > i64::MAX as u64
                {
                    return Err("Native waveform differs from its source format/extent".into());
                }
            }
        }
        self.validate_settings(&self.saved)?;
        if let Some(original) = &self.original {
            self.validate_settings(original)?;
        }
        let summary_bytes = self
            .channel_bins
            .iter()
            .try_fold(
                std::mem::size_of::<Self>()
                    + self.channel_bins.capacity() * std::mem::size_of::<Vec<WaveformBin>>(),
                |bytes, bins| {
                    bytes.checked_add(
                        bins.capacity()
                            .checked_mul(std::mem::size_of::<WaveformBin>())?,
                    )
                },
            )
            .ok_or("Waveform summary size overflow")?;
        let request_bytes = serde_json::to_vec(&self.request)
            .map_err(|error| error.to_string())?
            .len();
        if summary_bytes
            .checked_add(request_bytes)
            .is_none_or(|n| n > MAX_WAVEFORM_SUMMARY_BYTES)
        {
            return Err("Waveform summary exceeds its memory budget".into());
        }
        let count = (self.request.bins as u64).min(self.frame_count) as usize;
        let mut content_frames = 0u64;
        for (channel, bins) in self.channel_bins.iter().enumerate() {
            if bins.len() != count {
                return Err("Incomplete waveform channel bins".into());
            }
            for (i, bin) in bins.iter().enumerate() {
                let start = self.source_start_frame
                    + (self.frame_count as u128 * i as u128 / count as u128) as u64;
                let end = self.source_start_frame
                    + (self.frame_count as u128 * (i + 1) as u128 / count as u128) as u64;
                if bin.start_frame != start
                    || bin.frame_count != end - start
                    || bin.content_frames.checked_add(bin.gap_frames) != Some(bin.frame_count)
                {
                    return Err(
                        "Waveform bin coverage differs from its exact source interval".into(),
                    );
                }
                if channel > 0
                    && (bin.content_frames != self.channel_bins[0][i].content_frames
                        || bin.gap_frames != self.channel_bins[0][i].gap_frames)
                {
                    return Err("Waveform channel coverage differs".into());
                }
                if channel == 0 {
                    content_frames = content_frames
                        .checked_add(bin.content_frames)
                        .ok_or("Waveform sample work overflow")?;
                    if matches!(self.request.source, WaveformSourceKey::LegacyWav { .. })
                        && bin.gap_frames != 0
                    {
                        return Err("WAV waveform cannot invent gap intervals".into());
                    }
                }
                match (bin.content_frames, bin.min, bin.max) {
                    (0, None, None) => {}
                    (_, Some(lo), Some(hi))
                        if bin.content_frames > 0
                            && lo.is_finite()
                            && hi.is_finite()
                            && lo <= hi => {}
                    _ => return Err("Invalid waveform extrema; gaps are not silence".into()),
                }
            }
        }
        if content_frames
            .checked_mul(u64::from(self.channels))
            .is_none_or(|n| n > MAX_WAVEFORM_SAMPLES)
        {
            return Err("Waveform exceeds its content-sample budget".into());
        }
        Ok(())
    }

    fn validate_settings(&self, settings: &WaveformSettings) -> Result<(), String> {
        match (&self.request.source, settings) {
            (
                WaveformSourceKey::LegacyWav { .. },
                WaveformSettings::Legacy {
                    declared_duration_seconds,
                    start_tick,
                    trim_start_seconds,
                    trim_end_seconds,
                    fade_in_seconds,
                    fade_out_seconds,
                },
            ) => {
                let duration = *declared_duration_seconds;
                let start = trim_start_seconds.unwrap_or(0.0);
                let end = trim_end_seconds.unwrap_or(duration);
                let fade_in = fade_in_seconds.unwrap_or(0.0);
                let fade_out = fade_out_seconds.unwrap_or(0.0);
                if !duration.is_finite()
                    || !(0.001..=86400.0).contains(&duration)
                    || *start_tick > 1_000_000_000
                    || ![start, end, fade_in, fade_out]
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 86400.0)
                    || start >= end
                    || end > duration
                    || end - start + 1e-9 < 0.001
                    || fade_in + fade_out > end - start + 1e-9
                {
                    return Err("Invalid cached legacy waveform settings".into());
                }
            }
            (
                WaveformSourceKey::NativeAudio { .. },
                WaveformSettings::Native {
                    window,
                    position_frame,
                    fades,
                },
            ) => {
                let end = window
                    .start_frame
                    .checked_add(window.frame_count)
                    .ok_or("Cached native source extent overflow")?;
                let count = i64::try_from(window.frame_count)
                    .map_err(|_| "Cached native frame count overflow")?;
                if window.frame_count == 0
                    || window.start_frame < self.source_start_frame
                    || end > self.source_start_frame + self.frame_count
                    || position_frame.checked_add(count).is_none()
                    || fades.as_ref().is_some_and(|f| {
                        f.in_frames
                            .checked_add(f.out_frames)
                            .is_none_or(|sum| sum > window.frame_count)
                    })
                {
                    return Err("Invalid cached native waveform settings".into());
                }
            }
            _ => return Err("Waveform settings and source representation differ".into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::contracts::TakeId;
    fn data(native: bool) -> WaveformData {
        let sha = "a".repeat(64);
        let request = WaveformRequest {
            project_id: "9e66aab1-2977-47ae-ad6d-de572c481dbe".into(),
            expected_revision: 3,
            address: if native {
                DocumentClipAddress::NativeAudio {
                    track_id: "track".into(),
                    clip_id: "clip".into(),
                }
            } else {
                DocumentClipAddress::LegacyAudio {
                    track_id: "track".into(),
                }
            },
            source: if native {
                WaveformSourceKey::NativeAudio {
                    source: TakeReference {
                        take_id: TakeId("1a173ab4-30ba-49ac-8878-d254bece1b0d".into()),
                        stream_id: "audio".into(),
                        manifest_sha256: sha.clone(),
                    },
                    sample_rate: 48000,
                }
            } else {
                WaveformSourceKey::LegacyWav {
                    asset_id: format!("{sha}.wav"),
                    source_asset_id: None,
                }
            },
            bins: 1,
        };
        let start = if native { 9_007_199_254_740_993 } else { 0 };
        WaveformData {
            request,
            authenticated_sha256: sha,
            sample_rate: 48000,
            channels: 1,
            source_start_frame: start,
            frame_count: 48000,
            unknown_tail: false,
            channel_bins: vec![vec![WaveformBin {
                start_frame: start,
                frame_count: 48000,
                content_frames: 48000,
                gap_frames: 0,
                min: Some(0.2),
                max: Some(0.7),
            }]],
            saved: if native {
                WaveformSettings::Native {
                    window: AudioSourceWindow {
                        start_frame: start,
                        frame_count: 48000,
                    },
                    position_frame: -200,
                    fades: None,
                }
            } else {
                WaveformSettings::Legacy {
                    declared_duration_seconds: 1.0,
                    start_tick: 3,
                    trim_start_seconds: None,
                    trim_end_seconds: None,
                    fade_in_seconds: None,
                    fade_out_seconds: None,
                }
            },
            original: None,
        }
    }
    #[test]
    fn native_large_origin_is_exact_and_round_trips() {
        let source = data(true);
        source.validate().unwrap();
        let loaded: WaveformData =
            serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded, source);
    }
    #[test]
    fn known_gap_is_not_pcm_silence_and_does_not_spend_content_budget() {
        let mut source = data(true);
        source.frame_count = MAX_WAVEFORM_SAMPLES * 100;
        let bin = &mut source.channel_bins[0][0];
        bin.frame_count = source.frame_count;
        bin.content_frames = 0;
        bin.gap_frames = source.frame_count;
        bin.min = None;
        bin.max = None;
        source.validate().unwrap();
        source.channel_bins[0][0].min = Some(0.0);
        source.channel_bins[0][0].max = Some(0.0);
        assert!(
            source
                .validate()
                .unwrap_err()
                .contains("gaps are not silence")
        );
    }
    #[test]
    fn rejects_identity_format_coverage_and_settings_corruption() {
        let mut cases = vec![];
        let mut s = data(false);
        s.authenticated_sha256 = "b".repeat(64);
        cases.push(s);
        let mut s = data(false);
        s.unknown_tail = true;
        cases.push(s);
        let mut s = data(false);
        s.channel_bins[0][0].gap_frames = 1;
        s.channel_bins[0][0].content_frames -= 1;
        cases.push(s);
        let mut s = data(true);
        s.sample_rate = 44100;
        cases.push(s);
        let mut s = data(true);
        s.channel_bins[0][0].start_frame -= 1;
        cases.push(s);
        let mut s = data(true);
        s.channel_bins[0][0].min = Some(f32::NAN);
        cases.push(s);
        let mut s = data(true);
        if let WaveformSettings::Native { position_frame, .. } = &mut s.saved {
            *position_frame = i64::MAX;
        }
        cases.push(s);
        let mut s = data(true);
        if let WaveformSettings::Native { window, .. } = &mut s.saved {
            window.start_frame -= 1;
        }
        cases.push(s);
        let mut s = data(true);
        if let WaveformSettings::Native { fades, .. } = &mut s.saved {
            *fades = Some(AudioFades {
                in_frames: 30000,
                out_frames: 30000,
                curve: omniusong_core::native::FadeCurve::Linear,
            });
        }
        cases.push(s);
        let mut s = data(false);
        if let WaveformSettings::Legacy {
            trim_end_seconds, ..
        } = &mut s.saved
        {
            *trim_end_seconds = Some(1.0 + 5e-10);
        }
        cases.push(s);
        let mut s = data(false);
        s.original = Some(data(true).saved);
        cases.push(s);
        for (i, case) in cases.iter().enumerate() {
            assert!(case.validate().is_err(), "corruption {i} admitted");
        }
    }
    #[test]
    fn wav_declared_duration_mismatch_remains_inspectable() {
        let mut source = data(false);
        if let WaveformSettings::Legacy {
            declared_duration_seconds,
            ..
        } = &mut source.saved
        {
            *declared_duration_seconds = 2.0;
        }
        source.validate().unwrap(); // UI must explain that its overlay is outside actual decoded audio.
    }
    #[test]
    fn permits_32_wav_channels_but_never_silently_drops_native_channels() {
        let mut wav = data(false);
        wav.channels = 32;
        wav.channel_bins = vec![wav.channel_bins[0].clone(); 32];
        wav.validate().unwrap();
        let mut native = data(true);
        native.channels = 17;
        native.channel_bins = vec![native.channel_bins[0].clone(); 17];
        assert!(native.validate().is_err());
    }
    #[test]
    fn rejects_sample_work_and_actual_summary_capacity_overages() {
        let mut source = data(false);
        source.frame_count = MAX_WAVEFORM_SAMPLES + 1;
        let bin = &mut source.channel_bins[0][0];
        bin.frame_count = source.frame_count;
        bin.content_frames = source.frame_count;
        assert!(source.validate().unwrap_err().contains("content-sample"));
        let mut source = data(false);
        source.channel_bins[0]
            .reserve_exact(MAX_WAVEFORM_SUMMARY_BYTES / std::mem::size_of::<WaveformBin>() + 1);
        assert!(source.validate().unwrap_err().contains("memory budget"));
    }
}
