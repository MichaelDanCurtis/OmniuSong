//! Bounded offline preparation. Procedural audition of notes is not AI inference.
use crate::realtime::MAX_CHANNELS;
use omniusong_core::{
    ProjectDocument,
    native::{FadeCurve, RecordingPayload, TakeClip, TakeRange},
};
use omniusong_recording::{
    ChunkKind, PcmEncoding, PcmFormat, PcmLayout, RecordingError, SourceRange, StreamKind,
};
use omniusong_store::NativeStore;
use serde_json::Value;

pub const PREPARE_BUDGET: usize = 64 * 1024 * 1024;
pub const MAX_SECONDS: u64 = 120;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub data_offset: usize,
    pub data_bytes: usize,
    pub bits: u16,
    pub float: bool,
}
fn u16le(b: &[u8], p: usize) -> Result<u16, String> {
    let v = b.get(p..p + 2).ok_or("Truncated WAV")?;
    Ok(u16::from_le_bytes([v[0], v[1]]))
}
fn u32le(b: &[u8], p: usize) -> Result<u32, String> {
    let v = b.get(p..p + 4).ok_or("Truncated WAV")?;
    Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}
pub fn inspect_wav(bytes: &[u8]) -> Result<WavInfo, String> {
    inspect_wav_with_channel_limit(bytes, MAX_CHANNELS)
}
pub(crate) fn inspect_wav_with_channel_limit(
    bytes: &[u8],
    max_channels: usize,
) -> Result<WavInfo, String> {
    if bytes.len() > 30 * 1024 * 1024
        || bytes.get(..4) != Some(b"RIFF")
        || bytes.get(8..12) != Some(b"WAVE")
    {
        return Err("Only bounded RIFF/WAVE PCM16/24/32 or float32 assets are supported".into());
    }
    let total = (u32le(bytes, 4)? as usize)
        .checked_add(8)
        .ok_or("WAV size overflow")?;
    if total != bytes.len() {
        return Err("WAV RIFF length differs from original bytes".into());
    }
    let (mut pos, mut fmt, mut data) = (12, None, None);
    while pos < total {
        let id = bytes.get(pos..pos + 4).ok_or("Truncated WAV chunk")?;
        let n = u32le(bytes, pos + 4)? as usize;
        let start = pos.checked_add(8).ok_or("WAV overflow")?;
        let end = start
            .checked_add(n)
            .filter(|e| *e <= total)
            .ok_or("Truncated WAV chunk payload")?;
        if id == b"fmt " {
            if fmt.is_some() || n < 16 {
                return Err("Invalid/duplicate WAV format".into());
            }
            let tag = u16le(bytes, start)?;
            let channels = u16le(bytes, start + 2)?;
            let rate = u32le(bytes, start + 4)?;
            let bits = u16le(bytes, start + 14)?;
            let align = u16le(bytes, start + 12)?;
            if channels == 0
                || channels as usize > max_channels
                || !(8000..=768000).contains(&rate)
                || !((tag == 1 && matches!(bits, 16 | 24 | 32)) || (tag == 3 && bits == 32))
                || align as usize != channels as usize * (bits as usize / 8)
                || u32le(bytes, start + 8)? as u64 != rate as u64 * align as u64
            {
                return Err("Unsupported or inconsistent WAV format (extensible/compressed codecs are pending)".into());
            }
            fmt = Some((rate, channels, bits, tag == 3));
        } else if id == b"data" {
            if data.is_some() {
                return Err("Multiple WAV data chunks unsupported".into());
            }
            data = Some((start, n));
        }
        pos = end
            .checked_add(n % 2)
            .filter(|p| *p <= total)
            .ok_or("Missing WAV chunk padding")?;
    }
    let (rate, channels, bits, float) = fmt.ok_or("WAV has no format")?;
    let (offset, n) = data.ok_or("WAV has no audio data")?;
    let align = channels as usize * bits as usize / 8;
    if n == 0 || !n.is_multiple_of(align) {
        return Err("Invalid WAV audio length".into());
    }
    Ok(WavInfo {
        sample_rate: rate,
        channels,
        frames: (n / align) as u64,
        data_offset: offset,
        data_bytes: n,
        bits,
        float,
    })
}
fn signed_sample(bytes: &[u8], bits: u16, float: bool) -> Result<f32, String> {
    let v = match (bits, float) {
        (16, false) => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0,
        (24, false) => {
            let n = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
            ((n << 8) >> 8) as f32 / 8388608.0
        }
        (32, false) => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f32 / 2147483648.0,
        (32, true) => f32::from_le_bytes(bytes[..4].try_into().unwrap()),
        _ => return Err("Unsupported PCM encoding".into()),
    };
    if !v.is_finite() {
        return Err("Nonfinite original PCM".into());
    }
    Ok(v)
}
pub fn wav_sample(bytes: &[u8], info: &WavInfo, frame: u64, channel: u16) -> Result<f32, String> {
    wav_sample_with_channel_limit(bytes, info, frame, channel, MAX_CHANNELS)
}
pub(crate) fn wav_sample_with_channel_limit(
    bytes: &[u8],
    info: &WavInfo,
    frame: u64,
    channel: u16,
    max_channels: usize,
) -> Result<f32, String> {
    if frame >= info.frames
        || channel >= info.channels
        || info.channels == 0
        || info.channels as usize > max_channels
        || !matches!(
            (info.bits, info.float),
            (16, false) | (24, false) | (32, false) | (32, true)
        )
    {
        return Err("WAV sample/format out of range".into());
    }
    let width = info.bits as usize / 8;
    let frames = usize::try_from(info.frames).map_err(|_| "WAV frame extent overflow")?;
    let expected = frames
        .checked_mul(info.channels as usize)
        .and_then(|n| n.checked_mul(width))
        .ok_or("WAV extent overflow")?;
    if expected != info.data_bytes
        || info
            .data_offset
            .checked_add(info.data_bytes)
            .is_none_or(|n| n > bytes.len())
    {
        return Err("Invalid WAV data extent".into());
    }
    let index = usize::try_from(frame)
        .ok()
        .and_then(|n| n.checked_mul(info.channels as usize))
        .and_then(|n| n.checked_add(channel as usize))
        .and_then(|n| n.checked_mul(width))
        .ok_or("WAV sample offset overflow")?;
    let offset = info
        .data_offset
        .checked_add(index)
        .ok_or("WAV sample offset overflow")?;
    let end = offset
        .checked_add(width)
        .ok_or("WAV sample extent overflow")?;
    signed_sample(
        bytes.get(offset..end).ok_or("Truncated original PCM")?,
        info.bits,
        info.float,
    )
}
pub fn pcm_sample(
    bytes: &[u8],
    format: &PcmFormat,
    frames: u64,
    frame: u64,
    channel: u16,
) -> Result<f32, String> {
    pcm_sample_with_channel_limit(bytes, format, frames, frame, channel, MAX_CHANNELS)
}
pub(crate) fn pcm_sample_with_channel_limit(
    bytes: &[u8],
    format: &PcmFormat,
    frames: u64,
    frame: u64,
    channel: u16,
    max_channels: usize,
) -> Result<f32, String> {
    if format.channels == 0
        || format.channels as usize > max_channels
        || frame >= frames
        || channel >= format.channels
    {
        return Err("PCM sample out of range".into());
    }
    let width = format.encoding.bytes_per_sample() as usize;
    let frame_count = usize::try_from(frames).map_err(|_| "PCM frame extent overflow")?;
    let expected = frame_count
        .checked_mul(format.channels as usize)
        .and_then(|n| n.checked_mul(width))
        .ok_or("PCM length overflow")?;
    if bytes.len() != expected {
        return Err("PCM payload length mismatch".into());
    }
    let index = match format.layout {
        PcmLayout::Interleaved => frame as usize * format.channels as usize + channel as usize,
        PcmLayout::Planar => channel as usize * frame_count + frame as usize,
    };
    let bits = match format.encoding {
        PcmEncoding::S16Le => 16,
        PcmEncoding::S24Le => 24,
        PcmEncoding::F32Le => 32,
    };
    signed_sample(
        &bytes[index * width..(index + 1) * width],
        bits,
        format.encoding == PcmEncoding::F32Le,
    )
}
fn number(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}
fn tick_frame(tick: f64, tempo: f64, rate: u32) -> Result<u64, String> {
    let n = tick * 60.0 * rate as f64 / (tempo * 960.0);
    if !n.is_finite() || n < 0.0 || n > rate as f64 * MAX_SECONDS as f64 {
        return Err("Playback preparation exceeds 120 seconds".into());
    }
    Ok(n.round() as u64)
}
fn end_checked(frame: u64, rate: u32) -> Result<u64, String> {
    if frame > rate as u64 * MAX_SECONDS {
        return Err("Playback preparation exceeds 120 seconds".into());
    }
    Ok(frame)
}
fn channel(source: u16, target: u16, c: u16) -> Result<u16, String> {
    if source == target {
        Ok(c)
    } else if source == 1 {
        Ok(0)
    } else {
        Err("Channel conversion is unsupported; select matching channels or mono source".into())
    }
}
fn fade(index: u64, count: u64, input: u64, output: u64, equal: bool) -> f32 {
    let mut v = 1.0f64;
    if input > 0 && index < input {
        v *= index as f64 / input as f64;
    }
    let remain = count - 1 - index;
    if output > 0 && remain < output {
        v *= remain as f64 / output as f64;
    }
    if equal {
        (v * std::f64::consts::FRAC_PI_2).sin() as f32
    } else {
        v as f32
    }
}
fn native_payload(track: &Value) -> Result<RecordingPayload, String> {
    serde_json::from_value(track["recording"].clone()).map_err(|e| e.to_string())
}
/// Prepares one interleaved destination buffer. Source bytes are borrowed/decoded
/// directly: the checked budget includes the largest simultaneously retained source.
pub fn prepare(
    store: &NativeStore,
    document: &ProjectDocument,
    rate: u32,
    channels: u16,
) -> Result<Vec<f32>, String> {
    if !(8000..=768000).contains(&rate) || channels == 0 || channels as usize > MAX_CHANNELS {
        return Err("Unsupported output configuration".into());
    }
    let mut end = 0u64;
    let mut transient = 1024 * 1024 + 64 * 1024;
    for track in document.tracks() {
        if track["muted"] == true || number(track, "gain", 1.0) == 0.0 {
            continue;
        }
        let label = track["name"].as_str().unwrap_or("track");
        if track["kind"] == "recording" {
            for clip in native_payload(track)?.clips {
                clip.validate().map_err(|e| e.to_string())?;
                let TakeRange::AudioFrames { count, .. } = clip.range else {
                    return Err(format!(
                        "{label}: raw event clip {} has no implemented renderer",
                        clip.id
                    ));
                };
                if clip.position.sample_rate != rate {
                    return Err(format!(
                        "{label}: clip/output rate differs; resampling is pending"
                    ));
                }
                let finish = (clip.position.frame as i128 + count as i128).max(0) as u64;
                end = end.max(end_checked(finish, rate)?);
            }
            continue;
        }
        let notes = track["notes"].as_array().ok_or("Invalid note track")?;
        if !notes.is_empty()
            && !matches!(
                track["instrument"].as_str(),
                Some("keys" | "bass" | "pad" | "drums")
            )
        {
            return Err(format!(
                "{label}: active instrument requires an unimplemented sampler/voice renderer"
            ));
        }
        for note in notes {
            if note
                .get("expression")
                .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err(format!(
                    "{label}: note {} expression is not rendered",
                    note["id"]
                ));
            }
            let finish = number(note, "startTick", 0.0) + number(note, "durationTicks", 0.0);
            end = end.max(tick_frame(finish, document.tempo(), rate)?);
        }
        if let Some(id) = track.get("assetId").and_then(Value::as_str) {
            if track["kind"] != "audio" {
                return Err(format!(
                    "{label}: note-track asset requires an unimplemented sampler renderer"
                ));
            }
            if track.get("audio").is_none() {
                return Err(format!(
                    "{label}: asset {id} lacks explicit audio placement"
                ));
            }
            transient = 30 * 1024 * 1024;
            let a = &track["audio"];
            let source = (number(a, "trimStartSeconds", 0.0) * rate as f64).round() as u64;
            let source_end = (number(a, "trimEndSeconds", number(a, "durationSeconds", 0.0))
                * rate as f64)
                .round() as u64;
            let start = tick_frame(number(a, "startTick", 0.0), document.tempo(), rate)?;
            let count = source_end
                .checked_sub(source)
                .ok_or("Invalid legacy source window")?;
            end = end.max(end_checked(
                start.checked_add(count).ok_or("Audio placement overflow")?,
                rate,
            )?);
        } else if track.get("audio").is_some() {
            return Err(format!("{label}: active audio has no original asset"));
        }
    }
    if end == 0 {
        return Err("Project has no audible supported content".into());
    }
    let bytes = (end as usize)
        .checked_mul(channels as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("Playback buffer overflow")?;
    if bytes
        .checked_add(transient)
        .is_none_or(|n| n > PREPARE_BUDGET)
    {
        return Err("Prepared PCM and concurrent source bytes exceed 64 MiB".into());
    }
    let mut out = vec![0.0f32; bytes / 4];
    for track in document.tracks() {
        if track["muted"] == true {
            continue;
        }
        let gain = number(track, "gain", 1.0) as f32;
        if gain == 0.0 {
            continue;
        }
        if track["kind"] == "recording" {
            for clip in native_payload(track)?.clips {
                mix_take(store, &clip, rate, channels, gain, &mut out)?;
            }
            continue;
        }
        for note in track["notes"].as_array().ok_or("Invalid notes")? {
            let start = tick_frame(number(note, "startTick", 0.0), document.tempo(), rate)?;
            let finish = tick_frame(
                number(note, "startTick", 0.0) + number(note, "durationTicks", 0.0),
                document.tempo(),
                rate,
            )?;
            let count = finish.saturating_sub(start);
            if count == 0 {
                continue;
            }
            let pitch = number(note, "pitch", 60.0);
            let frequency = 440.0 * 2.0f64.powf((pitch - 69.0) / 12.0);
            let velocity = number(note, "velocity16", 0.0) as f32 / 65535.0;
            let drum = track["instrument"] == "drums";
            for f in 0..count {
                let time = f as f64 / rate as f64;
                let envelope = if drum {
                    (-time * 24.0).exp() as f32
                } else {
                    fade(f, count, (rate / 200) as u64, (rate / 100) as u64, false)
                };
                let sample = (std::f64::consts::TAU * frequency * time).sin() as f32
                    * envelope
                    * velocity
                    * gain
                    * 0.2;
                for c in 0..channels as usize {
                    out[(start + f) as usize * channels as usize + c] += sample;
                }
            }
        }
        if let Some(id) = track.get("assetId").and_then(Value::as_str) {
            let raw = store.asset(id).map_err(|e| e.to_string())?;
            let wav = inspect_wav(&raw).map_err(|e| format!("Asset {id}: {e}"))?;
            if wav.sample_rate != rate {
                return Err(format!(
                    "Asset {id}: source/output rate differs; resampling is pending"
                ));
            }
            let a = &track["audio"];
            let start = tick_frame(number(a, "startTick", 0.0), document.tempo(), rate)?;
            let source = (number(a, "trimStartSeconds", 0.0) * rate as f64).round() as u64;
            let source_end = (number(a, "trimEndSeconds", number(a, "durationSeconds", 0.0))
                * rate as f64)
                .round() as u64;
            if source_end > wav.frames || source >= source_end {
                return Err(format!("Asset {id}: declared trim exceeds actual PCM"));
            }
            let count = source_end - source;
            let fi = (number(a, "fadeInSeconds", 0.0) * rate as f64).round() as u64;
            let fo = (number(a, "fadeOutSeconds", 0.0) * rate as f64).round() as u64;
            if start + count > end {
                return Err("Original WAV placement differs from prepared duration".into());
            }
            for f in 0..count {
                for c in 0..channels {
                    let sample =
                        wav_sample(&raw, &wav, source + f, channel(wav.channels, channels, c)?)?;
                    out[(start + f) as usize * channels as usize + c as usize] +=
                        sample * gain * fade(f, count, fi, fo, false);
                }
            }
        }
    }
    if out.iter().any(|v| !v.is_finite()) {
        return Err("Mix overflow".into());
    }
    Ok(out)
}
fn mix_take(
    store: &NativeStore,
    clip: &TakeClip,
    rate: u32,
    channels: u16,
    gain: f32,
    out: &mut [f32],
) -> Result<(), String> {
    let TakeRange::AudioFrames { start, count } = clip.range else {
        return Err("Raw event playback is pending".into());
    };
    let reader = store.open_take(&clip.source).map_err(|e| e.to_string())?;
    let stream = reader
        .descriptor()
        .streams
        .iter()
        .find(|s| s.stream_id == clip.source.stream_id)
        .ok_or("Take stream missing")?;
    let StreamKind::Audio { format, .. } = &stream.kind else {
        return Err("Take stream is not PCM".into());
    };
    if format.sample_rate != rate || clip.position.sample_rate != rate {
        return Err("Native source/output rate differs; resampling pending".into());
    }
    channel(format.channels, channels, 0)?;
    let mut covered = 0u64;
    reader
        .visit_chunks(|_, record| {
            if record.header.stream_id != clip.source.stream_id {
                return Ok(());
            }
            let SourceRange::AudioFrames {
                start: chunk_start,
                count: chunk_count,
            } = record.header.range
            else {
                return Err(RecordingError::Invalid(
                    "Unexpected native event units".into(),
                ));
            };
            let lo = chunk_start.max(start);
            let hi = (chunk_start + chunk_count).min(start + count);
            if lo >= hi {
                return Ok(());
            }
            if !matches!(record.header.kind, ChunkKind::Content) {
                return Err(RecordingError::Invalid(format!(
                    "Clip {} intersects declared source loss; playback is not fabricated",
                    clip.id
                )));
            }
            covered += hi - lo;
            for source in lo..hi {
                let relative = source - start;
                let timeline = clip.position.frame as i128 + relative as i128;
                if timeline < 0 {
                    continue;
                }
                let frame = timeline as usize;
                if frame >= out.len() / channels as usize {
                    return Err(RecordingError::Invalid(
                        "Native placement exceeds prepared PCM".into(),
                    ));
                }
                let envelope = clip
                    .fades
                    .as_ref()
                    .map(|f| {
                        fade(
                            relative,
                            count,
                            f.in_frames,
                            f.out_frames,
                            f.curve == FadeCurve::EqualPower,
                        )
                    })
                    .unwrap_or(1.0);
                for c in 0..channels {
                    let sample = pcm_sample(
                        &record.payload,
                        format,
                        chunk_count,
                        source - chunk_start,
                        channel(format.channels, channels, c).map_err(RecordingError::Invalid)?,
                    )
                    .map_err(RecordingError::Invalid)?;
                    out[frame * channels as usize + c as usize] += sample * gain * envelope;
                }
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    if covered != count {
        return Err("Native clip source prefix is incomplete".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_pcm24_and_planar_are_exact() {
        let f = PcmFormat {
            sample_rate: 48000,
            channels: 2,
            encoding: PcmEncoding::S24Le,
            layout: PcmLayout::Planar,
        };
        let b = [0, 0, 128, 255, 255, 127, 0, 0, 0, 0, 0, 64];
        assert_eq!(pcm_sample(&b, &f, 2, 0, 0).unwrap(), -1.0);
        assert_eq!(pcm_sample(&b, &f, 2, 1, 1).unwrap(), 0.5);
    }
    #[test]
    fn truncated_wave_rejects() {
        assert!(inspect_wav(b"RIFF\x10\0\0\0WAVE").is_err());
    }
}
