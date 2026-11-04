//! Fixed-storage data-path primitives. Construct and destroy these off the audio callback.
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

pub const PACKET_FRAMES: usize = 256;
pub const MAX_CHANNELS: usize = 8;
pub const MAX_CALLBACK_FRAMES: usize = 16_384;
pub const RING_SLOTS: usize = 256;
pub const TRANSFER_OVERFLOW: u32 = 1;
pub const UNKNOWN_DEVICE_FAULT: u32 = 2;
pub const INVALID_INPUT: u32 = 4;
#[derive(Clone, Copy)]
pub struct AudioPacket {
    pub start_frame: u64,
    pub frames: u16,
    pub channels: u16,
    pub clock_start_ns: u64,
    pub clock_end_ns: u64,
    pub samples: [f32; PACKET_FRAMES * MAX_CHANNELS],
}
#[derive(Default)]
pub struct Meters {
    pub observed_frames: AtomicU64,
    pub dropped_frames: AtomicU64,
    pub clock_end_ns: AtomicU64,
    pub peak_bits: AtomicU32,
    pub faults: AtomicU32,
    pub xruns: AtomicU64,
    pub producer_destroyed: AtomicBool,
    pub playback_end_ns: AtomicU64,
    pub callback_clock_ns: AtomicU64,
    pub render_finished: AtomicBool,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterSnapshot {
    pub frames: u64,
    pub dropped_frames: u64,
    pub clock_end_ns: u64,
    pub peak: f32,
    pub faults: u32,
    pub xruns: u64,
    pub producer_destroyed: bool,
}
impl Meters {
    pub fn snapshot(&self) -> MeterSnapshot {
        let producer_destroyed = self.producer_destroyed.load(Ordering::Acquire);
        MeterSnapshot {
            frames: self.observed_frames.load(Ordering::Acquire),
            dropped_frames: self.dropped_frames.load(Ordering::Acquire),
            clock_end_ns: self.clock_end_ns.load(Ordering::Acquire),
            peak: f32::from_bits(self.peak_bits.load(Ordering::Relaxed)),
            faults: self.faults.load(Ordering::Acquire),
            xruns: self.xruns.load(Ordering::Relaxed),
            producer_destroyed,
        }
    }
    pub fn device_fault(&self) {
        self.faults
            .fetch_or(UNKNOWN_DEVICE_FAULT, Ordering::Release);
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }
}
pub struct CaptureIngress {
    producer: Option<Producer<AudioPacket>>,
    meters: Arc<Meters>,
    channels: usize,
    rate: u32,
    next: u64,
    last_start_ns: Option<u64>,
}
pub fn capture_queue(
    channels: u16,
    rate: u32,
    slots: usize,
) -> Result<(CaptureIngress, Consumer<AudioPacket>, Arc<Meters>), String> {
    if channels == 0
        || channels as usize > MAX_CHANNELS
        || !(8000..=768000).contains(&rate)
        || slots == 0
        || slots > RING_SLOTS
    {
        return Err("Unsupported capture queue configuration".into());
    }
    let (producer, consumer) = RingBuffer::new(slots);
    let meters = Arc::new(Meters::default());
    Ok((
        CaptureIngress {
            producer: Some(producer),
            meters: meters.clone(),
            channels: channels as usize,
            rate,
            next: 0,
            last_start_ns: None,
        },
        consumer,
        meters,
    ))
}
impl CaptureIngress {
    pub fn ingest_f32(&mut self, data: &[f32], start_ns: u64) {
        self.ingest(data, start_ns, |v| v);
    }
    pub fn ingest_i16(&mut self, data: &[i16], start_ns: u64) {
        self.ingest(data, start_ns, |v| v as f32 / 32768.0);
    }
    pub fn ingest_u16(&mut self, data: &[u16], start_ns: u64) {
        self.ingest(data, start_ns, |v| (v as f32 - 32768.0) / 32768.0);
    }
    fn ingest<T: Copy>(&mut self, data: &[T], start_ns: u64, convert: impl Fn(T) -> f32) {
        // Empty is the patched CoreAudio adapter's render/oversize failure sentinel.
        if data.is_empty() {
            self.meters.device_fault();
            return;
        }
        if !data.len().is_multiple_of(self.channels) {
            self.meters
                .faults
                .fetch_or(INVALID_INPUT, Ordering::Release);
            return;
        }
        let frames = data.len() / self.channels;
        // Host observations may overlap the previous inferred block end by
        // jitter/rounding; a regressing observed START is a different clock fault.
        if self.last_start_ns.is_some_and(|last| start_ns < last) {
            self.meters
                .faults
                .fetch_or(INVALID_INPUT, Ordering::Release);
        }
        self.last_start_ns = Some(start_ns);
        let Some(end) = self
            .next
            .checked_add(frames as u64)
            .filter(|n| *n <= i64::MAX as u64)
        else {
            self.meters
                .faults
                .fetch_or(INVALID_INPUT, Ordering::Release);
            return;
        };
        let duration = (frames as u64).saturating_mul(1_000_000_000) / self.rate as u64;
        let Some(end_ns) = start_ns.checked_add(duration) else {
            self.meters
                .faults
                .fetch_or(INVALID_INPUT, Ordering::Release);
            return;
        };
        if frames > MAX_CALLBACK_FRAMES {
            self.meters.device_fault();
        }
        let mut peak = 0.0f32;
        // Once overflow occurs every later callback is counted as loss until control stops it.
        if self.meters.faults.load(Ordering::Acquire) != 0 {
            self.meters
                .dropped_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
        } else {
            for offset in (0..frames).step_by(PACKET_FRAMES) {
                let count = (frames - offset).min(PACKET_FRAMES);
                let mut packet = AudioPacket {
                    start_frame: self.next + offset as u64,
                    frames: count as u16,
                    channels: self.channels as u16,
                    clock_start_ns: start_ns + (offset as u64 * 1_000_000_000 / self.rate as u64),
                    clock_end_ns: start_ns
                        + ((offset + count) as u64 * 1_000_000_000 / self.rate as u64),
                    samples: [0.0; PACKET_FRAMES * MAX_CHANNELS],
                };
                for (dst, src) in packet.samples[..count * self.channels]
                    .iter_mut()
                    .zip(&data[offset * self.channels..(offset + count) * self.channels])
                {
                    let sample = convert(*src);
                    if !sample.is_finite() {
                        self.meters
                            .faults
                            .fetch_or(INVALID_INPUT, Ordering::Release);
                    }
                    *dst = sample;
                    peak = peak.max(sample.abs());
                }
                if self.meters.faults.load(Ordering::Acquire) != 0
                    || self
                        .producer
                        .as_mut()
                        .expect("live producer")
                        .push(packet)
                        .is_err()
                {
                    self.meters
                        .faults
                        .fetch_or(TRANSFER_OVERFLOW, Ordering::Release);
                    self.meters.xruns.fetch_add(1, Ordering::Relaxed);
                    self.meters
                        .dropped_frames
                        .fetch_add((frames - offset) as u64, Ordering::Relaxed);
                    break;
                }
            }
        }
        if peak.is_finite() {
            self.meters
                .peak_bits
                .fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
        self.next = end;
        self.meters.clock_end_ns.store(end_ns, Ordering::Relaxed);
        self.meters.observed_frames.store(end, Ordering::Release);
    }
}
impl Drop for CaptureIngress {
    fn drop(&mut self) {
        // A zero active-callback count is not a lifetime barrier. This acknowledgment is.
        drop(self.producer.take());
        self.meters
            .producer_destroyed
            .store(true, Ordering::Release);
    }
}
/// Interleaved, already-prepared audio. No source access or allocation in render calls.
pub struct RenderCursor {
    samples: Vec<f32>,
    channels: usize,
    position: usize,
    meters: Arc<Meters>,
}
impl RenderCursor {
    pub fn new(samples: Vec<f32>, channels: u16) -> Result<(Self, Arc<Meters>), String> {
        if channels == 0
            || channels as usize > MAX_CHANNELS
            || !samples.len().is_multiple_of(channels as usize)
            || samples.iter().any(|s| !s.is_finite())
        {
            return Err("Invalid prepared PCM".into());
        }
        let meters = Arc::new(Meters::default());
        Ok((
            Self {
                samples,
                channels: channels as usize,
                position: 0,
                meters: meters.clone(),
            },
            meters,
        ))
    }
    pub fn render_into(&mut self, out: &mut [f32]) {
        self.render(out, |v| v);
    }
    pub fn render_i16(&mut self, out: &mut [i16]) {
        self.render(out, |v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16);
    }
    pub fn render_u16(&mut self, out: &mut [u16]) {
        self.render(out, |v| {
            ((v.clamp(-1.0, 1.0) * 32767.0) + 32768.0).round() as u16
        });
    }
    pub fn render_into_at(
        &mut self,
        out: &mut [f32],
        callback_ns: u64,
        playback_ns: u64,
        rate: u32,
    ) {
        self.render_timed(out, callback_ns, playback_ns, rate, |v| v);
    }
    pub fn render_i16_at(
        &mut self,
        out: &mut [i16],
        callback_ns: u64,
        playback_ns: u64,
        rate: u32,
    ) {
        self.render_timed(out, callback_ns, playback_ns, rate, |v| {
            (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
        });
    }
    pub fn render_u16_at(
        &mut self,
        out: &mut [u16],
        callback_ns: u64,
        playback_ns: u64,
        rate: u32,
    ) {
        self.render_timed(out, callback_ns, playback_ns, rate, |v| {
            ((v.clamp(-1.0, 1.0) * 32767.0) + 32768.0).round() as u16
        });
    }
    fn render_timed<T>(
        &mut self,
        out: &mut [T],
        callback_ns: u64,
        playback_ns: u64,
        rate: u32,
        convert: impl Fn(f32) -> T,
    ) {
        let available = out.len().min(self.samples.len() - self.position);
        self.render(out, convert);
        if available > 0 {
            let end = if rate > 0 {
                playback_ns
                    .checked_add((available / self.channels) as u64 * 1_000_000_000 / rate as u64)
            } else {
                None
            };
            if let Some(end) = end {
                self.meters.playback_end_ns.store(end, Ordering::Release);
            } else {
                self.meters.device_fault();
            }
        }
        self.meters
            .callback_clock_ns
            .store(callback_ns, Ordering::Release);
        if self.position == self.samples.len() {
            self.meters.render_finished.store(true, Ordering::Release);
        }
    }
    fn render<T>(&mut self, out: &mut [T], convert: impl Fn(f32) -> T) {
        if !out.len().is_multiple_of(self.channels) {
            self.meters
                .faults
                .fetch_or(INVALID_INPUT, Ordering::Release);
            for dst in out {
                *dst = convert(0.0);
            }
            return;
        }
        let mut peak = 0.0f32;
        for dst in out.iter_mut() {
            let v = self
                .samples
                .get(self.position)
                .copied()
                .unwrap_or(0.0)
                .clamp(-1.0, 1.0);
            if self.position < self.samples.len() {
                self.position += 1;
            }
            peak = peak.max(v.abs());
            *dst = convert(v);
        }
        self.meters
            .peak_bits
            .store(peak.to_bits(), Ordering::Relaxed);
        self.meters
            .observed_frames
            .store((self.position / self.channels) as u64, Ordering::Release);
    }
    pub fn is_finished(&self) -> bool {
        self.position == self.samples.len()
    }
    pub fn frame_count(&self) -> u64 {
        (self.samples.len() / self.channels) as u64
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_advances_every_frame_and_drop_ack_is_lifetime_end() {
        let (mut p, mut c, m) = capture_queue(1, 48000, 1).unwrap();
        p.ingest_f32(&[0.5; 512], 100);
        p.ingest_f32(&[0.2; 32], 20000);
        assert_eq!(m.snapshot().frames, 544);
        assert_eq!(m.snapshot().dropped_frames, 288);
        assert_eq!(c.pop().unwrap().frames, 256);
        assert!(!m.snapshot().producer_destroyed);
        drop(p);
        assert!(m.snapshot().producer_destroyed);
    }
    #[test]
    fn empty_native_failure_never_looks_contiguous() {
        let (mut p, _, m) = capture_queue(1, 48000, 1).unwrap();
        p.ingest_f32(&[], 0);
        assert_ne!(m.snapshot().faults & UNKNOWN_DEVICE_FAULT, 0);
    }
    #[test]
    fn output_tail_is_silence() {
        let (mut r, _) = RenderCursor::new(vec![0.25, -0.5], 1).unwrap();
        let mut out = [1.0; 5];
        r.render_into(&mut out);
        assert_eq!(out, [0.25, -0.5, 0.0, 0.0, 0.0]);
        assert!(r.is_finished());
    }
    #[test]
    fn supplying_final_buffer_precedes_predicted_playback_end() {
        let (mut r, m) = RenderCursor::new(vec![0.25; 48], 1).unwrap();
        let mut out = [0.0; 48];
        r.render_into_at(&mut out, 100, 1000, 48000);
        assert!(m.render_finished.load(Ordering::Acquire));
        assert!(
            m.callback_clock_ns.load(Ordering::Acquire) < m.playback_end_ns.load(Ordering::Acquire)
        );
        r.render_into_at(&mut out, 1_001_000, 1_002_000, 48000);
        assert!(
            m.callback_clock_ns.load(Ordering::Acquire)
                >= m.playback_end_ns.load(Ordering::Acquire)
        );
        assert_eq!(out, [0.0; 48]);
    }
    #[test]
    fn regressing_observed_start_latches_unknown_loss_but_counts_source_extent() {
        let (mut p, mut queue, m) = capture_queue(1, 48000, 4).unwrap();
        p.ingest_f32(&[0.25; 16], 10_000_000);
        p.ingest_f32(&[0.5; 16], 1_000_000);
        assert_eq!(queue.pop().unwrap().frames, 16);
        assert!(queue.pop().is_err());
        drop(p);
        let final_state = m.snapshot();
        assert_eq!(final_state.frames, 32);
        assert_eq!(final_state.dropped_frames, 16);
        assert_ne!(final_state.faults & INVALID_INPUT, 0);
    }
}
