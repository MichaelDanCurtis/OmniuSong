//! Bounded original-source peaks, independent of audio control and realtime DSP.
//! Authentication/decoding belongs on this worker. Cancellation is cooperative;
//! bounded SQLite verification and individual OS reads are not interruptible.
use crate::render::{
    inspect_wav_with_channel_limit, pcm_sample_with_channel_limit, wav_sample_with_channel_limit,
};
use crate::waveform_types::*;
use omniusong_core::{
    DocumentClipAddress, ProjectDocument,
    native::{TakeClip, TakeRange},
};
use omniusong_recording::{ChunkKind, SourceRange, StreamKind};
use omniusong_store::{NativeReader, StoreError, TakeObject};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
};

const CANCELLED: &str = "Waveform cancelled";
fn check(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}

enum Input {
    Wav {
        bytes: Vec<u8>,
        saved: WaveformSettings,
        original: Option<WaveformSettings>,
    },
    Native {
        object: Box<TakeObject>,
        saved: WaveformSettings,
        original: Option<WaveformSettings>,
    },
}
fn raw_clip<'a>(
    document: &'a ProjectDocument,
    address: &DocumentClipAddress,
) -> Result<&'a serde_json::Value, String> {
    let track_id = match address {
        DocumentClipAddress::LegacyAudio { track_id }
        | DocumentClipAddress::NativeAudio { track_id, .. } => track_id,
    };
    let track = document
        .tracks()
        .iter()
        .find(|track| track["id"].as_str() == Some(track_id))
        .ok_or("Waveform target track is absent")?;
    match address {
        DocumentClipAddress::LegacyAudio { .. } if track["kind"] == "audio" => track
            .get("audio")
            .filter(|v| v.is_object())
            .ok_or_else(|| "Audio track has no saved clip".into()),
        DocumentClipAddress::NativeAudio { clip_id, .. }
            if document.supports_native_takes() && track["kind"] == "recording" =>
        {
            track["recording"]["clips"]
                .as_array()
                .and_then(|clips| {
                    clips
                        .iter()
                        .find(|clip| clip["id"].as_str() == Some(clip_id))
                })
                .filter(|clip| clip["range"]["unit"] == "audio_frames")
                .ok_or_else(|| "Waveform target is not a declared audio-frame clip".into())
        }
        _ => Err("Waveform address selects the wrong track representation".into()),
    }
}
fn legacy_settings(
    raw: &serde_json::Value,
    declared_duration: f64,
) -> Result<WaveformSettings, String> {
    let optional = |key: &str| -> Result<Option<f64>, String> {
        raw.get(key)
            .map(|value| {
                value
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("Invalid waveform {key}"))
            })
            .transpose()
    };
    Ok(WaveformSettings::Legacy {
        declared_duration_seconds: declared_duration,
        start_tick: raw
            .get("startTick")
            .map(|v| v.as_u64().ok_or("Invalid waveform placement"))
            .transpose()?
            .unwrap_or(0),
        trim_start_seconds: optional("trimStartSeconds")?,
        trim_end_seconds: optional("trimEndSeconds")?,
        fade_in_seconds: optional("fadeInSeconds")?,
        fade_out_seconds: optional("fadeOutSeconds")?,
    })
}
fn native_settings(clip: &TakeClip) -> Result<WaveformSettings, String> {
    let TakeRange::AudioFrames { start, count } = clip.range else {
        return Err("Raw events do not have an audio waveform".into());
    };
    Ok(WaveformSettings::Native {
        window: omniusong_core::AudioSourceWindow {
            start_frame: start,
            frame_count: count,
        },
        position_frame: clip.position.frame,
        fades: clip.fades.clone(),
    })
}
fn metadata(
    reader: &NativeReader,
    request: &WaveformRequest,
    budget: &omniusong_recording::InspectionBudget,
) -> Result<Input, StoreError> {
    // Registry metadata is never source authority. Peak generation below still
    // authenticates only the exact selected media, independently of note claims.
    let document = reader.load_document_metadata_with_budget(&request.project_id, budget)?;
    if document.id() != request.project_id || document.revision() != request.expected_revision {
        return Err(invalid("Saved waveform project/revision changed"));
    }
    let raw = raw_clip(&document, &request.address).map_err(invalid)?;
    match &request.source {
        WaveformSourceKey::LegacyWav {
            asset_id,
            source_asset_id,
        } => {
            let DocumentClipAddress::LegacyAudio { track_id } = &request.address else {
                return Err(invalid("Wrong waveform representation"));
            };
            let track = document
                .tracks()
                .iter()
                .find(|track| track["id"].as_str() == Some(track_id))
                .ok_or_else(|| invalid("Waveform track disappeared"))?;
            if track["assetId"].as_str() != Some(asset_id)
                || track
                    .get("sourceAssetId")
                    .and_then(serde_json::Value::as_str)
                    != source_asset_id.as_deref()
            {
                return Err(invalid("Saved waveform asset identity changed"));
            }
            let duration = raw["durationSeconds"]
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| invalid("Invalid declared waveform duration"))?;
            let saved = legacy_settings(raw, duration).map_err(invalid)?;
            let original = raw
                .get("editSource")
                .map(|source| legacy_settings(&source["original"], duration))
                .transpose()
                .map_err(invalid)?;
            Ok(Input::Wav {
                bytes: reader.asset(asset_id)?,
                saved,
                original,
            })
        }
        WaveformSourceKey::NativeAudio {
            source,
            sample_rate,
        } => {
            let clip: TakeClip =
                serde_json::from_value(raw.clone()).map_err(|error| invalid(error.to_string()))?;
            clip.validate()?;
            if clip.source != *source || clip.position.sample_rate != *sample_rate {
                return Err(invalid("Saved waveform take/stream/rate identity changed"));
            }
            let saved = native_settings(&clip).map_err(invalid)?;
            let original = clip
                .original_view()?
                .as_ref()
                .map(native_settings)
                .transpose()
                .map_err(invalid)?;
            let object = reader.take_metadata(&source.take_id)?;
            if object.manifest_sha256 != source.manifest_sha256 {
                return Err(invalid("Waveform manifest identity changed"));
            }
            if object.byte_length > MAX_WAVEFORM_TAKE_BYTES
                || object.manifest.body.chunk_count > MAX_WAVEFORM_CHUNKS
            {
                return Err(invalid(
                    "Waveform take exceeds its physical-byte/chunk budget",
                ));
            }
            Ok(Input::Native {
                object: Box::new(object),
                saved,
                original,
            })
        }
    }
}

struct Bins {
    channels: Vec<Vec<WaveformBin>>,
    start: u64,
    end: u64,
    next: u64,
    index: usize,
    samples: u64,
}
impl Bins {
    fn new(start: u64, frames: u64, channels: u16, bins: usize) -> Result<Self, String> {
        if frames == 0
            || channels == 0
            || channels as usize > MAX_WAVEFORM_CHANNELS
            || !(1..=MAX_WAVEFORM_BINS).contains(&bins)
        {
            return Err("Invalid waveform extent/channel/bin limits".into());
        }
        let end = start
            .checked_add(frames)
            .ok_or("Waveform source range overflow")?;
        let count = (bins as u64).min(frames) as usize;
        let template: Vec<_> = (0..count)
            .map(|index| {
                let lo = start + (frames as u128 * index as u128 / count as u128) as u64;
                let hi = start + (frames as u128 * (index + 1) as u128 / count as u128) as u64;
                WaveformBin {
                    start_frame: lo,
                    frame_count: hi - lo,
                    content_frames: 0,
                    gap_frames: 0,
                    min: None,
                    max: None,
                }
            })
            .collect();
        Ok(Self {
            channels: vec![template; channels as usize],
            start,
            end,
            next: start,
            index: 0,
            samples: 0,
        })
    }
    fn range(&self, start: u64, count: u64) -> Result<u64, String> {
        let end = start
            .checked_add(count)
            .ok_or("Waveform chunk range overflow")?;
        if count == 0 || start != self.next || start < self.start || end > self.end {
            return Err(
                "Waveform chunks do not cover their contiguous selected source range".into(),
            );
        }
        Ok(end)
    }
    fn advance_bin(&mut self, frame: u64) -> Result<(), String> {
        while self.index < self.channels[0].len()
            && frame
                >= self.channels[0][self.index].start_frame
                    + self.channels[0][self.index].frame_count
        {
            self.index += 1;
        }
        if self.index >= self.channels[0].len() {
            return Err("Waveform frame is outside all bins".into());
        }
        Ok(())
    }
    fn content(
        &mut self,
        start: u64,
        count: u64,
        mut sample: impl FnMut(u64, u16) -> Result<f32, String>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let end = self.range(start, count)?;
        let work = count
            .checked_mul(self.channels.len() as u64)
            .and_then(|n| self.samples.checked_add(n))
            .ok_or("Waveform sample work overflow")?;
        if work > MAX_WAVEFORM_SAMPLES {
            return Err("Waveform exceeds the content-sample work budget".into());
        }
        for frame in start..end {
            self.advance_bin(frame)?;
            for channel in 0..self.channels.len() {
                if self.samples.is_multiple_of(4096) {
                    check(cancelled)?;
                }
                let value = sample(frame, channel as u16)?;
                if !value.is_finite() {
                    return Err("Nonfinite original PCM has no valid waveform".into());
                }
                let bin = &mut self.channels[channel][self.index];
                bin.min = Some(bin.min.map_or(value, |old| old.min(value)));
                bin.max = Some(bin.max.map_or(value, |old| old.max(value)));
                bin.content_frames += 1;
                self.samples += 1;
            }
        }
        self.next = end;
        check(cancelled)
    }
    fn gap(&mut self, start: u64, count: u64, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        let end = self.range(start, count)?;
        let mut frame = start;
        while frame < end {
            check(cancelled)?;
            self.advance_bin(frame)?;
            let bin = &self.channels[0][self.index];
            let hi = end.min(bin.start_frame + bin.frame_count);
            for channel in &mut self.channels {
                channel[self.index].gap_frames += hi - frame;
            }
            frame = hi;
        }
        self.next = end;
        Ok(())
    }
    fn finish(self) -> Result<Vec<Vec<WaveformBin>>, String> {
        if self.next != self.end {
            Err("Waveform original prefix is incomplete".into())
        } else {
            Ok(self.channels)
        }
    }
}
fn validate_window(settings: &WaveformSettings, start: u64, end: u64) -> Result<(), String> {
    let WaveformSettings::Native { window, .. } = settings else {
        return Err("Native waveform has non-native settings".into());
    };
    if window.start_frame < start
        || window.frame_count == 0
        || window
            .start_frame
            .checked_add(window.frame_count)
            .is_none_or(|hi| hi > end)
    {
        return Err("Current/original clip window exceeds its authenticated source prefix".into());
    }
    Ok(())
}

/// Prepare complete original-source bins. Nothing is published on cancellation
/// or failure; saved clip settings are cached overlays, never peak processing.
pub fn prepare_waveform(
    root: &Path,
    request: &WaveformRequest,
    cancelled: impl Fn() -> bool,
) -> Result<WaveformData, String> {
    request.validate()?;
    check(&cancelled)?;
    let mut reader = NativeReader::open(root).map_err(|error| error.to_string())?;
    check(&cancelled)?;
    let metadata_budget = omniusong_recording::InspectionBudget::new(
        Default::default(),
        Arc::new(AtomicBool::new(cancelled())),
    )
    .map_err(|error| error.to_string())?;
    // The callback's existing cancellation fences bracket this short metadata
    // snapshot; no new whole-operation cancellation/deadline promise is made.
    let input = reader
        .read_snapshot(|reader| metadata(reader, request, &metadata_budget))
        .map_err(|error| error.to_string())?;
    check(&cancelled)?;
    let data = match input {
        Input::Wav {
            bytes,
            saved,
            original,
        } => {
            let info = inspect_wav_with_channel_limit(&bytes, MAX_WAVEFORM_CHANNELS)?;
            check(&cancelled)?;
            if info
                .frames
                .checked_mul(u64::from(info.channels))
                .is_none_or(|n| n > MAX_WAVEFORM_SAMPLES)
            {
                return Err("Waveform WAV exceeds its content-sample work budget".into());
            }
            let mut bins = Bins::new(0, info.frames, info.channels, request.bins)?;
            bins.content(
                0,
                info.frames,
                |frame, channel| {
                    wav_sample_with_channel_limit(
                        &bytes,
                        &info,
                        frame,
                        channel,
                        MAX_WAVEFORM_CHANNELS,
                    )
                },
                &cancelled,
            )?;
            let WaveformSourceKey::LegacyWav { asset_id, .. } = &request.source else {
                return Err("Invalid WAV source key".into());
            };
            WaveformData {
                request: request.clone(),
                authenticated_sha256: asset_id[..64].into(),
                sample_rate: info.sample_rate,
                channels: info.channels,
                source_start_frame: 0,
                frame_count: info.frames,
                unknown_tail: false,
                channel_bins: bins.finish()?,
                saved,
                original,
            }
        }
        Input::Native {
            object,
            saved,
            original,
        } => {
            let WaveformSourceKey::NativeAudio {
                source,
                sample_rate,
            } = &request.source
            else {
                return Err("Invalid native source key".into());
            };
            let selected = object
                .manifest
                .body
                .descriptor
                .streams
                .iter()
                .find(|stream| stream.stream_id == source.stream_id)
                .ok_or("Waveform source stream is absent")?;
            let StreamKind::Audio { format, .. } = &selected.kind else {
                return Err("Waveform stream is not audio PCM".into());
            };
            let prefix = object
                .manifest
                .body
                .streams
                .iter()
                .find(|prefix| prefix.stream_id == source.stream_id)
                .ok_or("Waveform sealed prefix is absent")?;
            if format.sample_rate != *sample_rate || format.channels == 0 || format.channels > 16 {
                return Err("Native waveform source format/rate mismatch".into());
            }
            if prefix
                .recorded_units
                .checked_mul(u64::from(format.channels))
                .is_none_or(|n| n > MAX_WAVEFORM_SAMPLES)
            {
                return Err("Native waveform exceeds its content-sample work budget".into());
            }
            let start = selected.initial_source_position;
            let end = prefix.source_end.value();
            let frames = end
                .checked_sub(start)
                .ok_or("Native waveform source range is reversed")?;
            validate_window(&saved, start, end)?;
            if let Some(original) = &original {
                validate_window(original, start, end)?;
            }
            check(&cancelled)?;
            let sealed = reader
                .open_take_bounded(source, MAX_WAVEFORM_TAKE_BYTES, &cancelled)
                .map_err(|error| error.to_string())?;
            check(&cancelled)?;
            let expected_name = format!("{}.sqlite3", object.object_sha256);
            if sealed.manifest() != &object.manifest
                || sealed.path().file_name() != Some(OsStr::new(&expected_name))
            {
                return Err("Opened waveform original differs from its metadata snapshot".into());
            }
            let mut bins = Bins::new(start, frames, format.channels, request.bins)?;
            for sequence in 0..prefix.next_sequence {
                check(&cancelled)?;
                let chunk = sealed
                    .read_chunk(&source.stream_id, sequence)
                    .map_err(|error| error.to_string())?;
                check(&cancelled)?;
                let SourceRange::AudioFrames {
                    start: chunk_start,
                    count,
                } = chunk.header.range
                else {
                    return Err("Native waveform source has event units".into());
                };
                match chunk.header.kind {
                    ChunkKind::Gap { .. } => bins.gap(chunk_start, count, &cancelled)?,
                    ChunkKind::Content => bins.content(
                        chunk_start,
                        count,
                        |frame, channel| {
                            pcm_sample_with_channel_limit(
                                &chunk.payload,
                                format,
                                count,
                                frame - chunk_start,
                                channel,
                                16,
                            )
                        },
                        &cancelled,
                    )?,
                }
            }
            WaveformData {
                request: request.clone(),
                authenticated_sha256: object.object_sha256,
                sample_rate: format.sample_rate,
                channels: format.channels,
                source_start_frame: start,
                frame_count: frames,
                unknown_tail: object
                    .manifest
                    .body
                    .interrupted
                    .as_ref()
                    .is_some_and(|receipt| receipt.report.unknown_tail),
                channel_bins: bins.finish()?,
                saved,
                original,
            }
        }
    };
    check(&cancelled)?;
    data.validate()?;
    Ok(data)
}

#[derive(Debug)]
pub struct WaveformCompletion {
    pub ticket: u64,
    pub request: WaveformRequest,
    pub result: Result<WaveformData, String>,
}
struct Job {
    ticket: u64,
    request: WaveformRequest,
    generation: u64,
}
pub struct WaveformClient {
    requests: Option<SyncSender<Job>>,
    results: Receiver<WaveformCompletion>,
    pending: Option<Job>,
    generation: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    next_ticket: u64,
    retired: bool,
}
impl WaveformClient {
    fn retire(&mut self, keep_pending: bool) {
        self.alive.store(false, Ordering::Release);
        self.requests = None;
        self.retired = true;
        if !keep_pending {
            self.pending = None;
        }
    }
    pub fn spawn(root: PathBuf) -> Result<Self, String> {
        Self::spawn_backend(move |request, cancelled| prepare_waveform(&root, request, cancelled))
    }
    fn spawn_backend(
        mut backend: impl FnMut(&WaveformRequest, &dyn Fn() -> bool) -> Result<WaveformData, String>
        + Send
        + 'static,
    ) -> Result<Self, String> {
        let (request_tx, request_rx) = mpsc::sync_channel::<Job>(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let generation = Arc::new(AtomicU64::new(1));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_generation = generation.clone();
        let worker_alive = alive.clone();
        thread::Builder::new()
            .name("omniusong-waveform".into())
            .spawn(move || {
                while let Ok(job) = request_rx.recv() {
                    let cancelled = || {
                        !worker_alive.load(Ordering::Acquire)
                            || worker_generation.load(Ordering::Acquire) != job.generation
                    };
                    let mut result = if cancelled() {
                        Err(CANCELLED.into())
                    } else {
                        backend(&job.request, &cancelled)
                    };
                    if cancelled() {
                        result = Err(CANCELLED.into());
                    }
                    if result_tx
                        .send(WaveformCompletion {
                            ticket: job.ticket,
                            request: job.request,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| format!("Could not start waveform worker: {error}"))?;
        Ok(Self {
            requests: Some(request_tx),
            results: result_rx,
            pending: None,
            generation,
            alive,
            next_ticket: 1,
            retired: false,
        })
    }
    pub fn try_submit(&mut self, request: WaveformRequest) -> Result<u64, String> {
        request.validate()?;
        if self.retired || !self.alive.load(Ordering::Acquire) {
            return Err("Waveform worker is disconnected".into());
        }
        if self.pending.is_some() {
            return Err("A waveform request is still pending".into());
        }
        let ticket = self.next_ticket;
        let next = ticket
            .checked_add(1)
            .ok_or("Waveform ticket counter exhausted")?;
        let generation = self.generation.load(Ordering::Acquire);
        let job = Job {
            ticket,
            request: request.clone(),
            generation,
        };
        match self
            .requests
            .as_ref()
            .ok_or("Waveform worker is disconnected")?
            .try_send(job)
        {
            Ok(()) => {
                self.next_ticket = next;
                self.pending = Some(Job {
                    ticket,
                    request,
                    generation,
                });
                Ok(ticket)
            }
            Err(TrySendError::Full(_)) => Err("Waveform request queue is full".into()),
            Err(TrySendError::Disconnected(_)) => {
                self.retire(false);
                Err("Waveform worker is disconnected".into())
            }
        }
    }
    /// Revoke the active generation without returning its admission credit.
    pub fn cancel(&mut self) {
        if self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .is_err()
        {
            self.retire(true);
        }
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn try_poll(&mut self) -> Result<Option<WaveformCompletion>, String> {
        match self.results.try_recv() {
            Ok(mut completion) => {
                let Some(pending) = &self.pending else {
                    self.retire(false);
                    return Err("Unsolicited waveform completion".into());
                };
                if pending.ticket != completion.ticket || pending.request != completion.request {
                    self.retire(true);
                    return Err("Waveform completion correlation failed".into());
                }
                if pending.generation != self.generation.load(Ordering::Acquire) {
                    completion.result = Err(CANCELLED.into());
                }
                if let Ok(data) = &completion.result {
                    if data.request != completion.request {
                        self.retire(false);
                        return Err("Waveform result source/request differs".into());
                    }
                    if let Err(error) = data.validate() {
                        self.retire(false);
                        return Err(format!("Invalid waveform completion: {error}"));
                    }
                }
                self.pending = None;
                Ok(Some(completion))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.retire(false);
                Err("Waveform worker disconnected".into())
            }
        }
    }
}
impl Drop for WaveformClient {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.requests = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::{
        contracts::{
            ClockDomainId, ClockEpochId, HostTimestamp, OperationId, ProjectId, TakeId, TrackId,
        },
        native::{TakeReference, TimelinePosition},
    };
    use omniusong_recording::{
        ChunkHeader, HostClock, Journal, PcmEncoding, PcmFormat, PcmLayout, SealedJournal,
        StreamDescriptor, TakeDescriptor,
    };
    use omniusong_store::{NativeStore, TakePublication};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{
        cell::Cell,
        collections::BTreeMap,
        fs,
        sync::mpsc,
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.runtime/rust-waveform-owner");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn request() -> WaveformRequest {
        WaveformRequest {
            project_id: "waveform-project".into(),
            expected_revision: 0,
            address: DocumentClipAddress::LegacyAudio {
                track_id: "audio".into(),
            },
            source: WaveformSourceKey::LegacyWav {
                asset_id: format!("{}.wav", "a".repeat(64)),
                source_asset_id: None,
            },
            bins: 4,
        }
    }
    fn fake_data(request: &WaveformRequest) -> WaveformData {
        WaveformData {
            request: request.clone(),
            authenticated_sha256: "a".repeat(64),
            sample_rate: 48_000,
            channels: 1,
            source_start_frame: 0,
            frame_count: 1,
            unknown_tail: false,
            channel_bins: vec![vec![WaveformBin {
                start_frame: 0,
                frame_count: 1,
                content_frames: 1,
                gap_frames: 0,
                min: Some(0.25),
                max: Some(0.25),
            }]],
            saved: WaveformSettings::Legacy {
                declared_duration_seconds: 0.001,
                start_tick: 0,
                trim_start_seconds: None,
                trim_end_seconds: None,
                fade_in_seconds: None,
                fade_out_seconds: None,
            },
            original: None,
        }
    }
    fn poll(client: &mut WaveformClient) -> WaveformCompletion {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(completion) = client.try_poll().unwrap() {
                return completion;
            }
            assert!(Instant::now() < end, "waveform worker did not complete");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn wav_bytes(channels: u16, bits: u16, float: bool, bytes: &[u8]) -> Vec<u8> {
        let mut wav = Vec::new();
        let width = u32::from(channels) * u32::from(bits / 8);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + bytes.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&(48_000 * width).to_le_bytes());
        wav.extend_from_slice(&(width as u16).to_le_bytes());
        wav.extend_from_slice(&bits.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        wav.extend_from_slice(bytes);
        wav
    }

    #[test]
    fn extrema_have_no_zero_bias_and_include_last_samples_per_channel() {
        let mut bins = Bins::new(100, 7, 2, 3).unwrap();
        bins.content(
            100,
            7,
            |frame, channel| {
                Ok(if channel == 0 {
                    if frame == 106 { 0.9 } else { 0.25 }
                } else if frame == 106 {
                    -0.9
                } else {
                    -0.25
                })
            },
            &|| false,
        )
        .unwrap();
        let bins = bins.finish().unwrap();
        assert_eq!(
            bins[0].iter().map(|b| b.frame_count).collect::<Vec<_>>(),
            vec![2, 2, 3]
        );
        assert_eq!(bins[0][0].min, Some(0.25));
        assert_eq!(bins[0][0].max, Some(0.25));
        assert_eq!(bins[1][0].min, Some(-0.25));
        assert_eq!(bins[1][0].max, Some(-0.25));
        assert_eq!(bins[0][2].max, Some(0.9));
        assert_eq!(bins[1][2].min, Some(-0.9));
        assert_eq!(bins[0][2].start_frame, 104);
        assert_eq!(bins[0][2].content_frames, 3);
    }
    #[test]
    fn enormous_gap_is_coverage_not_silence_and_uses_only_bin_iterations() {
        let count = 1_000_000_000_000u64;
        let checks = Cell::new(0usize);
        let mut bins = Bins::new(100, count, 1, 1024).unwrap();
        bins.gap(100, count, &|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
        let bins = bins.finish().unwrap();
        assert!(checks.get() <= 1024);
        assert_eq!(bins[0].iter().map(|b| b.gap_frames).sum::<u64>(), count);
        assert!(
            bins[0]
                .iter()
                .all(|b| b.min.is_none() && b.max.is_none() && b.content_frames == 0)
        );
        assert_eq!(bins[0][0].start_frame, 100);
    }
    #[test]
    fn mixed_bins_keep_actual_content_and_exact_gap_counts() {
        let mut bins = Bins::new(100, 8, 1, 2).unwrap();
        bins.content(100, 2, |_, _| Ok(0.5), &|| false).unwrap();
        bins.gap(102, 4, &|| false).unwrap();
        bins.content(106, 2, |_, _| Ok(-0.75), &|| false).unwrap();
        let bins = bins.finish().unwrap();
        assert_eq!((bins[0][0].content_frames, bins[0][0].gap_frames), (2, 2));
        assert_eq!((bins[0][1].content_frames, bins[0][1].gap_frames), (2, 2));
        assert_eq!(bins[0][0].min, Some(0.5));
        assert_eq!(bins[0][1].max, Some(-0.75));
    }
    #[test]
    fn content_work_and_nonfinite_values_fail_before_any_result() {
        let mut bins = Bins::new(0, MAX_WAVEFORM_SAMPLES + 1, 1, 4).unwrap();
        assert!(
            bins.content(
                0,
                MAX_WAVEFORM_SAMPLES + 1,
                |_, _| panic!("budget must fail before decoding"),
                &|| false
            )
            .is_err()
        );
        let mut bins = Bins::new(0, 2, 1, 2).unwrap();
        assert!(
            bins.content(
                0,
                2,
                |frame, _| Ok(if frame == 1 { f32::NAN } else { 0.5 }),
                &|| false
            )
            .is_err()
        );
        assert!(Bins::new(u64::MAX, 1, 1, 1).is_err());
        assert!(Bins::new(0, 0, 1, 1).is_err());
        let mut bins = Bins::new(100, 4, 1, 4).unwrap();
        assert!(bins.gap(101, 3, &|| false).is_err());
        assert!(bins.finish().is_err());
    }
    #[test]
    fn decoding_checks_cancellation_within_4096_content_samples() {
        let checks = Cell::new(0usize);
        let samples = Cell::new(0usize);
        let mut bins = Bins::new(0, 10_000, 1, 4).unwrap();
        assert!(
            bins.content(
                0,
                10_000,
                |_, _| {
                    samples.set(samples.get() + 1);
                    Ok(0.5)
                },
                &|| {
                    let old = checks.get();
                    checks.set(old + 1);
                    old >= 1
                }
            )
            .is_err()
        );
        assert_eq!(samples.get(), 4096);
    }
    #[test]
    fn waveform_decoders_cover_all_wav_formats_and_preserve_playback_channel_limit() {
        for (bits, float) in [(16, false), (24, false), (32, false), (32, true)] {
            let mut pcm = Vec::new();
            for value in [0.25f32, -0.25, 0.5, -0.5] {
                match (bits, float) {
                    (16, _) => pcm.extend_from_slice(&((value * 32768.0) as i16).to_le_bytes()),
                    (24, _) => {
                        pcm.extend_from_slice(&((value * 8388608.0) as i32).to_le_bytes()[..3])
                    }
                    (32, false) => {
                        pcm.extend_from_slice(&((value as f64 * 2147483648.0) as i32).to_le_bytes())
                    }
                    _ => pcm.extend_from_slice(&value.to_le_bytes()),
                }
            }
            let wav = wav_bytes(2, bits, float, &pcm);
            let info = inspect_wav_with_channel_limit(&wav, 32).unwrap();
            let mut bins = Bins::new(0, 2, 2, 1).unwrap();
            bins.content(
                0,
                2,
                |frame, channel| wav_sample_with_channel_limit(&wav, &info, frame, channel, 32),
                &|| false,
            )
            .unwrap();
            let bins = bins.finish().unwrap();
            assert_eq!((bins[0][0].min, bins[0][0].max), (Some(0.25), Some(0.5)));
            assert_eq!((bins[1][0].min, bins[1][0].max), (Some(-0.5), Some(-0.25)));
        }
        let pcm: Vec<_> = (0..32).flat_map(|_| 0.25f32.to_le_bytes()).collect();
        let wav = wav_bytes(32, 32, true, &pcm);
        assert!(crate::render::inspect_wav(&wav).is_err());
        let mut info = inspect_wav_with_channel_limit(&wav, 32).unwrap();
        assert_eq!(
            wav_sample_with_channel_limit(&wav, &info, 0, 31, 32).unwrap(),
            0.25
        );
        assert!(crate::render::wav_sample(&wav, &info, 0, 0).is_err());
        info.data_offset = usize::MAX;
        assert!(wav_sample_with_channel_limit(&wav, &info, 0, 31, 32).is_err());
        let wav = wav_bytes(1, 32, true, &f32::INFINITY.to_le_bytes());
        let info = inspect_wav_with_channel_limit(&wav, 32).unwrap();
        assert!(wav_sample_with_channel_limit(&wav, &info, 0, 0, 32).is_err());
    }
    #[test]
    fn client_cancel_revokes_generation_but_never_reuses_unfinished_credit() {
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let mut first = true;
        let mut client = WaveformClient::spawn_backend(move |request, _| {
            if first {
                first = false;
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            Ok(fake_data(request))
        })
        .unwrap();
        let request = request();
        let ticket = client.try_submit(request.clone()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.cancel();
        assert!(client.pending());
        assert!(client.try_submit(request.clone()).is_err());
        release_tx.send(()).unwrap();
        let result = poll(&mut client);
        assert_eq!(result.ticket, ticket);
        assert_eq!(result.request, request);
        assert_eq!(result.result.unwrap_err(), CANCELLED);
        assert!(!client.pending());
        let next = client.try_submit(request).unwrap();
        assert!(next > ticket);
        assert!(poll(&mut client).result.is_ok());
    }
    #[test]
    fn cancel_also_rejects_success_already_queued_before_ui_poll() {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let mut client = WaveformClient::spawn_backend(move |request, _| {
            done_tx.send(()).unwrap();
            Ok(fake_data(request))
        })
        .unwrap();
        let ticket = client.try_submit(request()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.cancel();
        assert!(client.pending());
        let completion = poll(&mut client);
        assert_eq!(completion.ticket, ticket);
        assert_eq!(completion.result.unwrap_err(), CANCELLED);
    }
    #[test]
    fn correlated_malformed_result_retires_worker_and_clears_pending() {
        for wrong_request in [false, true] {
            let mut client = WaveformClient::spawn_backend(move |request, _| {
                let mut data = fake_data(request);
                if wrong_request {
                    data.request.expected_revision += 1;
                } else {
                    data.channel_bins[0][0].min = Some(f32::NAN);
                }
                Ok(data)
            })
            .unwrap();
            client.try_submit(request()).unwrap();
            let end = Instant::now() + Duration::from_secs(5);
            loop {
                match client.try_poll() {
                    Err(_) => break,
                    Ok(None) => {
                        assert!(Instant::now() < end);
                        thread::sleep(Duration::from_millis(1));
                    }
                    Ok(Some(_)) => panic!("Malformed result was accepted"),
                }
            }
            assert!(!client.pending());
            assert!(client.retired);
            assert!(!client.alive.load(Ordering::Acquire));
            assert!(client.requests.is_none());
            assert!(client.try_submit(request()).is_err());
        }
    }
    #[test]
    fn uncorrelated_completion_keeps_credit_only_until_worker_disconnect() {
        let (request_tx, request_rx) = mpsc::sync_channel::<Job>(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let request = request();
        let mut client = WaveformClient {
            requests: Some(request_tx),
            results: result_rx,
            pending: Some(Job {
                ticket: 7,
                request: request.clone(),
                generation: 1,
            }),
            generation: Arc::new(AtomicU64::new(1)),
            alive: Arc::new(AtomicBool::new(true)),
            next_ticket: 8,
            retired: false,
        };
        result_tx
            .send(WaveformCompletion {
                ticket: 8,
                request: request.clone(),
                result: Err("Unrelated completion".into()),
            })
            .unwrap();
        assert!(client.try_poll().unwrap_err().contains("correlation"));
        assert!(client.pending());
        assert!(client.requests.is_none());
        assert!(!client.alive.load(Ordering::Acquire));
        assert!(client.try_submit(request).is_err());
        assert!(matches!(
            request_rx.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        drop(result_tx);
        assert!(client.try_poll().unwrap_err().contains("disconnected"));
        assert!(!client.pending());
    }
    #[test]
    fn drop_does_not_join_an_uninterruptible_backend_call() {
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let mut client = WaveformClient::spawn_backend(move |request, _| {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            finished_tx.send(()).unwrap();
            Ok(fake_data(request))
        })
        .unwrap();
        client.try_submit(request()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            drop(client);
            dropped_tx.send(()).unwrap();
        });
        dropped_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Drop tried to join the blocked worker");
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    #[test]
    fn legacy_preparation_uses_actual_full_wav_and_original_declared_defaults_readonly() {
        let fixture = Fixture::new();
        let assets = fixture.0.join("assets");
        fs::create_dir(&assets).unwrap();
        let pcm: Vec<_> = [0.25f32, 0.5, 0.75, 0.9]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let wav = wav_bytes(1, 32, true, &pcm);
        let asset_id = format!("{}.wav", sha(&wav));
        fs::write(assets.join(&asset_id), &wav).unwrap();
        let mut value = ProjectDocument::new("Synthetic original WAV")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["kind"] = "audio".into();
        value["tracks"][0]["assetId"] = asset_id.clone().into();
        value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":1.0,"byteLength":wav.len(),"startTick":999,"trimStartSeconds":0.2,"trimEndSeconds":0.8,"fadeInSeconds":0.1,"editSource":{"version":1,"original":{}}});
        let document = ProjectDocument::from_value(value).unwrap();
        let source = fixture.0.join("source.json");
        fs::write(&source, document.to_json().unwrap()).unwrap();
        let root = fixture.0.join("store");
        let mut store = NativeStore::open(&root).unwrap();
        let saved = store.import_legacy(&source, &assets).unwrap();
        let address = DocumentClipAddress::LegacyAudio {
            track_id: saved.tracks()[0]["id"].as_str().unwrap().into(),
        };
        let request = WaveformRequest {
            project_id: saved.id().into(),
            expected_revision: saved.revision(),
            address,
            source: WaveformSourceKey::LegacyWav {
                asset_id,
                source_asset_id: None,
            },
            bins: 2,
        };
        drop(store);
        let before = fs::read(root.join("studio.sqlite3")).unwrap();
        let result = prepare_waveform(&root, &request, || false).unwrap();
        assert_eq!(result.frame_count, 4);
        assert_eq!(result.source_start_frame, 0);
        assert_eq!(result.channel_bins[0][0].min, Some(0.25));
        assert_eq!(result.channel_bins[0][1].max, Some(0.9));
        assert!(matches!(
            result.original,
            Some(WaveformSettings::Legacy {
                declared_duration_seconds: 1.0,
                trim_end_seconds: None,
                start_tick: 0,
                ..
            })
        ));
        assert_eq!(fs::read(root.join("studio.sqlite3")).unwrap(), before);
        assert_eq!(
            fs::read(assets.join(match &request.source {
                WaveformSourceKey::LegacyWav { asset_id, .. } => asset_id,
                _ => unreachable!(),
            }))
            .unwrap(),
            wav
        );
        let mut stale = request.clone();
        stale.expected_revision += 1;
        assert!(prepare_waveform(&root, &stale, || false).is_err());
        assert!(prepare_waveform(&root, &request, || true).is_err());
    }

    fn native_fixture(root: &Path, recovered: bool) -> (WaveformRequest, PathBuf) {
        let mut store = NativeStore::open(root.join("store")).unwrap();
        let project = store
            .create(
                &ProjectDocument::new("Synthetic sixteen-channel original")
                    .unwrap()
                    .upgrade_native()
                    .unwrap(),
            )
            .unwrap();
        let take = TakeDescriptor {
            version: 1,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "synthetic-segment".into(),
                source_id: "synthetic-source".into(),
                source_name: "Synthetic planar PCM, no device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("synthetic-clock".into()),
                    epoch: ClockEpochId("synthetic-epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 16,
                        encoding: PcmEncoding::S24Le,
                        layout: PcmLayout::Planar,
                    },
                    audio_epoch: ClockEpochId("synthetic-audio-epoch".into()),
                },
                initial_source_position: 100,
            }],
            provenance: BTreeMap::from([(
                "synthetic".into(),
                json!({"human":false,"deviceOpened":false}),
            )]),
        };
        let header = |sequence, start, count, kind| ChunkHeader {
            version: 1,
            take_id: take.take_id.clone(),
            stream_id: "audio".into(),
            segment_id: "synthetic-segment".into(),
            sequence,
            range: SourceRange::AudioFrames { start, count },
            interpretation: take.streams[0].kind.clone(),
            observed_start: HostTimestamp {
                domain: ClockDomainId("synthetic-clock".into()),
                epoch: ClockEpochId("synthetic-epoch".into()),
                ticks: start,
            },
            observed_end: HostTimestamp {
                domain: ClockDomainId("synthetic-clock".into()),
                epoch: ClockEpochId("synthetic-epoch".into()),
                ticks: start + count,
            },
            clock_anchors: vec![],
            kind,
        };
        let mut pcm = Vec::new();
        for channel in 0..16 {
            for value in [0.25f32, 0.5] {
                let value = if channel == 1 { -value } else { value };
                pcm.extend_from_slice(&((value * 8388608.0) as i32).to_le_bytes()[..3]);
            }
        }
        let path = root.join("original.sqlite3");
        let mut writer = Journal::create(&path, &take).unwrap();
        writer
            .append(&header(0, 100, 2, ChunkKind::Content), &pcm)
            .unwrap();
        if !recovered {
            writer
                .append(
                    &header(
                        1,
                        102,
                        1_000_000_000_000,
                        ChunkKind::Gap {
                            reason: omniusong_recording::GapReason::TransferOverflow,
                        },
                    ),
                    &[],
                )
                .unwrap();
            writer
                .append(&header(2, 1_000_000_000_102, 2, ChunkKind::Content), &pcm)
                .unwrap();
            writer.seal("synthetic-seal").unwrap();
            drop(writer);
        } else {
            drop(writer);
            let mut writer = Journal::open(&path).unwrap();
            writer.seal_recovered("synthetic-recovered-seal").unwrap();
            drop(writer);
        }
        let sealed = SealedJournal::open(&path).unwrap();
        let object = store.ingest_take(&sealed).unwrap();
        let source = TakeReference {
            take_id: take.take_id.clone(),
            manifest_sha256: object.manifest_sha256.clone(),
            stream_id: "audio".into(),
        };
        let result = store
            .publish_take(&TakePublication {
                operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
                project_id: ProjectId(project.id().into()),
                expected_revision: 0,
                track_id: TrackId("recording".into()),
                track_name: "Synthetic original".into(),
                clip: TakeClip {
                    id: "selected".into(),
                    source: source.clone(),
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 2,
                    },
                    position: TimelinePosition {
                        frame: -100,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            })
            .unwrap();
        (
            WaveformRequest {
                project_id: project.id().into(),
                expected_revision: result.document.revision(),
                address: DocumentClipAddress::NativeAudio {
                    track_id: "recording".into(),
                    clip_id: "selected".into(),
                },
                source: WaveformSourceKey::NativeAudio {
                    source,
                    sample_rate: 48_000,
                },
                bins: 4,
            },
            root.join("store/takes")
                .join(format!("{}.sqlite3", object.object_sha256)),
        )
    }
    #[test]
    fn native_original_sixteen_planar_channels_and_huge_gap_remain_source_relative() {
        let fixture = Fixture::new();
        let (request, path) = native_fixture(&fixture.0, false);
        let before = fs::read(&path).unwrap();
        let result = prepare_waveform(&fixture.0.join("store"), &request, || false).unwrap();
        assert_eq!(result.channels, 16);
        assert_eq!(result.source_start_frame, 100);
        assert_eq!(result.frame_count, 1_000_000_000_004);
        assert!(!result.unknown_tail);
        assert_eq!(result.channel_bins[0][0].min, Some(0.25));
        assert_eq!(result.channel_bins[1][0].max, Some(-0.25));
        assert!(result.channel_bins[0][1].min.is_none());
        assert_eq!(
            result.channel_bins[0]
                .iter()
                .map(|bin| bin.content_frames)
                .sum::<u64>(),
            4
        );
        assert_eq!(
            result.channel_bins[0]
                .iter()
                .map(|bin| bin.gap_frames)
                .sum::<u64>(),
            1_000_000_000_000
        );
        assert_eq!(result.authenticated_sha256, sha(&before));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn empty_projection_schema_preserves_authenticated_native_waveform() {
        let fixture = Fixture::new();
        let (mut request, path) = native_fixture(&fixture.0, true);
        let original = fs::read(&path).unwrap();
        let root = fixture.0.join("store");
        let before = prepare_waveform(&root, &request, || false).unwrap();
        let mut store = NativeStore::open_existing(&root).unwrap();
        let current = store.load(&request.project_id).unwrap();
        let promoted = current.upgrade_performance_projection_schema().unwrap();
        let saved = store.save(&promoted, current.revision()).unwrap();
        assert_eq!(saved.schema_version(), 3);
        request.expected_revision = saved.revision();
        drop(store);
        let after = prepare_waveform(&root, &request, || false).unwrap();
        assert_eq!(after.source_start_frame, before.source_start_frame);
        assert_eq!(after.frame_count, before.frame_count);
        assert_eq!(after.channels, before.channels);
        assert_eq!(after.channel_bins, before.channel_bins);
        assert_eq!(after.unknown_tail, before.unknown_tail);
        assert_eq!(after.authenticated_sha256, before.authenticated_sha256);
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn recovered_native_prefix_reports_unknown_tail_without_inventing_extent() {
        let fixture = Fixture::new();
        let (request, path) = native_fixture(&fixture.0, true);
        let before = fs::read(&path).unwrap();
        let result = prepare_waveform(&fixture.0.join("store"), &request, || false).unwrap();
        assert!(result.unknown_tail);
        assert_eq!(result.source_start_frame, 100);
        assert_eq!(result.frame_count, 2);
        assert_eq!(result.channel_bins[0].len(), 2);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
