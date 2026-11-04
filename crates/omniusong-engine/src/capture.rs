//! Incremental journal reduction, strictly outside the device callback.
use crate::{
    CapturedTake,
    realtime::{AudioPacket, MAX_CHANNELS, MeterSnapshot, PACKET_FRAMES, TRANSFER_OVERFLOW},
};
use omniusong_recording::{
    CHUNK_VERSION, ChunkHeader, ChunkKind, GapReason, HostTimestamp, Journal, SourceRange,
    TakeDescriptor,
};
use std::path::{Path, PathBuf};

pub struct CaptureJournal {
    journal: Journal,
    descriptor: TakeDescriptor,
    path: PathBuf,
    next: u64,
    sequence: u64,
    last_ns: u64,
    gaps: u64,
    bytes: Vec<u8>,
    pending_frames: u64,
    pending_start_ns: u64,
    pending_end_ns: u64,
    chunk_frames: u64,
    last_packet_start_ns: Option<u64>,
}
impl CaptureJournal {
    pub fn create(path: &Path, descriptor: TakeDescriptor) -> Result<Self, String> {
        if descriptor.streams.len() != 1
            || descriptor.streams[0].stream_id != "audio"
            || descriptor.streams[0].initial_source_position != 0
        {
            return Err("Engine capture requires one audio stream starting at zero".into());
        }
        let omniusong_recording::StreamKind::Audio { format, .. } = &descriptor.streams[0].kind
        else {
            return Err("Engine capture descriptor must be audio".into());
        };
        if format.channels == 0
            || format.channels as usize > MAX_CHANNELS
            || format.encoding != omniusong_recording::PcmEncoding::F32Le
            || format.layout != omniusong_recording::PcmLayout::Interleaved
            || descriptor.streams[0].host_clock.ticks_per_second_numerator != 1_000_000_000
            || descriptor.streams[0]
                .host_clock
                .ticks_per_second_denominator
                != 1
        {
            return Err("Unsupported engine capture descriptor interpretation".into());
        }
        let chunk_frames = (format.sample_rate as u64 / 4)
            .min(omniusong_recording::MAX_AUDIO_CHUNK_BYTES as u64 / (format.channels as u64 * 4));
        let capacity = chunk_frames as usize * format.channels as usize * 4;
        let journal = Journal::create(path, &descriptor).map_err(|e| e.to_string())?;
        Ok(Self {
            journal,
            descriptor,
            path: path.to_owned(),
            next: 0,
            sequence: 0,
            last_ns: 0,
            gaps: 0,
            bytes: Vec::with_capacity(capacity),
            pending_frames: 0,
            pending_start_ns: 0,
            pending_end_ns: 0,
            chunk_frames,
            last_packet_start_ns: None,
        })
    }
    fn header(
        &self,
        start: u64,
        count: u64,
        start_ns: u64,
        end_ns: u64,
        kind: ChunkKind,
    ) -> ChunkHeader {
        let stream = &self.descriptor.streams[0];
        ChunkHeader {
            version: CHUNK_VERSION,
            take_id: self.descriptor.take_id.clone(),
            stream_id: stream.stream_id.clone(),
            segment_id: stream.segment_id.clone(),
            sequence: self.sequence,
            range: SourceRange::AudioFrames { start, count },
            interpretation: stream.kind.clone(),
            observed_start: HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: start_ns,
            },
            observed_end: HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: end_ns,
            },
            clock_anchors: vec![],
            kind,
        }
    }
    fn flush(&mut self) -> Result<(), String> {
        if self.pending_frames > 0 {
            let header = self.header(
                self.next - self.pending_frames,
                self.pending_frames,
                self.pending_start_ns,
                self.pending_end_ns,
                ChunkKind::Content,
            );
            self.journal
                .append(&header, &self.bytes)
                .map_err(|e| e.to_string())?;
            self.sequence += 1;
            self.pending_frames = 0;
            self.bytes.clear();
        }
        Ok(())
    }
    fn gap_to(&mut self, end: u64, end_ns: u64) -> Result<(), String> {
        if end < self.next {
            return Err("Capture source positions regressed".into());
        }
        if end > self.next {
            if end_ns < self.last_ns {
                return Err("Capture gap observation clock regressed".into());
            }
            self.flush()?;
            let count = end - self.next;
            let header = self.header(
                self.next,
                count,
                self.last_ns,
                end_ns,
                ChunkKind::Gap {
                    reason: GapReason::TransferOverflow,
                },
            );
            self.journal
                .append(&header, &[])
                .map_err(|e| e.to_string())?;
            self.sequence += 1;
            self.next = end;
            self.gaps += count;
            self.last_ns = end_ns.max(self.last_ns);
        }
        Ok(())
    }
    pub fn append_packet(&mut self, packet: &AudioPacket) -> Result<(), String> {
        let omniusong_recording::StreamKind::Audio { format, .. } =
            &self.descriptor.streams[0].kind
        else {
            return Err("Capture descriptor is not PCM".into());
        };
        if packet.frames == 0
            || packet.frames as usize > PACKET_FRAMES
            || packet.channels != format.channels
            || format.encoding != omniusong_recording::PcmEncoding::F32Le
            || format.layout != omniusong_recording::PcmLayout::Interleaved
            || packet.clock_end_ns < packet.clock_start_ns
            || self
                .last_packet_start_ns
                .is_some_and(|last| packet.clock_start_ns < last)
        {
            return Err("Invalid capture packet".into());
        }
        let sample_count = (packet.frames as usize)
            .checked_mul(packet.channels as usize)
            .filter(|n| *n <= packet.samples.len())
            .ok_or("Capture packet sample extent exceeds backing storage")?;
        let source_end = packet
            .start_frame
            .checked_add(packet.frames as u64)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or("Capture source extent overflow")?;
        if packet.samples[..sample_count]
            .iter()
            .any(|s| !s.is_finite())
        {
            return Err(
                "Nonfinite source PCM; packet rejected without changing accepted pending content"
                    .into(),
            );
        }
        self.gap_to(packet.start_frame, packet.clock_start_ns)?;
        if self.pending_frames + packet.frames as u64 > self.chunk_frames {
            self.flush()?;
        }
        if self.pending_frames == 0 {
            self.pending_start_ns = packet.clock_start_ns;
        }
        for sample in &packet.samples[..sample_count] {
            self.bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.pending_frames += packet.frames as u64;
        self.pending_end_ns = packet.clock_end_ns;
        self.next = source_end;
        self.last_packet_start_ns = Some(packet.clock_start_ns);
        self.last_ns = packet.clock_end_ns;
        if self.pending_frames >= self.chunk_frames {
            self.flush()?;
        }
        Ok(())
    }
    /// Caller must drain the queue after the producer lifetime acknowledgment before calling.
    /// Abandonment never seals, even if its final known prefix is complete.
    pub fn finish(
        mut self,
        snapshot: MeterSnapshot,
        explicit: bool,
    ) -> Result<Option<CapturedTake>, String> {
        if !snapshot.producer_destroyed {
            return Err(format!(
                "Input callback lifetime not finished; unsealed original retained at {}",
                self.path.display()
            ));
        }
        self.flush()?;
        if snapshot.faults & !TRANSFER_OVERFLOW != 0 {
            return Err(format!(
                "Unknown device/input loss; unsealed original retained at {}",
                self.path.display()
            ));
        }
        let expected_gaps = snapshot
            .frames
            .checked_sub(self.next)
            .and_then(|tail| tail.checked_add(self.gaps));
        if expected_gaps != Some(snapshot.dropped_frames) {
            return Err(format!(
                "Undrained or inconsistent capture transfer; unsealed original retained at {}",
                self.path.display()
            ));
        }
        self.gap_to(snapshot.frames, snapshot.clock_end_ns)?;
        if !explicit {
            return Ok(None);
        }
        if self.next == 0 {
            return Err(format!(
                "No captured frames; unsealed original retained at {}",
                self.path.display()
            ));
        }
        let sealed = self
            .journal
            .seal(&uuid::Uuid::new_v4().to_string())
            .map_err(|e| format!("{}; original retained at {}", e, self.path.display()))?;
        let omniusong_recording::StreamKind::Audio { format, .. } =
            &self.descriptor.streams[0].kind
        else {
            unreachable!()
        };
        Ok(Some(CapturedTake {
            journal_path: self.path,
            take_id: self.descriptor.take_id.0.clone(),
            manifest_sha256: sealed.sha256,
            frames: self.next,
            sample_rate: format.sample_rate,
            channels: format.channels,
            gap_frames: self.gaps,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::{TRANSFER_OVERFLOW, capture_queue};
    use omniusong_recording::*;
    use std::collections::BTreeMap;
    fn fixture() -> (PathBuf, TakeDescriptor) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/engine-unit")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
        (
            dir.join("take.sqlite3"),
            TakeDescriptor {
                version: JOURNAL_VERSION,
                take_id: TakeId(uuid::Uuid::new_v4().to_string()),
                streams: vec![StreamDescriptor {
                    stream_id: "audio".into(),
                    segment_id: "segment".into(),
                    source_id: "device".into(),
                    source_name: "synthetic device-independent test".into(),
                    source_connection_generation: 1,
                    host_clock: HostClock {
                        domain: ClockDomainId("test-clock".into()),
                        epoch: epoch.clone(),
                        ticks_per_second_numerator: 1_000_000_000,
                        ticks_per_second_denominator: 1,
                    },
                    kind: StreamKind::Audio {
                        format: PcmFormat {
                            sample_rate: 48000,
                            channels: 1,
                            encoding: PcmEncoding::F32Le,
                            layout: PcmLayout::Interleaved,
                        },
                        audio_epoch: epoch,
                    },
                    initial_source_position: 0,
                }],
                provenance: BTreeMap::new(),
            },
        )
    }
    #[test]
    fn exact_trailing_transfer_loss_seals_only_after_explicit_finish() {
        let (path, d) = fixture();
        let mut j = CaptureJournal::create(&path, d).unwrap();
        let (mut ingress, mut queue, m) = capture_queue(1, 48000, 1).unwrap();
        ingress.ingest_f32(&[0.5; 512], 1_000_000);
        j.append_packet(&queue.pop().unwrap()).unwrap();
        drop(ingress);
        let result = j.finish(m.snapshot(), true).unwrap().unwrap();
        assert_eq!(result.frames, 512);
        assert_eq!(result.gap_frames, 256);
        let reader = SealedJournal::open(path).unwrap();
        assert_eq!(reader.manifest().body.chunk_count, 2);
    }
    #[test]
    fn omitted_accepted_packet_cannot_be_mislabeled_as_loss() {
        let (path, d) = fixture();
        let j = CaptureJournal::create(&path, d).unwrap();
        let (mut ingress, _queue, m) = capture_queue(1, 48000, 1).unwrap();
        ingress.ingest_f32(&[0.5; 32], 0);
        drop(ingress);
        assert!(j.finish(m.snapshot(), true).is_err());
        assert!(SealedJournal::open(&path).is_err());
        assert!(
            Journal::open(&path)
                .unwrap()
                .recovery_report()
                .unwrap()
                .unknown_tail
        );
    }
    #[test]
    fn abandonment_retains_unsealed_original() {
        let (path, d) = fixture();
        let mut j = CaptureJournal::create(&path, d).unwrap();
        let (mut ingress, mut queue, m) = capture_queue(1, 48000, 1).unwrap();
        ingress.ingest_f32(&[0.5; 32], 0);
        j.append_packet(&queue.pop().unwrap()).unwrap();
        drop(ingress);
        assert!(j.finish(m.snapshot(), false).unwrap().is_none());
        assert!(SealedJournal::open(&path).is_err());
        let r = Journal::open(&path).unwrap().recovery_report().unwrap();
        assert!(r.unknown_tail);
        assert_eq!(r.verified_chunks, 1);
    }
    #[test]
    fn false_dropped_count_rejects_and_invalid_descriptor_never_panics() {
        let (path, mut d) = fixture();
        if let StreamKind::Audio { format, .. } = &mut d.streams[0].kind {
            format.channels = 16;
        }
        assert!(CaptureJournal::create(&path, d).is_err());
        let (path, d) = fixture();
        let j = CaptureJournal::create(&path, d).unwrap();
        let (_, _, m) = capture_queue(1, 48000, 1).unwrap();
        let mut snapshot = m.snapshot();
        snapshot.producer_destroyed = true;
        snapshot.frames = 8;
        snapshot.dropped_frames = 7;
        snapshot.faults = TRANSFER_OVERFLOW;
        assert!(j.finish(snapshot, true).is_err());
    }
}
