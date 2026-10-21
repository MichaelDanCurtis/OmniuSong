use crate::{
    capture::CaptureJournal,
    realtime::{self, Meters, RenderCursor, TRANSFER_OVERFLOW},
    transport::{Action, BUILD, Request, Response, architecture, platform},
    *,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use omniusong_core::contracts::{ClockDomainId, ClockEpochId, TakeId, WorkerRole};
use omniusong_recording::{
    HostClock, JOURNAL_VERSION, PcmEncoding, PcmFormat, PcmLayout, StreamDescriptor, StreamKind,
    TakeDescriptor,
};
use omniusong_store::NativeStore;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

fn inventory() -> Result<DeviceInventory, String> {
    let host = cpal::default_host();
    fn collect(devices: impl Iterator<Item = cpal::Device>) -> Result<Vec<DeviceInfo>, String> {
        let mut result = Vec::new();
        for device in devices {
            if result.len() == 64 {
                return Err(
                    "More than 64 audio endpoints; explicit inventory bound exceeded".into(),
                );
            }
            let id = device.id().map_err(|e| e.to_string())?.to_string();
            let name = device.to_string();
            if id.len() > 1024 || name.len() > 1024 {
                return Err("Audio endpoint metadata exceeds engine bound".into());
            }
            result.push(DeviceInfo { id, name });
        }
        Ok(result)
    }
    let result = DeviceInventory {
        inputs: collect(host.input_devices().map_err(|e| e.to_string())?)?,
        outputs: collect(host.output_devices().map_err(|e| e.to_string())?)?,
    };
    if serde_json::to_vec(&result)
        .map_err(|e| e.to_string())?
        .len()
        > 48 * 1024
    {
        return Err("Audio device inventory exceeds the bounded control reply".into());
    }
    Ok(result)
}
fn device(input: bool, selected: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let default = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    let wanted = match selected {
        Some(s) => s.to_owned(),
        None => default
            .ok_or("No default endpoint available")?
            .id()
            .map_err(|e| e.to_string())?
            .to_string(),
    };
    // In particular, do not pass an output-only device to CoreAudio's input builder.
    let devices: Box<dyn Iterator<Item = cpal::Device>> = if input {
        Box::new(host.input_devices().map_err(|e| e.to_string())?)
    } else {
        Box::new(host.output_devices().map_err(|e| e.to_string())?)
    };
    for d in devices.take(65) {
        if d.id().map_err(|e| e.to_string())?.to_string() == wanted {
            return Ok(d);
        }
    }
    Err("Selected endpoint is unavailable or not capable of the requested direction".into())
}
pub(crate) fn output_device(selected: Option<&str>) -> Result<cpal::Device, String> {
    device(false, selected)
}
fn root_existing(root: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Native audio root must be an existing ordinary directory".into());
    }
    let canonical = fs::canonicalize(root).map_err(|e| e.to_string())?;
    if canonical != root {
        return Err("Native audio root must be canonical".into());
    }
    NativeStore::open_existing(&canonical).map_err(|e| e.to_string())?;
    Ok(canonical)
}
fn private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|e| e.to_string())
}
fn capture_path(root: &Path) -> Result<PathBuf, String> {
    root_existing(root)?;
    let captures = root.join("captures");
    match fs::symlink_metadata(&captures) {
        Ok(meta) => {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("Capture directory is not an ordinary owned directory".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            private_directory(&captures)?;
        }
        Err(e) => return Err(e.to_string()),
    }
    if fs::canonicalize(&captures).map_err(|e| e.to_string())? != captures {
        return Err("Capture directory redirects outside the native root".into());
    }
    let private = captures.join(uuid::Uuid::new_v4().to_string());
    private_directory(&private)?;
    Ok(private.join("take.sqlite3"))
}
struct Playback {
    stream: cpal::Stream,
    meters: Arc<Meters>,
    frames: u64,
    rate: u32,
    channels: u16,
    retire: Arc<AtomicBool>,
}
impl Playback {
    fn status(&self) -> EngineStatus {
        let rendered = self.meters.render_finished.load(Ordering::Acquire);
        let m = self.meters.snapshot();
        EngineStatus {
            state: if rendered
                && m.frames >= self.frames
                && self.meters.playback_end_ns.load(Ordering::Acquire) > 0
                && self.meters.callback_clock_ns.load(Ordering::Acquire)
                    >= self.meters.playback_end_ns.load(Ordering::Acquire)
            {
                "playback_finished"
            } else {
                "playing"
            }
            .into(),
            capture: None,
            sample_rate: self.rate,
            channels: self.channels,
            frames: m.frames,
            peak: m.peak,
            xruns: m.xruns,
        }
    }
    fn stop(self) -> Result<(), String> {
        if let Err(error) = self.stream.pause() {
            self.retire.store(true, Ordering::Release);
            let message = format!("Output pause failed; audio worker retired: {error}");
            std::mem::forget(self); // bounded child retirement; no unsafe driver teardown
            return Err(message);
        }
        drop(self);
        Ok(())
    }
}
struct ErrorSink {
    meters: Arc<Meters>,
    destroyed: Arc<AtomicBool>,
}
impl ErrorSink {
    fn fault(&self) {
        self.meters.device_fault();
    }
}
impl Drop for ErrorSink {
    fn drop(&mut self) {
        self.destroyed.store(true, Ordering::Release);
    }
}
struct Capture {
    stream: Option<cpal::Stream>,
    meters: Arc<Meters>,
    mode: Arc<AtomicU32>,
    writer: Option<JoinHandle<Result<Option<CapturedTake>, String>>>,
    identity: CaptureIdentity,
    rate: u32,
    channels: u16,
    quiesced: bool,
    error_destroyed: Arc<AtomicBool>,
    retire: Arc<AtomicBool>,
}
impl Capture {
    fn start(root: &Path, selected: Option<&str>, retire: Arc<AtomicBool>) -> Result<Self, String> {
        let d = device(true, selected)?;
        let supported = d.default_input_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        if !matches!(
            supported.sample_format(),
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err("Input native sample format unsupported; only float32/int16/uint16 convert losslessly to stored float32".into());
        }
        let (mut ingress, mut consumer, meters) =
            realtime::capture_queue(config.channels, config.sample_rate, realtime::RING_SLOTS)?;
        let journal_path = capture_path(root)?;
        let take_id = uuid::Uuid::new_v4().to_string();
        let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
        let domain = ClockDomainId(uuid::Uuid::new_v4().to_string());
        let mut provenance = BTreeMap::new();
        provenance.insert("omniusong:engine".into(),serde_json::json!({"version":1,"deviceId":d.id().map_err(|e|e.to_string())?.to_string(),"nativeSampleFormat":format!("{:?}",supported.sample_format()),"sampleRate":config.sample_rate,"channels":config.channels,"timestamp":"CPAL capture host-time estimate; stream local nanoseconds; no cross-device calibration","normalization":"native float32 or lossless 16-bit to float32; originals retained without editing","monitoring":false}));
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(take_id.clone()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: uuid::Uuid::new_v4().to_string(),
                source_id: uuid::Uuid::new_v4().to_string(),
                source_name: d.to_string(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain,
                    epoch: epoch.clone(),
                    ticks_per_second_numerator: 1_000_000_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: config.sample_rate,
                        channels: config.channels,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: epoch,
                },
                initial_source_position: 0,
            }],
            provenance,
        };
        let journal = CaptureJournal::create(&journal_path, descriptor)?;
        let mode = Arc::new(AtomicU32::new(0));
        let writer_mode = mode.clone();
        let writer_meters = meters.clone();
        let writer_path = journal_path.clone();
        let writer = thread::Builder::new()
            .name("audio-journal".into())
            .spawn(move || {
                let mut journal = journal;
                loop {
                    while let Ok(packet) = consumer.pop() {
                        if let Err(e) = journal.append_packet(&packet) {
                            writer_meters.device_fault();
                            return Err(format!(
                                "{e}; original retained at {}",
                                writer_path.display()
                            ));
                        }
                    }
                    let state = writer_mode.load(Ordering::Acquire);
                    if state != 0 {
                        while let Ok(packet) = consumer.pop() {
                            journal.append_packet(&packet)?;
                        }
                        return journal.finish(writer_meters.snapshot(), state == 2);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            })
            .map_err(|e| e.to_string())?;
        let error_destroyed = Arc::new(AtomicBool::new(false));
        let errors = ErrorSink {
            meters: meters.clone(),
            destroyed: error_destroyed.clone(),
        };
        let timestamp = |info: &cpal::InputCallbackInfo| {
            u64::try_from(info.timestamp().capture.as_nanos()).unwrap_or(u64::MAX)
        };
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => d.build_input_stream(
                config,
                move |data: &[f32], info| ingress.ingest_f32(data, timestamp(info)),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::I16 => d.build_input_stream(
                config,
                move |data: &[i16], info| ingress.ingest_i16(data, timestamp(info)),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::U16 => d.build_input_stream(
                config,
                move |data: &[u16], info| ingress.ingest_u16(data, timestamp(info)),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            _ => unreachable!(),
        };
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                mode.store(1, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(2);
                while (!meters.producer_destroyed.load(Ordering::Acquire)
                    || !error_destroyed.load(Ordering::Acquire))
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(2));
                }
                if meters.producer_destroyed.load(Ordering::Acquire)
                    && error_destroyed.load(Ordering::Acquire)
                {
                    let _ = writer.join();
                } else {
                    retire.store(true, Ordering::Release);
                    drop(writer);
                }
                return Err(format!(
                    "Input setup failed: {e}; unsealed original retained at {}",
                    journal_path.display()
                ));
            }
        };
        let capture = Self {
            stream: Some(stream),
            meters,
            mode,
            writer: Some(writer),
            identity: CaptureIdentity {
                journal_path,
                take_id,
            },
            rate: config.sample_rate,
            channels: config.channels,
            quiesced: false,
            error_destroyed,
            retire,
        };
        if let Err(e) = capture.stream.as_ref().expect("stream").play() {
            let path = capture.identity.journal_path.clone();
            let _ = capture.finish(false);
            return Err(format!(
                "Input start failed: {e}; unsealed original retained at {}",
                path.display()
            ));
        }
        Ok(capture)
    }
    fn stop_input(&mut self) -> Result<(), String> {
        if self.stream.is_none() {
            return if self.quiesced {
                Ok(())
            } else {
                Err("Input callback quiescence was not established".into())
            };
        }
        let stream = self.stream.take().expect("input stream");
        let paused = stream.pause().map_err(|e| e.to_string());
        if let Err(error) = paused {
            self.retire.store(true, Ordering::Release);
            self.meters.device_fault();
            // The backend's destructor retries stop and frees callback storage.
            // Preserve that ownership until immediate OS child retirement rather
            // than freeing it after the failed stop proof. This is process-local,
            // bounded by the terminal reply deadline/watchdog, never an RT path.
            std::mem::forget(stream);
            return Err(format!(
                "Input pause failed; original retained unsealed: {}",
                error
            ));
        }
        drop(stream);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.meters.producer_destroyed.load(Ordering::Acquire)
            || !self.error_destroyed.load(Ordering::Acquire)
        {
            if Instant::now() >= deadline {
                self.retire.store(true, Ordering::Release);
                self.meters.device_fault();
                return Err("Input producer destruction acknowledgment timed out; original retained unsealed".into());
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.quiesced = true;
        Ok(())
    }
    fn status(&mut self) -> EngineStatus {
        let mut m = self.meters.snapshot();
        if m.faults != 0 && self.stream.is_some() {
            let _ = self.stop_input();
            m = self.meters.snapshot();
        }
        EngineStatus {
            state: if m.faults & !TRANSFER_OVERFLOW != 0 {
                "capture_fault"
            } else if m.faults & TRANSFER_OVERFLOW != 0 {
                "capture_stopped_transfer_overflow"
            } else {
                "capturing"
            }
            .into(),
            capture: Some(self.identity.clone()),
            sample_rate: self.rate,
            channels: self.channels,
            frames: m.frames,
            peak: m.peak,
            xruns: m.xruns,
        }
    }
    fn finish(mut self, explicit: bool) -> Result<Option<CapturedTake>, String> {
        let stopped = self.stop_input();
        if self.retire.load(Ordering::Acquire) {
            self.mode.store(1, Ordering::Release);
            // No join: the producer lifetime is unproven. The control loop
            // returns the known failure then exits this isolated process.
            return Err(format!(
                "Audio worker retired after failed callback quiescence; unsealed original retained at {}",
                self.identity.journal_path.display()
            ));
        }
        let seal = explicit && stopped.is_ok();
        self.mode.store(if seal { 2 } else { 1 }, Ordering::Release);
        let result = self
            .writer
            .take()
            .ok_or("Capture writer missing")?
            .join()
            .map_err(|_| "Capture writer panicked; original retained unsealed".to_string())?;
        stopped?;
        result
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        if self.stream.is_some() {
            let _ = self.stop_input();
        }
        self.mode.store(1, Ordering::Release); /* Never join on exceptional unwinding; actor retirement remains bounded. */
    }
}
struct LiveOutput {
    stream: Option<cpal::Stream>,
    admission: live::LiveAdmission,
    clock: LiveClock,
    output: DeviceInfo,
    rate: u32,
    channels: u16,
    retire: Arc<AtomicBool>,
}
impl LiveOutput {
    fn start(
        generation: LiveId,
        selected: Option<&str>,
        clock: LiveClock,
        retire: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let device = device(false, selected)?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let output = DeviceInfo {
            id: device.id().map_err(|e| e.to_string())?.to_string(),
            name: device.to_string(),
        };
        if !clock.is_valid()
            || !(8000..=768000).contains(&config.sample_rate)
            || !(1..=8).contains(&config.channels)
            || output.id.len() > 1024
            || output.name.len() > 1024
        {
            return Err("Unsupported live output configuration or clock".into());
        }
        if !matches!(
            supported.sample_format(),
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err("Unsupported live output sample format".into());
        }
        let scope = LiveScope {
            worker_generation: generation,
            session_id: LiveId::random(),
            panic_generation: 1,
        };
        let (admission, mut renderer) =
            live::live_pair(scope, config.sample_rate, config.channels)?;
        let control = admission.control();
        let errors = live::LiveErrorGuard::new(control.clone());
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |data: &mut [f32], _| renderer.render_into(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                config,
                move |data: &mut [i16], _| renderer.render_i16(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                config,
                move |data: &mut [u16], _| renderer.render_u16(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            _ => unreachable!(),
        };
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                let deadline = Instant::now() + Duration::from_secs(2);
                while (!control.render_destroyed() || !control.error_destroyed())
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(1));
                }
                if !control.render_destroyed() || !control.error_destroyed() {
                    retire.store(true, Ordering::Release);
                    std::mem::forget(admission);
                }
                return Err(format!("Live output setup failed: {error}"));
            }
        };
        if let Err(error) = stream.play() {
            retire.store(true, Ordering::Release);
            std::mem::forget(stream);
            std::mem::forget(admission);
            return Err(format!("Live output start failed; worker retired: {error}"));
        }
        Ok(Self {
            stream: Some(stream),
            admission,
            clock,
            output,
            rate: config.sample_rate,
            channels: config.channels,
            retire,
        })
    }
    fn ready(&self) -> LiveReady {
        LiveReady {
            scope: self.admission.scope(),
            gui_clock: self.clock.clone(),
            sources: [
                LiveSourceKind::ComputerKeys,
                LiveSourceKind::PointerKeys,
                LiveSourceKind::Pads,
                LiveSourceKind::AccessiblePulse,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| LiveSourceBinding {
                kind,
                handle: (i + 1) as u8,
                connection_generation: 1,
            })
            .collect(),
            output: self.output.clone(),
            sample_rate: self.rate,
            channels: self.channels,
            progress: self.admission.progress(),
        }
    }
    fn panic(&mut self, scope: LiveScope) -> Result<LiveProgress, String> {
        self.admission
            .begin_panic(scope)
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.admission.finish_panic() {
                Ok(Some(progress)) => return Ok(progress),
                Err(e) => return Err(e.to_string()),
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                self.admission
                    .control()
                    .fault(LiveFaultKind::CallbackQuiescence, None);
                return Err("Live panic callback fence timed out; Stop is required".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn stop(mut self) -> Result<LiveProgress, String> {
        let stream = self.stream.take().ok_or("Live stream ownership missing")?;
        if let Err(error) = stream.pause() {
            self.retire.store(true, Ordering::Release);
            std::mem::forget(stream);
            std::mem::forget(self);
            return Err(format!("Live output pause failed; worker retired: {error}"));
        }
        drop(stream);
        let control = self.admission.control();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !control.render_destroyed() || !control.error_destroyed() {
            if Instant::now() >= deadline {
                control.fault(LiveFaultKind::CallbackQuiescence, None);
                self.retire.store(true, Ordering::Release);
                std::mem::forget(self);
                return Err(
                    "Live callback destruction acknowledgment timed out; worker retired".into(),
                );
            }
            thread::sleep(Duration::from_millis(1));
        }
        let mut progress = self.admission.progress();
        progress.state = LiveState::Stopped;
        progress.active_voices = 0;
        progress.release_tail_voices = 0;
        progress.lease_credit = None;
        Ok(progress)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PendingPerformanceStop {
    ticket: UrgentTicket,
    cancelled_through_control: u64,
    superseded_panic: Option<(UrgentTicket, LiveScope)>,
}
struct Runtime {
    generation: LiveId,
    performance: Option<crate::performance_session::PerformanceSession>,
    performance_request: Option<PerformanceStart>,
    performance_announced: bool,
    pending_performance_stop: Option<PendingPerformanceStop>,
    live: Option<LiveOutput>,
    last_live: Option<LiveProgress>,
    first_fault: Option<LiveFault>,
    urgent: UrgentProgress,
    pending_panic: Option<(UrgentTicket, LiveScope)>,
    update_revision: u64,
    root: PathBuf,
    store: NativeStore,
    playback: Option<Playback>,
    capture: Option<Capture>,
    last: Option<EngineStatus>,
    retire: Arc<AtomicBool>,
}
impl Runtime {
    fn new(root: PathBuf, generation: LiveId) -> Result<Self, String> {
        let root = root_existing(&root)?;
        let store = NativeStore::open_existing(&root).map_err(|e| e.to_string())?;
        Ok(Self {
            generation,
            performance: None,
            performance_request: None,
            performance_announced: false,
            pending_performance_stop: None,
            live: None,
            last_live: None,
            first_fault: None,
            urgent: UrgentProgress::default(),
            pending_panic: None,
            update_revision: 0,
            root,
            store,
            playback: None,
            capture: None,
            last: None,
            retire: Arc::new(AtomicBool::new(false)),
        })
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(session) = &mut self.performance {
            session.cancel("Stop requested; original performance retained");
        }
        let mut failure = None;
        if let Some(c) = self.capture.take()
            && let Err(e) = c.finish(false)
        {
            failure = Some(e);
        }
        if let Some(p) = self.playback.take()
            && let Err(e) = p.stop()
        {
            failure = Some(e);
        }
        if let Some(live) = self.live.take() {
            match live.stop() {
                Ok(progress) => {
                    if self.first_fault.is_none() {
                        self.first_fault = progress.fault;
                    }
                    self.last_live = Some(progress);
                }
                Err(e) => failure = Some(e),
            }
        }
        self.last = None;
        if let Some(e) = failure {
            Err(e)
        } else {
            Ok(())
        }
    }
    fn status(&mut self) -> Result<EngineStatus, String> {
        if let Some(c) = &mut self.capture {
            return Ok(c.status());
        }
        if let Some(p) = &self.playback {
            if p.meters.snapshot().faults != 0 {
                let p = self.playback.take().unwrap();
                let _ = p.stop();
                return Err("Output device reported an error; playback stopped".into());
            }
            let status = p.status();
            if status.state == "playback_finished" {
                self.playback.take().unwrap().stop()?;
                self.last = Some(status.clone());
            }
            return Ok(status);
        }
        Ok(self.last.clone().unwrap_or(EngineStatus {
            state: "idle".into(),
            capture: None,
            sample_rate: 0,
            channels: 0,
            frames: 0,
            peak: 0.0,
            xruns: 0,
        }))
    }
    fn announced_performance_progress(
        &mut self,
        progress: Option<LiveProgress>,
    ) -> Option<LiveProgress> {
        if !self.performance_announced {
            return None;
        }
        // Playback can replace the quiescent performance owner. Its previously
        // announced live scope must remain in cumulative updates until a new
        // authenticated Ready replaces it, just like a stopped audition scope.
        if let Some(progress) = progress
            && progress.scope.worker_generation == self.generation
        {
            self.last_live = Some(progress);
        }
        progress
    }
    fn update(&mut self) -> Result<EngineUpdate, String> {
        let performance = if let Some(session) = &mut self.performance {
            if let Some((ticket, scope)) = self.pending_panic {
                match session.finish_panic() {
                    Ok(Some(progress)) => {
                        self.urgent.panic = Some(PanicAck {
                            ticket,
                            requested_scope: scope,
                            outcome: PanicOutcome::Applied {
                                new_scope: progress.scope,
                            },
                        });
                        self.pending_panic = None;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        session.cancel("Performance Panic failed; originals retained");
                    }
                }
            }
            let status = session.snapshot();
            if session.is_quiescent()
                && let Some(stop) = self.pending_performance_stop.take()
            {
                let PendingPerformanceStop {
                    ticket,
                    cancelled_through_control,
                    superseded_panic: superseded,
                } = stop;
                self.urgent.stop = Some(StopAck {
                    ticket,
                    cancelled_through_control,
                });
                if let Some((panic_ticket, scope)) = superseded.or(self.pending_panic.take()) {
                    self.urgent.panic = Some(PanicAck {
                        ticket: panic_ticket,
                        requested_scope: scope,
                        outcome: PanicOutcome::SupersededByStop {
                            stop_ticket: ticket,
                        },
                    });
                }
            }
            status.live
        } else {
            None
        };
        let performance = self.announced_performance_progress(performance);
        let progress = self
            .live
            .as_ref()
            .map(|live| live.admission.progress())
            .or(performance)
            .or(self.last_live);
        if self.first_fault.is_none() {
            self.first_fault = progress.and_then(|p| p.fault);
        }
        self.update_revision = self.update_revision.checked_add(1).ok_or_else(|| {
            self.retire.store(true, Ordering::Release);
            "Live update sequence exhausted".to_string()
        })?;
        Ok(EngineUpdate {
            worker_generation: self.generation,
            revision: self.update_revision,
            live: progress,
            urgent: self.urgent,
            first_fault: self.first_fault,
        })
    }
    fn retirement_error(&self, fallback: &str) -> String {
        self.performance
            .as_ref()
            .and_then(|session| session.retirement_error())
            .unwrap_or(fallback)
            .to_owned()
    }
    fn live_action(&mut self, action: Action) -> Result<EngineResult, String> {
        if self.retire.load(Ordering::Acquire) {
            return Err(self.retirement_error("Audio worker generation is retired"));
        }
        match action {
            Action::LiveBatch { batch } => {
                if let Some(session) = &mut self.performance {
                    session
                        .admit(batch)
                        .map(EngineResult::LiveStatus)
                        .map_err(|e| e.to_string())
                } else {
                    self.live
                        .as_mut()
                        .ok_or("Live audition is not active")?
                        .admission
                        .admit(batch)
                        .map(EngineResult::LiveStatus)
                        .map_err(|e| e.to_string())
                }
            }
            Action::LiveFault {
                scope,
                kind,
                sequence,
            } => {
                if let Some(session) = &mut self.performance {
                    return session
                        .fault(scope, kind, sequence)
                        .map(EngineResult::LiveStatus)
                        .map_err(|e| e.to_string());
                }
                let live = self.live.as_mut().ok_or("Live audition is not active")?;
                if live.admission.scope() != scope {
                    return Err("Stale live fault scope".into());
                }
                live.admission.control().fault(kind, sequence);
                Ok(EngineResult::LiveStatus(live.admission.progress()))
            }
            Action::PanicLive { ticket, scope } => {
                if ticket.worker_generation != self.generation {
                    return Err("Stale panic worker generation".into());
                }
                if let Some(session) = &mut self.performance {
                    if self
                        .pending_panic
                        .is_some_and(|pending| pending != (ticket, scope))
                    {
                        return Err("Another Performance Panic is pending".into());
                    }
                    session.begin_panic(scope).map_err(|e| e.to_string())?;
                    self.pending_panic = Some((ticket, scope));
                    return session
                        .snapshot()
                        .live
                        .map(EngineResult::LiveStatus)
                        .ok_or("Performance is not ready".into());
                }
                let live = self.live.as_mut().ok_or("Live audition is not active")?;
                if live.admission.scope() != scope {
                    return Err("Stale panic scope".into());
                }
                self.pending_panic = Some((ticket, scope));
                let progress = live.panic(scope)?;
                self.urgent.panic = Some(PanicAck {
                    ticket,
                    requested_scope: scope,
                    outcome: PanicOutcome::Applied {
                        new_scope: progress.scope,
                    },
                });
                self.pending_panic = None;
                Ok(EngineResult::LiveStatus(progress))
            }
            Action::UrgentStop {
                ticket,
                cancelled_through_control,
                superseded_panic,
            } => {
                if ticket.worker_generation != self.generation {
                    return Err("Stale Stop worker generation".into());
                }
                if let Some(session) = &mut self.performance {
                    let pending = PendingPerformanceStop {
                        ticket,
                        cancelled_through_control,
                        superseded_panic,
                    };
                    if let Some(previous) = self.pending_performance_stop
                        && previous != pending
                    {
                        return Err("Another Performance Stop is pending".into());
                    }
                    self.pending_performance_stop = Some(pending);
                    let status =
                        session.cancel("Urgent Stop requested; original performance retained");
                    return Ok(EngineResult::Performance(Box::new(status)));
                }
                self.stop()?;
                let panic = superseded_panic.or(self.pending_panic.take());
                self.urgent.stop = Some(StopAck {
                    ticket,
                    cancelled_through_control,
                });
                if let Some((panic_ticket, scope)) = panic {
                    self.urgent.panic = Some(PanicAck {
                        ticket: panic_ticket,
                        requested_scope: scope,
                        outcome: PanicOutcome::SupersededByStop {
                            stop_ticket: ticket,
                        },
                    });
                }
                Ok(EngineResult::Stopped)
            }
            _ => Err("Invalid live action dispatch".into()),
        }
    }
    fn command(&mut self, command: EngineCommand) -> Result<EngineResult, String> {
        if self.retire.load(Ordering::Acquire) {
            return Err(self
                .retirement_error("Audio worker generation is retired; no new control admission"));
        }
        match command {
            EngineCommand::StartPerformance { request } => {
                if self
                    .performance_request
                    .as_ref()
                    .is_some_and(|old| old.id == request.id)
                {
                    if self
                        .performance
                        .as_ref()
                        .is_none_or(|s| s.request() != &request)
                    {
                        return Err("Start ID already binds a different performance intent".into());
                    }
                    return self
                        .performance
                        .as_mut()
                        .map(|s| EngineResult::Performance(Box::new(s.snapshot())))
                        .ok_or("Performance identity unavailable".into());
                }
                if self.capture.is_some()
                    || self.playback.is_some()
                    || self.live.is_some()
                    || self.performance.as_ref().is_some_and(|s| !s.is_quiescent())
                    || self.pending_performance_stop.is_some()
                    || self.pending_panic.is_some()
                {
                    return Err(
                        "Stop the active audio session before recording a performance".into(),
                    );
                }
                let mut session = crate::performance_session::PerformanceSession::start(
                    &self.root,
                    self.generation,
                    request.clone(),
                    self.retire.clone(),
                )?;
                let status = session.snapshot();
                let previous_progress = self
                    .performance
                    .as_mut()
                    .and_then(|previous| previous.snapshot().live);
                self.announced_performance_progress(previous_progress);
                self.performance = Some(session);
                self.performance_request = Some(request);
                self.performance_announced = false;
                self.last = None;
                Ok(EngineResult::Performance(Box::new(status)))
            }
            EngineCommand::FinishPerformance {
                start_id,
                scope,
                finish_id,
            } => {
                if self
                    .performance_request
                    .as_ref()
                    .is_none_or(|r| r.id != start_id)
                {
                    return Err("Stale Performance Finish identity".into());
                }
                let status = self
                    .performance
                    .as_mut()
                    .ok_or("No performance exists")?
                    .begin_finish(scope, finish_id)?;
                Ok(EngineResult::Performance(Box::new(status)))
            }
            EngineCommand::PerformanceStatus { start_id } => {
                if self
                    .performance_request
                    .as_ref()
                    .is_none_or(|r| r.id != start_id)
                {
                    return Err("Stale Performance status identity".into());
                }
                Ok(EngineResult::Performance(Box::new(
                    self.performance
                        .as_mut()
                        .ok_or("No performance exists")?
                        .snapshot(),
                )))
            }
            EngineCommand::Devices => Ok(EngineResult::Devices(inventory()?)),
            EngineCommand::Status => {
                if let Some(session) = &mut self.performance {
                    Ok(EngineResult::Performance(Box::new(session.snapshot())))
                } else if let Some(live) = &self.live {
                    Ok(EngineResult::LiveStatus(live.admission.progress()))
                } else {
                    Ok(EngineResult::Status(self.status()?))
                }
            }
            EngineCommand::StartLive {
                output_device,
                gui_clock,
            } => {
                if self.capture.is_some()
                    || self.playback.is_some()
                    || self.live.is_some()
                    || self.performance.as_ref().is_some_and(|s| !s.is_quiescent())
                    || self.pending_performance_stop.is_some()
                {
                    return Err("Stop the active audio session before live audition".into());
                }
                let live = LiveOutput::start(
                    self.generation,
                    output_device.as_deref(),
                    gui_clock,
                    self.retire.clone(),
                )?;
                let ready = live.ready();
                if !ready.is_valid() {
                    let _ = live.stop();
                    return Err("Live output faulted before start acknowledgment".into());
                }
                self.live = Some(live);
                self.performance = None;
                self.performance_request = None;
                self.last_live = None;
                self.first_fault = None;
                self.last = None;
                Ok(EngineResult::LiveStarted(ready))
            }
            EngineCommand::Stop => {
                self.stop()?;
                if let Some(session) = &mut self.performance
                    && !session.is_quiescent()
                {
                    return Ok(EngineResult::Performance(Box::new(session.snapshot())));
                }
                Ok(EngineResult::Stopped)
            }
            EngineCommand::StartCapture { input_device } => {
                if self.capture.is_some()
                    || self.playback.is_some()
                    || self.live.is_some()
                    || self.performance.as_ref().is_some_and(|s| !s.is_quiescent())
                    || self.pending_performance_stop.is_some()
                {
                    return Err("Solo capture requires stopped playback and no active take".into());
                }
                let mut capture =
                    Capture::start(&self.root, input_device.as_deref(), self.retire.clone())?;
                let status = capture.status();
                self.capture = Some(capture);
                self.performance = None;
                self.performance_request = None;
                self.last = None;
                Ok(EngineResult::Capturing(status))
            }
            EngineCommand::FinishCapture => {
                let capture = self.capture.take().ok_or("No capture is active")?;
                let result = capture
                    .finish(true)?
                    .ok_or("Capture was retained unsealed")?;
                self.last = None;
                Ok(EngineResult::Captured(result))
            }
            EngineCommand::StartPlayback {
                project_id,
                output_device,
            } => {
                if self.capture.is_some()
                    || self.playback.is_some()
                    || self.live.is_some()
                    || self.performance.as_ref().is_some_and(|s| !s.is_quiescent())
                    || self.pending_performance_stop.is_some()
                {
                    return Err("Stop the active audio session before starting playback".into());
                }
                let device = device(false, output_device.as_deref())?;
                let supported = device.default_output_config().map_err(|e| e.to_string())?;
                let config = supported.config();
                if !matches!(
                    supported.sample_format(),
                    cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
                ) {
                    return Err("Unsupported native output sample format".into());
                }
                let document = self.store.load(&project_id).map_err(|e| e.to_string())?;
                let samples = crate::render::prepare(
                    &self.store,
                    &document,
                    config.sample_rate,
                    config.channels,
                )?;
                let (mut cursor, meters) = RenderCursor::new(samples, config.channels)?;
                let frames = cursor.frame_count();
                let errors = meters.clone();
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::F32 => device.build_output_stream(
                        config,
                        move |data: &mut [f32], info| {
                            let t = info.timestamp();
                            cursor.render_into_at(
                                data,
                                u64::try_from(t.callback.as_nanos()).unwrap_or(u64::MAX),
                                u64::try_from(t.playback.as_nanos()).unwrap_or(u64::MAX),
                                config.sample_rate,
                            );
                        },
                        move |_| errors.device_fault(),
                        Some(Duration::from_secs(2)),
                    ),
                    cpal::SampleFormat::I16 => device.build_output_stream(
                        config,
                        move |data: &mut [i16], info| {
                            let t = info.timestamp();
                            cursor.render_i16_at(
                                data,
                                u64::try_from(t.callback.as_nanos()).unwrap_or(u64::MAX),
                                u64::try_from(t.playback.as_nanos()).unwrap_or(u64::MAX),
                                config.sample_rate,
                            );
                        },
                        move |_| errors.device_fault(),
                        Some(Duration::from_secs(2)),
                    ),
                    cpal::SampleFormat::U16 => device.build_output_stream(
                        config,
                        move |data: &mut [u16], info| {
                            let t = info.timestamp();
                            cursor.render_u16_at(
                                data,
                                u64::try_from(t.callback.as_nanos()).unwrap_or(u64::MAX),
                                u64::try_from(t.playback.as_nanos()).unwrap_or(u64::MAX),
                                config.sample_rate,
                            );
                        },
                        move |_| errors.device_fault(),
                        Some(Duration::from_secs(2)),
                    ),
                    _ => unreachable!(),
                }
                .map_err(|e| e.to_string())?;
                if let Err(error) = stream.play() {
                    self.retire.store(true, Ordering::Release);
                    let message = format!("Output start failed; audio worker retired: {error}");
                    std::mem::forget(stream);
                    return Err(message);
                }
                let playback = Playback {
                    stream,
                    meters,
                    frames,
                    rate: config.sample_rate,
                    channels: config.channels,
                    retire: self.retire.clone(),
                };
                let status = playback.status();
                self.playback = Some(playback);
                self.performance = None;
                self.performance_request = None;
                self.last = None;
                Ok(EngineResult::Playing(status))
            }
        }
    }
    fn response_candidate(
        &mut self,
        request_id: u64,
        mut result: Result<EngineResult, String>,
    ) -> Response {
        let update = if self.retire.load(Ordering::Acquire) {
            None
        } else {
            self.observe_result(&result);
            match self.update() {
                Ok(update) => Some(update),
                Err(error) => {
                    result = Err(error);
                    None
                }
            }
        };
        Response {
            reply: EngineReply { request_id, result },
            update,
        }
    }
    fn finish_response(&self, mut response: Response) -> PreparedResponse {
        let exit_after_send = self.retire.load(Ordering::Acquire);
        if exit_after_send {
            let fallback = response
                .reply
                .result
                .as_ref()
                .err()
                .map(String::as_str)
                .unwrap_or("Audio worker generation is retired; no new control admission");
            response.reply.result = Err(self.retirement_error(fallback));
            // A raced Ready is no longer being returned. Do not publish its
            // new scope (or any partial update) ahead of the terminal cause.
            response.update = None;
        }
        PreparedResponse {
            response,
            exit_after_send,
        }
    }
    fn observe_result(&mut self, result: &Result<EngineResult, String>) {
        if let Ok(EngineResult::Performance(status)) = result
            && status.ready.is_some()
            && !self.performance_announced
        {
            // Publish a new live scope only alongside a response carrying its
            // authenticated real Ready, never a raced Starting response.
            self.performance_announced = true;
            self.first_fault = None;
            self.last_live = None;
        }
    }
}
// This decision is bound to the final reply, not a later atomic reread after
// send. A later retirement must remain reachable through the next bounded IPC
// request instead of exiting behind an already-constructed normal completion.
struct PreparedResponse {
    response: Response,
    exit_after_send: bool,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
// This is armed only inside the separate worker process. A stuck filesystem
// cannot prevent retirement after parent contact ceases. Exit intentionally
// skips destructors and leaves the journal's atomic durable prefix unsealed.
struct Watchdog {
    done: Arc<AtomicBool>,
    last: Arc<AtomicU64>,
    epoch: Instant,
}
impl Watchdog {
    fn start() -> Result<Self, String> {
        let done = Arc::new(AtomicBool::new(false));
        let last = Arc::new(AtomicU64::new(0));
        let epoch = Instant::now();
        let thread_done = done.clone();
        let thread_last = last.clone();
        thread::Builder::new()
            .name("audio-parent-watchdog".into())
            .spawn(move || {
                while !thread_done.load(Ordering::Acquire) {
                    let now = epoch.elapsed().as_millis() as u64;
                    if now.saturating_sub(thread_last.load(Ordering::Acquire)) > 20_000 {
                        std::process::exit(74);
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { done, last, epoch })
    }
    fn contacted(&self) {
        self.last
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Release);
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
    }
}
#[cfg(unix)]
pub fn run_worker() -> Result<(), String> {
    use omniusong_ipc::unix::{WorkerFacts, connect_inherited, default_worker_limits};
    if !cfg!(target_os = "macos") {
        return Err("This native audio adapter is currently Mac-only".into());
    }
    let mut connection = connect_inherited(
        WorkerFacts {
            role: WorkerRole::Engine,
            platform: platform(),
            architecture: architecture(),
            build_id: BUILD.into(),
            limits: default_worker_limits(),
            capabilities: vec!["native-audio-control-v2".into(), LIVE_PROTOCOL.into()],
        },
        Duration::from_secs(5),
    )
    .map_err(|e| e.to_string())?;
    let watchdog = Watchdog::start()?;
    let mut runtime = None;
    let mut expected = 0u64;
    loop {
        let request = match connection.receive::<Request>(Duration::from_secs(3)) {
            Ok(Some(v)) => v.payload,
            Ok(None) => break,
            Err(e) => {
                drop(runtime);
                return Err(format!(
                    "Audio parent channel ended: {e}; active originals retained unsealed"
                ));
            }
        };
        if request.request_id != expected {
            return Err("Audio request sequence mismatch".into());
        }
        expected = expected
            .checked_add(1)
            .ok_or("Audio request sequence exhausted")?;
        let result = match request.action {
            Action::Initialize {
                store_root,
                worker_generation,
                profile,
            } => {
                if runtime.is_some() || request.request_id != 0 || profile != LIVE_PROTOCOL {
                    Err("Audio initialization/profile is only allowed once".into())
                } else {
                    Runtime::new(store_root, worker_generation).map(|r| {
                        runtime = Some(r);
                        EngineResult::Initialized {
                            profile: LIVE_PROTOCOL.into(),
                            worker_generation,
                        }
                    })
                }
            }
            Action::Command { command } => match runtime.as_mut() {
                Some(r) => r.command(command),
                None => Err("Audio worker is not initialized".into()),
            },
            action => match runtime.as_mut() {
                Some(r) => r.live_action(action),
                None => Err("Audio worker is not initialized".into()),
            },
        };
        let result = result.map_err(|e| {
            if e.len() > 16000 {
                e.chars().take(4000).collect()
            } else {
                e
            }
        });
        let prepared = if let Some(runtime) = &mut runtime {
            let candidate = runtime.response_candidate(request.request_id, result);
            runtime.finish_response(candidate)
        } else {
            PreparedResponse {
                response: Response {
                    update: None,
                    reply: EngineReply {
                        request_id: request.request_id,
                        result,
                    },
                },
                exit_after_send: false,
            }
        };
        let sent = connection.send_payload(prepared.response, Duration::from_secs(3));
        if prepared.exit_after_send {
            // The bounded send attempted the retained terminal cause. Avoid
            // unsafe driver/journal destructors even if the peer disconnected.
            // A retirement after a normal reply's fence does not take this exit.
            std::process::exit(75);
        }
        sent.map_err(|e| e.to_string())?;
        watchdog.contacted();
    }
    drop(runtime);
    Ok(())
}
#[cfg(not(unix))]
pub fn run_worker() -> Result<(), String> {
    Err("Private audio worker transport is not implemented on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn software_runtime() -> (PathBuf, Runtime) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-playback-lifecycle-review")
            .join(uuid::Uuid::new_v4().to_string());
        NativeStore::open(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let runtime = Runtime::new(root.clone(), LiveId::random()).unwrap();
        (root, runtime)
    }
    fn software_finished_progress(generation: LiveId) -> LiveProgress {
        let scope = LiveScope {
            worker_generation: generation,
            session_id: LiveId::random(),
            panic_generation: 1,
        };
        let (mut admission, mut renderer, mut owner) =
            crate::live::recording_live_pair(scope, 48_000, 2).unwrap();
        admission.admit(LiveBatch::empty(scope, 1, None)).unwrap();
        admission.begin_finish(scope, LiveId::random()).unwrap();
        renderer.render_into(&mut [0.0; 512]);
        while owner.try_pop().unwrap().is_some() {}
        assert_eq!(admission.finish_ack().unwrap().unwrap().cut_frame, 256);
        let progress = admission.progress();
        assert_eq!(progress.state, LiveState::FinishedCut);
        assert!(progress.is_valid());
        drop(renderer);
        drop(owner);
        drop(admission);
        progress
    }
    #[test]
    fn finished_performance_summary_survives_owner_removal_playback_status_and_stop() {
        let (root, mut runtime) = software_runtime();
        let terminal = software_finished_progress(runtime.generation);
        runtime.performance_announced = true;
        assert_eq!(
            runtime.announced_performance_progress(Some(terminal)),
            Some(terminal)
        );
        assert_eq!(runtime.last_live, Some(terminal));
        // The real playback branch removes performance after successful native
        // setup. Exercise the following update/status path without any device.
        runtime.performance = None;
        runtime.performance_request = None;
        runtime.last = Some(EngineStatus {
            state: "playback_finished".into(),
            capture: None,
            sample_rate: 48_000,
            channels: 2,
            frames: 1024,
            peak: 0.0,
            xruns: 0,
        });
        let first = runtime.update().unwrap();
        assert_eq!(first.live, Some(terminal));
        assert!(
            matches!(runtime.command(EngineCommand::Status).unwrap(), EngineResult::Status(status) if status.state == "playback_finished")
        );
        assert_eq!(runtime.update().unwrap().live, Some(terminal));
        assert!(matches!(
            runtime.command(EngineCommand::Stop).unwrap(),
            EngineResult::Stopped
        ));
        let stopped = runtime.update().unwrap();
        assert!(stopped.revision > first.revision);
        assert_eq!(stopped.live, Some(terminal));
        assert!(!runtime.retire.load(Ordering::Acquire));
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unannounced_or_foreign_performance_cannot_replace_the_retained_scope() {
        let (root, mut runtime) = software_runtime();
        let old = software_finished_progress(runtime.generation);
        runtime.last_live = Some(old);
        let newer = software_finished_progress(runtime.generation);
        assert_eq!(runtime.announced_performance_progress(Some(newer)), None);
        assert_eq!(runtime.update().unwrap().live, Some(old));
        runtime.performance_announced = true;
        let foreign = software_finished_progress(LiveId::random());
        // Do not silently change the evidence sent for a foreign observation:
        // the parent's worker-generation validator remains authoritative.
        assert_eq!(
            runtime.announced_performance_progress(Some(foreign)),
            Some(foreign)
        );
        assert_eq!(runtime.last_live, Some(old));
        assert_eq!(runtime.update().unwrap().live, Some(old));
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn first_authenticated_performance_ready_replaces_old_scope_only_once() {
        let (root, mut runtime) = software_runtime();
        let old = software_finished_progress(runtime.generation);
        runtime.last_live = Some(old);
        let scope = LiveScope {
            worker_generation: runtime.generation,
            session_id: LiveId::random(),
            panic_generation: 1,
        };
        let (admission, renderer) = crate::live::live_pair(scope, 48_000, 2).unwrap();
        let progress = admission.progress();
        let start = PerformanceStart {
            id: LiveId::random(),
            output_device: None,
            gui_clock: LiveClock {
                domain: "software-review".into(),
                epoch: "fixture".into(),
                ticks_per_second_numerator: 1000,
                ticks_per_second_denominator: 1,
            },
        };
        let ready = LiveReady {
            scope,
            gui_clock: start.gui_clock.clone(),
            sources: [
                LiveSourceKind::ComputerKeys,
                LiveSourceKind::PointerKeys,
                LiveSourceKind::Pads,
                LiveSourceKind::AccessiblePulse,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| LiveSourceBinding {
                kind,
                handle: i as u8 + 1,
                connection_generation: 1,
            })
            .collect(),
            output: DeviceInfo {
                id: "synthetic".into(),
                name: "Protocol fixture; no native device".into(),
            },
            sample_rate: 48_000,
            channels: 2,
            progress,
        };
        assert!(ready.is_valid());
        let mut status = PerformanceStatus {
            revision: 1,
            identity: PerformanceIdentity {
                start_id: start.id,
                capture: CaptureIdentity {
                    journal_path: root
                        .join("captures")
                        .join(start.directory_name())
                        .join("take.sqlite3"),
                    take_id: start.take_id(),
                },
                descriptor_sha256: None,
            },
            phase: PerformancePhase::Starting,
            ready: None,
            native_sample_format: None,
            live: None,
            writer: None,
            finish: None,
            cut: None,
            native: NativeRetirementStatus::default(),
            sealed: None,
            error: None,
        };
        runtime.observe_result(&Ok(EngineResult::Performance(Box::new(status.clone()))));
        assert_eq!(runtime.update().unwrap().live, Some(old));
        status.revision = 2;
        status.identity.descriptor_sha256 = Some("a".repeat(64));
        status.phase = PerformancePhase::Recording;
        status.ready = Some(ready);
        status.native_sample_format = Some("f32".into());
        status.live = Some(progress);
        assert!(status.is_valid());
        let response = Ok(EngineResult::Performance(Box::new(status)));
        runtime.observe_result(&response);
        assert!(runtime.performance_announced);
        assert!(runtime.last_live.is_none());
        assert_eq!(
            runtime.announced_performance_progress(Some(progress)),
            Some(progress)
        );
        runtime.observe_result(&response);
        assert_eq!(runtime.update().unwrap().live, Some(progress));
        drop(renderer);
        drop(admission);
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn expired_performance_rejects_immediately_with_exact_cause_before_status_poll() {
        let (root, mut runtime) = software_runtime();
        let earlier_completion = runtime.command(EngineCommand::Status).unwrap();
        runtime.performance = Some(
            crate::performance_session::PerformanceSession::expired_sealing_for_test(
                runtime.retire.clone(),
            ),
        );
        let cause =
            "Recorded output sealing deadline expired; worker retired and original retained";
        // No performance snapshot/update has run after expiration. Even an
        // earlier ordinary completion cannot replace the immutable cause.
        assert!(matches!(earlier_completion, EngineResult::Status(_)));
        for command in [
            EngineCommand::Status,
            EngineCommand::PerformanceStatus {
                start_id: LiveId([7; 16]),
            },
            EngineCommand::Devices,
            EngineCommand::Stop,
            EngineCommand::StartPlayback {
                project_id: "must not be opened".into(),
                output_device: None,
            },
        ] {
            assert_eq!(runtime.command(command).unwrap_err(), cause);
        }
        let scope = LiveScope {
            worker_generation: runtime.generation,
            session_id: LiveId([7; 16]),
            panic_generation: 1,
        };
        assert_eq!(
            runtime
                .live_action(Action::LiveBatch {
                    batch: LiveBatch::empty(scope, 1, None),
                })
                .unwrap_err(),
            cause
        );
        assert!(runtime.capture.is_none() && runtime.playback.is_none() && runtime.live.is_none());
        assert!(runtime.retire.load(Ordering::Acquire));
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    fn wire_response(response: &Response) -> Response {
        use omniusong_core::contracts::Validate;
        response.validate().unwrap();
        let bytes = serde_json::to_vec(response).unwrap();
        let decoded: Response = serde_json::from_slice(&bytes).unwrap();
        decoded.validate().unwrap();
        decoded
    }

    #[test]
    fn retirement_between_candidate_and_reply_fence_suppresses_ready_and_preserves_cause() {
        let (root, mut runtime) = software_runtime();
        let (_, _, recording) =
            crate::live_supervisor::performance_fixture(runtime.generation, &root);
        let candidate =
            runtime.response_candidate(41, Ok(EngineResult::Performance(Box::new(recording))));
        assert!(
            runtime.performance_announced,
            "actual observe_result ran before retirement"
        );
        assert!(
            matches!(&candidate.reply.result, Ok(EngineResult::Performance(status)) if status.ready.is_some())
        );
        // Retire at the precise boundary after observe_result/update, before
        // final wire construction; no status poll may be needed to get cause.
        runtime.performance = Some(
            crate::performance_session::PerformanceSession::expired_sealing_for_test(
                runtime.retire.clone(),
            ),
        );
        let prepared = runtime.finish_response(candidate);
        assert!(prepared.exit_after_send);
        let reply = wire_response(&prepared.response);
        assert_eq!(reply.reply.request_id, 41);
        assert_eq!(
            reply.reply.result.unwrap_err(),
            "Recorded output sealing deadline expired; worker retired and original retained"
        );
        assert!(
            reply.update.is_none(),
            "retired Ready cannot announce a new scope"
        );
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retirement_after_normal_reply_fence_waits_for_exact_terminal_reply() {
        let (root, mut runtime) = software_runtime();
        let result = runtime.command(EngineCommand::Status).unwrap();
        let candidate = runtime.response_candidate(51, Ok(result));
        let prepared = runtime.finish_response(candidate);
        assert!(!prepared.exit_after_send);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        assert!(sender.try_send(wire_response(&prepared.response)).is_ok());
        // This is the old problematic interval: the normal completion is
        // already constructed/queued when the retirement flag becomes true.
        runtime.performance = Some(
            crate::performance_session::PerformanceSession::expired_sealing_for_test(
                runtime.retire.clone(),
            ),
        );
        assert!(runtime.retire.load(Ordering::Acquire));
        assert!(
            !prepared.exit_after_send,
            "cannot exit behind the queued normal reply"
        );
        let normal = receiver.try_recv().unwrap();
        assert_eq!(normal.reply.request_id, 51);
        assert!(normal.reply.result.is_ok());
        let rejected = runtime.command(EngineCommand::Status);
        let candidate = runtime.response_candidate(52, rejected);
        let terminal = runtime.finish_response(candidate);
        assert!(terminal.exit_after_send);
        assert!(sender.try_send(wire_response(&terminal.response)).is_ok());
        let terminal = receiver.try_recv().unwrap();
        assert_eq!(terminal.reply.request_id, 52);
        assert_eq!(
            terminal.reply.result.unwrap_err(),
            "Recorded output sealing deadline expired; worker retired and original retained"
        );
        assert!(terminal.update.is_none());
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retirement_inside_update_becomes_terminal_wire_error_not_stale_success() {
        let (root, mut runtime) = software_runtime();
        let result = runtime.command(EngineCommand::Status).unwrap();
        runtime.update_revision = u64::MAX;
        let candidate = runtime.response_candidate(61, Ok(result));
        assert!(runtime.retire.load(Ordering::Acquire));
        let prepared = runtime.finish_response(candidate);
        assert!(prepared.exit_after_send);
        let response = wire_response(&prepared.response);
        assert_eq!(response.reply.request_id, 61);
        assert_eq!(
            response.reply.result.unwrap_err(),
            "Live update sequence exhausted"
        );
        assert!(response.update.is_none());
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retired_generation_rejects_device_and_mutating_commands_before_native_access() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/engine-unit")
            .join(uuid::Uuid::new_v4().to_string());
        NativeStore::open(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut runtime = Runtime::new(root, LiveId::random()).unwrap();
        runtime.retire.store(true, Ordering::Release);
        for command in [
            EngineCommand::Devices,
            EngineCommand::StartCapture { input_device: None },
            EngineCommand::StartPlayback {
                project_id: "absent".into(),
                output_device: None,
            },
            EngineCommand::FinishCapture,
            EngineCommand::Status,
        ] {
            assert!(
                runtime
                    .command(command)
                    .unwrap_err()
                    .contains("generation is retired")
            );
        }
        assert!(runtime.capture.is_none() && runtime.playback.is_none());
    }
}
